//! CLI entry point using clap.
//!
//! Subcommands mirror the Python version exactly:
//!   init, mine, mine-convos, search, wake-up, status,
//!   compress, split, repair, mcp

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::warn;

use crate::config::PalaceConfig;
use crate::convo_miner::ExtractMode;

#[derive(Parser)]
#[command(
    name = "palace",
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
    /// Detect rooms and write palace.yaml for a project
    Init {
        /// Project directory
        dir: PathBuf,
        /// Accept detected rooms without prompting
        #[arg(long)]
        yes: bool,
        /// Mine immediately after writing palace.yaml
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
        /// Project directory (must contain palace.yaml)
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
    /// Show automatic MCP usage gains and estimated savings
    Gain {
        /// Filter to one project
        #[arg(long)]
        project: Option<String>,
        /// Time window: 7d, 24h, 30d, or all
        #[arg(long, default_value = "30d")]
        since: String,
        /// Show recent usage events instead of the summary
        #[arg(long)]
        history: bool,
        /// History event limit
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Delete gain usage events, optionally only for --project
        #[arg(long)]
        reset: bool,
    },
    /// Compress drawers to AAAK format
    Compress,
    /// Split concatenated Claude Code mega transcripts into per-session files
    Split {
        /// Source directory (default: PALACE_SOURCE_DIR or ~/Desktop/transcripts)
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
    /// Register the MCP server with local AI clients
    Install {
        /// Client to configure: cursor, codex, claude, claude-desktop, or all
        #[arg(long, default_value = "all")]
        client: String,
        /// Configure all supported clients
        #[arg(long)]
        all: bool,
        /// Config scope: user or project
        #[arg(long, default_value = "user")]
        scope: String,
        /// Project directory for project-scoped Cursor config
        #[arg(long)]
        path: Option<PathBuf>,
        /// Preview changes without writing files
        #[arg(long)]
        dry_run: bool,
        /// Skip installing agent rule files
        #[arg(long)]
        no_rule: bool,
        /// Reserved for future overwrite prompts
        #[arg(long)]
        force: bool,
    },
    /// Remove the Palace MCP server from local AI clients
    Uninstall {
        /// Client to configure: cursor, codex, claude, claude-desktop, or all
        #[arg(long, default_value = "all")]
        client: String,
        /// Remove all supported clients
        #[arg(long)]
        all: bool,
        /// Config scope: user or project
        #[arg(long, default_value = "user")]
        scope: String,
        /// Project directory for project-scoped Cursor config
        #[arg(long)]
        path: Option<PathBuf>,
        /// Preview changes without writing files
        #[arg(long)]
        dry_run: bool,
        /// Skip removing agent rule files
        #[arg(long)]
        no_rule: bool,
    },
    /// Show MCP client configuration and palace status
    Doctor {
        /// Client to inspect: cursor, codex, claude, claude-desktop, or all
        #[arg(long, default_value = "all")]
        client: String,
        /// Inspect all supported clients
        #[arg(long)]
        all: bool,
        /// Config scope: user or project
        #[arg(long, default_value = "user")]
        scope: String,
        /// Project directory for project-scoped Cursor config
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Seed durable KG facts that help installed agents use Palace consistently
    #[command(name = "seed-adoption-facts")]
    SeedAdoptionFacts {
        /// Project/entity name to seed facts for
        #[arg(long)]
        project: Option<String>,
    },
    /// Export all palace drawers to a portable JSON file (embeddings excluded)
    Export {
        /// Output file path (default: palace-export.json)
        #[arg(long, default_value = "palace-export.json")]
        output: PathBuf,
    },
    /// Import palace drawers from a JSON export file
    Import {
        /// Path to the export JSON file produced by `palace export`
        file: PathBuf,
    },
    /// Watch a project directory and re-mine changed files automatically
    Watch {
        /// Project directory (must contain palace.yaml)
        dir: PathBuf,
        /// Override wing name
        #[arg(long)]
        wing: Option<String>,
    },
    /// Start the MCP stdio server
    Mcp,
    /// Handle a Cursor agent hook event (used by hooks.json)
    Hook {
        /// Hook event name (e.g. session-start)
        event: String,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let config = PalaceConfig::new();

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
                crate::miner::mine(&mut conn, &dir, None, "palace", 0, false, true, &[])?;
            } else {
                println!("\n  Next step:\n    palace mine {}\n", dir.display());
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
                "palace",
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
                "palace",
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
                warn!("No palace found. Run: palace init <dir> && palace mine <dir>");
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
                    "{}\n\n## L1 — No palace found. Run: palace mine <dir>",
                    std::fs::read_to_string(config.identity_path()).unwrap_or_else(|_| {
                        "## L0 — No identity configured. Create ~/.palace/identity.txt".to_string()
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
                println!("  Run: palace init <dir> then palace mine <dir>");
                return Ok(());
            }
            let conn = crate::db::open(&db_path)?;
            crate::miner::status(&conn, &db_path)?;
        }

        Commands::Gain {
            project,
            since,
            history,
            limit,
            json,
            reset,
        } => {
            let db_path = config.palace_db_path();
            if !db_path.exists() {
                println!("\n  No palace found at {}", db_path.display());
                println!("  Run: palace init <dir> then palace mine <dir>");
                return Ok(());
            }
            let conn = crate::db::open(&db_path)?;
            let since = crate::gain::SinceWindow::parse(&since)?;
            let options = crate::gain::GainOptions {
                project: project.clone(),
                since,
            };

            if reset {
                let deleted = crate::gain::reset(&conn, project.as_deref())?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "success": true,
                            "deleted": deleted,
                            "project": project
                        }))?
                    );
                } else if let Some(project) = project {
                    println!("Deleted {deleted} MemPalace gain events for project {project}.");
                } else {
                    println!("Deleted {deleted} MemPalace gain events.");
                }
                return Ok(());
            }

            if history {
                let events = crate::gain::history(&conn, &options, limit)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&events)?);
                } else {
                    print!("{}", crate::gain::render_history(&events));
                }
            } else {
                let report = crate::gain::summarize(&conn, &options)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", crate::gain::render_text(&report));
                }
            }
        }

        Commands::Compress => {
            println!("AAAK compression is written by the AI in AAAK format.");
            println!("Use palace_diary_write via MCP to store AAAK-compressed memories.");
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

        Commands::Install {
            client,
            all,
            scope,
            path,
            dry_run,
            no_rule,
            force,
        } => {
            let options = install_options(&client, all, &scope, path, dry_run, force)?;
            let options = with_rule_option(options, !no_rule);
            let report = crate::install::install_clients(&options)?;
            let action = if dry_run { "would update" } else { "updated" };
            crate::install::print_install_report(action, &report);
            if !dry_run {
                println!("  Restart Cursor, Codex, Claude Code, or Claude Desktop to load the MCP server.");
            }
        }

        Commands::Uninstall {
            client,
            all,
            scope,
            path,
            dry_run,
            no_rule,
        } => {
            let options = install_options(&client, all, &scope, path, dry_run, false)?;
            let options = with_rule_option(options, !no_rule);
            let report = crate::install::uninstall_clients(&options)?;
            let action = if dry_run { "would update" } else { "updated" };
            crate::install::print_install_report(action, &report);
        }

        Commands::Doctor {
            client,
            all,
            scope,
            path,
        } => {
            let options = install_options(&client, all, &scope, path, false, false)?;
            let report = crate::install::doctor(&options)?;
            crate::install::print_doctor_report(&report);
        }

        Commands::SeedAdoptionFacts { project } => {
            let db_path = config.palace_db_path();
            let conn = crate::db::open(&db_path)?;
            let project = project.unwrap_or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "current project".to_string())
            });
            let report = crate::knowledge_graph::seed_agent_adoption_facts(&conn, &project)?;
            println!(
                "  Seeded adoption facts for {project}: {} inserted, {} unchanged.",
                report.inserted, report.unchanged
            );
        }

        Commands::Export { output } => {
            let db_path = config.palace_db_path();
            if !db_path.exists() {
                warn!("No palace found. Run: palace init <dir> && palace mine <dir>");
                std::process::exit(1);
            }
            let conn = crate::db::open(&db_path)?;
            let doc = crate::export::export_drawers(&conn)?;
            let json = serde_json::to_string_pretty(&doc)?;
            std::fs::write(&output, &json)?;
            println!("  Exported {} drawer(s) to {}", doc.total, output.display());
        }

        Commands::Import { file } => {
            let db_path = config.palace_db_path();
            let json = std::fs::read_to_string(&file)?;
            let doc: crate::export::ExportDoc = serde_json::from_str(&json)?;
            let conn = crate::db::open(&db_path)?;
            let inserted = crate::export::import_drawers(&conn, &doc)?;
            println!(
                "  Imported {inserted} new drawer(s) from {} (skipped {} already present)",
                file.display(),
                doc.total.saturating_sub(inserted)
            );
        }

        Commands::Watch { dir, wing } => {
            let db_path = config.palace_db_path();
            if !db_path.exists() {
                warn!("No palace found. Run: palace init <dir> && palace mine <dir>");
                std::process::exit(1);
            }
            crate::watcher::watch(&db_path, &dir, wing.as_deref())?;
        }

        Commands::Mcp => {
            crate::mcp_server::run()?;
        }

        Commands::Hook { event } => {
            crate::install::run_hook(&event)?;
        }
    }

    Ok(())
}

fn install_options(
    client: &str,
    all: bool,
    scope: &str,
    path: Option<PathBuf>,
    dry_run: bool,
    force: bool,
) -> Result<crate::install::InstallOptions> {
    let client = if all {
        crate::install::Client::All
    } else {
        client.parse::<crate::install::Client>()?
    };
    let scope = scope.parse::<crate::install::Scope>()?;
    let project_dir = match (scope, path) {
        (crate::install::Scope::Project, Some(path)) => Some(path),
        (crate::install::Scope::Project, None) => Some(std::env::current_dir()?),
        (crate::install::Scope::User, path) => path,
    };
    let mut options =
        crate::install::InstallOptions::for_current_process(vec![client], scope, project_dir)?;
    options.dry_run = dry_run;
    options.force = force;
    Ok(options)
}

fn with_rule_option(
    mut options: crate::install::InstallOptions,
    install_rule: bool,
) -> crate::install::InstallOptions {
    options.install_rule = install_rule;
    options
}
