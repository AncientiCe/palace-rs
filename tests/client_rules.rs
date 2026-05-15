use std::fs;
use std::path::{Path, PathBuf};

use palace::install::{
    cursor_hook_installed, doctor, install_clients, install_cursor_hook,
    session_start_hook_response, uninstall_clients, uninstall_cursor_hook, Client, InstallOptions,
    Scope,
};
use serde_json::Value;
use tempfile::TempDir;
use toml_edit::DocumentMut;

fn options(home: &Path, binary_path: &Path, clients: Vec<Client>) -> InstallOptions {
    InstallOptions {
        clients,
        scope: Scope::User,
        project_dir: None,
        home_dir: home.to_path_buf(),
        binary_path: binary_path.to_path_buf(),
        dry_run: false,
        force: false,
        install_rule: true,
    }
}

fn fake_binary(home: &Path) -> PathBuf {
    if cfg!(windows) {
        home.join("bin").join("palace.exe")
    } else {
        home.join("bin").join("palace")
    }
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn without_rule(mut options: InstallOptions) -> InstallOptions {
    options.install_rule = false;
    options
}

#[test]
fn install_writes_cursor_config_to_temp_home() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    let report =
        install_clients(&options(temp.path(), &binary_path, vec![Client::Cursor])).unwrap();

    assert_eq!(report.changed.len(), 1);
    let config = read_json(&temp.path().join(".cursor").join("mcp.json"));
    let server = &config["mcpServers"]["palace"];
    assert_eq!(
        server["command"].as_str(),
        Some(binary_path.to_string_lossy().as_ref())
    );
    assert_eq!(server["args"], serde_json::json!(["mcp"]));
}

#[test]
fn install_writes_cursor_rule_mdc() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());

    install_clients(&options(temp.path(), &binary_path, vec![Client::Cursor])).unwrap();

    let rule = fs::read_to_string(temp.path().join(".cursor/rules/palace.mdc")).unwrap();
    assert!(rule.contains("alwaysApply: true"));
    // Block 1 — SESSION START
    assert!(rule.contains("SESSION START"));
    assert!(rule.contains("palace_status"));
    assert!(rule.contains("palace_session_context"));
    assert!(rule.contains("palace_diary_search"));
    // Block 2 — BEFORE ANSWERING
    assert!(rule.contains("BEFORE ANSWERING"));
    assert!(rule.contains("palace_search"));
    assert!(rule.contains("palace_kg_query"));
    // Block 3 — AFTER WORK
    assert!(rule.contains("AFTER WORK"));
    assert!(rule.contains("palace_diary_write"));
    assert!(rule.contains("palace_kg_add"));
}

#[test]
fn install_inserts_managed_block_into_existing_codex_agents_md() {
    let temp = TempDir::new().unwrap();
    let codex_dir = temp.path().join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(
        codex_dir.join("AGENTS.md"),
        "# Existing guidance\n\nKeep this line.\n",
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    install_clients(&options(temp.path(), &binary_path, vec![Client::Codex])).unwrap();

    let rule = fs::read_to_string(codex_dir.join("AGENTS.md")).unwrap();
    assert!(rule.contains("# Existing guidance"));
    assert!(rule.contains("Keep this line."));
    assert!(rule.contains("<!-- BEGIN PALACE -->"));
    assert!(rule.contains("palace_kg_query"));
    assert!(rule.contains("<!-- END PALACE -->"));
}

#[test]
fn install_merges_into_existing_cursor_config() {
    let temp = TempDir::new().unwrap();
    let cursor_dir = temp.path().join(".cursor");
    fs::create_dir_all(&cursor_dir).unwrap();
    fs::write(
        cursor_dir.join("mcp.json"),
        r#"{"mcpServers":{"other":{"command":"other-tool","args":["serve"]}}}"#,
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    install_clients(&options(temp.path(), &binary_path, vec![Client::Cursor])).unwrap();

    let config = read_json(&cursor_dir.join("mcp.json"));
    assert_eq!(config["mcpServers"]["other"]["command"], "other-tool");
    assert_eq!(
        config["mcpServers"]["palace"]["command"].as_str(),
        Some(binary_path.to_string_lossy().as_ref())
    );
    assert!(cursor_dir.join("mcp.json.bak").exists());
}

#[test]
fn install_removes_legacy_mempalace_entry() {
    let temp = TempDir::new().unwrap();
    let cursor_dir = temp.path().join(".cursor");
    fs::create_dir_all(&cursor_dir).unwrap();
    // Simulate a 0.1.x install: has "mempalace" key
    fs::write(
        cursor_dir.join("mcp.json"),
        r#"{"mcpServers":{"mempalace":{"command":"mempalace","args":["mcp"]},"other":{"command":"other-tool"}}}"#,
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    install_clients(&options(temp.path(), &binary_path, vec![Client::Cursor])).unwrap();

    let config = read_json(&cursor_dir.join("mcp.json"));
    // Legacy entry gone
    assert!(config["mcpServers"]["mempalace"].is_null());
    // New entry written
    assert_eq!(
        config["mcpServers"]["palace"]["command"].as_str(),
        Some(binary_path.to_string_lossy().as_ref())
    );
    // Other server preserved
    assert_eq!(config["mcpServers"]["other"]["command"], "other-tool");
}

#[test]
fn install_migrates_legacy_rule_block() {
    let temp = TempDir::new().unwrap();
    let claude_dir = temp.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    // Simulate a 0.1.x install: has old BEGIN MEMPALACE markers
    fs::write(
        claude_dir.join("CLAUDE.md"),
        "before\n\n<!-- BEGIN MEMPALACE -->\nold_content\n<!-- END MEMPALACE -->\n\nafter\n",
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    install_clients(&options(temp.path(), &binary_path, vec![Client::Claude])).unwrap();

    let rule = fs::read_to_string(claude_dir.join("CLAUDE.md")).unwrap();
    assert!(rule.contains("before"));
    assert!(rule.contains("after"));
    assert!(!rule.contains("old_content"));
    assert!(!rule.contains("<!-- BEGIN MEMPALACE -->"));
    assert!(rule.contains("<!-- BEGIN PALACE -->"));
    assert!(rule.contains("palace_status"));
}

#[test]
fn install_merges_codex_toml_preserving_comments() {
    let temp = TempDir::new().unwrap();
    let codex_dir = temp.path().join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(
        codex_dir.join("config.toml"),
        "# keep this comment\nmodel = \"gpt-5\"\n",
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    install_clients(&options(temp.path(), &binary_path, vec![Client::Codex])).unwrap();

    let config = fs::read_to_string(codex_dir.join("config.toml")).unwrap();
    assert!(config.contains("# keep this comment"));
    assert!(config.contains("model = \"gpt-5\""));
    assert!(config.contains("[mcp_servers.palace]"));
    let parsed = config.parse::<DocumentMut>().unwrap();
    assert_eq!(
        parsed["mcp_servers"]["palace"]["command"].as_str(),
        Some(binary_path.to_string_lossy().as_ref())
    );
    assert_eq!(
        parsed["mcp_servers"]["palace"]["args"][0].as_str(),
        Some("mcp")
    );
}

#[test]
fn install_is_idempotent() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    let install_options = options(temp.path(), &binary_path, vec![Client::Cursor]);

    let first = install_clients(&install_options).unwrap();
    let second = install_clients(&install_options).unwrap();

    assert_eq!(first.changed.len(), 1);
    assert!(second.changed.is_empty());
    let config = read_json(&temp.path().join(".cursor").join("mcp.json"));
    assert_eq!(
        config["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| key.as_str() == "palace")
            .count(),
        1
    );
    assert!(!temp.path().join(".cursor").join("mcp.json.bak").exists());
}

#[test]
fn install_replaces_existing_managed_block_idempotently() {
    let temp = TempDir::new().unwrap();
    let claude_dir = temp.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(
        claude_dir.join("CLAUDE.md"),
        "before\n\n<!-- BEGIN PALACE -->\nstale\n<!-- END PALACE -->\n\nafter\n",
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    let install_options = options(temp.path(), &binary_path, vec![Client::Claude]);
    let first = install_clients(&install_options).unwrap();
    let second = install_clients(&install_options).unwrap();

    assert!(first
        .rule_changed
        .iter()
        .any(|path| path.ends_with("CLAUDE.md")));
    assert!(second.rule_changed.is_empty());
    let rule = fs::read_to_string(claude_dir.join("CLAUDE.md")).unwrap();
    assert!(rule.contains("before"));
    assert!(rule.contains("after"));
    assert!(!rule.contains("stale"));
    assert_eq!(rule.matches("<!-- BEGIN PALACE -->").count(), 1);
    assert!(claude_dir.join("CLAUDE.md.bak").exists());
    assert!(!claude_dir.join("CLAUDE.md.bak.bak").exists());
}

#[test]
fn install_with_no_rule_skips_rule_files() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    let install_options = without_rule(options(temp.path(), &binary_path, vec![Client::Cursor]));

    let report = install_clients(&install_options).unwrap();

    assert_eq!(report.changed.len(), 1);
    assert!(report.rule_changed.is_empty());
    assert!(temp.path().join(".cursor/mcp.json").exists());
    assert!(!temp.path().join(".cursor/rules/palace.mdc").exists());
}

#[test]
fn uninstall_removes_only_palace_entry() {
    let temp = TempDir::new().unwrap();
    let cursor_dir = temp.path().join(".cursor");
    fs::create_dir_all(&cursor_dir).unwrap();
    fs::write(
        cursor_dir.join("mcp.json"),
        r#"{"mcpServers":{"palace":{"command":"palace","args":["mcp"]},"other":{"command":"other-tool"}}}"#,
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    uninstall_clients(&options(temp.path(), &binary_path, vec![Client::Cursor])).unwrap();

    let config = read_json(&cursor_dir.join("mcp.json"));
    assert!(config["mcpServers"]["palace"].is_null());
    assert_eq!(config["mcpServers"]["other"]["command"], "other-tool");
}

#[test]
fn uninstall_removes_legacy_mempalace_entry() {
    let temp = TempDir::new().unwrap();
    let cursor_dir = temp.path().join(".cursor");
    fs::create_dir_all(&cursor_dir).unwrap();
    // User has only the old 0.1.x entry
    fs::write(
        cursor_dir.join("mcp.json"),
        r#"{"mcpServers":{"mempalace":{"command":"mempalace","args":["mcp"]},"other":{"command":"other-tool"}}}"#,
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    uninstall_clients(&options(temp.path(), &binary_path, vec![Client::Cursor])).unwrap();

    let config = read_json(&cursor_dir.join("mcp.json"));
    assert!(config["mcpServers"]["mempalace"].is_null());
    assert_eq!(config["mcpServers"]["other"]["command"], "other-tool");
}

#[test]
fn uninstall_removes_managed_block_only() {
    let temp = TempDir::new().unwrap();
    let codex_dir = temp.path().join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(
        codex_dir.join("AGENTS.md"),
        "before\n\n<!-- BEGIN PALACE -->\nmanaged\n<!-- END PALACE -->\n\nafter\n",
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    uninstall_clients(&options(temp.path(), &binary_path, vec![Client::Codex])).unwrap();

    let rule = fs::read_to_string(codex_dir.join("AGENTS.md")).unwrap();
    assert!(rule.contains("before"));
    assert!(rule.contains("after"));
    assert!(!rule.contains("managed"));
    assert!(!rule.contains("<!-- BEGIN PALACE -->"));
}

#[test]
fn uninstall_removes_legacy_managed_block() {
    let temp = TempDir::new().unwrap();
    let codex_dir = temp.path().join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(
        codex_dir.join("AGENTS.md"),
        "before\n\n<!-- BEGIN MEMPALACE -->\nold\n<!-- END MEMPALACE -->\n\nafter\n",
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    uninstall_clients(&options(temp.path(), &binary_path, vec![Client::Codex])).unwrap();

    let rule = fs::read_to_string(codex_dir.join("AGENTS.md")).unwrap();
    assert!(rule.contains("before"));
    assert!(rule.contains("after"));
    assert!(!rule.contains("old"));
    assert!(!rule.contains("<!-- BEGIN MEMPALACE -->"));
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn install_all_skips_claude_desktop_on_unsupported_platform() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    // Must not error on platforms where Claude Desktop is not supported
    let report = install_clients(&without_rule(options(
        temp.path(),
        &binary_path,
        vec![Client::All],
    )))
    .unwrap();
    let paths: Vec<_> = report
        .changed
        .iter()
        .chain(report.unchanged.iter())
        .collect();
    assert!(
        paths
            .iter()
            .all(|p| !p.to_string_lossy().contains("claude_desktop")),
        "claude_desktop_config.json should not appear on this platform"
    );
}

#[test]
fn install_claude_writes_to_dot_claude_json_not_subdirectory() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    let report =
        install_clients(&options(temp.path(), &binary_path, vec![Client::Claude])).unwrap();

    assert_eq!(report.changed.len(), 1);
    // Must be ~/.claude.json (top-level), not ~/.claude/mcp_servers.json
    assert!(report.changed[0].ends_with(".claude.json"));
    assert!(!report.changed[0]
        .to_string_lossy()
        .contains("mcp_servers.json"));

    let config = read_json(&temp.path().join(".claude.json"));
    let server = &config["mcpServers"]["palace"];
    assert_eq!(
        server["command"].as_str(),
        Some(binary_path.to_string_lossy().as_ref())
    );
    assert_eq!(server["args"], serde_json::json!(["mcp"]));
}

#[test]
fn install_claude_inserts_rule_block_into_claude_md() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    install_clients(&options(temp.path(), &binary_path, vec![Client::Claude])).unwrap();

    let rule = fs::read_to_string(temp.path().join(".claude/CLAUDE.md")).unwrap();
    assert!(rule.contains("<!-- BEGIN PALACE -->"));
    assert!(rule.contains("palace_status"));
    assert!(rule.contains("MEMORY-FIRST"));
    assert!(rule.contains("CODE-SEARCH-FIRST"));
    assert!(rule.contains("<!-- END PALACE -->"));
}

#[test]
fn installed_rules_include_memory_routing_for_codex_managed_block() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());

    install_clients(&options(temp.path(), &binary_path, vec![Client::Codex])).unwrap();

    let codex_rule = fs::read_to_string(temp.path().join(".codex/AGENTS.md")).unwrap();

    // Block 1
    assert!(codex_rule.contains("SESSION START"));
    assert!(codex_rule.contains("palace_status"));
    assert!(codex_rule.contains("palace_session_context"));
    assert!(codex_rule.contains("palace_diary_search"));
    // Block 2
    assert!(codex_rule.contains("BEFORE ANSWERING"));
    assert!(codex_rule.contains("palace_search"));
    assert!(codex_rule.contains("palace_kg_query"));
    // Block 3
    assert!(codex_rule.contains("AFTER WORK"));
    assert!(codex_rule.contains("palace_diary_write"));
}

#[test]
fn installed_rules_include_full_memory_lifecycle_for_all_clients() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());

    install_clients(&options(
        temp.path(),
        &binary_path,
        vec![Client::Cursor, Client::Codex, Client::Claude],
    ))
    .unwrap();

    let rule_paths = [
        temp.path().join(".cursor/rules/palace.mdc"),
        temp.path().join(".codex/AGENTS.md"),
        temp.path().join(".claude/CLAUDE.md"),
    ];

    for path in rule_paths {
        let rule = fs::read_to_string(&path).unwrap();
        assert!(rule.contains("SESSION START"));
        assert!(rule.contains("palace_status"));
        assert!(rule.contains("palace_session_context"));
        assert!(rule.contains("palace_diary_search"));
        assert!(rule.contains("BEFORE ANSWERING"));
        assert!(rule.contains("palace_search"));
        assert!(rule.contains("palace_kg_query"));
        assert!(rule.contains("AFTER WORK"));
        assert!(rule.contains("palace_diary_write"));
        assert!(rule.contains("palace_kg_add"));
        assert!(rule.contains("palace_kg_invalidate"));
        assert!(rule.contains("MEMORY ROUTING"));
    }
}

#[test]
fn project_scoped_install_writes_strengthened_rules() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let binary_path = fake_binary(temp.path());
    let mut install_options = options(
        temp.path(),
        &binary_path,
        vec![Client::Cursor, Client::Codex, Client::Claude],
    );
    install_options.scope = Scope::Project;
    install_options.project_dir = Some(project.clone());

    install_clients(&install_options).unwrap();

    for path in [
        project.join(".cursor/rules/palace.mdc"),
        project.join("AGENTS.md"),
        project.join("CLAUDE.md"),
    ] {
        let rule = fs::read_to_string(&path).unwrap();
        assert!(rule.contains("palace_session_context"));
        assert!(rule.contains("palace_diary_search"));
        assert!(rule.contains("palace_kg_add"));
        assert!(rule.contains("MEMORY ROUTING"));
    }
}

#[test]
fn doctor_marks_rule_weak_without_full_memory_lifecycle() {
    let temp = TempDir::new().unwrap();
    let codex_dir = temp.path().join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(
        codex_dir.join("AGENTS.md"),
        "<!-- BEGIN PALACE -->\n# Palace Memory Protocol\n\nCall palace_status and palace_search.\n<!-- END PALACE -->\n",
        // Missing SESSION START block, BEFORE ANSWERING, AFTER WORK, diary_search, kg_query
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    let install_options = options(temp.path(), &binary_path, vec![Client::Codex]);
    let report = doctor(&install_options).unwrap();

    assert!(report
        .clients
        .iter()
        .any(|s| s.client == Client::Codex && s.rule_weak));
    assert!(report
        .adoption_warnings
        .iter()
        .any(|warning| { warning.contains("Codex") && warning.contains("session warm-start") }));
}

#[test]
fn doctor_reports_no_adoption_warnings_after_full_install() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    let install_options = options(temp.path(), &binary_path, vec![Client::Cursor]);

    install_clients(&install_options).unwrap();
    let report = doctor(&install_options).unwrap();

    assert!(report.adoption_warnings.is_empty());
}

#[test]
fn uninstall_claude_removes_entry_from_dot_claude_json() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    let install_options = options(temp.path(), &binary_path, vec![Client::Claude]);
    install_clients(&install_options).unwrap();
    let report = uninstall_clients(&without_rule(install_options)).unwrap();

    assert_eq!(report.changed.len(), 1);
    let config = read_json(&temp.path().join(".claude.json"));
    assert!(config["mcpServers"]["palace"].is_null());
}

#[cfg(target_os = "macos")]
#[test]
fn install_claude_desktop_writes_to_library_application_support() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    let report = install_clients(&options(
        temp.path(),
        &binary_path,
        vec![Client::ClaudeDesktop],
    ))
    .unwrap();

    assert_eq!(report.changed.len(), 1);
    let expected = temp
        .path()
        .join("Library/Application Support/Claude/claude_desktop_config.json");
    assert_eq!(report.changed[0], expected);

    let config = read_json(&expected);
    let server = &config["mcpServers"]["palace"];
    assert_eq!(
        server["command"].as_str(),
        Some(binary_path.to_string_lossy().as_ref())
    );
    assert_eq!(server["args"], serde_json::json!(["mcp"]));
}

#[cfg(target_os = "macos")]
#[test]
fn install_claude_desktop_preserves_existing_keys() {
    let temp = TempDir::new().unwrap();
    let desktop_dir = temp.path().join("Library/Application Support/Claude");
    fs::create_dir_all(&desktop_dir).unwrap();
    fs::write(
        desktop_dir.join("claude_desktop_config.json"),
        r#"{"preferences":{"theme":"dark"}}"#,
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    install_clients(&options(
        temp.path(),
        &binary_path,
        vec![Client::ClaudeDesktop],
    ))
    .unwrap();

    let config = read_json(&desktop_dir.join("claude_desktop_config.json"));
    assert_eq!(config["preferences"]["theme"], "dark");
    assert_eq!(
        config["mcpServers"]["palace"]["command"].as_str(),
        Some(binary_path.to_string_lossy().as_ref())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn uninstall_claude_desktop_removes_only_palace() {
    let temp = TempDir::new().unwrap();
    let desktop_dir = temp.path().join("Library/Application Support/Claude");
    fs::create_dir_all(&desktop_dir).unwrap();
    fs::write(
        desktop_dir.join("claude_desktop_config.json"),
        r#"{"mcpServers":{"palace":{"command":"palace","args":["mcp"]},"other":{"command":"other"}}}"#,
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    let install_options = without_rule(options(
        temp.path(),
        &binary_path,
        vec![Client::ClaudeDesktop],
    ));
    uninstall_clients(&install_options).unwrap();

    let config = read_json(&desktop_dir.join("claude_desktop_config.json"));
    assert!(config["mcpServers"]["palace"].is_null());
    assert_eq!(config["mcpServers"]["other"]["command"], "other");
}

#[cfg(target_os = "macos")]
#[test]
fn doctor_reports_claude_and_claude_desktop_paths() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    let install_options = options(
        temp.path(),
        &binary_path,
        vec![Client::Claude, Client::ClaudeDesktop],
    );

    let before = doctor(&install_options).unwrap();
    assert!(before
        .clients
        .iter()
        .any(|s| s.client == Client::Claude && !s.configured && s.path.ends_with(".claude.json")));
    assert!(before.clients.iter().any(|s| {
        s.client == Client::ClaudeDesktop
            && !s.configured
            && s.path.ends_with("claude_desktop_config.json")
    }));

    install_clients(&install_options).unwrap();
    let after = doctor(&install_options).unwrap();
    assert!(after
        .clients
        .iter()
        .all(|s| s.configured && s.points_to_expected_binary));
}

#[test]
fn doctor_reports_status_correctly() {
    let temp = TempDir::new().unwrap();
    let binary_path = fake_binary(temp.path());
    let install_options = options(temp.path(), &binary_path, vec![Client::Cursor]);

    let before = doctor(&install_options).unwrap();
    assert!(before.clients.iter().any(|status| {
        status.client == Client::Cursor
            && !status.configured
            && status.path.ends_with(".cursor/mcp.json")
    }));

    install_clients(&install_options).unwrap();
    let after = doctor(&install_options).unwrap();
    assert!(after.clients.iter().any(|status| {
        status.client == Client::Cursor
            && status.configured
            && status.points_to_expected_binary
            && status.rule_installed
            && status.rule_path.ends_with(".cursor/rules/palace.mdc")
    }));
}

#[test]
fn doctor_detects_stale_legacy_rule() {
    let temp = TempDir::new().unwrap();
    let cursor_dir = temp.path().join(".cursor/rules");
    fs::create_dir_all(&cursor_dir).unwrap();
    // Write an old 0.1.x rule file
    fs::write(
        cursor_dir.join("palace.mdc"),
        "---\nalwaysApply: true\n---\n\nmempalace_status mempalace_search\n",
    )
    .unwrap();

    let binary_path = fake_binary(temp.path());
    let install_options = options(temp.path(), &binary_path, vec![Client::Cursor]);
    let report = doctor(&install_options).unwrap();

    assert!(report
        .clients
        .iter()
        .any(|s| s.client == Client::Cursor && s.rule_stale));
}

// ── Cursor sessionStart hook tests ───────────────────────────────────────────

#[test]
fn install_cursor_hook_writes_hooks_json() {
    let temp = TempDir::new().unwrap();
    let binary = fake_binary(temp.path());

    let changed = install_cursor_hook(temp.path(), &binary, false).unwrap();
    assert!(changed, "first install should report changed=true");

    let hooks_json = temp.path().join(".cursor").join("hooks.json");
    assert!(hooks_json.exists(), "hooks.json should be created");

    let val: Value = read_json(&hooks_json);
    let commands: Vec<_> = val["hooks"]["sessionStart"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["command"].as_str())
        .collect();
    assert!(
        commands.iter().any(|c| c.contains("palace")),
        "sessionStart should contain a palace entry, got: {commands:?}"
    );
}

#[test]
fn install_cursor_hook_is_idempotent() {
    let temp = TempDir::new().unwrap();
    let binary = fake_binary(temp.path());

    install_cursor_hook(temp.path(), &binary, false).unwrap();
    let changed = install_cursor_hook(temp.path(), &binary, false).unwrap();
    assert!(
        !changed,
        "second install with same binary should be unchanged"
    );

    let hooks_json = temp.path().join(".cursor").join("hooks.json");
    let val: Value = read_json(&hooks_json);
    let count = val["hooks"]["sessionStart"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["command"]
                .as_str()
                .map(|c| c.contains("palace"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(count, 1, "should have exactly one palace entry");
}

#[test]
fn install_cursor_hook_preserves_existing_entries() {
    let temp = TempDir::new().unwrap();
    let binary = fake_binary(temp.path());

    // Pre-populate hooks.json with an unrelated entry.
    let hooks_dir = temp.path().join(".cursor");
    fs::create_dir_all(&hooks_dir).unwrap();
    let hooks_json = hooks_dir.join("hooks.json");
    fs::write(
        &hooks_json,
        r#"{"version":1,"hooks":{"sessionStart":[{"command":"./hooks/other.sh"}]}}"#,
    )
    .unwrap();

    install_cursor_hook(temp.path(), &binary, false).unwrap();

    let val: Value = read_json(&hooks_json);
    let arr = val["hooks"]["sessionStart"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "should have both other and palace entries");
    assert!(arr
        .iter()
        .any(|e| e["command"].as_str() == Some("./hooks/other.sh")));
}

#[test]
fn uninstall_cursor_hook_removes_entry() {
    let temp = TempDir::new().unwrap();
    let binary = fake_binary(temp.path());

    install_cursor_hook(temp.path(), &binary, false).unwrap();
    assert!(cursor_hook_installed(temp.path()));

    let changed = uninstall_cursor_hook(temp.path(), false).unwrap();
    assert!(changed);
    assert!(!cursor_hook_installed(temp.path()));
}

#[test]
fn uninstall_cursor_hook_is_idempotent() {
    let temp = TempDir::new().unwrap();
    let changed = uninstall_cursor_hook(temp.path(), false).unwrap();
    assert!(
        !changed,
        "uninstall with nothing to remove should be unchanged"
    );
}

#[test]
fn install_clients_cursor_installs_hook() {
    let temp = TempDir::new().unwrap();
    let binary = fake_binary(temp.path());
    let opts = options(temp.path(), &binary, vec![Client::Cursor]);

    install_clients(&opts).unwrap();

    assert!(
        cursor_hook_installed(temp.path()),
        "install_clients should register the Cursor sessionStart hook"
    );
}

#[test]
fn doctor_reports_hook_installed_after_install() {
    let temp = TempDir::new().unwrap();
    let binary = fake_binary(temp.path());
    let opts = options(temp.path(), &binary, vec![Client::Cursor]);

    let before = doctor(&opts).unwrap();
    assert!(before
        .clients
        .iter()
        .any(|s| s.client == Client::Cursor && !s.hook_installed));

    install_clients(&opts).unwrap();
    let after = doctor(&opts).unwrap();
    assert!(after
        .clients
        .iter()
        .any(|s| s.client == Client::Cursor && s.hook_installed));
}

#[test]
fn session_start_hook_response_contains_protocol() {
    let json_str = session_start_hook_response().unwrap();
    let val: Value = serde_json::from_str(&json_str).unwrap();
    let ctx = val["additional_context"].as_str().unwrap_or("");
    assert!(
        ctx.contains("palace_status"),
        "additional_context should mention palace_status, got: {ctx}"
    );
    assert!(
        ctx.contains("Palace Memory Protocol"),
        "additional_context should include protocol header, got: {ctx}"
    );
}
