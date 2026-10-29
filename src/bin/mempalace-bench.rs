#[cfg(feature = "benchmarks")]
use anyhow::Result;
#[cfg(feature = "benchmarks")]
use clap::{Args, Parser, Subcommand};
#[cfg(feature = "benchmarks")]
use palace::benchmarks::{
    prepare_beam_jsonl, run_benchmark, BenchmarkSummary, Chunking, RunOptions,
};
#[cfg(feature = "benchmarks")]
use palace::embedding_models::{self, ModelSpec};
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
#[derive(Args)]
struct RunArgs {
    /// Input file: LongMemEval JSON or canonical BEAM JSONL.
    input: PathBuf,
    /// Directory for outputs; each model writes *_cases.jsonl and
    /// *_summary.json under <output_dir>/<model-id>/.
    output_dir: PathBuf,
    /// Maximum questions to evaluate.
    #[arg(long)]
    limit: Option<usize>,
    /// Use BM25-only retrieval without embedding model downloads.
    #[arg(long)]
    bm25_only: bool,
    /// Embedding model ids, comma-separated, or `all` for every model.
    #[arg(long, value_delimiter = ',', default_value = embedding_models::DEFAULT_MODEL_ID)]
    model: Vec<String>,
    /// How sessions become drawers; chunked modes rank a session by its best
    /// chunk. Non-session modes write to <output_dir>/<model-id>-<mode>/.
    #[arg(long, value_enum, default_value_t = Chunking::Session)]
    chunking: Chunking,
    /// Also write each case's best N chunk scores to the cases file, for
    /// comparing ranking rules offline.
    #[arg(long, default_value_t = 0)]
    dump_chunks: usize,
}

#[cfg(feature = "benchmarks")]
#[derive(Subcommand)]
enum Command {
    /// Run LongMemEval retrieval recall over an official JSON file.
    LongMemEval(RunArgs),
    /// Run BEAM retrieval recall over canonical JSONL.
    Beam(RunArgs),
    /// Normalize a JSONL BEAM export into the canonical MemPalace benchmark shape.
    PrepareBeam {
        /// Input JSONL exported from BEAM/Hugging Face tooling.
        input: PathBuf,
        /// Output canonical JSONL.
        output: PathBuf,
    },
}

#[cfg(feature = "benchmarks")]
fn models(ids: &[String]) -> Result<Vec<&'static ModelSpec>> {
    if ids.iter().any(|id| id == "all") {
        return Ok(embedding_models::MODELS.iter().collect());
    }
    ids.iter()
        .map(|id| {
            embedding_models::find(id).ok_or_else(|| {
                let known: Vec<&str> = embedding_models::MODELS.iter().map(|m| m.id).collect();
                anyhow::anyhow!("unknown model {id}; known: {}", known.join(", "))
            })
        })
        .collect()
}

#[cfg(feature = "benchmarks")]
fn run(benchmark: &str, args: RunArgs) -> Result<()> {
    let models = if args.bm25_only {
        vec![embedding_models::default_model()]
    } else {
        models(&args.model)?
    };
    let mut summaries = Vec::new();
    for model in models {
        let name = if args.bm25_only {
            "bm25-only"
        } else {
            model.id
        };
        let output_dir = if args.chunking == Chunking::Session {
            args.output_dir.join(name)
        } else {
            args.output_dir
                .join(format!("{name}-{}", args.chunking.as_str()))
        };
        let summary = run_benchmark(&RunOptions {
            benchmark: benchmark.to_string(),
            input_path: args.input.clone(),
            output_dir,
            limit: args.limit,
            bm25_only: args.bm25_only,
            model,
            chunking: args.chunking,
            dump_chunks: args.dump_chunks,
        })?;
        println!("{}", serde_json::to_string_pretty(&summary)?);
        summaries.push(summary);
    }
    if summaries.len() > 1 {
        println!("{}", summary_table(&summaries));
    }
    Ok(())
}

/// Markdown table of aggregate and per-question-type recall@1/@5/@10.
#[cfg(feature = "benchmarks")]
fn summary_table(summaries: &[BenchmarkSummary]) -> String {
    let mut out = String::from(
        "| Model | R@1 | R@5 | R@10 | Ingest | Query |\n|---|---:|---:|---:|---:|---:|\n",
    );
    for s in summaries {
        out.push_str(&format!(
            "| `{}` | {:.3} | {:.3} | {:.3} | {}s | {}ms |\n",
            s.model,
            s.recall_at_1,
            s.recall_at_5,
            s.recall_at_10,
            s.ingest_ms / 1000,
            s.query_ms
        ));
    }
    let types: Vec<&String> = summaries
        .iter()
        .flat_map(|s| s.per_question_type.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for question_type in types {
        out.push_str(&format!(
            "\n{question_type}\n\n| Model | R@1 | R@5 | R@10 |\n|---|---:|---:|---:|\n"
        ));
        for s in summaries {
            if let Some(t) = s.per_question_type.get(question_type) {
                out.push_str(&format!(
                    "| `{}` | {:.3} | {:.3} | {:.3} |\n",
                    s.model, t.recall_at_1, t.recall_at_5, t.recall_at_10
                ));
            }
        }
    }
    out
}

#[cfg(feature = "benchmarks")]
fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::LongMemEval(args) => run("longmemeval", args)?,
        Command::Beam(args) => run("beam", args)?,
        Command::PrepareBeam { input, output } => {
            let rows = prepare_beam_jsonl(&input, &output)?;
            println!("Prepared {rows} BEAM rows into {}", output.display());
        }
    }
    Ok(())
}

#[cfg(not(feature = "benchmarks"))]
fn main() {}
