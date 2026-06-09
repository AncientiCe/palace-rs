//! Configuration system for Palace.
//!
//! Load order: env vars > ~/.palace/config.json > defaults.

use anyhow::Result;
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_COLLECTION_NAME: &str = "palace_drawers";

pub const DEFAULT_TOPIC_WINGS: &[&str] = &[
    "emotions",
    "consciousness",
    "memory",
    "technical",
    "identity",
    "family",
    "creative",
];

pub fn normalize_wing_name(name: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_separator = false;
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch);
            previous_was_separator = false;
        } else if !previous_was_separator && !normalized.is_empty() {
            normalized.push('_');
            previous_was_separator = true;
        }
    }
    normalized.trim_matches('_').to_string()
}

fn default_hall_keywords() -> HashMap<String, Vec<String>> {
    let mut m = HashMap::new();
    m.insert(
        "emotions".into(),
        vec![
            "scared", "afraid", "worried", "happy", "sad", "love", "hate", "feel", "cry", "tears",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m.insert(
        "consciousness".into(),
        vec![
            "consciousness",
            "conscious",
            "aware",
            "real",
            "genuine",
            "soul",
            "exist",
            "alive",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m.insert(
        "memory".into(),
        vec![
            "memory", "remember", "forget", "recall", "archive", "palace", "store",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m.insert(
        "technical".into(),
        vec![
            "code", "python", "script", "bug", "error", "function", "api", "database", "server",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m.insert(
        "identity".into(),
        vec!["identity", "name", "who am i", "persona", "self"]
            .into_iter()
            .map(String::from)
            .collect(),
    );
    m.insert(
        "family".into(),
        vec![
            "family", "kids", "children", "daughter", "son", "parent", "mother", "father",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m.insert(
        "creative".into(),
        vec![
            "game", "gameplay", "player", "app", "design", "art", "music", "story",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FileConfig {
    palace_path: Option<String>,
    collection_name: Option<String>,
    topic_wings: Option<Vec<String>>,
    hall_keywords: Option<HashMap<String, Vec<String>>>,
    people_map: Option<HashMap<String, String>>,
    entity_languages: Option<Vec<String>>,
    /// MCP server mode: "local" (default) or "remote".
    mcp_mode: Option<String>,
    /// Remote palace-server endpoint (host, base URL, or full /mcp URL).
    remote_endpoint: Option<String>,
    /// API key (ps_*) for the remote palace-server.
    remote_api_key: Option<String>,
}

/// Normalise a configured endpoint into the palace-server `/mcp` URL.
/// Accepts a bare host, a base URL, or a URL already ending in `/mcp`.
pub fn normalize_mcp_url(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    if with_scheme.ends_with("/mcp") {
        with_scheme
    } else {
        format!("{with_scheme}/mcp")
    }
}

/// Runtime configuration for Palace.
///
/// Load priority: environment variables > ~/.palace/config.json > defaults.
#[derive(Debug, Clone)]
pub struct PalaceConfig {
    pub config_dir: PathBuf,
    file_config: FileConfig,
}

impl PalaceConfig {
    pub fn new() -> Self {
        Self::with_config_dir(None)
    }

    pub fn with_config_dir(config_dir: Option<&Path>) -> Self {
        let dir = config_dir
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_config_dir);

        let config_file = dir.join("config.json");
        let file_config: FileConfig = if config_file.exists() {
            std::fs::read_to_string(&config_file)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            FileConfig::default()
        };

        Self {
            config_dir: dir,
            file_config,
        }
    }

    fn default_config_dir() -> PathBuf {
        UserDirs::new()
            .map(|u| u.home_dir().join(".palace"))
            .unwrap_or_else(|| PathBuf::from(".palace"))
    }

    /// Path to the SQLite palace database file.
    pub fn palace_db_path(&self) -> PathBuf {
        self.palace_path().join("palace.db")
    }

    /// Palace data directory.
    pub fn palace_path(&self) -> PathBuf {
        if let Ok(v) = std::env::var("PALACE_PALACE_PATH") {
            return PathBuf::from(v);
        }
        self.file_config
            .palace_path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.config_dir.join("palace"))
    }

    pub fn collection_name(&self) -> &str {
        self.file_config
            .collection_name
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION_NAME)
    }

    pub fn topic_wings(&self) -> Vec<String> {
        self.file_config
            .topic_wings
            .clone()
            .unwrap_or_else(|| DEFAULT_TOPIC_WINGS.iter().map(|s| s.to_string()).collect())
    }

    pub fn hall_keywords(&self) -> HashMap<String, Vec<String>> {
        self.file_config
            .hall_keywords
            .clone()
            .unwrap_or_else(default_hall_keywords)
    }

    pub fn people_map(&self) -> HashMap<String, String> {
        let people_map_file = self.config_dir.join("people_map.json");
        if people_map_file.exists() {
            if let Ok(s) = std::fs::read_to_string(&people_map_file) {
                if let Ok(m) = serde_json::from_str(&s) {
                    return m;
                }
            }
        }
        self.file_config.people_map.clone().unwrap_or_default()
    }

    pub fn entity_languages(&self) -> Vec<String> {
        if let Ok(value) = std::env::var("PALACE_ENTITY_LANGUAGES") {
            let langs: Vec<String> = value
                .split(',')
                .filter_map(crate::i18n::canonical_language)
                .collect();
            if !langs.is_empty() {
                return langs;
            }
        }
        self.file_config
            .entity_languages
            .clone()
            .unwrap_or_else(|| vec!["en".to_string()])
    }

    pub fn identity_path(&self) -> PathBuf {
        self.config_dir.join("identity.txt")
    }

    pub fn known_names_path(&self) -> PathBuf {
        self.config_dir.join("known_names.json")
    }

    /// MCP server mode: "local" (default) or "remote".
    /// Priority: env `PALACE_MCP_MODE` > config.json > "local".
    pub fn mcp_mode(&self) -> String {
        if let Ok(v) = std::env::var("PALACE_MCP_MODE") {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_lowercase();
            }
        }
        self.file_config
            .mcp_mode
            .clone()
            .unwrap_or_else(|| "local".to_string())
    }

    /// Raw remote endpoint as configured (env `PALACE_REMOTE_ENDPOINT` > config.json).
    pub fn remote_endpoint(&self) -> Option<String> {
        std::env::var("PALACE_REMOTE_ENDPOINT")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| self.file_config.remote_endpoint.clone())
    }

    /// API key (ps_*) for the remote server (env `PALACE_API_KEY` > config.json).
    pub fn remote_api_key(&self) -> Option<String> {
        std::env::var("PALACE_API_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| self.file_config.remote_api_key.clone())
    }

    /// The configured endpoint normalised to a full `/mcp` URL, if set.
    pub fn remote_endpoint_url(&self) -> Option<String> {
        self.remote_endpoint().map(|e| normalize_mcp_url(&e))
    }

    /// Persist remote-mode settings into ~/.palace/config.json, preserving other keys.
    /// The file is written with owner-only (0600) permissions since it may hold the key.
    /// Any argument left `None` is left unchanged on disk.
    pub fn save_remote_settings(
        &mut self,
        mode: Option<&str>,
        endpoint: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.config_dir)?;
        let config_file = self.config_dir.join("config.json");

        // Load existing JSON object (preserving unknown keys) or start fresh.
        let mut root: serde_json::Value = if config_file.exists() {
            std::fs::read_to_string(&config_file)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        if !root.is_object() {
            root = serde_json::json!({});
        }
        let obj = root.as_object_mut().expect("root is an object");

        if let Some(mode) = mode {
            obj.insert("mcp_mode".into(), serde_json::json!(mode));
            self.file_config.mcp_mode = Some(mode.to_string());
        }
        if let Some(endpoint) = endpoint {
            obj.insert("remote_endpoint".into(), serde_json::json!(endpoint));
            self.file_config.remote_endpoint = Some(endpoint.to_string());
        }
        if let Some(api_key) = api_key {
            obj.insert("remote_api_key".into(), serde_json::json!(api_key));
            self.file_config.remote_api_key = Some(api_key.to_string());
        }

        std::fs::write(&config_file, serde_json::to_string_pretty(&root)?)?;
        restrict_permissions(&config_file)?;
        Ok(config_file)
    }

    /// Create config directory and write default config.json if it doesn't exist.
    pub fn init(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.config_dir)?;
        let config_file = self.config_dir.join("config.json");
        if !config_file.exists() {
            let default = serde_json::json!({
                "palace_path": self.palace_path().to_string_lossy(),
                "collection_name": DEFAULT_COLLECTION_NAME,
                "topic_wings": DEFAULT_TOPIC_WINGS,
                "hall_keywords": default_hall_keywords(),
                "entity_languages": ["en"],
            });
            std::fs::write(&config_file, serde_json::to_string_pretty(&default)?)?;
        }
        Ok(config_file)
    }

    pub fn save_people_map(&self, map: &HashMap<String, String>) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.config_dir)?;
        let path = self.config_dir.join("people_map.json");
        std::fs::write(&path, serde_json::to_string_pretty(map)?)?;
        Ok(path)
    }
}

impl Default for PalaceConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Restrict a file to owner read/write (0600) on Unix; no-op elsewhere.
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
