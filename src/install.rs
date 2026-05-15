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
pub const RULE_BODY: &str = "**MANDATORY — three hard triggers, every session, no exceptions.**\n\n## SESSION START\n*Always, before anything else — no skipping.*\n→ `palace_status` → `palace_session_context(agent_name)` → `palace_diary_search` (recent work in this project)\n\n## BEFORE ANSWERING\n*Any question about past decisions, preferences, people, projects, commands, or \"what happened last time?\"*\n→ `palace_search` + `palace_kg_query` — never answer from training data alone.\n- MEMORY-FIRST: call palace_search before grep/file-search for remembered decisions, fixes, conventions, or session history.\n- CODE-SEARCH-FIRST: use grep only for current symbols, exact definitions, or implementation details that may have changed.\n- For preferences/conventions: also call `palace_preference_search`.\n- When Palace answers, cite provenance (wing, room, source file, drawer id).\n\n## AFTER WORK\n*After any substantive task, fix, decision, or discovery.*\n→ `palace_diary_write` (what happened, what you learned, what matters)\n→ `palace_kg_add` for stable facts; `palace_kg_invalidate` + `palace_kg_add` when facts change.\n- To file a key fact: `palace_remember` (importance=5). To delete outdated info: `palace_forget`.\n\n**MEMORY ROUTING**: use Palace for prior decisions, user preferences, previous fixes, commands that worked, project history, and \"what happened last time?\". Use KG for stable facts. Use diary for session continuity. Use code search first only for current source symbols, exact definitions, and implementation details that may have changed.\n\nSkipping any trigger is a protocol violation. Storage is not memory; this protocol is.";

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

        // Install the Cursor sessionStart hook for automatic context injection.
        if client == Client::Cursor && options.scope == Scope::User {
            let hooks_json = options.home_dir.join(".cursor").join("hooks.json");
            let changed =
                install_cursor_hook(&options.home_dir, &options.binary_path, options.dry_run)?;
            if changed {
                report.hook_changed.push(hooks_json);
            } else {
                report.hook_unchanged.push(hooks_json);
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

        if client == Client::Cursor && options.scope == Scope::User {
            let hooks_json = options.home_dir.join(".cursor").join("hooks.json");
            let changed = uninstall_cursor_hook(&options.home_dir, options.dry_run)?;
            if changed {
                report.hook_changed.push(hooks_json);
            } else {
                report.hook_unchanged.push(hooks_json);
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
        let hook_installed = if client == Client::Cursor && options.scope == Scope::User {
            cursor_hook_installed(&options.home_dir)
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
        if status.client == Client::Cursor {
            let hook_state = if status.hook_installed {
                "✓ session hook installed"
            } else {
                "✗ session hook missing — run: palace install"
            };
            println!("      {hook_state}");
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
pub fn install_cursor_hook(home_dir: &Path, binary_path: &Path, dry_run: bool) -> Result<bool> {
    let hooks_dir = home_dir.join(".cursor").join("hooks");
    let hooks_json = home_dir.join(".cursor").join("hooks.json");

    let script_path = write_hook_script(&hooks_dir, binary_path, dry_run)?;
    let script_rel = format!(
        "./hooks/{}",
        script_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );
    let changed = upsert_hooks_json(&hooks_json, &script_rel, dry_run)?;
    Ok(changed)
}

/// Remove the palace `sessionStart` hook from `~/.cursor/hooks.json` and
/// delete the hook script.
pub fn uninstall_cursor_hook(home_dir: &Path, dry_run: bool) -> Result<bool> {
    let hooks_json = home_dir.join(".cursor").join("hooks.json");
    let changed = remove_hook_entry(&hooks_json, dry_run)?;

    let hooks_dir = home_dir.join(".cursor").join("hooks");
    for name in hook_script_names() {
        let path = hooks_dir.join(name);
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

/// Handle a Cursor hook event received on stdin and write the response to stdout.
/// Called as `palace hook session-start` by `~/.cursor/hooks/palace-session-start.*`.
pub fn run_hook(event: &str) -> Result<()> {
    // Read one line from stdin (the JSON event payload).
    // We intentionally read only one line rather than all of stdin so the
    // hook does not block when stdin is a terminal or when Cursor closes the
    // pipe after sending the single-line JSON object.
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();

    match event {
        "session-start" | "sessionStart" => {
            println!("{}", session_start_hook_response()?);
        }
        other => {
            // Unknown event — exit cleanly (fail-open per Cursor hook spec).
            tracing::warn!(event = other, "palace hook: unknown event, ignoring");
        }
    }
    Ok(())
}

fn hook_script_names() -> &'static [&'static str] {
    &[
        "palace-session-start.bat",
        "palace-session-start.sh",
        "palace-session-start",
    ]
}

fn write_hook_script(
    hooks_dir: &Path,
    binary_path: &Path,
    dry_run: bool,
) -> Result<std::path::PathBuf> {
    let binary_str = path_to_string(binary_path);

    #[cfg(windows)]
    let (filename, content) = (
        "palace-session-start.bat",
        format!("@echo off\r\n\"{binary_str}\" hook session-start\r\n"),
    );
    #[cfg(not(windows))]
    let (filename, content) = (
        "palace-session-start.sh",
        format!("#!/bin/sh\n\"{binary_str}\" hook session-start\n"),
    );

    let script_path = hooks_dir.join(filename);

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

fn upsert_hooks_json(path: &Path, script_command: &str, dry_run: bool) -> Result<bool> {
    let existing = read_hooks_json(path)?;
    let mut next = existing.clone();

    let hooks = next
        .as_object_mut()
        .ok_or_else(|| anyhow!("hooks.json root is not an object"))?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("hooks.json 'hooks' is not an object"))?;

    let session_start = hooks
        .entry("sessionStart")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow!("sessionStart is not an array"))?;

    // Check if palace entry already exists with the right command.
    let already_present = session_start.iter().any(|e| {
        e.get("command")
            .and_then(Value::as_str)
            .map(|c| c.contains(CURSOR_HOOK_KEY))
            .unwrap_or(false)
    });

    if already_present {
        // Update the command in case the binary path changed.
        for entry in session_start.iter_mut() {
            if entry
                .get("command")
                .and_then(Value::as_str)
                .map(|c| c.contains(CURSOR_HOOK_KEY))
                .unwrap_or(false)
            {
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert(
                        "command".to_string(),
                        serde_json::Value::String(script_command.to_string()),
                    );
                }
            }
        }
        // Re-check if anything actually changed.
        if existing == next {
            return Ok(false);
        }
    } else {
        session_start.push(json!({ "command": script_command }));
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

    let Some(session_start) = hooks.get_mut("sessionStart").and_then(Value::as_array_mut) else {
        return Ok(false);
    };

    let before = session_start.len();
    session_start.retain(|e| {
        !e.get("command")
            .and_then(Value::as_str)
            .map(|c| c.contains(CURSOR_HOOK_KEY))
            .unwrap_or(false)
    });

    if session_start.len() == before {
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
