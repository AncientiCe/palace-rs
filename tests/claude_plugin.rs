//! The Claude Code plugin in `plugin/` must stay in step with what
//! `palace install --client claude` writes, and install must not duplicate the
//! plugin's MCP server or hooks once the plugin is enabled.

use std::fs;
use std::path::{Path, PathBuf};

use palace::install::{
    claude_plugin_enabled, doctor, install_clients, Client, InstallOptions, Scope,
    CLAUDE_PLUGIN_NAME,
};
use serde_json::{json, Value};
use tempfile::TempDir;

const PLUGIN_TOOL_MATCHER: &str = "mcp__plugin_palace_palace__.*";
const INSTALL_TOOL_MATCHER: &str = "mcp__palace__.*";

fn repo_file(rel: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap()
}

fn options(home: &Path) -> InstallOptions {
    InstallOptions {
        clients: vec![Client::Claude],
        scope: Scope::User,
        project_dir: None,
        home_dir: home.to_path_buf(),
        binary_path: binary(home),
        dry_run: false,
        force: false,
        install_rule: true,
        profile: palace::config::Profile::Coding,
    }
}

fn binary(home: &Path) -> PathBuf {
    home.join("bin").join("palace")
}

/// `(event, matcher, "<cli-event> --client claude")` for every hook handler.
fn hook_entries(config: &Value, strip: &dyn Fn(&str) -> String) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (event, groups) in config["hooks"].as_object().unwrap() {
        for group in groups.as_array().unwrap() {
            let matcher = group["matcher"].as_str().unwrap_or("").to_string();
            for handler in group["hooks"].as_array().unwrap() {
                let command = handler["command"].as_str().unwrap();
                out.push((event.clone(), matcher.clone(), strip(command)));
            }
        }
    }
    out.sort();
    out
}

fn enable_plugin(home: &Path) {
    let settings = home.join(".claude").join("settings.json");
    let mut config: Value = fs::read_to_string(&settings)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}));
    config["enabledPlugins"] = json!({ format!("{CLAUDE_PLUGIN_NAME}@palace-rs"): true });
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(&settings, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

#[test]
fn plugin_hooks_match_what_install_writes() {
    let temp = TempDir::new().unwrap();
    install_clients(&options(temp.path())).unwrap();
    let installed = repo_file_from(&temp.path().join(".claude/settings.json"));

    let from_install = hook_entries(&installed, &|command| {
        let (_, rest) = command.split_once(" hook ").unwrap();
        rest.to_string()
    });
    let from_plugin = hook_entries(&repo_file("plugin/hooks/hooks.json"), &|command| {
        command
            .strip_prefix("palace hook ")
            .and_then(|rest| rest.strip_suffix(" --plugin"))
            .unwrap_or_else(|| panic!("plugin hook must run `palace hook … --plugin`: {command}"))
            .to_string()
    })
    .into_iter()
    .map(|(event, matcher, command)| {
        (
            event,
            matcher.replace(PLUGIN_TOOL_MATCHER, INSTALL_TOOL_MATCHER),
            command,
        )
    })
    .collect::<Vec<_>>();

    assert_eq!(from_plugin, from_install);
}

fn repo_file_from(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn plugin_and_marketplace_names_agree() {
    let plugin = repo_file("plugin/.claude-plugin/plugin.json");
    let marketplace = repo_file(".claude-plugin/marketplace.json");
    assert_eq!(plugin["name"], CLAUDE_PLUGIN_NAME);
    let entry = &marketplace["plugins"][0];
    assert_eq!(entry["name"], CLAUDE_PLUGIN_NAME);
    assert_eq!(entry["source"], "./plugin");
    // The PostToolUse matcher is built from the plugin and server names.
    let mcp = repo_file("plugin/.mcp.json");
    assert_eq!(mcp["mcpServers"]["palace"]["args"], json!(["mcp"]));
    assert_eq!(
        PLUGIN_TOOL_MATCHER,
        format!("mcp__plugin_{CLAUDE_PLUGIN_NAME}_palace__.*")
    );
}

#[test]
fn plugin_enabled_reads_enabled_plugins() {
    let temp = TempDir::new().unwrap();
    assert!(!claude_plugin_enabled(temp.path()));
    enable_plugin(temp.path());
    assert!(claude_plugin_enabled(temp.path()));

    let settings = temp.path().join(".claude/settings.json");
    fs::write(
        &settings,
        r#"{"enabledPlugins": {"palace@palace-rs": false, "palace-extra@x": true}}"#,
    )
    .unwrap();
    assert!(!claude_plugin_enabled(temp.path()));
}

#[test]
fn install_with_plugin_enabled_keeps_rule_and_drops_duplicates() {
    let temp = TempDir::new().unwrap();
    let home = temp.path();
    install_clients(&options(home)).unwrap();
    assert!(read(home, ".claude.json")["mcpServers"]["palace"].is_object());

    enable_plugin(home);
    install_clients(&options(home)).unwrap();

    assert!(read(home, ".claude.json")["mcpServers"]
        .get("palace")
        .is_none());
    let settings = read(home, ".claude/settings.json");
    let commands = serde_json::to_string(&settings["hooks"]).unwrap();
    assert!(
        !commands.contains(" hook "),
        "palace hooks left: {commands}"
    );
    assert_eq!(
        settings["enabledPlugins"]["palace@palace-rs"],
        json!(true),
        "unrelated settings are kept"
    );
    let rule = fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap();
    assert!(rule.contains("<!-- BEGIN PALACE -->"));

    let report = doctor(&options(home)).unwrap();
    let claude = report
        .clients
        .iter()
        .find(|status| status.client == Client::Claude)
        .unwrap();
    assert!(claude.via_plugin);
    assert!(claude.configured && claude.hook_installed);
    assert!(
        report.adoption_warnings.is_empty(),
        "{:?}",
        report.adoption_warnings
    );
}

fn read(home: &Path, rel: &str) -> Value {
    repo_file_from(&home.join(rel))
}
