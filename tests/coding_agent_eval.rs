use palace::db;
use palace::ranker::hybrid_search;
use palace::store::{add_drawer, DrawerFilter};

struct EvalDoc {
    id: &'static str,
    room: &'static str,
    text: &'static str,
}

struct EvalQuestion {
    query: &'static str,
    gold_source: &'static str,
    category: &'static str,
}

const DOCS: &[EvalDoc] = &[
    EvalDoc { id: "decisions/sqlite.md", room: "decisions", text: "We chose bundled SQLite because users should not install a system database or run Chroma for coding-agent memory." },
    EvalDoc { id: "decisions/facade.md", room: "decisions", text: "We decided the Palace facade stays small because public Rust APIs are hard to change after downstream apps embed them." },
    EvalDoc { id: "decisions/mcp.md", room: "decisions", text: "We settled on MCP-first retrieval because coding agents already call tools and should not require a separate dashboard." },
    EvalDoc { id: "decisions/verbatim.md", room: "decisions", text: "We chose verbatim drawers over generated summaries because trust requires the original source text to remain citable." },
    EvalDoc { id: "fixes/migration.md", room: "problems", text: "The migration test failed because legacy drawers lacked metadata; the fix was an idempotent add_column_if_missing migration." },
    EvalDoc { id: "fixes/bm25.md", room: "problems", text: "Search missed keyword-only memories when embeddings were absent; the fix was BM25 fallback over indexed drawer terms." },
    EvalDoc { id: "fixes/install.md", room: "problems", text: "The install script smoke test broke on Windows because the archive path differed; the fix was using the local zip environment variable." },
    EvalDoc { id: "fixes/dedupe.md", room: "problems", text: "Duplicate filing happened after repeated hook runs; the fix was deterministic drawer IDs from wing, room, source file, and chunk index." },
    EvalDoc { id: "commands/fmt.md", room: "commands", text: "Run cargo fmt --all before final verification so formatting matches the workspace." },
    EvalDoc { id: "commands/clippy.md", room: "commands", text: "Run cargo clippy --all-targets --all-features -- -D warnings to catch warnings as errors." },
    EvalDoc { id: "commands/test.md", room: "commands", text: "Run cargo test --all-features for the full Rust test suite before marking a task complete." },
    EvalDoc { id: "commands/audit.md", room: "commands", text: "Run cargo audit to check dependency advisories as part of the release gate." },
    EvalDoc { id: "conventions/tdd.md", room: "conventions", text: "TDD convention: write behavioral tests first, see them fail, implement, then make them pass." },
    EvalDoc { id: "conventions/api.md", room: "conventions", text: "Project convention: avoid breaking public library APIs and route ergonomic use through the Palace facade." },
    EvalDoc { id: "conventions/source.md", room: "conventions", text: "Project convention: search results must include provenance so agents can cite wing, room, source, and score details." },
    EvalDoc { id: "conventions/no_cli_sprawl.md", room: "conventions", text: "Project convention: avoid new CLI commands unless they directly prove coding-agent retrieval value." },
    EvalDoc { id: "preferences/focus.md", room: "preferences", text: "The user prefers narrow coding-agent memory value instead of broad memory features, broad personal-memory features, or celebrity-driven storytelling." },
    EvalDoc { id: "preferences/cli.md", room: "preferences", text: "The user does not want more CLI commands when the MCP path already works perfectly for coding agents." },
    EvalDoc { id: "preferences/proof.md", room: "preferences", text: "The user prefers proof from practical evaluations over vague visibility work or generic adoption advice." },
    EvalDoc { id: "preferences/rust.md", room: "preferences", text: "The user values the Rust version as a dependable single-binary memory engine with one SQLite file." },
    EvalDoc { id: "preferences/public_api.md", room: "preferences", text: "I prefer small public APIs that route through the Palace facade instead of exposing internal modules." },
    EvalDoc { id: "preferences/retrieval.md", room: "preferences", text: "My style is to keep retrieval source-grounded with score provenance rather than returning opaque snippets." },
    EvalDoc { id: "current/positioning.md", room: "current", text: "Current direction changed from broad MemPalace parity to a narrow local-first memory retrieval engine for coding agents." },
    EvalDoc { id: "current/benchmark.md", room: "current", text: "Current proof priority is a coding-agent eval suite with recall at one and recall at five over realistic project-memory questions." },
    EvalDoc { id: "current/readme.md", room: "current", text: "Current README positioning should lead with local-first memory retrieval for coding agents, not a wide personal-memory universe." },
    EvalDoc { id: "current/trust.md", room: "current", text: "Current trust model keeps verbatim drawers as source of truth and treats extracted memories as indexes or pointers." },
    EvalDoc { id: "current/session_context.md", room: "current", text: "Current session continuity comes from recent diary entries with project path, topic, timestamp, compact text, and tags." },
    EvalDoc { id: "current/release_theme.md", room: "current", text: "Current release theme for version 0.1.9 is agent memory reliability, especially preference recall and warm-start context." },
    EvalDoc { id: "routing/memory_first.md", room: "conventions", text: "Memory-first routing rule: search Palace before grep for remembered decisions, prior fixes, conventions, preferences, prior commands, and what happened last time." },
    EvalDoc { id: "routing/code_first.md", room: "conventions", text: "Code-search-first routing rule: use grep or code search first for current symbols, exact definitions, exact files, and implementation details that may have changed since mining." },
    EvalDoc { id: "fixes/retrieval.md", room: "problems", text: "Agents kept using grep for remembered project history; the fix was stronger memory-first rules plus Palace verification and recall-check diagnostics." },
    EvalDoc { id: "current/conflicts.md", room: "current", text: "Current reliability work includes surfacing stale or contradictory memories so agents do not cite outdated facts as current truth." },
];

const QUESTIONS: &[EvalQuestion] = &[
    EvalQuestion {
        query: "why did we choose bundled sqlite?",
        gold_source: "decisions/sqlite.md",
        category: "decision",
    },
    EvalQuestion {
        query: "why keep the Palace facade small?",
        gold_source: "decisions/facade.md",
        category: "decision",
    },
    EvalQuestion {
        query: "why are we MCP first?",
        gold_source: "decisions/mcp.md",
        category: "decision",
    },
    EvalQuestion {
        query: "why verbatim drawers instead of summaries?",
        gold_source: "decisions/verbatim.md",
        category: "decision",
    },
    EvalQuestion {
        query: "how did we fix the metadata migration failure?",
        gold_source: "fixes/migration.md",
        category: "fix",
    },
    EvalQuestion {
        query: "what fixed keyword search when embeddings were missing?",
        gold_source: "fixes/bm25.md",
        category: "fix",
    },
    EvalQuestion {
        query: "what broke the Windows install smoke test?",
        gold_source: "fixes/install.md",
        category: "fix",
    },
    EvalQuestion {
        query: "how did we stop duplicate filing after repeated hooks?",
        gold_source: "fixes/dedupe.md",
        category: "fix",
    },
    EvalQuestion {
        query: "what formatting command should I run?",
        gold_source: "commands/fmt.md",
        category: "command",
    },
    EvalQuestion {
        query: "what clippy command should I run?",
        gold_source: "commands/clippy.md",
        category: "command",
    },
    EvalQuestion {
        query: "what test command runs the full suite?",
        gold_source: "commands/test.md",
        category: "command",
    },
    EvalQuestion {
        query: "what command checks dependency advisories?",
        gold_source: "commands/audit.md",
        category: "command",
    },
    EvalQuestion {
        query: "what is the TDD convention?",
        gold_source: "conventions/tdd.md",
        category: "convention",
    },
    EvalQuestion {
        query: "what is the public API convention?",
        gold_source: "conventions/api.md",
        category: "convention",
    },
    EvalQuestion {
        query: "what provenance should search results include?",
        gold_source: "conventions/source.md",
        category: "convention",
    },
    EvalQuestion {
        query: "what is the rule about adding CLI commands?",
        gold_source: "conventions/no_cli_sprawl.md",
        category: "convention",
    },
    EvalQuestion {
        query: "what does the user prefer instead of broad memory features?",
        gold_source: "preferences/focus.md",
        category: "preference",
    },
    EvalQuestion {
        query: "how does the user feel about more CLI commands?",
        gold_source: "preferences/cli.md",
        category: "preference",
    },
    EvalQuestion {
        query: "what kind of proof does the user prefer?",
        gold_source: "preferences/proof.md",
        category: "preference",
    },
    EvalQuestion {
        query: "what does the user value about the Rust version?",
        gold_source: "preferences/rust.md",
        category: "preference",
    },
    EvalQuestion {
        query: "how should I shape the public interface?",
        gold_source: "preferences/public_api.md",
        category: "preference",
    },
    EvalQuestion {
        query: "how should retrieval results explain themselves?",
        gold_source: "preferences/retrieval.md",
        category: "preference",
    },
    EvalQuestion {
        query: "what changed in the current product direction?",
        gold_source: "current/positioning.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "what is the current proof priority?",
        gold_source: "current/benchmark.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "what should the README lead with now?",
        gold_source: "current/readme.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "what is the current trust model?",
        gold_source: "current/trust.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "what should agents use for session continuity?",
        gold_source: "current/session_context.md",
        category: "session",
    },
    EvalQuestion {
        query: "what is the 0.1.9 release theme?",
        gold_source: "current/release_theme.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "what database did we choose so users avoid Chroma?",
        gold_source: "decisions/sqlite.md",
        category: "decision",
    },
    EvalQuestion {
        query: "what should embedded Rust apps use as the ergonomic API?",
        gold_source: "conventions/api.md",
        category: "convention",
    },
    EvalQuestion {
        query: "what should agents cite from search results?",
        gold_source: "conventions/source.md",
        category: "convention",
    },
    EvalQuestion {
        query: "what did we decide about dashboards?",
        gold_source: "decisions/mcp.md",
        category: "decision",
    },
    EvalQuestion {
        query: "what was the fix for missing embeddings keyword retrieval?",
        gold_source: "fixes/bm25.md",
        category: "fix",
    },
    EvalQuestion {
        query: "what verification command catches warnings as errors?",
        gold_source: "commands/clippy.md",
        category: "command",
    },
    EvalQuestion {
        query: "what does the user not want if MCP already works?",
        gold_source: "preferences/cli.md",
        category: "preference",
    },
    EvalQuestion {
        query: "what is the current narrow product lane?",
        gold_source: "current/positioning.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "why keep the original text?",
        gold_source: "decisions/verbatim.md",
        category: "decision",
    },
    EvalQuestion {
        query: "what test gate should run before done?",
        gold_source: "commands/test.md",
        category: "command",
    },
    EvalQuestion {
        query: "what changed away from MemPalace parity?",
        gold_source: "current/positioning.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "what proves coding-agent retrieval value?",
        gold_source: "current/benchmark.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "what should extracted memories be treated as?",
        gold_source: "current/trust.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "what does the user prefer over visibility advice?",
        gold_source: "preferences/proof.md",
        category: "preference",
    },
    EvalQuestion {
        query: "what interface style does the user prefer?",
        gold_source: "preferences/public_api.md",
        category: "preference",
    },
    EvalQuestion {
        query: "what warm-start context should diary entries provide?",
        gold_source: "current/session_context.md",
        category: "session",
    },
    EvalQuestion {
        query: "what command checks advisories?",
        gold_source: "commands/audit.md",
        category: "command",
    },
    EvalQuestion {
        query: "what fixed duplicate hook filings?",
        gold_source: "fixes/dedupe.md",
        category: "fix",
    },
    EvalQuestion {
        query: "when should agents search Palace before grep?",
        gold_source: "routing/memory_first.md",
        category: "memory-first",
    },
    EvalQuestion {
        query: "when is grep still the right first tool?",
        gold_source: "routing/code_first.md",
        category: "code-first",
    },
    EvalQuestion {
        query: "why did we improve the rules about grep?",
        gold_source: "fixes/retrieval.md",
        category: "fix",
    },
    EvalQuestion {
        query: "what should prevent agents from citing stale facts?",
        gold_source: "current/conflicts.md",
        category: "temporal",
    },
    EvalQuestion {
        query: "what happened last time agents used grep instead of memory?",
        gold_source: "fixes/retrieval.md",
        category: "memory-first",
    },
];

#[test]
fn coding_agent_memory_eval_has_stable_recall() {
    let conn = db::open_in_memory().expect("db should open");
    for doc in DOCS {
        add_drawer(
            &conn,
            "mempalace_rs",
            doc.room,
            doc.text,
            None,
            doc.id,
            0,
            "eval",
            3.0,
        )
        .expect("eval drawer should insert");
    }

    let mut top1 = 0usize;
    let mut top5 = 0usize;
    let mut failures = Vec::new();
    let mut top1_misses = Vec::new();

    for question in QUESTIONS {
        let results = hybrid_search(&conn, question.query, None, &DrawerFilter::default(), 5)
            .expect("eval search should work");
        if results
            .first()
            .is_some_and(|result| result.drawer.source_file == question.gold_source)
        {
            top1 += 1;
        } else {
            top1_misses.push((
                question.category,
                question.query,
                question.gold_source,
                results
                    .first()
                    .map(|result| result.drawer.source_file.clone())
                    .unwrap_or_else(|| "<none>".to_string()),
            ));
        }
        if results
            .iter()
            .any(|result| result.drawer.source_file == question.gold_source)
        {
            top5 += 1;
        } else {
            failures.push((question.category, question.query, question.gold_source));
        }
    }

    let recall_at_1 = top1 as f64 / QUESTIONS.len() as f64;
    let recall_at_5 = top5 as f64 / QUESTIONS.len() as f64;
    eprintln!(
        "coding-agent eval: recall@1={recall_at_1:.3} recall@5={recall_at_5:.3} failures={failures:?}"
    );

    assert!(
        recall_at_1 >= 0.86,
        "recall@1 too low: {recall_at_1:.3}; first_top1_misses={:?}",
        top1_misses
            .iter()
            .take(1)
            .map(|(_, _, gold, got)| {
                format!(
                    "{}!={}",
                    got.rsplit('/').next().unwrap_or(got),
                    gold.rsplit('/').next().unwrap_or(gold)
                )
            })
            .collect::<Vec<_>>()
    );
    assert!(recall_at_5 >= 0.98, "recall@5 too low: {recall_at_5:.3}");
}
