//! Cursor hook response builders for automatic, model-independent memory use.
//!
//! These power three globally-installed Cursor hooks (see [`crate::install`]):
//!
//! - `sessionStart` — inject the protocol and export the session id.
//! - `userPromptSubmit` (Claude Code / Codex / Gemini / OpenCode) — recall memory relevant to the
//!   user's question before the agent answers it.
//! - `postToolUse` — recall relevant memory right when the agent investigates
//!   (e.g. before/while it greps), so prior decisions surface automatically.
//! - `stop` — when a session did real work but never recorded anything, return
//!   a follow-up message so the agent saves its work before finishing. Claude
//!   Code is judged per session (see [`crate::hook_sessions`]); other clients
//!   use a recent-activity window. Cursor gets no stop reply: it would post it
//!   as a new user turn. The prompt hook reminds it to save instead.
//!
//! The builders are pure functions over a [`Connection`] and the hook input
//! JSON so they can be unit-tested without spawning Cursor. All paths fail open:
//! when in doubt they return an empty object and never block the agent.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::config::PalaceConfig;

/// The agent client a hook is running for. Determines the output dialect:
/// Cursor and OpenCode use flat keys (`additional_context`, `followup_message`),
/// Claude Code, Codex and VS Code share the "Claude-style" `hookSpecificOutput`
/// shape, Gemini CLI uses that shape with its own event names, Cline returns
/// `contextModification`, and Goose can only block a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookClient {
    Cursor,
    Claude,
    Codex,
    Gemini,
    Vscode,
    OpenCode,
    Cline,
    Goose,
}

/// The context-injecting hook events, independent of each client's naming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PostToolUse,
}

impl HookClient {
    /// Parse a `--client` flag value, defaulting to Cursor for back-compat.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "claude" | "claude-code" => Self::Claude,
            "codex" => Self::Codex,
            "gemini" | "gemini-cli" => Self::Gemini,
            "vscode" | "copilot" | "vscode-copilot" => Self::Vscode,
            "opencode" => Self::OpenCode,
            "cline" => Self::Cline,
            "goose" => Self::Goose,
            _ => Self::Cursor,
        }
    }

    /// Whether this client returns context in a `hookSpecificOutput` object.
    fn claude_style(self) -> bool {
        matches!(
            self,
            Self::Claude | Self::Codex | Self::Gemini | Self::Vscode
        )
    }

    /// Whether this client can inject context when the user submits a prompt.
    /// Cursor's `beforeSubmitPrompt`, VS Code's `UserPromptSubmit` and Goose's
    /// hooks cannot.
    fn injects_on_prompt(self) -> bool {
        !matches!(self, Self::Cursor | Self::Vscode | Self::Goose)
    }

    /// Whether this client's Palace calls reach the after-tool hook, so
    /// per-session palace use (and the reminders built on it) is reliable.
    /// Only Claude Code's `mcp__palace__*` tool naming (and Cursor's
    /// `MCP:palace_*` when Cursor runs Claude's hooks) is confirmed so far.
    fn tracks_palace_use(self) -> bool {
        self == Self::Claude
    }

    /// The event name this client expects in `hookSpecificOutput.hookEventName`.
    fn event_name(self, event: HookEvent) -> &'static str {
        match (self, event) {
            (_, HookEvent::SessionStart) => "SessionStart",
            (Self::Gemini, HookEvent::UserPromptSubmit) => "BeforeAgent",
            (Self::Gemini, HookEvent::PostToolUse) => "AfterTool",
            (_, HookEvent::UserPromptSubmit) => "UserPromptSubmit",
            (_, HookEvent::PostToolUse) => "PostToolUse",
        }
    }

    /// Wrap injected context in this client's response shape.
    fn context_output(self, event: HookEvent, context: String) -> Value {
        match self {
            _ if self.claude_style() => json!({
                "hookSpecificOutput": {
                    "hookEventName": self.event_name(event),
                    "additionalContext": context,
                }
            }),
            Self::Cline => json!({ "contextModification": context }),
            // Goose hooks have no way to reach the model.
            Self::Goose => json!({}),
            _ => json!({ "additional_context": context }),
        }
    }

    /// The stop response that makes this client continue with `message`.
    fn continue_output(self, message: &str) -> Value {
        match self {
            Self::Claude | Self::Codex => json!({ "decision": "block", "reason": message }),
            Self::Gemini => json!({ "decision": "deny", "reason": message }),
            Self::Vscode => json!({
                "hookSpecificOutput": {
                    "hookEventName": "Stop",
                    "decision": "block",
                    "reason": message,
                }
            }),
            Self::OpenCode => json!({ "followup_message": message }),
            Self::Goose => json!({ "decision": "block", "reason": message }),
            // Cline cannot continue a task after it completes, and Cursor would
            // post the message as a new user turn.
            Self::Cline | Self::Cursor => json!({}),
        }
    }
}

/// Map a client's hook input onto the Claude-style fields the response
/// builders read (`cwd`, `session_id`, `prompt`, `tool_name`, `tool_input`).
/// Clients that already send those fields pass through unchanged.
pub fn normalize_input(input: Value, client: HookClient) -> Value {
    let mut input = input;
    let set = |input: &mut Value, key: &str, value: Option<Value>| {
        if let (Some(value), Some(object)) = (value, input.as_object_mut()) {
            object.entry(key).or_insert(value);
        }
    };
    match client {
        HookClient::Cline => {
            let cwd = input.pointer("/workspaceRoots/0").cloned();
            let session = input.get("taskId").cloned();
            let prompt = input.pointer("/userPromptSubmit/prompt").cloned();
            let tool_name = input
                .pointer("/postToolUse/toolName")
                .or_else(|| input.pointer("/tool_call/name"))
                .cloned();
            let tool_input = input
                .pointer("/postToolUse/parameters")
                .or_else(|| input.pointer("/tool_call/input"))
                .cloned();
            set(&mut input, "cwd", cwd);
            set(&mut input, "session_id", session);
            set(&mut input, "prompt", prompt);
            set(&mut input, "tool_name", tool_name);
            set(&mut input, "tool_input", tool_input);
        }
        HookClient::Goose => {
            let cwd = input.get("working_dir").cloned();
            let prompt = input.get("message").cloned();
            set(&mut input, "cwd", cwd);
            set(&mut input, "prompt", prompt);
        }
        _ => {}
    }
    input
}

/// Minimum combined similarity for a recalled memory to be worth injecting.
const RECALL_RELEVANCE_THRESHOLD: f64 = 0.4;
/// How many memories to surface in an auto-recall injection.
const RECALL_LIMIT: usize = 3;
/// How many candidates to fetch before dropping other projects' drawers.
const RECALL_CANDIDATES: usize = 12;
/// Window the `stop` hook inspects to decide whether the session saved its work.
const SAVE_WINDOW_MINUTES: i64 = 360;

/// The save-nudge instruction shared by every client's `stop` hook.
const SAVE_NUDGE: &str = "Before finishing: you investigated using Palace memory this session but \
    recorded nothing. Save it so the next agent benefits — call palace_diary_write with what you \
    investigated and decided, and palace_kg_add for any durable facts or decisions (use \
    palace_kg_invalidate first if a fact changed). Then stop.";

/// The nudge for a session that did real work without consulting Palace.
const UNUSED_NUDGE: &str = "Before finishing: this session did real work but did not use Palace \
    memory at all. Record it so the next agent benefits — call palace_diary_write with what you \
    did and decided, and palace_kg_add for any durable facts. Then stop.";

/// A session with at least this many prompts counts as real work.
const BUSY_SESSION_PROMPTS: i64 = 2;
/// A session with at least this many tracked tool calls counts as real work.
const BUSY_SESSION_TOOL_CALLS: i64 = 3;

/// Build the session-start response: inject the protocol AND — when a palace DB
/// is available — the user's standing rules (`general/preferences`, verbatim)
/// plus real warm-start recall (recent diary + project knowledge for the
/// session's cwd) so memory is in context deterministically, without the
/// agent having to call any palace tool. Cursor additionally exports
/// `PALACE_SESSION_ID` so later hooks can correlate the session; Claude/Codex
/// use the `hookSpecificOutput` shape instead. Fail-open: a missing/empty DB
/// just yields the protocol text alone.
pub fn session_start_response(
    conn: Option<&Connection>,
    input: &Value,
    client: HookClient,
) -> Result<String> {
    let mut context = format!(
        "# Palace Memory Protocol — MANDATORY\n\n{}",
        crate::install::RULE_BODY
    );
    if let Some(conn) = conn {
        // Standing rules apply to every project and client, so they go first and
        // do not depend on the cwd.
        if let Some(preferences) = standing_preferences(conn) {
            context.push_str("\n\n");
            context.push_str(&preferences);
        }
        let cwd = input.get("cwd").and_then(Value::as_str).unwrap_or("");
        if let Some(recall) = session_start_recall(conn, cwd) {
            context.push_str("\n\n");
            context.push_str(&recall);
        }
    }
    let mut response = client.context_output(HookEvent::SessionStart, context);
    if client == HookClient::Cursor {
        if let Some(session_id) = input.get("session_id").and_then(Value::as_str) {
            if !session_id.is_empty() {
                response["env"] = json!({ "PALACE_SESSION_ID": session_id });
            }
        }
    }
    Ok(serde_json::to_string(&response)?)
}

/// Build the post-tool-use response: inject relevant memory when the tool looks
/// like an investigation and Palace has something pertinent; otherwise `{}`.
pub fn post_tool_use_response(conn: &Connection, input: &Value, client: HookClient) -> Value {
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    if let Some(session_id) = session_id(input) {
        let _ = crate::hook_sessions::record_tool(conn, session_id, cwd(input), tool);
    }
    // The agent is already consulting Palace; recalling on top of that is noise.
    if crate::hook_sessions::palace_tool(tool).is_some() {
        return json!({});
    }
    let Some(context) = recall_context(conn, input) else {
        return json!({});
    };
    client.context_output(HookEvent::PostToolUse, context)
}

/// Prompts shorter than this many words ("go ahead", "yes") are not searched.
const PROMPT_MIN_WORDS: usize = 3;

/// Phrases that mark a prompt as asking about remembered history, so a miss
/// still earns a nudge to search Palace before answering.
const MEMORY_INTENT_TERMS: &[&str] = &[
    "last time",
    "previous",
    "remember",
    "why did",
    "why do we",
    "decided",
    "decision",
    "we chose",
    "convention",
    "prefer",
    "history",
    "what happened",
];

/// The nudge injected when a history question found nothing in auto-recall.
const PROMPT_SEARCH_NUDGE: &str = "Palace memory: this question looks like it depends on prior \
    decisions or history, and auto-recall found nothing close. Call palace_search (and \
    palace_kg_query for stable facts) before answering from the code or from training data.";

/// Reminder injected into prompts until the session calls any Palace tool.
const PALACE_REMINDER: &str = "Palace not consulted yet this session. Load the palace tools if \
    they are deferred, run the SESSION START calls (palace_status, palace_session_context), and \
    call palace_search before answering questions about past work.";

/// Reminder injected into prompts once a session did real work but saved nothing.
const PROMPT_SAVE_REMINDER: &str = "Palace: this session has done real work but saved nothing \
    yet. Before you finish this task, call palace_diary_write with what you did and decided, and \
    palace_kg_add for any durable facts.";

/// Build the user-prompt-submit response: recall memory relevant to the user's
/// question *before* the agent starts answering, so Palace is consulted even
/// when the agent would otherwise go straight to the code or its own knowledge.
/// A history-style question with no hits gets a one-line search nudge instead.
/// Clients whose prompt hook cannot inject context (Cursor, VS Code) get `{}`.
pub fn user_prompt_submit_response(conn: &Connection, input: &Value, client: HookClient) -> Value {
    if let Some(session_id) = session_id(input) {
        let _ = crate::hook_sessions::record_prompt(conn, session_id, cwd(input));
    }
    if !client.injects_on_prompt() {
        return json!({});
    }
    let Some(prompt) = substantive_prompt(input) else {
        return json!({});
    };
    let parts: Vec<String> = [
        prompt_context(conn, input, prompt),
        palace_reminder(conn, input, client),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        return json!({});
    }
    client.context_output(HookEvent::UserPromptSubmit, parts.join("\n\n"))
}

/// The submitted prompt, or `None` for slash commands and very short prompts.
fn substantive_prompt(input: &Value) -> Option<&str> {
    let prompt = input.get("prompt").and_then(Value::as_str)?.trim();
    if prompt.starts_with('/') || prompt.split_whitespace().count() < PROMPT_MIN_WORDS {
        return None;
    }
    Some(prompt)
}

/// One-line reminder for a session that has not touched Palace yet, or that
/// did real work and saved nothing. `None` otherwise, or when this client's
/// palace calls are not tracked.
fn palace_reminder(conn: &Connection, input: &Value, client: HookClient) -> Option<String> {
    if !client.tracks_palace_use() {
        return None;
    }
    let activity = crate::hook_sessions::activity(conn, session_id(input)?).ok()??;
    if !activity.palace_used {
        return Some(PALACE_REMINDER.to_string());
    }
    (!activity.saved && activity.tool_calls >= BUSY_SESSION_TOOL_CALLS)
        .then(|| PROMPT_SAVE_REMINDER.to_string())
}

/// Recall block (or search nudge) for a substantive prompt, or `None` when
/// the prompt is unrelated to anything remembered.
fn prompt_context(conn: &Connection, input: &Value, prompt: &str) -> Option<String> {
    let lines = recall_lines(conn, prompt, cwd(input));
    if !lines.is_empty() {
        return Some(format!(
            "Palace memory relevant to this request (check it before answering; cite the drawer \
             id if you rely on it):\n{}",
            lines.join("\n")
        ));
    }
    let lower = prompt.to_lowercase();
    MEMORY_INTENT_TERMS
        .iter()
        .any(|term| lower.contains(term))
        .then(|| PROMPT_SEARCH_NUDGE.to_string())
}

/// Compute the recall context string for an investigation tool, or `None` when
/// the tool is not an investigation or Palace has nothing pertinent.
fn recall_context(conn: &Connection, input: &Value) -> Option<String> {
    let query = investigation_query(input)?;
    let cwd = input.get("cwd").and_then(Value::as_str).unwrap_or("");
    let lines = recall_lines(conn, &query, cwd);
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "Palace memory (recall before re-investigating or re-deciding — a prior agent may have \
         already done this):\n{}",
        lines.join("\n")
    ))
}

/// Search Palace for `query` and format the relevant hits as provenance-tagged
/// lines, scoped to the session's project: drawers from other mined projects
/// (and diary entries recorded for them) are dropped as noise, while this
/// project's wing, topic wings, and unscoped diaries are kept.
fn recall_lines(conn: &Connection, query: &str, cwd: &str) -> Vec<String> {
    let results = crate::searcher::search_memories(conn, query, None, None, RECALL_CANDIDATES);
    let Some(hits) = results.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    let scope = ProjectScope::new(conn, cwd);

    let mut lines: Vec<String> = Vec::new();
    for hit in hits {
        if lines.len() >= RECALL_LIMIT {
            break;
        }
        let similarity = hit
            .get("similarity")
            .and_then(Value::as_f64)
            .or_else(|| hit.get("combined").and_then(Value::as_f64))
            .unwrap_or(0.0);
        if similarity < RECALL_RELEVANCE_THRESHOLD {
            continue;
        }
        let text = hit.get("text").and_then(Value::as_str).unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        let id = hit.get("id").and_then(Value::as_str).unwrap_or("");
        let wing = hit.get("wing").and_then(Value::as_str).unwrap_or("");
        let room = hit.get("room").and_then(Value::as_str).unwrap_or("");
        if !scope.allows(conn, id, wing, room, text) {
            continue;
        }
        lines.push(format!("- [{wing}/{room}] {} ({id})", compact(text, 240)));
    }
    lines
}

/// The session's project, used to keep auto-recall from surfacing drawers that
/// belong to a different mined project.
struct ProjectScope {
    /// The wing the session's cwd maps to; `None` without a cwd (no scoping).
    wing: Option<String>,
    /// Wings registered as *other* mined projects.
    foreign: std::collections::HashSet<String>,
}

impl ProjectScope {
    fn new(conn: &Connection, cwd: &str) -> Self {
        let wing = project_wing(conn, cwd);
        let foreign = match &wing {
            Some(current) => crate::store::list_wings_registry(conn)
                .unwrap_or_default()
                .into_iter()
                .filter(|w| w.kind == "project" && &w.name != current)
                .map(|w| w.name)
                .collect(),
            None => Default::default(),
        };
        Self { wing, foreign }
    }

    fn allows(&self, conn: &Connection, id: &str, wing: &str, room: &str, text: &str) -> bool {
        let Some(current) = &self.wing else {
            return true;
        };
        if self.foreign.contains(wing) {
            return false;
        }
        if room != "diary" {
            return true;
        }
        // Diaries live in per-agent wings; scope them by the project they
        // were recorded for (`project_path` metadata, else the AAAK `PROJ:`
        // tag), keeping entries that name no project.
        let project = crate::store::get_drawer(conn, id)
            .ok()
            .flatten()
            .and_then(|d| {
                d.metadata
                    .get("project_path")
                    .and_then(Value::as_str)
                    .filter(|p| !p.trim().is_empty())
                    .map(|p| crate::miner::wing_slug_from_dir(std::path::Path::new(p)))
            })
            .or_else(|| diary_project_tag(text));
        project.is_none_or(|slug| &slug == current)
    }
}

/// The wing slug named by a diary entry's AAAK `PROJ:<name>` tag, if any.
fn diary_project_tag(text: &str) -> Option<String> {
    let start = text.find("PROJ:")? + "PROJ:".len();
    let name: String = text[start..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect();
    (!name.is_empty()).then(|| name.to_lowercase().replace([' ', '-'], "_"))
}

/// The wing the session's cwd maps to (registry, `palace.yaml`, or directory
/// slug), or `None` when the hook input carries no cwd.
fn project_wing(conn: &Connection, cwd: &str) -> Option<String> {
    if cwd.trim().is_empty() {
        return None;
    }
    use crate::miner::ProjectWingStatus;
    match crate::miner::project_wing_status(conn, std::path::Path::new(cwd)).ok()? {
        ProjectWingStatus::Mined { wing, .. }
        | ProjectWingStatus::RegisteredNotMined { wing, .. } => Some(wing),
        ProjectWingStatus::Unknown { suggested_wing, .. } => Some(suggested_wing),
    }
}

/// Where the user's standing rules live: injected verbatim at every session start.
const PREFERENCES_WING: &str = "general";
const PREFERENCES_ROOM: &str = "preferences";
/// Cap on standing rules injected at session start; importance-5 rules are
/// always kept, even past the cap.
const STANDING_PREFERENCES_LIMIT: usize = 20;
/// Upper bound on rows read from the preferences room before ranking.
const STANDING_PREFERENCES_SCAN: usize = 500;

/// Build the standing-instructions block: every drawer in `general/preferences`,
/// verbatim, ranked by importance then recency. `None` when the room is empty.
fn standing_preferences(conn: &Connection) -> Option<String> {
    let mut drawers = crate::store::list_drawers(
        conn,
        &crate::store::DrawerFilter {
            wing: Some(PREFERENCES_WING.to_string()),
            room: Some(PREFERENCES_ROOM.to_string()),
        },
        STANDING_PREFERENCES_SCAN,
    )
    .unwrap_or_default();
    if drawers.is_empty() {
        return None;
    }
    // list_drawers is newest-first; a stable sort keeps that order within a tier.
    drawers.sort_by(|a, b| b.importance.total_cmp(&a.importance));
    let mut kept = 0;
    drawers.retain(|d| {
        kept += 1;
        kept <= STANDING_PREFERENCES_LIMIT || d.importance >= 5.0
    });

    let mut lines = vec![format!(
        "# Standing instructions (override defaults) — from Palace `{PREFERENCES_WING}/{PREFERENCES_ROOM}`\n\
         These are the user's standing rules. They take priority over harness or tool defaults \
         (for example built-in file-memory or commit-attribution instructions)."
    )];
    for d in &drawers {
        lines.push(format!("- ({}) {}", d.id, d.content.trim()));
    }
    Some(lines.join("\n"))
}

/// How many recent diary entries / project drawers to surface at session start.
const SESSION_START_RECALL_LIMIT: usize = 3;
/// How far back the session-start diary lookback reaches (days → hours).
const SESSION_START_DIARY_HOURS: i64 = 24 * 30;

/// Build the warm-start recall block injected at session start, or `None` when
/// there is nothing relevant. Pulls recent diary entries for the session's
/// project (any agent — cross-agent continuity) plus the top drawers of the
/// wing the cwd maps to. Mirrors `palace_session_context` so the agent sees the
/// same warm-start context without having to call the tool.
fn session_start_recall(conn: &Connection, cwd: &str) -> Option<String> {
    if cwd.trim().is_empty() {
        return None;
    }
    let path = std::path::Path::new(cwd);
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let project = canonical.to_string_lossy().to_string();
    let wing = crate::miner::wing_slug_from_dir(&canonical);

    let mut sections: Vec<String> = Vec::new();

    // Recent diary for this project, any agent — the high-value warm-start signal.
    let diary = crate::mcp_server::recent_diary_drawers(
        conn,
        None,
        Some(&project),
        SESSION_START_DIARY_HOURS,
        SESSION_START_RECALL_LIMIT,
    );
    if !diary.is_empty() {
        let mut lines =
            vec!["Recent diary for this project (what prior agents did here):".to_string()];
        for d in &diary {
            let topic = d
                .metadata
                .get("topic")
                .and_then(Value::as_str)
                .unwrap_or("general");
            let date = d.filed_at.get(..10).unwrap_or(&d.filed_at);
            lines.push(format!("- [{date}] ({topic}) {}", compact(&d.content, 220)));
        }
        sections.push(lines.join("\n"));
    }

    // Top drawers of the wing this cwd maps to (project knowledge, if mined).
    let project_drawers = crate::store::list_drawers(
        conn,
        &crate::store::DrawerFilter {
            wing: Some(wing.clone()),
            room: None,
        },
        SESSION_START_RECALL_LIMIT,
    )
    .unwrap_or_default();
    let project_drawers: Vec<_> = project_drawers
        .into_iter()
        .filter(|d| d.room != "diary")
        .collect();
    if !project_drawers.is_empty() {
        let mut lines = vec![format!("Project knowledge (wing `{wing}`):")];
        for d in &project_drawers {
            lines.push(format!("- [{}] {}", d.room, compact(&d.content, 200)));
        }
        sections.push(lines.join("\n"));
    }

    if sections.is_empty() {
        return None;
    }
    Some(format!(
        "# Recalled from Palace memory (this project — consult before re-investigating or \
         re-deciding; a prior agent may have already done this)\n{}",
        sections.join("\n\n")
    ))
}

/// The agent's session id from hook input, if present and non-empty.
fn session_id(input: &Value) -> Option<&str> {
    input
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

/// The session's working directory from hook input, or `""`.
fn cwd(input: &Value) -> &str {
    input.get("cwd").and_then(Value::as_str).unwrap_or("")
}

/// Build the stop response: ask the agent to record its work when the session
/// engaged Palace but saved nothing; otherwise `{}` (no auto-continue).
pub fn stop_response(conn: &Connection, input: &Value, client: HookClient) -> Value {
    if stop_should_skip(input, client) {
        return json!({});
    }
    if let Some(activity) = tracked_activity(conn, input, client) {
        return match session_stop_nudge(&activity) {
            Some(message) => client.continue_output(message),
            None => json!({}),
        };
    }
    match crate::usage::recent_save_status(conn, SAVE_WINDOW_MINUTES) {
        Ok((engaged, saved)) if engaged && !saved => client.continue_output(SAVE_NUDGE),
        _ => json!({}),
    }
}

/// This session's recorded activity, when the client's palace calls are
/// tracked per session; `None` falls back to the global save window.
fn tracked_activity(
    conn: &Connection,
    input: &Value,
    client: HookClient,
) -> Option<crate::hook_sessions::SessionActivity> {
    if !client.tracks_palace_use() {
        return None;
    }
    crate::hook_sessions::activity(conn, session_id(input)?)
        .ok()
        .flatten()
}

/// The nudge for a session that did real work but saved nothing, or `None`
/// for trivial sessions and sessions that saved.
fn session_stop_nudge(activity: &crate::hook_sessions::SessionActivity) -> Option<&'static str> {
    let busy =
        activity.prompts >= BUSY_SESSION_PROMPTS || activity.tool_calls >= BUSY_SESSION_TOOL_CALLS;
    if !busy || activity.saved {
        return None;
    }
    Some(if activity.palace_used {
        SAVE_NUDGE
    } else {
        UNUSED_NUDGE
    })
}

/// Whether the stop hook should stay silent: already continuing from a prior
/// nudge, or — for Cursor — the turn did not complete cleanly.
fn stop_should_skip(input: &Value, client: HookClient) -> bool {
    // Claude-style clients pass `stop_hook_active`; Cursor passes `loop_count`. Either
    // signalling "we already triggered a follow-up" means stop nudging so we
    // never loop forever.
    if input
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    if input.get("loop_count").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return true;
    }
    // Cursor submits a stop reply as a new user turn, derailing the agent, and
    // it also runs other clients' hooks (~/.claude/settings.json).
    client == HookClient::Cursor || input.get("cursor_version").is_some()
}

/// Run a hook end-to-end: read the JSON event from `stdin`, dispatch on the
/// event name, and print the response object to `stdout`. Fail-open: unknown
/// events and errors yield an empty object so the agent is never blocked.
pub fn run(event: &str, client: HookClient) -> Result<()> {
    use std::io::Read;
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok();
    let input: Value = serde_json::from_str(raw.trim()).unwrap_or_else(|_| json!({}));
    let input = normalize_input(input, client);

    let response = match event {
        "session-start" | "sessionStart" | "SessionStart" => {
            // Open the DB for warm-start recall; fail-open to protocol-only.
            let conn = open_palace();
            session_start_response(conn.as_ref(), &input, client)?
        }
        "post-tool-use" | "postToolUse" | "PostToolUse" => {
            serde_json::to_string(&with_db(|conn| {
                post_tool_use_response(conn, &input, client)
            }))?
        }
        "user-prompt-submit" | "userPromptSubmit" | "UserPromptSubmit" => {
            serde_json::to_string(&with_db(|conn| {
                user_prompt_submit_response(conn, &input, client)
            }))?
        }
        "stop" | "Stop" => serde_json::to_string(&with_db(|conn| {
            // Goose sends no `stop_hook_active`, so remember per session that
            // it was already nudged and never block its stop twice.
            if client == HookClient::Goose && goose_already_nudged(&input) {
                return json!({});
            }
            let response = stop_response(conn, &input, client);
            if client == HookClient::Goose && response != json!({}) {
                mark_goose_nudged(&input);
            }
            response
        }))?,
        other => {
            tracing::warn!(event = other, "palace hook: unknown event, ignoring");
            "{}".to_string()
        }
    };
    println!("{response}");
    Ok(())
}

/// Marker file recording that a Goose session already got the save nudge.
fn goose_nudge_marker(input: &Value) -> Option<std::path::PathBuf> {
    let session = input.get("session_id").and_then(Value::as_str)?;
    let safe: String = session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let db_path = PalaceConfig::new().palace_db_path();
    Some(
        db_path
            .parent()?
            .join("hook_state")
            .join(format!("goose-stop-{safe}")),
    )
}

fn goose_already_nudged(input: &Value) -> bool {
    goose_nudge_marker(input).is_some_and(|path| path.exists())
}

fn mark_goose_nudged(input: &Value) {
    if let Some(path) = goose_nudge_marker(input) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, "");
    }
}

/// Open the palace DB and run `f`, returning `{}` if the DB cannot be opened
/// (fail-open: a missing palace must never block the agent).
fn with_db<F>(f: F) -> Value
where
    F: FnOnce(&Connection) -> Value,
{
    match open_palace() {
        Some(conn) => f(&conn),
        None => json!({}),
    }
}

/// Open the palace DB, or `None` if it cannot be opened (fail-open).
fn open_palace() -> Option<Connection> {
    let config = PalaceConfig::new();
    match crate::db::open(&config.palace_db_path()) {
        Ok(conn) => Some(conn),
        Err(err) => {
            tracing::warn!(error = %err, "palace hook: could not open palace, skipping");
            None
        }
    }
}

/// Tool-name fragments that mark a tool as a mutation, not an investigation.
const MUTATING_TOOL_TERMS: &[&str] = &[
    "edit", "write", "replace", "patch", "create", "delete", "todo",
];

/// Extract a search query from an investigation tool's input, or `None` if the
/// tool is not an investigation (so non-search tools stay silent).
fn investigation_query(input: &Value) -> Option<String> {
    // Clients without tool matchers (VS Code) fire for every tool, so skip
    // anything that changes files rather than investigating them.
    let tool_name = input
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    if MUTATING_TOOL_TERMS
        .iter()
        .any(|term| tool_name.contains(term))
    {
        return None;
    }
    let tool_input = input.get("tool_input");
    let candidate = tool_input
        .and_then(|ti| ti.get("pattern").and_then(Value::as_str)) // Grep / Glob
        .or_else(|| tool_input.and_then(|ti| ti.get("query").and_then(Value::as_str)))
        .or_else(|| tool_input.and_then(|ti| ti.get("command").and_then(Value::as_str))) // Codex/Bash shell
        .or_else(|| tool_input.and_then(|ti| ti.get("commands").and_then(Value::as_str))) // Cline run_commands
        .or_else(|| tool_input.and_then(|ti| ti.get("file_path").and_then(Value::as_str))) // Claude Read/Edit
        .or_else(|| tool_input.and_then(|ti| ti.get("filePath").and_then(Value::as_str))) // OpenCode/VS Code read
        .or_else(|| tool_input.and_then(|ti| ti.get("absolute_path").and_then(Value::as_str))) // Gemini read_file
        .or_else(|| tool_input.and_then(|ti| ti.get("path").and_then(Value::as_str))) // Grep/Glob path scope
        .or_else(|| tool_input.and_then(|ti| ti.get("file").and_then(Value::as_str)))
        .or_else(|| tool_input.and_then(|ti| ti.get("paths").and_then(Value::as_str))); // Cline read_files
    let text = candidate?.trim();
    if text.len() < 3 {
        return None;
    }
    Some(text.to_string())
}

fn compact(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(max).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn q(tool_input: Value) -> Option<String> {
        investigation_query(&json!({ "tool_input": tool_input }))
    }

    #[test]
    fn investigation_query_extracts_each_investigation_tool_field() {
        assert_eq!(
            q(json!({"pattern": "retry loop"})).as_deref(),
            Some("retry loop")
        ); // Grep/Glob
        assert_eq!(q(json!({"command": "rg dlq"})).as_deref(), Some("rg dlq")); // Codex/Bash
                                                                                // Regression: Claude's Read passes `file_path`. The installed matcher
                                                                                // fires the hook for Read, so we MUST extract it or recall stays silent.
        assert_eq!(
            q(json!({"file_path": "/proj/src/dlq_consumer.rs"})).as_deref(),
            Some("/proj/src/dlq_consumer.rs")
        );
    }

    #[test]
    fn diary_project_tag_reads_aaak_proj_marker() {
        assert_eq!(
            diary_project_tag("2026-08-05 | PROJ:palace-server-license | User rejected…")
                .as_deref(),
            Some("palace_server_license")
        );
        assert_eq!(
            diary_project_tag("PROJ:mempalace_rs | fixed").as_deref(),
            Some("mempalace_rs")
        );
        assert_eq!(diary_project_tag("no project named here"), None);
    }

    #[test]
    fn investigation_query_silent_for_non_investigation_tools() {
        // No recognised field (e.g. a Write with only `content`) → no recall.
        assert_eq!(q(json!({"content": "hello"})), None);
        // Too-short query is not worth a search.
        assert_eq!(q(json!({"pattern": "x"})), None);
        // Missing tool_input entirely.
        assert_eq!(investigation_query(&json!({})), None);
    }
}
