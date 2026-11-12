# Benchmarks

Retrieval quality and speed of Palace. No LLM at any stage, no GPU, no API
calls: a local embedder (`all-MiniLM-L6-v2`, int8) and Palace's hybrid search
on one SQLite file.

## LongMemEval

[LongMemEval](https://github.com/xiaowu0162/LongMemEval) asks questions about a
long chat history. Each question comes with its own haystack of past sessions,
and the retriever has to find the session that holds the answer.

### `s_cleaned`: about 50 sessions (115k tokens) per question

| Questions | R@1 | R@5 | R@10 |
|---|---:|---:|---:|
| All 500 | 0.912 | **0.986** | 0.994 |
| 470, abstention excluded (the usual convention) | 0.915 | **0.987** | 0.996 |

| Question type | Questions | R@1 | R@5 | R@10 |
|---|---:|---:|---:|---:|
| knowledge-update | 72 | 0.958 | 1.000 | 1.000 |
| multi-session | 121 | 0.909 | 0.992 | 1.000 |
| single-session-assistant | 56 | 1.000 | 1.000 | 1.000 |
| single-session-preference | 30 | 0.767 | 0.933 | 0.933 |
| single-session-user | 64 | 0.984 | 1.000 | 1.000 |
| temporal-reasoning | 127 | 0.858 | 0.976 | 1.000 |
| abstention | 30 | 0.867 | 0.967 | 0.967 |

### `m_cleaned`: about 500 sessions (1.5M tokens) per question

Ten times the haystack of `s_cleaned`: around 4,000 drawers to search per
question.

| Questions | R@1 | R@5 | R@10 |
|---|---:|---:|---:|
| 470, abstention excluded | 0.732 | **0.919** | 0.949 |

| Question type | Questions | R@1 | R@5 | R@10 |
|---|---:|---:|---:|---:|
| knowledge-update | 72 | 0.889 | 0.986 | 1.000 |
| multi-session | 121 | 0.678 | 0.934 | 0.975 |
| single-session-assistant | 56 | 0.964 | 1.000 | 1.000 |
| single-session-preference | 30 | 0.300 | 0.700 | 0.733 |
| single-session-user | 64 | 0.719 | 0.953 | 0.953 |
| temporal-reasoning | 127 | 0.701 | 0.866 | 0.921 |

### Method

- **Data:** `longmemeval_s_cleaned.json` and `longmemeval_m_cleaned.json`
  from [xiaowu0162/longmemeval-cleaned](https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned).
- **Storage:** each session goes through the same normaliser and chunker as
  `palace mine-convos`, one drawer per user turn and its reply, stored
  verbatim. Nothing is filtered, summarised, or extracted by an LLM.
- **Retrieval:** Palace's normal hybrid search (BM25 fused with cosine
  similarity, plus its ranking rules), `minilm-l6-v2-q8`. A session ranks by
  its best drawer. Each question is asked on its own `question_date`, so
  relative dates ("two weeks ago") resolve as they would in a live palace.
- **Metric:** `recall_any@K` at session level: a hit if any gold session is in
  the top K.
- **No parameters were fitted on `s_cleaned`.**
- **Hardware:** Windows, 16 threads, CPU only. Average query time per
  question, including embedding the question: 6 ms on `s_cleaned`, 78 ms on
  `m_cleaned`.

The runner, `palace-bench`, lives on the
[`benchmarks/longmemeval-beam-runner`](https://github.com/AncientiCe/palace-rs/tree/benchmarks/longmemeval-beam-runner)
branch:

```bash
palace-bench long-mem-eval data/longmemeval/longmemeval_s_cleaned.json \
  data/out/longmemeval/s_cleaned --model minilm-l6-v2-q8 --chunking exchange \
  --include-abstention
```

Leave out `--include-abstention` for the 470-question set. See the branch's
`BENCHMARKS.md` for datasets and setup.

## Coding-agent memory eval

A fixture of realistic project memories (decisions, past failures, commands,
conventions, preferences, current direction) with 51 questions such as "why
did we choose bundled sqlite?" or "what clippy command should I run?".

| Model | R@1 | R@5 |
|---|---:|---:|
| `minilm-l6-v2-q8` (default) | 0.922 | 1.000 |
| `minilm-l6-v2` | 0.922 | 1.000 |
| Keyword-only | 0.863 | 1.000 |

```bash
cargo test --test coding_agent_eval -- --nocapture
```

The ranker's coding-agent boosts include a few phrases taken from this eval's
questions. They have no effect on LongMemEval or on real-palace probes.

## Embedding model bake-off

Every [embedding model](../README.md#embedding-models) on the coding-agent
eval, on 200 known-item probes from a real 10,800-drawer palace, and on
speed.

| Model | Coding eval R@1 / R@5 | Palace probes R@1 / R@5 | Indexing | Query p95 |
|---|---:|---:|---:|---:|
| `minilm-l6-v2-q8` (default) | **0.922** / 1.000 | **0.900** / 0.970 | **150 drawers/s** | **2.0 ms** |
| `minilm-l6-v2` | **0.922** / 1.000 | 0.890 / 0.975 | 132 drawers/s | 2.1 ms |
| `bge-small` | 0.882 / 1.000 | **0.905 / 0.980** | 68 drawers/s | 5.9 ms |
| `bge-small-q8` | 0.882 / 1.000 | 0.890 / 0.980 | 79 drawers/s | 4.5 ms |
| `bge-base-q8` | 0.882 / 1.000 | 0.900 / 0.980 | 40 drawers/s | 6.5 ms |

```bash
PALACE_RUN_BAKEOFF=1 PALACE_BAKEOFF_DB=~/.palace/palace/palace.db \
  cargo test --release --test embedding_bakeoff -- --nocapture
```

Windows, 16 threads. Each probe is an inner sentence of a drawer with about
30% of its words dropped.
