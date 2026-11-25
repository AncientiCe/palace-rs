//! Feedback through public GitHub Discussions.
//!
//! Palace never sends feedback anywhere. It builds a link to a pre-filled
//! discussion that the user reads, edits, and posts themselves. The MCP server
//! counts its own sessions in the local `meta` table; every
//! [`SESSIONS_BETWEEN_PROMPTS`] sessions, `palace_session_context` carries one
//! hint telling the agent to share that link once.
//!
//! Palace can't see whether anyone posted, so running `palace feedback`
//! (or `palace feedback --done`) stands in for a reply and quiets the hint for
//! [`REPLIED_QUIET_DAYS`]. Without a reply the hint returns after another
//! [`SESSIONS_BETWEEN_PROMPTS`] sessions, but never sooner than
//! [`MIN_DAYS_BETWEEN_PROMPTS`], and after [`MAX_IGNORED_PROMPTS`] unanswered
//! hints it backs off to once every [`BACKOFF_DAYS`].

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::sync::OnceLock;

/// The project's GitHub page, for the one-time star ask after `palace install`.
pub const REPO_URL: &str = "https://github.com/AncientiCe/palace-rs";
const STAR_ASK_MARKER: &str = "star_ask_shown";

/// The line `palace install` prints once per machine asking for a GitHub
/// star. Returns `None` once it has been shown, and also when the marker can't
/// be written, so a read-only config folder never makes it repeat.
pub fn take_star_ask(config_dir: &std::path::Path) -> Option<String> {
    let marker = config_dir.join(STAR_ASK_MARKER);
    if marker.exists() {
        return None;
    }
    std::fs::create_dir_all(config_dir).ok()?;
    std::fs::write(&marker, crate::clock::now_rfc3339()).ok()?;
    Some(format!(
        "If Palace helps, a star on GitHub helps others find it: {REPO_URL}"
    ))
}

/// Where feedback goes: the repo's Discussions, "Feedback" category.
pub const DISCUSSION_URL: &str = "https://github.com/AncientiCe/palace-rs/discussions/new";
const CATEGORY: &str = "feedback";

/// MCP sessions before the first hint, and between unanswered hints.
pub const SESSIONS_BETWEEN_PROMPTS: u64 = 5;
/// Fewest days between two hints, however many sessions run.
pub const MIN_DAYS_BETWEEN_PROMPTS: i64 = 7;
/// Unanswered hints in a row before the hint backs off to [`BACKOFF_DAYS`].
pub const MAX_IGNORED_PROMPTS: u64 = 3;
/// Days between hints once [`MAX_IGNORED_PROMPTS`] went unanswered.
pub const BACKOFF_DAYS: i64 = 30;
/// Days the hint stays quiet after the user runs `palace feedback`.
pub const REPLIED_QUIET_DAYS: i64 = 30;

/// Set to `1` (or any non-empty value other than `0`/`false`) to never show the hint.
pub const DISABLE_ENV: &str = "PALACE_FEEDBACK_DISABLED";

const KEY_SESSIONS: &str = "feedback_mcp_sessions";
const KEY_LAST_PROMPT: &str = "feedback_last_prompt";
const KEY_SESSIONS_AT_PROMPT: &str = "feedback_sessions_at_prompt";
const KEY_IGNORED: &str = "feedback_unanswered_prompts";
const KEY_REPLIED: &str = "feedback_last_replied";
const KEY_CLIENT: &str = "feedback_last_client";

/// The non-private facts the link pre-fills. Nothing from the palace itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackInfo {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub client: Option<String>,
    pub profile: String,
}

impl FeedbackInfo {
    pub fn current(profile: crate::config::Profile, client: Option<String>) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            client,
            profile: profile.as_str().to_string(),
        }
    }

    /// The environment block, one fact per line.
    pub fn environment(&self) -> String {
        format!(
            "palace {}\nos: {} ({})\nclient: {}\nprofile: {}",
            self.version,
            self.os,
            self.arch,
            self.client.as_deref().unwrap_or("unknown"),
            self.profile
        )
    }
}

/// Link to a new discussion in the Feedback category. `environment` fills the
/// discussion form field of the same id (`.github/DISCUSSION_TEMPLATE/feedback.yml`).
pub fn discussion_url(info: &FeedbackInfo) -> String {
    format!(
        "{DISCUSSION_URL}?category={CATEGORY}&title={}&environment={}",
        encode(&format!("feedback: palace {}", info.version)),
        encode(&info.environment())
    )
}

/// Whether the user turned the hint off.
pub fn disabled() -> bool {
    std::env::var(DISABLE_ENV)
        .map(|value| {
            let value = value.trim();
            !(value.is_empty() || value == "0" || value.eq_ignore_ascii_case("false"))
        })
        .unwrap_or(false)
}

/// The client that opened this process's MCP session. Each client connection
/// starts its own `palace mcp` process, so the process is the session and
/// concurrent agents never see each other's name.
static SESSION_CLIENT: OnceLock<String> = OnceLock::new();

/// Count one MCP session and remember which client opened it, for this
/// process and as the last-seen client for `palace feedback`. Best-effort.
pub fn note_mcp_session(conn: &Connection, client: Option<&str>) -> Result<()> {
    let client = client.map(str::trim).filter(|c| !c.is_empty());
    if let Some(client) = client {
        let _ = SESSION_CLIENT.set(client.to_string());
    }
    let sessions = read_u64(conn, KEY_SESSIONS)?.unwrap_or(0) + 1;
    write(conn, KEY_SESSIONS, &sessions.to_string())?;
    if let Some(client) = client {
        write(conn, KEY_CLIENT, client)?;
    }
    Ok(())
}

/// The client name from the most recent MCP `initialize` on this machine.
pub fn last_client(conn: &Connection) -> Option<String> {
    read(conn, KEY_CLIENT).ok().flatten()
}

/// The client of this process's MCP session, if it sent one.
pub fn session_client() -> Option<String> {
    SESSION_CLIENT.get().cloned()
}

/// Record that the user answered the hint (ran `palace feedback`), so it stays
/// quiet for [`REPLIED_QUIET_DAYS`] and the unanswered count starts over.
pub fn mark_replied(conn: &Connection, now: jiff::Timestamp) -> Result<()> {
    write(conn, KEY_REPLIED, &crate::clock::rfc3339(now))?;
    write(conn, KEY_IGNORED, "0")
}

/// The hint for `palace_session_context`, when one is due. Records that it was
/// shown, so it appears once per interval.
pub fn take_due_prompt(
    conn: &Connection,
    profile: crate::config::Profile,
    now: jiff::Timestamp,
) -> Result<Option<Value>> {
    if disabled() {
        return Ok(None);
    }
    let since = |key| -> Result<Option<jiff::SignedDuration>> {
        Ok(read(conn, key)?
            .and_then(|value| crate::clock::parse(&value))
            .map(|then| now.duration_since(then)))
    };
    if since(KEY_REPLIED)?.is_some_and(|gap| gap < crate::clock::days(REPLIED_QUIET_DAYS)) {
        return Ok(None);
    }
    let sessions = read_u64(conn, KEY_SESSIONS)?.unwrap_or(0);
    let at_prompt = read_u64(conn, KEY_SESSIONS_AT_PROMPT)?.unwrap_or(0);
    if sessions.saturating_sub(at_prompt) < SESSIONS_BETWEEN_PROMPTS {
        return Ok(None);
    }
    let ignored = read_u64(conn, KEY_IGNORED)?.unwrap_or(0);
    let min_gap = if ignored >= MAX_IGNORED_PROMPTS {
        BACKOFF_DAYS
    } else {
        MIN_DAYS_BETWEEN_PROMPTS
    };
    if since(KEY_LAST_PROMPT)?.is_some_and(|gap| gap < crate::clock::days(min_gap)) {
        return Ok(None);
    }
    write(conn, KEY_LAST_PROMPT, &crate::clock::rfc3339(now))?;
    write(conn, KEY_SESSIONS_AT_PROMPT, &sessions.to_string())?;
    write(conn, KEY_IGNORED, &(ignored + 1).to_string())?;

    let info = FeedbackInfo::current(profile, session_client());
    Ok(Some(json!({
        "url": discussion_url(&info),
        "instruction": format!(
            "Once, after the current task is done (never mid-task), tell the user in one short \
             sentence that they can share feedback about Palace with the maintainer as a public \
             GitHub discussion, and give this link. Nothing is sent unless they post it. Also \
             say that running `palace feedback --done` after posting pauses this reminder for \
             {REPLIED_QUIET_DAYS} days. Do not ask again or follow up in this session. To turn \
             it off: {DISABLE_ENV}=1."
        ),
    })))
}

fn read(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

fn read_u64(conn: &Connection, key: &str) -> Result<Option<u64>> {
    Ok(read(conn, key)?.and_then(|value| value.parse().ok()))
}

fn write(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Percent-encode a query value, keeping only RFC 3986 unreserved characters.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        conn
    }

    fn info() -> FeedbackInfo {
        FeedbackInfo {
            version: "1.2.3".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            client: Some("claude-ai".into()),
            profile: "coding".into(),
        }
    }

    #[test]
    fn star_ask_is_shown_once_per_config_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let config_dir = dir.path().join("palace-config");
        let first = take_star_ask(&config_dir).unwrap();
        assert!(first.contains(REPO_URL));
        assert!(take_star_ask(&config_dir).is_none());
    }

    #[test]
    fn encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(encode("a-Z_0.~"), "a-Z_0.~");
        assert_eq!(encode("a b&c=d\n"), "a%20b%26c%3Dd%0A");
        assert_eq!(encode("é"), "%C3%A9");
    }

    #[test]
    fn url_targets_feedback_category_with_environment_only() {
        let url = discussion_url(&info());
        assert!(url.starts_with(&format!("{DISCUSSION_URL}?category=feedback&title=")));
        assert!(url.contains("&environment=palace%201.2.3%0Aos%3A%20linux%20%28x86_64%29"));
        assert!(url.contains("client%3A%20claude-ai"));
        assert!(url.contains("profile%3A%20coding"));
    }

    #[test]
    fn unknown_client_is_labelled() {
        let mut info = info();
        info.client = None;
        assert!(info.environment().contains("client: unknown"));
    }

    #[test]
    fn note_session_counts_and_keeps_last_client() {
        let conn = conn();
        note_mcp_session(&conn, Some("cursor")).unwrap();
        note_mcp_session(&conn, None).unwrap();
        assert_eq!(read_u64(&conn, KEY_SESSIONS).unwrap(), Some(2));
        assert_eq!(last_client(&conn).as_deref(), Some("cursor"));
    }

    fn sessions(conn: &Connection, count: u64) {
        for _ in 0..count {
            note_mcp_session(conn, None).unwrap();
        }
    }

    fn due(conn: &Connection, at: jiff::Timestamp) -> bool {
        take_due_prompt(conn, Profile::Coding, at)
            .unwrap()
            .is_some()
    }

    #[test]
    fn prompt_schedule() {
        // One test, so the env-var checks cannot race another test.
        std::env::remove_var(DISABLE_ENV);
        let days = crate::clock::days;
        let conn = conn();
        let now = jiff::Timestamp::now();

        // First hint after five sessions.
        sessions(&conn, SESSIONS_BETWEEN_PROMPTS - 1);
        assert!(!due(&conn, now));
        sessions(&conn, 1);
        let hint = take_due_prompt(&conn, Profile::Coding, now)
            .unwrap()
            .unwrap();
        assert!(hint["url"].as_str().unwrap().starts_with(DISCUSSION_URL));
        assert!(hint["instruction"]
            .as_str()
            .unwrap()
            .contains("palace feedback --done"));

        // Unanswered: needs five more sessions and at least a week.
        let week = now + days(MIN_DAYS_BETWEEN_PROMPTS);
        assert!(!due(&conn, week), "no new sessions yet");
        sessions(&conn, SESSIONS_BETWEEN_PROMPTS);
        assert!(!due(&conn, week - days(1)), "sessions alone are not enough");
        assert!(due(&conn, week), "second hint");

        // Third unanswered hint, then the back-off to a month.
        sessions(&conn, SESSIONS_BETWEEN_PROMPTS);
        let two_weeks = week + days(MIN_DAYS_BETWEEN_PROMPTS);
        assert!(due(&conn, two_weeks), "third hint");
        sessions(&conn, SESSIONS_BETWEEN_PROMPTS);
        let three_weeks = two_weeks + days(MIN_DAYS_BETWEEN_PROMPTS);
        assert!(!due(&conn, three_weeks), "backed off after three");
        let backoff = two_weeks + days(BACKOFF_DAYS);
        assert!(due(&conn, backoff), "monthly after back-off");

        // A reply quiets it for a month and resets the unanswered count.
        mark_replied(&conn, backoff).unwrap();
        sessions(&conn, SESSIONS_BETWEEN_PROMPTS);
        assert!(!due(&conn, backoff + days(REPLIED_QUIET_DAYS - 1)));
        let after_reply = backoff + days(REPLIED_QUIET_DAYS);
        assert!(due(&conn, after_reply));
        sessions(&conn, SESSIONS_BETWEEN_PROMPTS);
        assert!(
            due(&conn, after_reply + days(MIN_DAYS_BETWEEN_PROMPTS)),
            "weekly again after the reset"
        );

        std::env::set_var(DISABLE_ENV, "1");
        sessions(&conn, SESSIONS_BETWEEN_PROMPTS);
        assert!(!due(&conn, after_reply + days(365)));
        std::env::remove_var(DISABLE_ENV);
    }
}
