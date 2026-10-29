# Benchmarks Runbook

Runbook for the `palace-bench` retrieval benchmarks (LongMemEval and BEAM).
Copy/paste commands one at a time and watch each summary print before moving on.

> Heads-up on cost (single-CPU, model already cached):
> - LongMemEval `oracle` hybrid: ~1–3 min
> - LongMemEval `s_cleaned` hybrid: **~20–60 min** (no progress output, see "Monitoring")
> - BEAM `100K` hybrid: ~5–15 min · `500K`: ~30–90 min · `1M`: hours
> - BM25-only variants are **10–50× faster** and don't download the MiniLM model
> - First hybrid run also pulls `all-MiniLM-L6-v2` via `hf-hub` (~90 MB, one-time)

---

## 0. Prerequisites

```bash
# Build + install the binary once (drops it at ~/.cargo/bin/palace-bench)
cargo install --path . --no-default-features --features benchmarks --bin palace-bench

# Sanity check
which palace-bench && palace-bench --version

# Output dirs
mkdir -p data/out/longmemeval data/out/beam
```

If you'd rather not install, alias for the session instead:

```bash
alias palace-bench='cargo run --release --no-default-features --features benchmarks --bin palace-bench --'
```

---

## 1. Download datasets (one-time)

LongMemEval (JSON, ready to use):

```bash
mkdir -p data/longmemeval
curl -L -o data/longmemeval/longmemeval_oracle.json \
  https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_oracle.json
curl -L -o data/longmemeval/longmemeval_s_cleaned.json \
  https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json
# Optional, 2.7 GB:
# curl -L -o data/longmemeval/longmemeval_m_cleaned.json \
#   https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_m_cleaned.json
```

BEAM (parquet, must be converted to canonical JSONL first):

```bash
mkdir -p data/beam/raw data/beam/canonical
curl -L -o data/beam/raw/beam_100K.parquet \
  https://huggingface.co/datasets/Mohammadta/BEAM/resolve/main/data/100K-00000-of-00001.parquet
curl -L -o data/beam/raw/beam_500K.parquet \
  https://huggingface.co/datasets/Mohammadta/BEAM/resolve/main/data/500K-00000-of-00001.parquet
curl -L -o data/beam/raw/beam_1M.parquet \
  https://huggingface.co/datasets/Mohammadta/BEAM/resolve/main/data/1M-00000-of-00001.parquet
# Optional, 343 MB parquet → ~15+ GB JSONL:
# curl -L -o data/beam/raw/beam_10M.parquet \
#   https://huggingface.co/datasets/Mohammadta/BEAM-10M/resolve/main/data/10M-00000-of-00001.parquet

# Parquet → canonical JSONL via the in-repo Rust converter
cargo run --manifest-path tools/beam-convert/Cargo.toml --release -- \
  data/beam/raw/beam_100K.parquet  data/beam/canonical/beam_100K.jsonl
cargo run --manifest-path tools/beam-convert/Cargo.toml --release -- \
  data/beam/raw/beam_500K.parquet  data/beam/canonical/beam_500K.jsonl
cargo run --manifest-path tools/beam-convert/Cargo.toml --release -- \
  data/beam/raw/beam_1M.parquet    data/beam/canonical/beam_1M.jsonl
```

---

## 2. Smoke tests (always do these first)

Validates the full pipeline in seconds without touching the embedding model:

```bash
palace-bench long-mem-eval data/longmemeval/longmemeval_oracle.json \
  data/out/smoke --limit 3 --bm25-only

palace-bench beam data/beam/canonical/beam_100K.jsonl \
  data/out/smoke --limit 3 --bm25-only
```

Each prints a JSON summary to stdout and writes `<bench>_cases.jsonl` + `<bench>_summary.json` into the output dir.

---

## 3. BM25-only sweep (fast — minutes total)

Skip the embedding model; tests lexical retrieval only.

```bash
# LongMemEval
palace-bench long-mem-eval data/longmemeval/longmemeval_oracle.json \
  data/out/longmemeval/oracle_bm25 --bm25-only

palace-bench long-mem-eval data/longmemeval/longmemeval_s_cleaned.json \
  data/out/longmemeval/s_cleaned_bm25 --bm25-only

# BEAM
palace-bench beam data/beam/canonical/beam_100K.jsonl \
  data/out/beam/100K_bm25 --bm25-only

palace-bench beam data/beam/canonical/beam_500K.jsonl \
  data/out/beam/500K_bm25 --bm25-only

palace-bench beam data/beam/canonical/beam_1M.jsonl \
  data/out/beam/1M_bm25 --bm25-only
```

---

## 4. Full hybrid sweep (BM25 + embeddings — slow)

Run these one at a time and let them finish. Each model writes its outputs to
`<output_dir>/<model-id>/` (`bm25-only/` with `--bm25-only`).

`--model` picks the embedding model (default `minilm-l6-v2`). It takes a
comma-separated list or `all`; models run one after another and a markdown
table comparing them prints at the end. Each model ranks with its own cosine
floor and cosine/BM25 weights from `embedding_models`.

```bash
# Every embedding model on LongMemEval s_cleaned (~1–1.5 h on 16 threads)
palace-bench long-mem-eval data/longmemeval/longmemeval_s_cleaned.json \
  data/out/longmemeval/s_cleaned --model all
```

`--chunking` sets how sessions become drawers: `session` (default, one drawer
per session), `text` (the project miner's 800-character chunks) or `exchange`
(the conversation miner's path: `normalize_content`, then one chunk per user
turn and its reply). Chunked modes rank a session by its best chunk and write
to `<output_dir>/<model-id>-<mode>/`. `exchange` is how Palace stores mined
chats, and gives the README's exchange-pair row (~20 min on 16 threads):

```bash
palace-bench long-mem-eval data/longmemeval/longmemeval_s_cleaned.json \
  data/out/longmemeval/s_cleaned --model minilm-l6-v2-q8 --chunking exchange
```

```bash
# LongMemEval — fast
palace-bench long-mem-eval data/longmemeval/longmemeval_oracle.json \
  data/out/longmemeval/oracle

# LongMemEval — long (~20–60 min)
palace-bench long-mem-eval data/longmemeval/longmemeval_s_cleaned.json \
  data/out/longmemeval/s_cleaned

# BEAM
palace-bench beam data/beam/canonical/beam_100K.jsonl \
  data/out/beam/100K

palace-bench beam data/beam/canonical/beam_500K.jsonl \
  data/out/beam/500K

palace-bench beam data/beam/canonical/beam_1M.jsonl \
  data/out/beam/1M
```

---

## 5. Monitoring a long-running benchmark

The runner now prints a progress line to **stderr** every N questions (interval scales with total: 1/10/25/50). The final JSON summary still goes to **stdout**, so you can split them:

```bash
palace-bench long-mem-eval data/longmemeval/longmemeval_s_cleaned.json \
  data/out/longmemeval/s_cleaned \
  > summary.json 2> progress.log
tail -f progress.log    # in another terminal
```

Or watch progress live while still seeing the summary:

```bash
palace-bench long-mem-eval ... 2>&1 | tee run.log
```

A progress line looks like:

```
[longmemeval] 200/470  recall@5=0.978  failed=0  unscored=0  0.6q/s  eta=7m32s
```

For lower-level monitoring, from a second terminal:

```bash
# CPU / runtime
ps -o pid,pcpu,pmem,etime,command -p "$(pgrep -f palace-bench)"

# Per-question progress (BufWriter flushes the cases file every few KB)
watch -n 5 'wc -l data/out/longmemeval/s_cleaned/longmemeval_cases.jsonl'

# Tail the latest result line
tail -1 data/out/longmemeval/s_cleaned/longmemeval_cases.jsonl | jq .
```

Healthy signs: `pcpu` ≈ 95–100%, line count climbs steadily.

To abort:

```bash
pkill -INT palace-bench
```

---

## 6. Output layout

Each run writes two files into its `output_dir` (see `run_benchmark` in `src/benchmarks.rs`):

- `<benchmark>_cases.jsonl` — one line per question with `ranked_session_ids`, `hit_at_{1,5,10}`, timings, error if any.
- `<benchmark>_summary.json` — aggregate `recall@1/5/10`, `ingest_ms`, `query_ms`, `per_question_type` breakdown. Same JSON is also pretty-printed to stdout.

Where `<benchmark>` is `longmemeval` or `beam`.

---

## 7. Notes on BEAM scoring

BEAM labels evidence at *turn* (`chat_id`) granularity, while `palace-bench` retrieves at *session* granularity. The Rust converter (`tools/beam-convert/`) maps each evidence chat_id back to the session that contains it.

- `abstention` questions have `source_chat_ids: None` → they ingest+query but show up as `unscored_questions` in the summary. Expected, not a bug.
- All other categories (`information_extraction`, `multi_session_reasoning`, `temporal_reasoning`, etc.) score normally against Recall@k.
