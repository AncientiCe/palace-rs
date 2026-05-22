#[cfg(feature = "benchmarks")]
use anyhow::Result;
#[cfg(feature = "benchmarks")]
use clap::{Parser, Subcommand};
#[cfg(feature = "benchmarks")]
use palace::benchmarks::{prepare_beam_jsonl, run_benchmark, RunOptions};
#[cfg(feature = "benchmarks")]
use std::path::PathBuf;

#[cfg(feature = "benchmarks")]
#[derive(Parser)]
#[command(
    name = "mempalace-bench",
    version = env!("CARGO_PKG_VERSION"),
    about = "Retrieval benchmark harness for MemPalace"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[cfg(feature = "benchmarks")]
#[derive(Subcommand)]
enum Command {
    /// Run LongMemEval retrieval recall over an official JSON file.
    LongMemEval {
        /// Path to longmemeval_oracle.json or longmemeval_s_cleaned.json.
        input: PathBuf,
        /// Directory for *_cases.jsonl and *_summary.json outputs.
        output_dir: PathBuf,
        /// Maximum questions to evaluate.
        #[arg(long)]
        limit: Option<usize>,
        /// Use BM25-only retrieval without embedding model downloads.
        #[arg(long)]
        bm25_only: bool,
    },
    /// Run BEAM retrieval recall over canonical JSONL.
    Beam {
        /// Path to canonical BEAM JSONL.
        input: PathBuf,
        /// Directory for *_cases.jsonl and *_summary.json outputs.
        output_dir: PathBuf,
        /// Maximum questions to evaluate.
        #[arg(long)]
        limit: Option<usize>,
        /// Use BM25-only retrieval without embedding model downloads.
        #[arg(long)]
        bm25_only: bool,
    },
    /// Normalize a JSONL BEAM export into the canonical MemPalace benchmark shape.
    PrepareBeam {
        /// Input JSONL exported from BEAM/Hugging Face tooling.
        input: PathBuf,
        /// Output canonical JSONL.
        output: PathBuf,
    },
}

#[cfg(feature = "benchmarks")]
fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::LongMemEval {
            input,
            output_dir,
            limit,
            bm25_only,
        } => {
            let summary = run_benchmark(&RunOptions {
                benchmark: "longmemeval".to_string(),
                input_path: input,
                output_dir,
                limit,
                bm25_only,
            })?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        Command::Beam {
            input,
            output_dir,
            limit,
            bm25_only,
        } => {
            let summary = run_benchmark(&RunOptions {
                benchmark: "beam".to_string(),
                input_path: input,
                output_dir,
                limit,
                bm25_only,
            })?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        Command::PrepareBeam { input, output } => {
            let rows = prepare_beam_jsonl(&input, &output)?;
            println!("Prepared {rows} BEAM rows into {}", output.display());
        }
    }
    Ok(())
}

#[cfg(not(feature = "benchmarks"))]
fn main() {}
