//! Interactive first-run wizard.
//!
//! Detects entities (people, projects) from a project directory and persists them.
//! Port of onboarding.py + entity_detector.py + entity_registry.py.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityEntry {
    pub name: String,
    pub entity_type: String,
    pub aliases: Vec<String>,
    pub source: String,
}

/// Load entity registry from a JSON file.
pub fn load_entity_registry(path: &Path) -> HashMap<String, EntityEntry> {
    if !path.exists() {
        return HashMap::new();
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Save entity registry to a JSON file.
pub fn save_entity_registry(path: &Path, registry: &HashMap<String, EntityEntry>) -> Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(registry)?)?;
    Ok(())
}

/// Heuristically detect possible people names and project names from a text corpus.
fn detect_entities_from_text(text: &str) -> (Vec<String>, Vec<String>) {
    let mut people: Vec<String> = Vec::new();
    let mut projects: Vec<String> = Vec::new();

    // Very simple heuristic: look for capitalized words that appear multiple times
    let mut word_counts: HashMap<String, usize> = HashMap::new();
    for word in text.split_whitespace() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
        if clean.len() > 2
            && clean.chars().next().is_some_and(|c| c.is_uppercase())
            && clean.chars().skip(1).all(|c| c.is_alphabetic() || c == '-')
        {
            *word_counts.entry(clean.to_string()).or_default() += 1;
        }
    }

    // Common titles/words to skip
    static SKIP: &[&str] = &[
        "The", "This", "That", "With", "From", "Your", "When", "They", "What", "Which", "Where",
        "There", "Into", "About", "After", "Before", "During", "While", "Using", "Each", "Every",
        "Some", "More",
    ];

    for (word, count) in word_counts.iter() {
        if SKIP.contains(&word.as_str()) || *count < 2 {
            continue;
        }
        // Very rough people/project detection
        if word.len() <= 12 && !word.contains('-') {
            people.push(word.clone());
        } else {
            projects.push(word.clone());
        }
    }

    people.sort();
    projects.sort();
    (people, projects)
}

/// Run the interactive onboarding wizard.
pub fn run_onboarding(project_dir: &Path, config_dir: &Path) -> Result<()> {
    println!("\n{}", "=".repeat(55));
    println!("  MemPalace Onboarding");
    println!("{}", "=".repeat(55));
    println!("\n  Scanning project for entities...\n");

    // Read a sample of text from the project
    let mut sample_text = String::new();
    if let Ok(entries) = std::fs::read_dir(project_dir) {
        for entry in entries.flatten().take(20) {
            let path = entry.path();
            if path.is_file() {
                let ext = path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase();
                if matches!(ext.as_str(), "md" | "txt" | "rs" | "py" | "js" | "ts") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        sample_text.push_str(&content[..content.len().min(2000)]);
                        sample_text.push('\n');
                    }
                }
            }
        }
    }

    let (people, projects) = detect_entities_from_text(&sample_text);

    if !people.is_empty() {
        println!("  Detected possible people: {}", people.join(", "));
    }
    if !projects.is_empty() {
        println!("  Detected possible projects: {}", projects.join(", "));
    }

    let registry_path = config_dir.join("entities.json");
    let mut registry = load_entity_registry(&registry_path);

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    // Confirm people
    for name in &people {
        print!("  Is '{name}' a person? [y/N]: ");
        stdout.flush()?;
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        if line.trim().to_lowercase() == "y" {
            registry.insert(
                name.to_lowercase(),
                EntityEntry {
                    name: name.clone(),
                    entity_type: "person".to_string(),
                    aliases: vec![],
                    source: "onboarding".to_string(),
                },
            );
        }
    }

    // Confirm projects
    for name in &projects {
        print!("  Is '{name}' a project? [y/N]: ");
        stdout.flush()?;
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        if line.trim().to_lowercase() == "y" {
            registry.insert(
                name.to_lowercase(),
                EntityEntry {
                    name: name.clone(),
                    entity_type: "project".to_string(),
                    aliases: vec![],
                    source: "onboarding".to_string(),
                },
            );
        }
    }

    if !registry.is_empty() {
        std::fs::create_dir_all(config_dir)?;
        save_entity_registry(&registry_path, &registry)?;
        println!(
            "\n  Saved {} entities to {}",
            registry.len(),
            registry_path.display()
        );
    } else {
        println!("\n  No entities confirmed. You can add them manually later.");
    }

    println!("\n  Setup complete! Next:");
    println!("    palace mine {}", project_dir.display());
    println!("{}\n", "=".repeat(55));

    Ok(())
}
