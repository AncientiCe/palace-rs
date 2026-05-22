//! Room detection from folder structure and filename patterns.
//! Writes and reads palace.yaml for a project.
//! Port of room_detector_local.py.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub wing: String,
    pub rooms: Vec<Room>,
}

static SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "env",
    "dist",
    "build",
    ".next",
    "coverage",
    "target",
];

fn folder_room_map() -> HashMap<&'static str, &'static str> {
    [
        ("frontend", "frontend"),
        ("front_end", "frontend"),
        ("client", "frontend"),
        ("ui", "frontend"),
        ("views", "frontend"),
        ("components", "frontend"),
        ("pages", "frontend"),
        ("backend", "backend"),
        ("back_end", "backend"),
        ("server", "backend"),
        ("api", "backend"),
        ("routes", "backend"),
        ("services", "backend"),
        ("controllers", "backend"),
        ("models", "backend"),
        ("database", "backend"),
        ("db", "backend"),
        ("docs", "documentation"),
        ("doc", "documentation"),
        ("documentation", "documentation"),
        ("wiki", "documentation"),
        ("readme", "documentation"),
        ("notes", "documentation"),
        ("design", "design"),
        ("designs", "design"),
        ("mockups", "design"),
        ("wireframes", "design"),
        ("assets", "design"),
        ("costs", "costs"),
        ("cost", "costs"),
        ("budget", "costs"),
        ("finance", "costs"),
        ("pricing", "costs"),
        ("invoices", "costs"),
        ("meetings", "meetings"),
        ("meeting", "meetings"),
        ("calls", "meetings"),
        ("standup", "meetings"),
        ("minutes", "meetings"),
        ("team", "team"),
        ("staff", "team"),
        ("hr", "team"),
        ("hiring", "team"),
        ("employees", "team"),
        ("people", "team"),
        ("research", "research"),
        ("references", "research"),
        ("papers", "research"),
        ("planning", "planning"),
        ("roadmap", "planning"),
        ("strategy", "planning"),
        ("specs", "planning"),
        ("requirements", "planning"),
        ("tests", "testing"),
        ("test", "testing"),
        ("testing", "testing"),
        ("qa", "testing"),
        ("scripts", "scripts"),
        ("tools", "scripts"),
        ("utils", "scripts"),
        ("config", "configuration"),
        ("configs", "configuration"),
        ("settings", "configuration"),
        ("infrastructure", "configuration"),
        ("infra", "configuration"),
        ("deploy", "configuration"),
    ]
    .iter()
    .cloned()
    .collect()
}

/// Detect rooms from top-level folder structure.
pub fn detect_rooms_from_folders(project_dir: &Path) -> Vec<Room> {
    let map = folder_room_map();
    let mut found: HashMap<String, String> = HashMap::new();
    let skip: std::collections::HashSet<&str> = SKIP_DIRS.iter().cloned().collect();

    if let Ok(entries) = std::fs::read_dir(project_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if skip.contains(name.as_str()) {
                continue;
            }
            let key = name.to_lowercase().replace('-', "_");
            if let Some(&room_name) = map.get(key.as_str()) {
                found
                    .entry(room_name.to_string())
                    .or_insert_with(|| name.clone());
            } else if name.len() > 2 && name.chars().next().is_some_and(|c| c.is_alphabetic()) {
                let clean = name.to_lowercase().replace(['-', ' '], "_");
                found.entry(clean).or_insert_with(|| name.clone());
            }
        }
    }

    // Walk one level deeper
    if let Ok(entries) = std::fs::read_dir(project_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let parent_name = entry.file_name().to_string_lossy().to_string();
            if skip.contains(parent_name.as_str()) {
                continue;
            }
            if let Ok(sub_entries) = std::fs::read_dir(entry.path()) {
                for sub in sub_entries.flatten() {
                    if !sub.path().is_dir() {
                        continue;
                    }
                    let sub_name = sub.file_name().to_string_lossy().to_string();
                    if skip.contains(sub_name.as_str()) {
                        continue;
                    }
                    let key = sub_name.to_lowercase().replace('-', "_");
                    if let Some(&room_name) = map.get(key.as_str()) {
                        found
                            .entry(room_name.to_string())
                            .or_insert_with(|| sub_name.clone());
                    }
                }
            }
        }
    }

    let mut rooms: Vec<Room> = found
        .iter()
        .map(|(name, original)| Room {
            name: name.clone(),
            description: format!("Files from {original}/"),
            keywords: vec![name.clone(), original.to_lowercase()],
        })
        .collect();

    if !rooms.iter().any(|r| r.name == "general") {
        rooms.push(Room {
            name: "general".into(),
            description: "Files that don't fit other rooms".into(),
            keywords: vec![],
        });
    }

    rooms
}

/// Fallback: detect rooms from filename keyword frequency.
pub fn detect_rooms_from_files(project_dir: &Path) -> Vec<Room> {
    let map = folder_room_map();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let skip: std::collections::HashSet<&str> = SKIP_DIRS.iter().cloned().collect();

    fn walk(
        dir: &Path,
        map: &HashMap<&str, &str>,
        counts: &mut HashMap<String, usize>,
        skip: &std::collections::HashSet<&str>,
    ) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if !skip.contains(name.as_str()) {
                        walk(&path, map, counts, skip);
                    }
                } else {
                    let fname = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase()
                        .replace(['-', ' '], "_");
                    for (kw, room) in map {
                        if fname.contains(kw) {
                            *counts.entry(room.to_string()).or_default() += 1;
                        }
                    }
                }
            }
        }
    }

    walk(project_dir, &map, &mut counts, &skip);

    let mut sorted: Vec<(String, usize)> = counts.into_iter().filter(|(_, c)| *c >= 2).collect();
    sorted.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    sorted.truncate(6);

    if sorted.is_empty() {
        return vec![Room {
            name: "general".into(),
            description: "All project files".into(),
            keywords: vec![],
        }];
    }

    sorted
        .into_iter()
        .map(|(name, _)| Room {
            name: name.clone(),
            description: format!("Files related to {name}"),
            keywords: vec![name],
        })
        .collect()
}

/// Write a palace.yaml file for a project.
pub fn save_config(project_dir: &Path, project_name: &str, rooms: &[Room]) -> Result<PathBuf> {
    let config = ProjectConfig {
        wing: project_name.to_string(),
        rooms: rooms.to_vec(),
    };
    let yaml = serde_yaml::to_string(&config)?;
    let config_path = project_dir.join("palace.yaml");
    std::fs::write(&config_path, yaml)?;
    Ok(config_path)
}

/// Load an existing palace.yaml for the project.
pub fn load_config(project_dir: &Path) -> Result<ProjectConfig> {
    let path = project_dir.join("palace.yaml");
    if !path.exists() {
        anyhow::bail!(
            "No palace.yaml found in {}. Run: palace init {}",
            project_dir.display(),
            project_dir.display()
        );
    }
    let content = std::fs::read_to_string(&path)?;
    let config: ProjectConfig = serde_yaml::from_str(&content)?;
    Ok(config)
}

/// Interactive first-run detection: detect rooms, print proposal, ask approval.
pub fn detect_rooms_interactive(project_dir: &Path, yes: bool) -> Result<Vec<Room>> {
    let mut rooms = detect_rooms_from_folders(project_dir);
    let source = if rooms.len() <= 1 {
        rooms = detect_rooms_from_files(project_dir);
        "filename patterns"
    } else {
        "folder structure"
    };
    if rooms.is_empty() {
        rooms = vec![Room {
            name: "general".into(),
            description: "All project files".into(),
            keywords: vec![],
        }];
    }

    println!("\n{}", "=".repeat(55));
    println!("  MemPalace Init — Local setup");
    println!("{}", "=".repeat(55));
    println!(
        "\n  WING: {}",
        project_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );
    println!("  (rooms detected from {source})\n");
    for room in &rooms {
        println!("    ROOM: {}", room.name);
        println!("          {}", room.description);
    }
    println!("\n{}", "-".repeat(55));

    if yes {
        return Ok(rooms);
    }

    print!("  Accept rooms? [Y/n]: ");
    std::io::stdout().flush()?;
    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let answer = line.trim().to_lowercase();
    if answer == "n" || answer == "no" {
        println!("  Edit palace.yaml manually after it's written.");
    }

    Ok(rooms)
}
