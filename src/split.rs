//! Split concatenated Claude Code mega-transcript files into per-session files.
//!
//! Identifies true session starts by "Claude Code v" headers (not context restores),
//! extracts timestamps, people names, and subject from first prompt,
//! writes per-session files named: STEM__DATE_TIME_People_subject.txt
//!
//! Port of split_mega_files.py.

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

static TS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"⏺\s+(\d{1,2}:\d{2}\s+[AP]M)\s+\w+,\s+(\w+)\s+(\d{1,2}),\s+(\d{4})").unwrap()
});

static SKIP_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(\./|cd |ls |python|bash|git |cat |source |export |claude|./activate)").unwrap()
});

static MONTHS: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {
    [
        ("January", "01"),
        ("February", "02"),
        ("March", "03"),
        ("April", "04"),
        ("May", "05"),
        ("June", "06"),
        ("July", "07"),
        ("August", "08"),
        ("September", "09"),
        ("October", "10"),
        ("November", "11"),
        ("December", "12"),
    ]
    .iter()
    .cloned()
    .collect()
});

fn is_true_session_start(lines: &[&str], idx: usize) -> bool {
    let nearby: String = lines[idx..idx.saturating_add(6).min(lines.len())].join("");
    !nearby.contains("Ctrl+E") && !nearby.contains("previous messages")
}

fn find_session_boundaries(lines: &[&str]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(i, line)| line.contains("Claude Code v") && is_true_session_start(lines, *i))
        .map(|(i, _)| i)
        .collect()
}

fn extract_timestamp(lines: &[&str]) -> (Option<String>, Option<String>) {
    for line in lines.iter().take(50) {
        if let Some(cap) = TS_RE.captures(line) {
            let time_str = cap.get(1).map_or("", |m| m.as_str());
            let month = cap.get(2).map_or("", |m| m.as_str());
            let day = cap.get(3).map_or("", |m| m.as_str());
            let year = cap.get(4).map_or("", |m| m.as_str());
            let mon = MONTHS.get(month).copied().unwrap_or("00");
            let day_z = format!("{:02}", day.parse::<u32>().unwrap_or(1));
            let time_safe = time_str.replace([':', ' '], "");
            let iso = format!("{year}-{mon}-{day_z}");
            let human = format!("{year}-{mon}-{day_z}_{time_safe}");
            return (Some(human), Some(iso));
        }
    }
    (None, None)
}

fn load_known_people(config_dir: &Path) -> Vec<String> {
    let path = config_dir.join("known_names.json");
    if path.exists() {
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                let names = if let Some(arr) = v.as_array() {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                } else if let Some(obj) = v.as_object() {
                    obj.get("names")
                        .and_then(|n| n.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                if !names.is_empty() {
                    return names;
                }
            }
        }
    }
    vec!["Alice", "Ben", "Riley", "Max", "Sam", "Devon", "Jordan"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn extract_people(lines: &[&str], known: &[String]) -> Vec<String> {
    let text = lines
        .iter()
        .take(100)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let mut found = std::collections::HashSet::new();
    for person in known {
        let re = Regex::new(&format!(r"(?i)\b{}\b", regex::escape(person))).unwrap();
        if re.is_match(&text) {
            found.insert(person.clone());
        }
    }
    let mut sorted: Vec<String> = found.into_iter().collect();
    sorted.sort();
    sorted
}

fn extract_subject(lines: &[&str]) -> String {
    for line in lines {
        if let Some(stripped) = line.strip_prefix("> ") {
            let prompt = stripped.trim();
            if prompt.is_empty() || SKIP_RE.is_match(prompt) || prompt.len() <= 5 {
                continue;
            }
            let subject: String = prompt
                .chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-')
                .collect();
            let subject = subject.split_whitespace().collect::<Vec<_>>().join("-");
            let subject = &subject[..subject.len().min(60)];
            return subject.to_string();
        }
    }
    "session".to_string()
}

fn sanitize_filename(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let re = Regex::new(r"_+").unwrap();
    re.replace_all(&s, "_").to_string()
}

fn split_file(
    filepath: &Path,
    output_dir: Option<&Path>,
    dry_run: bool,
    known_people: &[String],
) -> Result<Vec<PathBuf>> {
    let content = std::fs::read_to_string(filepath)?;
    let lines: Vec<&str> = content.lines().collect();
    let boundaries = find_session_boundaries(&lines);

    if boundaries.len() < 2 {
        return Ok(vec![]);
    }

    let mut boundaries = boundaries;
    boundaries.push(lines.len());

    let out_dir = output_dir.unwrap_or_else(|| filepath.parent().unwrap_or(Path::new(".")));
    let stem = filepath.file_stem().unwrap_or_default().to_string_lossy();
    let stem_safe = &sanitize_filename(&stem)[..stem.len().min(40)];

    let mut written = Vec::new();

    for (i, (start, end)) in boundaries.iter().zip(boundaries.iter().skip(1)).enumerate() {
        let chunk: Vec<&str> = lines[*start..*end].to_vec();
        if chunk.len() < 10 {
            continue;
        }

        let (ts_human, _) = extract_timestamp(&chunk);
        let people = extract_people(&chunk, known_people);
        let subject = extract_subject(&chunk);

        let ts_part = ts_human.unwrap_or_else(|| format!("part{:02}", i + 1));
        let people_part = if people.is_empty() {
            "unknown".to_string()
        } else {
            people.iter().take(3).cloned().collect::<Vec<_>>().join("-")
        };

        let name = sanitize_filename(&format!(
            "{stem_safe}__{ts_part}_{people_part}_{subject}.txt"
        ));
        let out_path = out_dir.join(&name);

        if dry_run {
            println!(
                "  [{}/{}] {name}  ({} lines)",
                i + 1,
                boundaries.len() - 1,
                chunk.len()
            );
        } else {
            std::fs::write(&out_path, chunk.join("\n"))?;
            println!("  ✓ {name}  ({} lines)", chunk.len());
        }
        written.push(out_path);
    }

    Ok(written)
}

/// Run the split command.
pub fn run(
    source: Option<&Path>,
    output_dir: Option<&Path>,
    min_sessions: usize,
    dry_run: bool,
    file: Option<&Path>,
) -> Result<()> {
    let home = directories::UserDirs::new()
        .map(|u| u.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let src_dir = source
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("MEMPALACE_SOURCE_DIR")
                .ok()
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| home.join("Desktop/transcripts"));

    let config_dir = home.join(".palace");
    let known_people = load_known_people(&config_dir);

    let files: Vec<PathBuf> = if let Some(f) = file {
        vec![f.to_path_buf()]
    } else {
        let mut f: Vec<PathBuf> = std::fs::read_dir(&src_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "txt"))
            .collect();
        f.sort();
        f
    };

    let mega_files: Vec<(PathBuf, usize)> = files
        .iter()
        .filter_map(|f| {
            let content = std::fs::read_to_string(f).ok()?;
            let lines: Vec<&str> = content.lines().collect();
            let n = find_session_boundaries(&lines).len();
            if n >= min_sessions {
                Some((f.clone(), n))
            } else {
                None
            }
        })
        .collect();

    if mega_files.is_empty() {
        println!(
            "No mega-files found in {} (min {} sessions).",
            src_dir.display(),
            min_sessions
        );
        return Ok(());
    }

    println!("\n{}", "=".repeat(60));
    println!(
        "  Mega-file splitter — {}",
        if dry_run { "DRY RUN" } else { "SPLITTING" }
    );
    println!("{}", "=".repeat(60));
    println!("  Source:      {}", src_dir.display());
    println!(
        "  Output:      {}",
        output_dir
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "same dir as source".to_string())
    );
    println!("  Mega-files:  {}", mega_files.len());
    println!("{}\n", "-".repeat(60));

    let mut total_written = 0usize;
    for (f, n_sessions) in &mega_files {
        println!(
            "  {}  ({n_sessions} sessions, {}KB)",
            f.file_name().unwrap_or_default().to_string_lossy(),
            f.metadata().map(|m| m.len() / 1024).unwrap_or(0)
        );
        let written = split_file(f, output_dir, dry_run, &known_people)?;
        total_written += written.len();

        if !dry_run && !written.is_empty() {
            let backup = f.with_extension("mega_backup");
            std::fs::rename(f, &backup)?;
            println!(
                "  → Original renamed to {}\n",
                backup.file_name().unwrap_or_default().to_string_lossy()
            );
        } else {
            println!();
        }
    }

    println!("{}", "-".repeat(60));
    if dry_run {
        println!(
            "  DRY RUN — would create {total_written} files from {} mega-files",
            mega_files.len()
        );
    } else {
        println!(
            "  Done — created {total_written} files from {} mega-files",
            mega_files.len()
        );
    }
    println!();
    Ok(())
}
