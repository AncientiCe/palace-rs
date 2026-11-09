//! Feedback through public GitHub Discussions.
//!
//! Palace never sends feedback anywhere. It builds a link to a pre-filled
//! discussion that the user reads, edits, and posts themselves. The MCP server
//! counts its own sessions in the local `meta` table; after enough sessions,
//! `palace_session_context` carries one hint telling the agent to share that
//! link once. The hint then stays quiet for [`PROMPT_INTERVAL_DAYS`].

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

/// Where feedback goes: the repo's Discussions, "Feedback" category.
pub const DISCUSSION_URL: &str = "https://github.com/AncientiCe/palace-rs/discussions/new";
const CATEGORY: &str = "feedback";

/// MCP sessions before the first hint.
pub const SESSIONS_BEFORE_PROMPT: u64 = 10;
/// Days between hints.
pub const PROMPT_INTERVAL_DAYS: i64 = 30;

/// Set to `1` (or any non-empty value other than `0`/`false`) to never show the hint.
pub const DISABLE_ENV: &str = "PALACE_FEEDBACK_DISABLED";

const KEY_SESSIONS: &str = "feedback_mcp_sessions";
const KEY_LAST_PROMPT: &str = "feedback_last_prompt";
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

/// Link to a new discussion in the Feedback category with the environment
/// pre-filled. `environment` targets the discussion form field; `body` is the
/// fallback when the category has no form.
pub fn discussion_url(info: &FeedbackInfo) -> String {
    let environment = info.environment();
    let body = format!(
        "**What helped**\n\n\n**What got in the way**\n\n\n**What's missing**\n\n\n**Environment**\n```\n{environment}\n```\n"
    );
    format!(
        "{DISCUSSION_URL}?category={CATEGORY}&title={}&environment={}&body={}",
        encode(&format!("feedback: palace {}", info.version)),
        encode(&environment),
        encode(&body)
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

/// Count one MCP session and remember which client opened it. Best-effort.
pub fn note_mcp_session(conn: &Connection, client: Option<&str>) -> Result<()> {
    let sessions = read_u64(conn, KEY_SESSIONS)?.unwrap_or(0) + 1;
    write(conn, KEY_SESSIONS, &sessions.to_string())?;
    if let Some(client) = client.map(str::trim).filter(|c| !c.is_empty()) {
        write(conn, KEY_CLIENT, client)?;
    }
    Ok(())
}

/// The client name from the most recent MCP `initialize`, if any.
pub fn last_client(conn: &Connection) -> Option<String> {
    read(conn, KEY_CLIENT).ok().flatten()
}

/// The hint for `palace_session_context`, when one is due. Records that it was
/// shown, so it appears once per interval.
pub fn take_due_prompt(
    conn: &Connection,
    profile: crate::config::Profile,
    now: DateTime<Utc>,
) -> Result<Option<Value>> {
    if disabled() {
        return Ok(None);
    }
    let sessions = read_u64(conn, KEY_SESSIONS)?.unwrap_or(0);
    if sessions < SESSIONS_BEFORE_PROMPT {
        return Ok(None);
    }
    let last = read(conn, KEY_LAST_PROMPT)?
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc));
    if last.is_some_and(|last| now - last < Duration::days(PROMPT_INTERVAL_DAYS)) {
        return Ok(None);
    }
    write(conn, KEY_LAST_PROMPT, &now.to_rfc3339())?;

    let info = FeedbackInfo::current(profile, last_client(conn));
    Ok(Some(json!({
        "url": discussion_url(&info),
        "instruction": format!(
            "Once, after the current task is done (never mid-task), tell the user in one short \
             sentence that they can share feedback about Palace with the maintainer as a public \
             GitHub discussion, and give this link. Nothing is sent unless they post it. Do not \
             ask again or follow up; this hint returns in {PROMPT_INTERVAL_DAYS} days. To turn it \
             off: {DISABLE_ENV}=1."
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

    #[test]
    fn prompt_waits_for_sessions_then_respects_interval() {
        // Runs in one test so the env-var check cannot race another test.
        std::env::remove_var(DISABLE_ENV);
        let conn = conn();
        let now = Utc::now();
        for _ in 0..SESSIONS_BEFORE_PROMPT - 1 {
            note_mcp_session(&conn, None).unwrap();
        }
        assert!(take_due_prompt(&conn, Profile::Coding, now)
            .unwrap()
            .is_none());

        note_mcp_session(&conn, None).unwrap();
        let hint = take_due_prompt(&conn, Profile::Coding, now)
            .unwrap()
            .unwrap();
        assert!(hint["url"].as_str().unwrap().starts_with(DISCUSSION_URL));

        let soon = now + Duration::days(PROMPT_INTERVAL_DAYS - 1);
        assert!(take_due_prompt(&conn, Profile::Coding, soon)
            .unwrap()
            .is_none());
        let later = now + Duration::days(PROMPT_INTERVAL_DAYS);
        assert!(take_due_prompt(&conn, Profile::Coding, later)
            .unwrap()
            .is_some());

        std::env::set_var(DISABLE_ENV, "1");
        let much_later = later + Duration::days(PROMPT_INTERVAL_DAYS);
        assert!(take_due_prompt(&conn, Profile::Coding, much_later)
            .unwrap()
            .is_none());
        std::env::remove_var(DISABLE_ENV);
    }
}
