use palace::config::PalaceConfig;
use std::sync::Mutex;
use tempfile::TempDir;

// Serialise tests that touch env vars (env is process-global)
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn default_config_paths_are_stable() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("PALACE_PALACE_PATH");
    let tmp = TempDir::new().unwrap();
    let config = PalaceConfig::with_config_dir(Some(tmp.path()));
    assert_eq!(config.palace_path(), tmp.path().join("palace"));
    assert_eq!(config.palace_db_path(), tmp.path().join("palace/palace.db"));
    assert_eq!(config.collection_name(), "palace_drawers");
}

#[test]
fn init_creates_config_file() {
    let tmp = TempDir::new().unwrap();
    let config = PalaceConfig::with_config_dir(Some(tmp.path()));
    let config_file = config.init().unwrap();
    assert!(config_file.exists());
    let content = std::fs::read_to_string(config_file).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(parsed.get("palace_path").is_some());
}

#[test]
fn remote_settings_round_trip_and_preserve_other_keys() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("PALACE_MCP_MODE");
    std::env::remove_var("PALACE_REMOTE_ENDPOINT");
    std::env::remove_var("PALACE_API_KEY");

    let tmp = TempDir::new().unwrap();
    // Seed a config with an unrelated key that must survive writes.
    let config_file = tmp.path().join("config.json");
    std::fs::write(&config_file, r#"{"collection_name":"custom_coll"}"#).unwrap();

    let mut config = PalaceConfig::with_config_dir(Some(tmp.path()));
    assert_eq!(config.mcp_mode(), "local"); // default before any write

    config
        .save_remote_settings(
            Some("remote"),
            Some("example.com:8080"),
            Some("ps_secret_key_1234"),
        )
        .unwrap();

    // Re-read from disk to confirm persistence.
    let reread = PalaceConfig::with_config_dir(Some(tmp.path()));
    assert_eq!(reread.mcp_mode(), "remote");
    assert_eq!(
        reread.remote_endpoint().as_deref(),
        Some("example.com:8080")
    );
    assert_eq!(
        reread.remote_endpoint_url().as_deref(),
        Some("https://example.com:8080/mcp")
    );
    assert_eq!(
        reread.remote_api_key().as_deref(),
        Some("ps_secret_key_1234")
    );
    // Unrelated key preserved.
    assert_eq!(reread.collection_name(), "custom_coll");

    // The config file now holds the secret, so it must be owner-only (0600) on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&config_file)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "config.json must be 0600");
    }

    // Toggling off changes only the mode.
    let mut config2 = PalaceConfig::with_config_dir(Some(tmp.path()));
    config2
        .save_remote_settings(Some("local"), None, None)
        .unwrap();
    let reread2 = PalaceConfig::with_config_dir(Some(tmp.path()));
    assert_eq!(reread2.mcp_mode(), "local");
    assert_eq!(
        reread2.remote_api_key().as_deref(),
        Some("ps_secret_key_1234")
    );
}

#[test]
fn normalize_mcp_url_variants() {
    use palace::config::normalize_mcp_url;
    assert_eq!(normalize_mcp_url("example.com"), "https://example.com/mcp");
    assert_eq!(
        normalize_mcp_url("http://localhost:8080"),
        "http://localhost:8080/mcp"
    );
    assert_eq!(
        normalize_mcp_url("https://palace.co/mcp/"),
        "https://palace.co/mcp"
    );
    assert_eq!(
        normalize_mcp_url("https://palace.co/mcp"),
        "https://palace.co/mcp"
    );
}

#[test]
fn topic_wings_returns_defaults() {
    let tmp = TempDir::new().unwrap();
    let config = PalaceConfig::with_config_dir(Some(tmp.path()));
    let wings = config.topic_wings();
    assert!(wings.contains(&"technical".to_string()));
    assert!(wings.contains(&"emotions".to_string()));
}

#[test]
fn env_var_overrides_palace_path() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let custom_path = tmp.path().join("custom_palace");
    std::env::set_var("PALACE_PALACE_PATH", custom_path.to_string_lossy().as_ref());
    let config = PalaceConfig::with_config_dir(Some(tmp.path()));
    let result = config.palace_path();
    std::env::remove_var("PALACE_PALACE_PATH");
    assert_eq!(result, custom_path);
}
