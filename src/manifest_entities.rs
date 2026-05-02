//! Detect project entities from common package manifests.

use anyhow::{Context, Result};
use regex::Regex;
use std::collections::BTreeSet;
use std::path::Path;

pub fn detect_manifest_projects(project_dir: &Path) -> Result<Vec<String>> {
    let mut projects = BTreeSet::new();

    collect_name_from_file(
        &mut projects,
        &project_dir.join("Cargo.toml"),
        r#"(?m)^\s*name\s*=\s*"([^"]+)""#,
    )?;
    collect_name_from_file(
        &mut projects,
        &project_dir.join("package.json"),
        r#""name"\s*:\s*"([^"]+)""#,
    )?;
    collect_name_from_file(
        &mut projects,
        &project_dir.join("pyproject.toml"),
        r#"(?m)^\s*name\s*=\s*"([^"]+)""#,
    )?;
    collect_name_from_file(
        &mut projects,
        &project_dir.join("go.mod"),
        r#"(?m)^\s*module\s+([^\s]+)"#,
    )?;

    Ok(projects.into_iter().collect())
}

fn collect_name_from_file(
    projects: &mut BTreeSet<String>,
    path: &Path,
    pattern: &str,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading manifest {}", path.display()))?;
    let regex = Regex::new(pattern).context("compiling manifest regex")?;
    for capture in regex.captures_iter(&text) {
        if let Some(name) = capture.get(1) {
            let name = name.as_str().trim();
            if !name.is_empty() {
                projects.insert(name.to_string());
            }
        }
    }
    Ok(())
}
