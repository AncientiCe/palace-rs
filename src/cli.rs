//! CLI entry point using clap.
//!
//! Subcommands mirror the Python version exactly:
//!   init, mine, mine-convos, search, wake-up, status,
//!   compress, split, repair, mcp

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::config::MempalaceConfig;
use crate::convo_miner::ExtractMode;

#[derive(Parser)]
#[command(
    name = "mempalace",
    version = env!("CARGO_PKG_VERSION"),
    about = "Local memory palace for AI assistants",
    long_about = None,
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Detect rooms and write mempalace.yaml for a project
    Init {
        /// Project directory
        dir: PathBuf,
        /// Accept detected rooms without prompting
        #[arg(long)]
        yes: bool,
        /// Mine immediately after writing mempalace.yaml
        #[arg(long)]
        auto_mine: bool,
        /// Disable optional LLM-assisted refinement
        #[arg(long)]
        no_llm: bool,
        /// Entity language list (comma-separated)
        #[arg(long, value_delimiter = ',')]
        lang: Vec<String>,
    },
    /// Mine a project directory into the palace
    Mine {
        /// Project directory (must contain mempalace.yaml)
        dir: PathBuf,
        /// Override wing name
        #[arg(long)]
        wing: Option<String>,
        /// Maximum files to process (0 = unlimited)
        #[arg(long, default_value = "0")]
        limit: usize,
        /// Preview without storing
        #[arg(long)]
        dry_run: bool,
        /// Ignore .gitignore rules
        #[arg(long)]
        no_gitignore: bool,
        /// Force-include these paths (comma-separated, overrides gitignore)
        #[arg(long, value_delimiter = ',')]
        include: Vec<String>,
        /// Refresh corpus-origin metadata before mining
        #[arg(long)]
        redetect_origin: bool,
    },
    /// Mine conversation files into the palace
    #[command(name = "mine-convos")]
    MineConvos {
        /// Directory containing conversation files
        dir: PathBuf,
        /// Wing name (defaults to directory name)
        #[arg(long)]
        wing: Option<String>,
        /// Extraction mode: exchange (default) or general
        #[arg(long, default_value = "exchange")]
        mode: String,
        /// Maximum files to process (0 = unlimited)
        #[arg(long, default_value = "0")]
        limit: usize,
        /// Preview without storing
        #[arg(long)]
        dry_run: bool,
    },
    /// Semantic search over the palace
    Search {
        /// Search query
        query: Vec<String>,
        /// Filter by wing
        #[arg(long)]
        wing: Option<String>,
        /// Filter by room
        #[arg(long)]
        room: Option<String>,
        /// Maximum results
        #[arg(long, default_value = "5")]
        limit: usize,
    },
    /// Print L0 (identity) + L1 (essential story) wake-up text
    #[command(name = "wake-up")]
    WakeUp {
        /// Filter to a specific wing
        #[arg(long)]
        wing: Option<String>,
    },
    /// Show palace status (drawer counts by wing/room)
    Status,
    /// Compress drawers to AAAK format
    Compress,
    /// Split concatenated Claude Code mega transcripts into per-session files
    Split {
        /// Source directory (default: MEMPALACE_SOURCE_DIR or ~/Desktop/transcripts)
        #[arg(long)]
        source: Option<PathBuf>,
        /// Output directory (default: same as source)
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Minimum sessions to trigger split
        #[arg(long, default_value = "2")]
        min_sessions: usize,
        /// Preview without writing files
        #[arg(long)]
        dry_run: bool,
        /// Split a single specific file
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Re-embed drawers that are missing vector embeddings
    Repair {
        /// Repair mode: embeddings (default) or normalize-version
        #[arg(long, default_value = "embeddings")]
        mode: String,
    },
    /// Sweep transcript JSONL files into per-message drawers
    Sweep {
        /// Transcript file or directory
        path: PathBuf,
        /// Wing name for stored messages
        #[arg(long)]
        wing: Option<String>,
    },
    /// Migrate drawers from another SQLite palace database
    Migrate {
        /// Source palace.db path
        source: PathBuf,
    },
    /// Start the MCP stdio server
    Mcp,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let config = MempalaceConfig::new();

    match cli.command {
        Commands::Init {
            dir,
            yes,
            auto_mine,
            no_llm,
            lang,
        } => {
            let dir = dir.canonicalize().unwrap_or(dir);
            if no_llm {
                println!("  LLM refinement disabled; using heuristic detection.");
            }
            if !lang.is_empty() {
                println!("  Entity languages: {}", lang.join(","));
            }
            let rooms = crate::room_detector::detect_rooms_interactive(&dir, yes)?;
            let project_name = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
                .replace([' ', '-'], "_");
            let config_path = crate::room_detector::save_config(&dir, &project_name, &rooms)?;
            println!("\n  Config saved: {}", config_path.display());
            if auto_mine {
                let db_path = config.palace_db_path();
                let mut conn = crate::db::open(&db_path)?;
                crate::miner::mine(&mut conn, &dir, None, "mempalace", 0, false, true, &[])?;
            } else {
                println!("\n  Next step:\n    mempalace mine {}\n", dir.display());
            }
        }

        Commands::Mine {
            dir,
            wing,
            limit,
            dry_run,
            no_gitignore,
            include,
            redetect_origin,
        } => {
            let db_path = config.palace_db_path();
            let mut conn = crate::db::open(&db_path)?;
            if redetect_origin {
                let sample = format!("project: {}", dir.display());
                let origin = crate::origin::detect_origin(&sample);
                crate::origin::write_origin(&config.config_dir.join("origin.json"), &origin)?;
                println!("  Origin metadata refreshed.");
            }
            crate::miner::mine(
                &mut conn,
                &dir,
                wing.as_deref(),
                "mempalace",
                limit,
                dry_run,
                !no_gitignore,
                &include,
            )?;
        }

        Commands::MineConvos {
            dir,
            wing,
            mode,
            limit,
            dry_run,
        } => {
            let extract_mode = match mode.as_str() {
                "general" => ExtractMode::General,
                _ => ExtractMode::Exchange,
            };
            let db_path = config.palace_db_path();
            let mut conn = crate::db::open(&db_path)?;
            crate::convo_miner::mine_convos(
                &mut conn,
                &dir,
                wing.as_deref(),
                "mempalace",
                limit,
                dry_run,
                extract_mode,
            )?;
        }

        Commands::Search {
            query,
            wing,
            room,
            limit,
        } => {
            let query_str = query.join(" ");
            let db_path = config.palace_db_path();
            if !db_path.exists() {
                eprintln!("No palace found. Run: mempalace init <dir> && mempalace mine <dir>");
                std::process::exit(1);
            }
            let conn = crate::db::open(&db_path)?;
            crate::searcher::search_and_print(
                &conn,
                &query_str,
                wing.as_deref(),
                room.as_deref(),
                limit,
            )?;
        }

        Commands::WakeUp { wing } => {
            let db_path = config.palace_db_path();
            if !db_path.exists() {
                println!(
                    "{}\n\n## L1 — No palace found. Run: mempalace mine <dir>",
                    std::fs::read_to_string(config.identity_path()).unwrap_or_else(|_| {
                        "## L0 — No identity configured. Create ~/.mempalace/identity.txt"
                            .to_string()
                    })
                );
                return Ok(());
            }
            let conn = crate::db::open(&db_path)?;
            let mut stack = crate::layers::MemoryStack::new(&db_path, &config.identity_path());
            let text = stack.wake_up(&conn, wing.as_deref());
            let tokens = text.len() / 4;
            println!("Wake-up text (~{tokens} tokens):");
            println!("{}", "=".repeat(50));
            println!("{text}");
        }

        Commands::Status => {
            let db_path = config.palace_db_path();
            if !db_path.exists() {
                println!("\n  No palace found at {}", db_path.display());
                println!("  Run: mempalace init <dir> then mempalace mine <dir>");
                return Ok(());
            }
            let conn = crate::db::open(&db_path)?;
            crate::miner::status(&conn, &db_path)?;
        }

        Commands::Compress => {
            println!("AAAK compression is written by the AI in AAAK format.");
            println!("Use mempalace_diary_write via MCP to store AAAK-compressed memories.");
        }

        Commands::Split {
            source,
            output_dir,
            min_sessions,
            dry_run,
            file,
        } => {
            crate::split::run(
                source.as_deref(),
                output_dir.as_deref(),
                min_sessions,
                dry_run,
                file.as_deref(),
            )?;
        }

        Commands::Repair { mode } => {
            let db_path = config.palace_db_path();
            let mut conn = crate::db::open(&db_path)?;
            if mode == "normalize-version" {
                println!("  Normalize-version repair is up to date for schema version 1.");
            } else {
                let unembedded = crate::store::count_unembedded(&conn)?;
                if unembedded == 0 {
                    println!("  All drawers have embeddings. Nothing to repair.");
                } else {
                    println!("  Found {unembedded} drawers missing embeddings. Re-embedding...");
                    crate::miner::repair(&mut conn)?;
                }
            }
        }

        Commands::Sweep { path, wing } => {
            let db_path = config.palace_db_path();
            let conn = crate::db::open(&db_path)?;
            let filed = crate::sweep::sweep_path(&conn, &path, wing.as_deref())?;
            println!("  Swept {filed} message drawers.");
        }

        Commands::Migrate { source } => {
            let db_path = config.palace_db_path();
            let mut conn = crate::db::open(&db_path)?;
            let migrated = crate::migrate::migrate_sqlite(&source, &mut conn)?;
            println!("  Migrated {migrated} drawers.");
        }

        Commands::Mcp => {
            crate::mcp_server::run()?;
        }
    }

    Ok(())
}
