use anyhow::{anyhow, Context, Result};
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{value, Array, DocumentMut, Item, Table};

// ── Current marker names (0.2.0+) ────────────────────────────────────────────
const RULE_BEGIN: &str = "<!-- BEGIN PALACE -->";
const RULE_END: &str = "<!-- END PALACE -->";
pub const RULE_BODY: &str = "**MANDATORY — three hard triggers, every session, no exceptions.**\n\n## SESSION START\n*Always, before anything else — no skipping.*\n→ `palace_status` → `palace_session_context(agent_name)` → `palace_diary_search` (recent work in this project)\n\n## PROJECT CHECK\n*When you enter a workspace or a new topic comes up.*\n→ `palace_project_status(project_path)`. Mining is ONLY for code repositories (a real folder of files on disk). If a repo is not mined, ASK the user first — accepting runs `palace_mine`, which implicitly initialises and mines the repo in one step. For chats or non-repo topics (Claude/Codex sessions, sales notes — nothing to mine), DON'T mine; offer `palace_create_wing` instead. Never mine silently.\n\n## BEFORE ANSWERING\n*Any question about past decisions, preferences, people, projects, commands, or \"what happened last time?\"*\n→ `palace_search` + `palace_kg_query` — never answer from training data alone.\n- MEMORY-FIRST: call palace_search before grep/file-search for remembered decisions, fixes, conventions, or session history.\n- CODE-SEARCH-FIRST: use grep only for current symbols, exact definitions, or implementation details that may have changed.\n- For preferences/conventions: also call `palace_preference_search`.\n- When Palace answers, cite provenance (wing, room, source file, drawer id).\n\n## AFTER WORK\n*After any substantive task, fix, decision, or discovery.*\n→ `palace_diary_write` (what happened, what you learned, what matters)\n→ `palace_kg_add` for stable facts; `palace_kg_invalidate` + `palace_kg_add` when facts change.\n- To file a key fact: `palace_remember` (importance=5). To delete outdated info: `palace_forget`.\n\n**MEMORY ROUTING**: use Palace for prior decisions, user preferences, previous fixes, commands that worked, project history, and \"what happened last time?\". Use KG for stable facts. Use diary for session continuity. Use code search first only for current source symbols, exact definitions, and implementation details that may have changed.\n\nSkipping any trigger is a protocol violation. Storage is not memory; this protocol is.";

// ── Legacy marker names (0.1.x) — recognized for migration, never written ────
const LEGACY_RULE_BEGIN: &str = "<!-- BEGIN MEMPALACE -->";
const LEGACY_RULE_END: &str = "<!-- END MEMPALACE -->";
const LEGACY_MCP_KEY: &str = "mempalace";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Client {
    Cursor,
    Codex,
    Claude,
    ClaudeDesktop,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    User,
    Project,
}

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub clients: Vec<Client>,
    pub scope: Scope,
    pub project_dir: Option<PathBuf>,
    pub home_dir: PathBuf,
    pub binary_path: PathBuf,
    pub dry_run: bool,
    pub force: bool,
    pub install_rule: bool,
}

#[derive(Clone, Debug, Default)]
pub struct InstallReport {
    pub changed: Vec<PathBuf>,
    pub unchanged: Vec<PathBuf>,
    pub rule_changed: Vec<PathBuf>,
    pub rule_unchanged: Vec<PathBuf>,
    pub hook_changed: Vec<PathBuf>,
    pub hook_unchanged: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientStatus {
    pub client: Client,
    pub path: PathBuf,
    pub configured: bool,
    pub points_to_expected_binary: bool,
    pub command: Option<String>,
    pub rule_path: PathBuf,
    pub rule_installed: bool,
    /// True if the rule file exists but contains stale/legacy content
    /// (e.g. old `mempalace_*` tool names from 0.1.x).
    pub rule_stale: bool,
    /// True if the rule exists but lacks memory-vs-code-search routing guidance.
    pub rule_weak: bool,
    pub hook_installed: bool,
}

#[derive(Clone, Debug)]
pub struct DoctorReport {
    pub binary_path: PathBuf,
    pub palace_db_path: PathBuf,
    pub drawer_count: Option<i64>,
    /// Size of palace.db on disk in bytes (None if DB does not exist).
    pub db_size_bytes: Option<u64>,
    /// Whether the embedding model files are present in the HuggingFace cache.
    pub embedding_model_cached: bool,
    /// Number of drawers whose embedding blob has the wrong byte length.
    pub corrupted_embeddings: usize,
    /// Result of SQLite PRAGMA integrity_check ("ok" = healthy).
    pub db_integrity: Option<String>,
    pub clients: Vec<ClientStatus>,
    pub adoption_warnings: Vec<String>,
}

impl fmt::Display for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Client::Cursor => f.write_str("cursor"),
            Client::Codex => f.write_str("codex"),
            Client::Claude => f.write_str("claude"),
            Client::ClaudeDesktop => f.write_str("claude-desktop"),
            Client::All => f.write_str("all"),
        }
    }
}

impl std::str::FromStr for Client {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "cursor" => Ok(Client::Cursor),
            "codex" => Ok(Client::Codex),
            "claude" | "claude-code" => Ok(Client::Claude),
            "claude-desktop" => Ok(Client::ClaudeDesktop),
            "all" => Ok(Client::All),
            other => Err(anyhow!("unknown MCP client: {other}")),
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::User => f.write_str("user"),
            Scope::Project => f.write_str("project"),
        }
    }
}

impl std::str::FromStr for Scope {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "user" => Ok(Scope::User),
            "project" => Ok(Scope::Project),
            other => Err(anyhow!("unknown install scope: {other}")),
        }
    }
}

impl InstallOptions {
    pub fn for_current_process(
        clients: Vec<Client>,
        scope: Scope,
        project_dir: Option<PathBuf>,
    ) -> Result<Self> {
        Ok(Self {
            clients,
            scope,
            project_dir,
            home_dir: home_dir()?,
            binary_path: std::env::current_exe().context("failed to resolve current executable")?,
            dry_run: false,
            force: false,
            install_rule: true,
        })
    }
}

/// Install the palace MCP server and rules.
///
/// Before writing new entries, this automatically removes any legacy
/// `mempalace` entries left from palace-rs 0.1.x so users get a clean
/// upgrade with no duplicated servers.
pub fn install_clients(options: &InstallOptions) -> Result<InstallReport> {
    let mut report = InstallReport::default();
    for client in expand_clients(&options.clients) {
        let path = config_path(options, client)?;

        let changed = match client {
            Client::Cursor | Client::Claude | Client::ClaudeDesktop => {
                // Remove legacy 0.1.x entries before writing new ones.
                remove_legacy_json_entry(&path, options.dry_run)?;
                write_json_client(&path, &options.binary_path, options.dry_run)?
            }
            Client::Codex => {
                remove_legacy_codex_entry(&path, options.dry_run)?;
                write_codex_client(&path, &options.binary_path, options.dry_run)?
            }
            Client::All => false,
        };
        if changed {
            report.changed.push(path);
        } else {
            report.unchanged.push(path);
        }
        if options.install_rule {
            let target = rule_target(options, client)?;
            let changed = install_rule(&target, options.dry_run)?;
            if changed {
                report.rule_changed.push(target.path);
            } else {
                report.rule_unchanged.push(target.path);
            }
        }

        // Install automatic memory hooks at user scope so recall/save is
        // automatic in every project without per-project rule edits.
        if options.scope == Scope::User {
            if client == Client::Cursor {
                let hooks_json = options.home_dir.join(".cursor").join("hooks.json");
                let changed =
                    install_cursor_hook(&options.home_dir, &options.binary_path, options.dry_run)?;
                if changed {
                    report.hook_changed.push(hooks_json);
                } else {
                    report.hook_unchanged.push(hooks_json);
                }
            } else if let Some((path, changed)) = install_client_hooks(options, client)? {
                if changed {
                    report.hook_changed.push(path);
                } else {
                    report.hook_unchanged.push(path);
                }
            }
        }
    }
    Ok(report)
}

/// Remove the palace MCP server and rules.
///
/// Removes both current (`palace`) and legacy (`mempalace`) entries so
/// either install can be cleaned up with a single command.
pub fn uninstall_clients(options: &InstallOptions) -> Result<InstallReport> {
    let mut report = InstallReport::default();
    for client in expand_clients(&options.clients) {
        let path = config_path(options, client)?;
        let changed = match client {
            Client::Cursor | Client::Claude | Client::ClaudeDesktop => {
                let a = remove_json_client(&path, options.dry_run)?;
                let b = remove_legacy_json_entry(&path, options.dry_run)?;
                a || b
            }
            Client::Codex => {
                let a = remove_codex_client(&path, options.dry_run)?;
                let b = remove_legacy_codex_entry(&path, options.dry_run)?;
                a || b
            }
            Client::All => false,
        };
        if changed {
            report.changed.push(path);
        } else {
            report.unchanged.push(path);
        }
        if options.install_rule {
            let target = rule_target(options, client)?;
            let changed = uninstall_rule(&target, options.dry_run)?;
            if changed {
                report.rule_changed.push(target.path);
            } else {
                report.rule_unchanged.push(target.path);
            }
        }

        if options.scope == Scope::User {
            if client == Client::Cursor {
                let hooks_json = options.home_dir.join(".cursor").join("hooks.json");
                let changed = uninstall_cursor_hook(&options.home_dir, options.dry_run)?;
                if changed {
                    report.hook_changed.push(hooks_json);
                } else {
                    report.hook_unchanged.push(hooks_json);
                }
            } else if let Some((path, changed)) = uninstall_client_hooks(options, client)? {
                if changed {
                    report.hook_changed.push(path);
                } else {
                    report.hook_unchanged.push(path);
                }
            }
        }
    }
    Ok(report)
}

pub fn doctor(options: &InstallOptions) -> Result<DoctorReport> {
    let config = crate::config::PalaceConfig::new();
    let palace_db_path = config.palace_db_path();

    let db_size_bytes = palace_db_path.metadata().map(|m| m.len()).ok();

    let (drawer_count, corrupted_embeddings, db_integrity) = if palace_db_path.exists() {
        match crate::db::open(&palace_db_path) {
            Ok(conn) => {
                let count = crate::store::count_drawers(&conn).ok();
                let corrupted = count_corrupted_embeddings(&conn);
                let integrity = conn
                    .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                    .ok();
                (count, corrupted, integrity)
            }
            Err(_) => (None, 0, Some("error opening database".to_string())),
        }
    } else {
        (None, 0, None)
    };

    let embedding_model_cached = check_embedding_model_cached();

    let mut clients = Vec::new();
    for client in expand_clients(&options.clients) {
        let path = config_path(options, client)?;
        let target = rule_target(options, client)?;
        let command = read_configured_command(client, &path)?;
        let expected = path_to_string(&options.binary_path);
        let rule_installed = rule_installed(&target)?;
        let rule_stale = rule_is_stale(&target)?;
        let rule_weak = rule_is_weak(&target)?;
        let hook_installed = if options.scope == Scope::User {
            if client == Client::Cursor {
                cursor_hook_installed(&options.home_dir)
            } else if let Some((path, _, _)) = nested_hook_target(&options.home_dir, client) {
                nested_hook_installed(&path)
            } else {
                false
            }
        } else {
            false
        };
        clients.push(ClientStatus {
            client,
            path,
            configured: command.is_some(),
            points_to_expected_binary: command.as_deref() == Some(expected.as_str()),
            command,
            rule_path: target.path,
            rule_installed,
            rule_stale,
            rule_weak,
            hook_installed,
        });
    }
    let adoption_warnings = adoption_warnings(&clients);

    Ok(DoctorReport {
        binary_path: options.binary_path.clone(),
        palace_db_path,
        drawer_count,
        db_size_bytes,
        embedding_model_cached,
        corrupted_embeddings,
        db_integrity,
        clients,
        adoption_warnings,
    })
}

/// Count drawers whose embedding blob has the wrong number of bytes.
fn count_corrupted_embeddings(conn: &rusqlite::Connection) -> usize {
    let expected_bytes = (crate::embedder::EMBEDDING_DIM * std::mem::size_of::<f32>()) as i64;
    conn.query_row(
        "SELECT COUNT(*) FROM drawers WHERE embedding IS NOT NULL AND length(embedding) != ?1",
        rusqlite::params![expected_bytes],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0) as usize
}

/// Check whether the ONNX model file is present in the HuggingFace cache.
fn check_embedding_model_cached() -> bool {
    let cache_base = if let Ok(p) = std::env::var("HF_HUB_CACHE") {
        std::path::PathBuf::from(p)
    } else if let Some(home) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
        home.join(".cache").join("huggingface").join("hub")
    } else {
        return false;
    };

    // Model is stored as models--Qdrant--all-MiniLM-L6-v2-onnx inside the hub cache.
    let model_dir = cache_base.join("models--Qdrant--all-MiniLM-L6-v2-onnx");
    model_dir.exists()
}

pub fn print_install_report(action: &str, report: &InstallReport) {
    for path in &report.changed {
        println!("  {action}: {}", path.display());
    }
    for path in &report.unchanged {
        println!("  unchanged: {}", path.display());
    }
    for path in &report.rule_changed {
        println!("  rule {action}: {}", path.display());
    }
    for path in &report.rule_unchanged {
        println!("  rule unchanged: {}", path.display());
    }
    for path in &report.hook_changed {
        println!("  hook {action}: {}", path.display());
    }
    for path in &report.hook_unchanged {
        println!("  hook unchanged: {}", path.display());
    }
    // Codex requires a one-time interactive trust review before newly written
    // hooks will run, so call it out explicitly.
    let codex_hooks_touched = report
        .hook_changed
        .iter()
        .chain(report.hook_unchanged.iter())
        .any(|p| p.ends_with("hooks.json") && p.components().any(|c| c.as_os_str() == ".codex"));
    if codex_hooks_touched {
        println!("  Codex: run `/hooks` in Codex once to review and trust the Palace hooks.");
    }
}

pub fn print_doctor_report(report: &DoctorReport) {
    println!("\n  Palace Doctor");
    println!("  {}", "─".repeat(52));
    println!("  Binary:   {}", report.binary_path.display());
    println!("  Database: {}", report.palace_db_path.display());

    match report.drawer_count {
        Some(count) => {
            let size_str = report
                .db_size_bytes
                .map(|b| format!(" ({:.1} MB)", b as f64 / 1_048_576.0))
                .unwrap_or_default();
            println!("  Drawers:  {count}{size_str}");
        }
        None => println!("  Drawers:  no palace database found yet"),
    }

    let integrity_ok = report
        .db_integrity
        .as_deref()
        .map(|s| s == "ok")
        .unwrap_or(false);
    if report.palace_db_path.exists() {
        let integrity_str = if integrity_ok {
            "ok"
        } else {
            report.db_integrity.as_deref().unwrap_or("not checked")
        };
        println!("  Integrity:{integrity_str:>5}");

        if report.corrupted_embeddings > 0 {
            println!(
                "  ⚠ {} drawer(s) have corrupt embeddings — run: palace repair",
                report.corrupted_embeddings
            );
        }
    }

    let model_state = if report.embedding_model_cached {
        "cached"
    } else {
        "not cached (will download on first use)"
    };
    println!("  Model:    all-MiniLM-L6-v2 — {model_state}");

    println!("  {}", "─".repeat(52));
    for status in &report.clients {
        let state = if status.points_to_expected_binary {
            "✓ configured"
        } else if status.configured {
            "⚠ configured elsewhere"
        } else {
            "✗ missing"
        };
        println!("  {:<16} {state}", format!("{}:", status.client));
        println!("      config: {}", status.path.display());
        let rule_state = if status.rule_stale {
            "⚠ rule stale — run: palace install"
        } else if status.rule_weak {
            "⚠ rule weak — run: palace install so agents search memory before grep"
        } else if status.rule_installed {
            "✓ rule installed"
        } else {
            "✗ rule missing — run: palace install"
        };
        println!("      {rule_state}");
        if matches!(
            status.client,
            Client::Cursor | Client::Claude | Client::Codex
        ) {
            let hook_state = if status.hook_installed {
                "✓ memory hooks installed (session-start, post-tool-use recall, stop save)"
            } else {
                "✗ memory hooks missing — run: palace install"
            };
            println!("      {hook_state}");
            if status.client == Client::Codex && status.hook_installed {
                println!("      ↳ run `/hooks` in Codex once to trust the Palace hooks");
            }
        }
    }
    println!("  {}", "─".repeat(52));
    if report.drawer_count.unwrap_or(0) == 0 {
        println!("  Next: palace init <project> && palace mine <project>");
    }
    for warning in &report.adoption_warnings {
        println!("  ⚠ {warning}");
    }
    println!();
}

fn expand_clients(clients: &[Client]) -> Vec<Client> {
    if clients.is_empty() || clients.contains(&Client::All) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        return vec![
            Client::Cursor,
            Client::Codex,
            Client::Claude,
            Client::ClaudeDesktop,
        ];
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return vec![Client::Cursor, Client::Codex, Client::Claude];
    } else {
        clients
            .iter()
            .copied()
            .filter(|client| *client != Client::All)
            .collect()
    }
}

fn config_path(options: &InstallOptions, client: Client) -> Result<PathBuf> {
    match client {
        Client::Cursor => match options.scope {
            Scope::User => Ok(options.home_dir.join(".cursor").join("mcp.json")),
            Scope::Project => {
                let project_dir = options.project_dir.as_ref().ok_or_else(|| {
                    anyhow!("--path is required for project-scope Cursor installs")
                })?;
                Ok(project_dir.join(".cursor").join("mcp.json"))
            }
        },
        Client::Codex => Ok(options.home_dir.join(".codex").join("config.toml")),
        // Claude Code CLI reads from ~/.claude.json (top-level file, not ~/.claude/ directory)
        Client::Claude => Ok(options.home_dir.join(".claude.json")),
        Client::ClaudeDesktop => claude_desktop_config_path(&options.home_dir),
        Client::All => Err(anyhow!("all is not a concrete client")),
    }
}

fn claude_desktop_config_path(_home_dir: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(_home_dir.join("Library/Application Support/Claude/claude_desktop_config.json"))
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var_os("APPDATA").ok_or_else(|| anyhow!("APPDATA not set"))?;
        Ok(PathBuf::from(appdata)
            .join("Claude")
            .join("claude_desktop_config.json"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(anyhow!("claude-desktop is not supported on this platform"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuleKind {
    Standalone,
    ManagedBlock,
}

#[derive(Clone, Debug)]
struct RuleTarget {
    path: PathBuf,
    kind: RuleKind,
}

fn rule_target(options: &InstallOptions, client: Client) -> Result<RuleTarget> {
    let project_dir = || {
        options
            .project_dir
            .as_ref()
            .ok_or_else(|| anyhow!("--path is required for project-scope rule installs"))
    };
    let path = match (client, options.scope) {
        (Client::Cursor, Scope::User) => options.home_dir.join(".cursor/rules/palace.mdc"),
        (Client::Cursor, Scope::Project) => project_dir()?.join(".cursor/rules/palace.mdc"),
        (Client::Codex, Scope::User) => options.home_dir.join(".codex/AGENTS.md"),
        (Client::Codex, Scope::Project) => project_dir()?.join("AGENTS.md"),
        (Client::Claude, Scope::User) => options.home_dir.join(".claude/CLAUDE.md"),
        (Client::Claude, Scope::Project) => project_dir()?.join("CLAUDE.md"),
        // Claude Desktop has no rules/prompts file to inject into
        (Client::ClaudeDesktop, _) => options.home_dir.join(".claude/CLAUDE.md"),
        (Client::All, _) => return Err(anyhow!("all is not a concrete client")),
    };
    let kind = match client {
        Client::Cursor => RuleKind::Standalone,
        Client::Codex | Client::Claude | Client::ClaudeDesktop => RuleKind::ManagedBlock,
        Client::All => return Err(anyhow!("all is not a concrete client")),
    };
    Ok(RuleTarget { path, kind })
}

fn home_dir() -> Result<PathBuf> {
    // Prefer the `directories` crate which uses platform-native resolution
    // (SHGetKnownFolderPath on Windows, $HOME on Unix). This is reliable on
    // Windows where $HOME is often unset.
    if let Some(dirs) = UserDirs::new() {
        return Ok(dirs.home_dir().to_path_buf());
    }
    // Fallback for unusual environments (containers, CI overrides).
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(profile));
    }
    Err(anyhow!("could not determine home directory"))
}

fn write_json_client(path: &Path, binary_path: &Path, dry_run: bool) -> Result<bool> {
    let existing = read_json_config(path)?;
    let mut next = existing.clone();
    ensure_json_server(&mut next, binary_path)?;
    write_if_changed(path, existing, next, dry_run)
}

fn remove_json_client(path: &Path, dry_run: bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let existing = read_json_config(path)?;
    let mut next = existing.clone();
    let Some(servers) = next.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    if servers.remove("palace").is_none() {
        return Ok(false);
    }
    write_if_changed(path, existing, next, dry_run)
}

/// Remove the legacy `mempalace` entry from a JSON MCP config (0.1.x cleanup).
fn remove_legacy_json_entry(path: &Path, dry_run: bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let existing = read_json_config(path)?;
    let mut next = existing.clone();
    let Some(servers) = next.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    if servers.remove(LEGACY_MCP_KEY).is_none() {
        return Ok(false);
    }
    write_if_changed(path, existing, next, dry_run)
}

fn read_json_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

fn ensure_json_server(config: &mut Value, binary_path: &Path) -> Result<()> {
    if !config.is_object() {
        *config = json!({});
    }
    let object = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("JSON config root is not an object"))?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("mcpServers must be a JSON object"))?;
    servers.insert(
        "palace".to_string(),
        json!({
            "command": path_to_string(binary_path),
            "args": ["mcp"],
        }),
    );
    Ok(())
}

fn write_if_changed(path: &Path, existing: Value, next: Value, dry_run: bool) -> Result<bool> {
    if existing == next {
        return Ok(false);
    }
    if dry_run {
        return Ok(true);
    }
    backup_existing(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("config path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let text = serde_json::to_string_pretty(&next)?;
    fs::write(path, format!("{text}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

fn write_codex_client(path: &Path, binary_path: &Path, dry_run: bool) -> Result<bool> {
    let existing = read_toml_document(path)?;
    let mut next = existing.clone();
    ensure_codex_server(&mut next, binary_path);
    write_toml_if_changed(path, &existing, &next, dry_run)
}

fn remove_codex_client(path: &Path, dry_run: bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let existing = read_toml_document(path)?;
    let mut next = existing.clone();
    let Some(servers) = next
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
    else {
        return Ok(false);
    };
    if servers.remove("palace").is_none() {
        return Ok(false);
    }
    write_toml_if_changed(path, &existing, &next, dry_run)
}

/// Remove the legacy `mempalace` entry from Codex config.toml (0.1.x cleanup).
fn remove_legacy_codex_entry(path: &Path, dry_run: bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let existing = read_toml_document(path)?;
    let mut next = existing.clone();
    let Some(servers) = next
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
    else {
        return Ok(false);
    };
    if servers.remove(LEGACY_MCP_KEY).is_none() {
        return Ok(false);
    }
    write_toml_if_changed(path, &existing, &next, dry_run)
}

fn read_toml_document(path: &Path) -> Result<DocumentMut> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    text.parse::<DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))
}

fn ensure_codex_server(document: &mut DocumentMut, binary_path: &Path) {
    if !document.contains_key("mcp_servers") || !document["mcp_servers"].is_table_like() {
        document["mcp_servers"] = Item::Table(Table::new());
    }

    let mut server = Table::new();
    server["command"] = value(path_to_string(binary_path));
    let mut args = Array::new();
    args.push("mcp");
    server["args"] = value(args);
    document["mcp_servers"]["palace"] = Item::Table(server);
}

fn write_toml_if_changed(
    path: &Path,
    existing: &DocumentMut,
    next: &DocumentMut,
    dry_run: bool,
) -> Result<bool> {
    if existing.to_string() == next.to_string() {
        return Ok(false);
    }
    if dry_run {
        return Ok(true);
    }
    backup_existing(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("config path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    fs::write(path, next.to_string())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

fn install_rule(target: &RuleTarget, dry_run: bool) -> Result<bool> {
    let original = read_text_file(&target.path)?;
    // Migrate any legacy block to the new markers before upserting.
    let migrated = migrate_legacy_rule_block(&original)?;
    let next = match target.kind {
        RuleKind::Standalone => cursor_rule_text(),
        RuleKind::ManagedBlock => upsert_managed_rule(&migrated)?,
    };
    // Compare against the original file content so that a legacy-only migration
    // (where migrated == next) still triggers a write.
    write_text_if_changed(&target.path, &original, &next, dry_run)
}

fn uninstall_rule(target: &RuleTarget, dry_run: bool) -> Result<bool> {
    if !target.path.exists() {
        return Ok(false);
    }
    let existing = read_text_file(&target.path)?;
    // Remove both the current and legacy blocks.
    let without_legacy = remove_legacy_rule_block(&existing)?;
    let next = match target.kind {
        RuleKind::Standalone => String::new(),
        RuleKind::ManagedBlock => remove_managed_rule(&without_legacy)?,
    };
    if target.kind == RuleKind::Standalone {
        if dry_run {
            return Ok(true);
        }
        backup_existing(&target.path)?;
        fs::remove_file(&target.path)
            .with_context(|| format!("failed to remove {}", target.path.display()))?;
        return Ok(true);
    }
    write_text_if_changed(&target.path, &existing, &next, dry_run)
}

fn rule_installed(target: &RuleTarget) -> Result<bool> {
    if !target.path.exists() {
        return Ok(false);
    }
    let text = read_text_file(&target.path)?;
    Ok(match target.kind {
        RuleKind::Standalone => text == cursor_rule_text(),
        RuleKind::ManagedBlock => find_managed_block(&text)?.is_some(),
    })
}

/// True if the rule file exists and contains legacy `mempalace_*` tool names
/// but not the current `palace_*` names — i.e. needs `palace install` to refresh.
fn rule_is_stale(target: &RuleTarget) -> Result<bool> {
    if !target.path.exists() {
        return Ok(false);
    }
    let text = read_text_file(&target.path)?;
    let has_legacy = text.contains(LEGACY_RULE_BEGIN) || text.contains("mempalace_status");
    let has_current = match target.kind {
        RuleKind::Standalone => text == cursor_rule_text(),
        RuleKind::ManagedBlock => find_managed_block(&text)?.is_some(),
    };
    Ok(has_legacy && !has_current)
}

fn rule_is_weak(target: &RuleTarget) -> Result<bool> {
    if !target.path.exists() {
        return Ok(false);
    }
    let text = read_text_file(&target.path)?;
    if find_managed_block(&text)?.is_none() && target.kind == RuleKind::ManagedBlock {
        return Ok(false);
    }
    let lower = text.to_lowercase();
    let required = [
        "session start",
        "palace_status",
        "palace_session_context",
        "palace_diary_search",
        "before answering",
        "palace_search",
        "palace_kg_query",
        "after work",
        "palace_diary_write",
        "palace_kg_add",
        "palace_kg_invalidate",
        "memory routing",
    ];
    Ok(required.iter().any(|needle| !lower.contains(needle)))
}

fn adoption_warnings(clients: &[ClientStatus]) -> Vec<String> {
    let mut warnings = Vec::new();
    for status in clients {
        let client = display_client_name(status.client);
        if !status.configured {
            warnings.push(format!(
                "{client} is missing MCP config, so it cannot call Palace before grep."
            ));
        }
        if status.configured && !status.rule_installed {
            warnings.push(format!(
                "{client} has MCP config but no Palace rule file, so agents may skip memory."
            ));
        }
        if status.rule_stale {
            warnings.push(format!(
                "{client} has a stale Palace rule; reinstall so it uses current memory tools."
            ));
        } else if status.rule_weak {
            warnings.push(format!(
                "{client} rule lacks session warm-start, KG/diary recall, or memory-vs-code routing; reinstall so remembered context is searched first."
            ));
        }
    }
    warnings
}

fn display_client_name(client: Client) -> &'static str {
    match client {
        Client::Cursor => "Cursor",
        Client::Codex => "Codex",
        Client::Claude => "Claude Code",
        Client::ClaudeDesktop => "Claude Desktop",
        Client::All => "all",
    }
}

fn read_text_file(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

fn write_text_if_changed(path: &Path, existing: &str, next: &str, dry_run: bool) -> Result<bool> {
    if existing == next {
        return Ok(false);
    }
    if dry_run {
        return Ok(true);
    }
    backup_existing(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("rule path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    fs::write(path, next).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

fn cursor_rule_text() -> String {
    format!(
        "---\ndescription: Consult Palace memory before answering about remembered facts\nalwaysApply: true\n---\n\n# Palace Memory Protocol — MANDATORY\n\n{RULE_BODY}\n"
    )
}

fn managed_rule_block() -> String {
    format!("{RULE_BEGIN}\n# Palace Memory Protocol — MANDATORY\n\n{RULE_BODY}\n{RULE_END}")
}

fn upsert_managed_rule(existing: &str) -> Result<String> {
    let block = managed_rule_block();
    if let Some((start, end)) = find_managed_block(existing)? {
        let mut next = String::with_capacity(existing.len() + block.len());
        next.push_str(&existing[..start]);
        next.push_str(&block);
        next.push_str(&existing[end..]);
        return Ok(next);
    }

    if existing.is_empty() {
        return Ok(format!("{block}\n"));
    }

    let separator = if existing.ends_with("\n\n") {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    Ok(format!("{existing}{separator}{block}\n"))
}

fn remove_managed_rule(existing: &str) -> Result<String> {
    let Some((start, end)) = find_managed_block(existing)? else {
        return Ok(existing.to_string());
    };
    let mut next = String::with_capacity(existing.len());
    next.push_str(&existing[..start]);
    next.push_str(&existing[end..]);
    while next.contains("\n\n\n") {
        next = next.replace("\n\n\n", "\n\n");
    }
    Ok(next)
}

fn find_managed_block(text: &str) -> Result<Option<(usize, usize)>> {
    let Some(start) = text.find(RULE_BEGIN) else {
        return Ok(None);
    };
    let search_from = start + RULE_BEGIN.len();
    let end_relative = text[search_from..]
        .find(RULE_END)
        .ok_or_else(|| anyhow!("managed Palace rule block is missing end marker"))?;
    let end = search_from + end_relative + RULE_END.len();
    Ok(Some((start, end)))
}

/// Replace a legacy `<!-- BEGIN MEMPALACE -->…<!-- END MEMPALACE -->` block
/// with the new `<!-- BEGIN PALACE -->…<!-- END PALACE -->` block.
fn migrate_legacy_rule_block(text: &str) -> Result<String> {
    let Some(start) = text.find(LEGACY_RULE_BEGIN) else {
        return Ok(text.to_string());
    };
    let search_from = start + LEGACY_RULE_BEGIN.len();
    let end_relative = text[search_from..]
        .find(LEGACY_RULE_END)
        .ok_or_else(|| anyhow!("legacy Palace rule block is missing end marker"))?;
    let block_end = search_from + end_relative + LEGACY_RULE_END.len();

    // Build a new string with the legacy block replaced by the current block.
    let mut result = String::with_capacity(text.len() + managed_rule_block().len());
    result.push_str(&text[..start]);
    result.push_str(&managed_rule_block());
    result.push_str(&text[block_end..]);
    Ok(result)
}

/// Remove a legacy `<!-- BEGIN MEMPALACE -->…<!-- END MEMPALACE -->` block.
fn remove_legacy_rule_block(text: &str) -> Result<String> {
    let Some(start) = text.find(LEGACY_RULE_BEGIN) else {
        return Ok(text.to_string());
    };
    let search_from = start + LEGACY_RULE_BEGIN.len();
    let end_relative = text[search_from..]
        .find(LEGACY_RULE_END)
        .ok_or_else(|| anyhow!("legacy Palace rule block is missing end marker"))?;
    let block_end = search_from + end_relative + LEGACY_RULE_END.len();

    let mut result = String::with_capacity(text.len());
    result.push_str(&text[..start]);
    result.push_str(&text[block_end..]);
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    Ok(result)
}

fn backup_existing(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let backup = path.with_file_name(format!(
        "{}.bak",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("invalid config filename: {}", path.display()))?
    ));
    if !backup.exists() {
        fs::copy(path, &backup).with_context(|| {
            format!(
                "failed to back up {} to {}",
                path.display(),
                backup.display()
            )
        })?;
    }
    Ok(())
}

fn read_configured_command(client: Client, path: &Path) -> Result<Option<String>> {
    match client {
        Client::Cursor | Client::Claude | Client::ClaudeDesktop => {
            if !path.exists() {
                return Ok(None);
            }
            let config = read_json_config(path)?;
            Ok(config
                .get("mcpServers")
                .and_then(|servers| servers.get("palace"))
                .and_then(|server| server.get("command"))
                .and_then(Value::as_str)
                .map(String::from))
        }
        Client::Codex => {
            if !path.exists() {
                return Ok(None);
            }
            let config = read_toml_document(path)?;
            Ok(config
                .get("mcp_servers")
                .and_then(|servers| servers.get("palace"))
                .and_then(|server| server.get("command"))
                .and_then(Item::as_str)
                .map(String::from))
        }
        Client::All => Ok(None),
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// ── Cursor hooks.json integration ────────────────────────────────────────────

const CURSOR_HOOK_KEY: &str = "palace";

/// Install or update the palace `sessionStart` entry in `~/.cursor/hooks.json`.
///
/// The hook script outputs `{"additional_context": "<palace protocol>"}` at
/// the start of every Cursor conversation, giving every workspace automatic
/// palace coverage without any manual User Rules setup.
/// A Cursor hook that Palace installs at user scope so memory use is automatic
/// in every project without per-project `AGENTS.md` edits.
struct CursorHook {
    /// hooks.json event key (e.g. "sessionStart").
    event: &'static str,
    /// CLI sub-event passed to `palace hook <cli_event>`.
    cli_event: &'static str,
    /// Script filename stem under `~/.cursor/hooks/`.
    script_stem: &'static str,
    /// Optional tool matcher (postToolUse fires only for these tools).
    matcher: Option<&'static str>,
    /// Optional auto-continue loop cap (used by `stop`).
    loop_limit: Option<i64>,
}

/// The Cursor hooks Palace manages:
/// - `sessionStart`: inject protocol + export session id.
/// - `postToolUse`: auto-recall relevant memory when the agent investigates.
/// - `stop`: nudge the agent to record its work if it engaged Palace but saved nothing.
fn cursor_hooks() -> &'static [CursorHook] {
    &[
        CursorHook {
            event: "sessionStart",
            cli_event: "session-start",
            script_stem: "palace-session-start",
            matcher: None,
            loop_limit: None,
        },
        CursorHook {
            event: "postToolUse",
            cli_event: "post-tool-use",
            script_stem: "palace-post-tool-use",
            matcher: Some("Grep|Read"),
            loop_limit: None,
        },
        CursorHook {
            event: "stop",
            cli_event: "stop",
            script_stem: "palace-stop",
            matcher: None,
            loop_limit: Some(1),
        },
    ]
}

pub fn install_cursor_hook(home_dir: &Path, binary_path: &Path, dry_run: bool) -> Result<bool> {
    let hooks_dir = home_dir.join(".cursor").join("hooks");
    let hooks_json = home_dir.join(".cursor").join("hooks.json");

    let mut entries: Vec<(&CursorHook, String)> = Vec::new();
    for hook in cursor_hooks() {
        let script_path = write_hook_script(&hooks_dir, binary_path, hook, dry_run)?;
        let script_rel = format!(
            "./hooks/{}",
            script_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        entries.push((hook, script_rel));
    }
    let changed = upsert_hooks_json(&hooks_json, &entries, dry_run)?;
    Ok(changed)
}

/// Remove the palace `sessionStart` hook from `~/.cursor/hooks.json` and
/// delete the hook script.
pub fn uninstall_cursor_hook(home_dir: &Path, dry_run: bool) -> Result<bool> {
    let hooks_json = home_dir.join(".cursor").join("hooks.json");
    let changed = remove_hook_entry(&hooks_json, dry_run)?;

    let hooks_dir = home_dir.join(".cursor").join("hooks");
    for name in hook_script_names() {
        let path = hooks_dir.join(&name);
        if path.exists() && !dry_run {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }
    Ok(changed)
}

/// True if the palace sessionStart hook is registered in hooks.json.
pub fn cursor_hook_installed(home_dir: &Path) -> bool {
    let hooks_json = home_dir.join(".cursor").join("hooks.json");
    read_hooks_json(&hooks_json)
        .ok()
        .and_then(|v| {
            v.get("hooks")
                .and_then(|h| h.get("sessionStart"))
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter().any(|e| {
                        e.get("command")
                            .and_then(Value::as_str)
                            .map(|c| c.contains(CURSOR_HOOK_KEY))
                            .unwrap_or(false)
                    })
                })
        })
        .unwrap_or(false)
}

/// Build the JSON response for a `sessionStart` hook call.
///
/// Returns a JSON string ready to be written to stdout by the hook script.
/// Public so tests can assert the exact content without spawning a subprocess.
pub fn session_start_hook_response() -> Result<String> {
    let context = format!("# Palace Memory Protocol — MANDATORY\n\n{RULE_BODY}");
    let response = json!({ "additional_context": context });
    Ok(serde_json::to_string(&response)?)
}

/// Handle a hook event received on stdin and write the response to stdout.
/// Called as `palace hook session-start --client <cursor|claude|codex>` by the
/// per-client hook scripts/commands. `client` selects the output dialect.
pub fn run_hook(event: &str, client: &str) -> Result<()> {
    crate::hooks::run(event, crate::hooks::HookClient::parse(client))
}

fn hook_script_names() -> Vec<String> {
    cursor_hooks()
        .iter()
        .flat_map(|hook| {
            [
                format!("{}.bat", hook.script_stem),
                format!("{}.sh", hook.script_stem),
                hook.script_stem.to_string(),
            ]
        })
        .collect()
}

fn write_hook_script(
    hooks_dir: &Path,
    binary_path: &Path,
    hook: &CursorHook,
    dry_run: bool,
) -> Result<std::path::PathBuf> {
    let binary_str = path_to_string(binary_path);
    let cli_event = hook.cli_event;

    #[cfg(windows)]
    let (filename, content) = (
        format!("{}.bat", hook.script_stem),
        format!("@echo off\r\n\"{binary_str}\" hook {cli_event}\r\n"),
    );
    #[cfg(not(windows))]
    let (filename, content) = (
        format!("{}.sh", hook.script_stem),
        format!("#!/bin/sh\n\"{binary_str}\" hook {cli_event}\n"),
    );

    let script_path = hooks_dir.join(&filename);

    if dry_run {
        return Ok(script_path);
    }

    fs::create_dir_all(hooks_dir)
        .with_context(|| format!("failed to create {}", hooks_dir.display()))?;

    let existing = if script_path.exists() {
        fs::read_to_string(&script_path).unwrap_or_default()
    } else {
        String::new()
    };

    if existing != content {
        fs::write(&script_path, &content)
            .with_context(|| format!("failed to write {}", script_path.display()))?;

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms)?;
        }
    }

    Ok(script_path)
}

fn read_hooks_json(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({ "version": 1, "hooks": {} }));
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(json!({ "version": 1, "hooks": {} }));
    }
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

fn is_palace_entry(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .map(|c| c.contains(CURSOR_HOOK_KEY))
        .unwrap_or(false)
}

/// Build the desired hooks.json entry for a Palace hook.
fn hook_entry(hook: &CursorHook, script_command: &str) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("command".to_string(), json!(script_command));
    if let Some(matcher) = hook.matcher {
        obj.insert("matcher".to_string(), json!(matcher));
    }
    if let Some(loop_limit) = hook.loop_limit {
        obj.insert("loop_limit".to_string(), json!(loop_limit));
    }
    Value::Object(obj)
}

fn upsert_hooks_json(
    path: &Path,
    entries: &[(&CursorHook, String)],
    dry_run: bool,
) -> Result<bool> {
    let existing = read_hooks_json(path)?;
    let mut next = existing.clone();

    {
        let hooks = next
            .as_object_mut()
            .ok_or_else(|| anyhow!("hooks.json root is not an object"))?
            .entry("hooks")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow!("hooks.json 'hooks' is not an object"))?;

        for (hook, script_command) in entries {
            let arr = hooks
                .entry(hook.event)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or_else(|| anyhow!("hooks.{} is not an array", hook.event))?;

            let desired = hook_entry(hook, script_command);
            // Replace the existing palace entry (binary path / matcher may have
            // changed) or append a fresh one, leaving unrelated entries intact.
            if let Some(slot) = arr.iter_mut().find(|e| is_palace_entry(e)) {
                *slot = desired;
            } else {
                arr.push(desired);
            }
        }
    }

    if existing == next {
        return Ok(false);
    }

    if dry_run {
        return Ok(true);
    }

    backup_existing(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("hooks.json path has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let text = serde_json::to_string_pretty(&next)?;
    fs::write(path, format!("{text}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

fn remove_hook_entry(path: &Path, dry_run: bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let existing = read_hooks_json(path)?;
    let mut next = existing.clone();

    let Some(hooks) = next.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(false);
    };

    let mut removed_any = false;
    for hook in cursor_hooks() {
        if let Some(arr) = hooks.get_mut(hook.event).and_then(Value::as_array_mut) {
            let before = arr.len();
            arr.retain(|e| !is_palace_entry(e));
            removed_any |= arr.len() != before;
        }
    }

    if !removed_any {
        return Ok(false);
    }

    if dry_run {
        return Ok(true);
    }

    backup_existing(path)?;
    let text = serde_json::to_string_pretty(&next)?;
    fs::write(path, format!("{text}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

// ── Claude Code / Codex nested-hooks integration ─────────────────────────────
//
// Claude Code (`~/.claude/settings.json`) and Codex (`~/.codex/hooks.json`)
// share a nested config shape that differs from Cursor's flat `hooks.json`:
// each event maps to an array of matcher-groups, and each group holds a `hooks`
// array of command handlers. Both clients also share a "Claude-style" output
// dialect, so a single command runner (`palace hook <event> --client <c>`)
// serves both — only the registration shape and tool matchers differ.

/// A hook in the nested (Claude/Codex) config shape.
struct NestedHook {
    /// Event key (e.g. "SessionStart").
    event: &'static str,
    /// CLI sub-event passed to `palace hook <cli_event>`.
    cli_event: &'static str,
    /// Optional tool-name matcher (omitted = matches every tool).
    matcher: Option<&'static str>,
}

/// Claude Code hooks: investigations surface through the Grep/Read/Glob tools.
fn claude_hooks() -> &'static [NestedHook] {
    &[
        NestedHook {
            event: "SessionStart",
            cli_event: "session-start",
            matcher: None,
        },
        NestedHook {
            event: "PostToolUse",
            cli_event: "post-tool-use",
            matcher: Some("Grep|Read|Glob"),
        },
        NestedHook {
            event: "Stop",
            cli_event: "stop",
            matcher: None,
        },
    ]
}

/// Codex hooks: investigations run through the shell, so PostToolUse matches Bash.
fn codex_hooks() -> &'static [NestedHook] {
    &[
        NestedHook {
            event: "SessionStart",
            cli_event: "session-start",
            matcher: None,
        },
        NestedHook {
            event: "PostToolUse",
            cli_event: "post-tool-use",
            matcher: Some("Bash"),
        },
        NestedHook {
            event: "Stop",
            cli_event: "stop",
            matcher: None,
        },
    ]
}

/// The nested-hooks config path, hook set, and `--client` flag for a client, or
/// `None` for clients without a nested hook system (Cursor uses its own flat
/// format; Claude Desktop has no hooks).
fn nested_hook_target(
    home_dir: &Path,
    client: Client,
) -> Option<(PathBuf, &'static [NestedHook], &'static str)> {
    match client {
        Client::Claude => Some((
            home_dir.join(".claude").join("settings.json"),
            claude_hooks(),
            "claude",
        )),
        Client::Codex => Some((
            home_dir.join(".codex").join("hooks.json"),
            codex_hooks(),
            "codex",
        )),
        _ => None,
    }
}

/// Read a JSON object file, returning `{}` when missing or empty so unrelated
/// keys in an existing config are always preserved.
fn read_json_object(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

/// True if a nested matcher-group contains a palace command handler.
fn nested_group_is_palace(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .map(|handlers| {
            handlers.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .map(|c| c.contains(CURSOR_HOOK_KEY))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// The full command for a nested hook handler (quoted binary + event + client).
fn nested_hook_command(binary_path: &Path, cli_event: &str, client_flag: &str) -> String {
    format!(
        "\"{}\" hook {} --client {}",
        path_to_string(binary_path),
        cli_event,
        client_flag
    )
}

/// Build the desired matcher-group for a Palace nested hook.
fn nested_group(matcher: Option<&str>, command: &str) -> Value {
    let mut group = serde_json::Map::new();
    if let Some(matcher) = matcher {
        group.insert("matcher".to_string(), json!(matcher));
    }
    group.insert(
        "hooks".to_string(),
        json!([{ "type": "command", "command": command }]),
    );
    Value::Object(group)
}

/// Install or update Palace hooks in a nested-format config file, preserving
/// every unrelated key and matcher-group.
fn install_nested_hooks(
    path: &Path,
    defs: &[NestedHook],
    binary_path: &Path,
    client_flag: &str,
    dry_run: bool,
) -> Result<bool> {
    let existing = read_json_object(path)?;
    let mut next = existing.clone();

    {
        let hooks = next
            .as_object_mut()
            .ok_or_else(|| anyhow!("{} root is not an object", path.display()))?
            .entry("hooks")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow!("{} 'hooks' is not an object", path.display()))?;

        for def in defs {
            let arr = hooks
                .entry(def.event)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or_else(|| anyhow!("hooks.{} is not an array", def.event))?;

            let desired = nested_group(
                def.matcher,
                &nested_hook_command(binary_path, def.cli_event, client_flag),
            );
            // Replace the existing palace group (binary path / matcher may have
            // changed) or append a fresh one, leaving unrelated groups intact.
            if let Some(slot) = arr.iter_mut().find(|g| nested_group_is_palace(g)) {
                *slot = desired;
            } else {
                arr.push(desired);
            }
        }
    }

    if existing == next {
        return Ok(false);
    }
    if dry_run {
        return Ok(true);
    }

    backup_existing(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let text = serde_json::to_string_pretty(&next)?;
    fs::write(path, format!("{text}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

/// Remove Palace matcher-groups from a nested-format config file.
fn remove_nested_hooks(path: &Path, defs: &[NestedHook], dry_run: bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let existing = read_json_object(path)?;
    let mut next = existing.clone();

    let Some(hooks) = next.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(false);
    };

    let mut removed_any = false;
    for def in defs {
        if let Some(arr) = hooks.get_mut(def.event).and_then(Value::as_array_mut) {
            let before = arr.len();
            arr.retain(|g| !nested_group_is_palace(g));
            removed_any |= arr.len() != before;
        }
    }

    if !removed_any {
        return Ok(false);
    }
    if dry_run {
        return Ok(true);
    }

    backup_existing(path)?;
    let text = serde_json::to_string_pretty(&next)?;
    fs::write(path, format!("{text}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

/// True if any Palace hook is registered in a nested-format config file.
fn nested_hook_installed(path: &Path) -> bool {
    read_json_object(path)
        .ok()
        .and_then(|v| {
            v.get("hooks").and_then(Value::as_object).map(|hooks| {
                hooks.values().any(|groups| {
                    groups
                        .as_array()
                        .map(|gs| gs.iter().any(nested_group_is_palace))
                        .unwrap_or(false)
                })
            })
        })
        .unwrap_or(false)
}

/// Install the automatic memory hooks for a client at user scope. Returns the
/// hook config path and whether it changed, or `None` for clients without a
/// nested hook system (Cursor is handled separately; Claude Desktop has none).
fn install_client_hooks(
    options: &InstallOptions,
    client: Client,
) -> Result<Option<(PathBuf, bool)>> {
    let Some((path, defs, client_flag)) = nested_hook_target(&options.home_dir, client) else {
        return Ok(None);
    };
    let changed = install_nested_hooks(
        &path,
        defs,
        &options.binary_path,
        client_flag,
        options.dry_run,
    )?;
    Ok(Some((path, changed)))
}

/// Uninstall the automatic memory hooks for a client at user scope.
fn uninstall_client_hooks(
    options: &InstallOptions,
    client: Client,
) -> Result<Option<(PathBuf, bool)>> {
    let Some((path, defs, _)) = nested_hook_target(&options.home_dir, client) else {
        return Ok(None);
    };
    let changed = remove_nested_hooks(&path, defs, options.dry_run)?;
    Ok(Some((path, changed)))
}
