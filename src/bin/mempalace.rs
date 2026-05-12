//! Backwards-compatibility shim: `mempalace` has been renamed to `palace`.
//!
//! This binary prints a deprecation notice to stderr and then runs the real
//! `palace` binary with all arguments forwarded. It ships in 0.2.x releases
//! only and will be removed in 0.3.0.

use anyhow::{Context, Result};
use std::env;
use std::process::Command;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .without_time()
        .init();
    tracing::warn!(
        "`mempalace` has been renamed to `palace`. \
         Please update your scripts and MCP configs. \
         Run `palace install` to migrate automatically. \
         This shim will be removed in 0.3.0."
    );

    let palace_bin = env::current_exe()
        .context("failed to resolve current exe")?
        .parent()
        .context("current exe has no parent directory")?
        .join(if cfg!(windows) {
            "palace.exe"
        } else {
            "palace"
        });

    let args: Vec<String> = env::args().skip(1).collect();

    let status = Command::new(&palace_bin)
        .args(&args)
        .status()
        .with_context(|| format!("failed to run palace at {}", palace_bin.display()))?;

    std::process::exit(status.code().unwrap_or(1));
}
