use palace::db;
use palace::ranker::hybrid_search;
use palace::store::{add_drawer, DrawerFilter};

mod common;

use common::coding_eval::{DOCS, QUESTIONS};

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
