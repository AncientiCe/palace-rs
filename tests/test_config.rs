use palace::config::PalaceConfig;
use std::sync::Mutex;
use tempfile::TempDir;

// Serialise tests that touch env vars (env is process-global)
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn default_config_paths_are_stable() {
    let _guard = ENV_LOCK.lock().unwrap();
    // Ensure our special env vars aren't set from a prior test
    std::env::remove_var("PALACE_PALACE_PATH");
    std::env::remove_var("MEMPALACE_PALACE_PATH");
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

#[test]
fn migrate_legacy_from_copies_db_when_new_dir_already_exists() {
    let tmp = TempDir::new().unwrap();
    let legacy_dir = tmp.path().join(".mempalace");
    let new_dir = tmp.path().join(".palace");

    // Legacy layout: ~/.mempalace/palace/palace.db (+ identity)
    std::fs::create_dir_all(legacy_dir.join("palace")).unwrap();
    std::fs::write(legacy_dir.join("palace/palace.db"), b"legacy-db").unwrap();
    std::fs::write(legacy_dir.join("identity.txt"), b"i am here").unwrap();

    // New dir already exists (e.g. created by `palace init` or `mcp install`)
    // but contains no db yet.
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::write(new_dir.join("config.json"), b"{}").unwrap();

    let config = PalaceConfig::with_config_dir(Some(&new_dir));
    config.migrate_legacy_from(&legacy_dir);

    let migrated_db = new_dir.join("palace/palace.db");
    assert!(migrated_db.exists(), "db should be migrated from legacy");
    assert_eq!(std::fs::read(&migrated_db).unwrap(), b"legacy-db");
    assert_eq!(
        std::fs::read(new_dir.join("identity.txt")).unwrap(),
        b"i am here"
    );
    // Existing files in new dir are not overwritten.
    assert_eq!(std::fs::read(new_dir.join("config.json")).unwrap(), b"{}");
    // Breadcrumb is left behind so users can clean up.
    assert!(legacy_dir.join("MIGRATED_TO_PALACE.txt").exists());
}

#[test]
fn migrate_legacy_from_is_idempotent_and_preserves_new_files() {
    let tmp = TempDir::new().unwrap();
    let legacy_dir = tmp.path().join(".mempalace");
    let new_dir = tmp.path().join(".palace");

    std::fs::create_dir_all(legacy_dir.join("palace")).unwrap();
    std::fs::write(legacy_dir.join("palace/palace.db"), b"legacy-db").unwrap();

    std::fs::create_dir_all(new_dir.join("palace")).unwrap();
    std::fs::write(new_dir.join("palace/palace.db"), b"new-db").unwrap();

    let config = PalaceConfig::with_config_dir(Some(&new_dir));
    config.migrate_legacy_from(&legacy_dir);
    config.migrate_legacy_from(&legacy_dir);

    // Existing new db is not clobbered.
    assert_eq!(
        std::fs::read(new_dir.join("palace/palace.db")).unwrap(),
        b"new-db"
    );
}

#[test]
fn migrate_legacy_from_is_noop_without_legacy_dir() {
    let tmp = TempDir::new().unwrap();
    let new_dir = tmp.path().join(".palace");
    std::fs::create_dir_all(&new_dir).unwrap();

    let config = PalaceConfig::with_config_dir(Some(&new_dir));
    config.migrate_legacy_from(&tmp.path().join(".mempalace"));

    assert!(!new_dir.join("palace").exists());
}

#[test]
fn legacy_env_var_still_works() {
    let _lock = ENV_LOCK.lock().unwrap();
    std::env::remove_var("PALACE_PALACE_PATH");
    let tmp = TempDir::new().unwrap();
    let custom_path = tmp.path().join("legacy_palace");
    std::env::set_var(
        "MEMPALACE_PALACE_PATH",
        custom_path.to_string_lossy().as_ref(),
    );
    let config = PalaceConfig::with_config_dir(Some(tmp.path()));
    let result = config.palace_path();
    std::env::remove_var("MEMPALACE_PALACE_PATH");
    assert_eq!(result, custom_path);
}
