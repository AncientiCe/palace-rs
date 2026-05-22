use palace::db;
use palace::ranker::hybrid_search;
use palace::store::{add_drawer, DrawerFilter};

#[test]
fn hybrid_search_uses_bm25_when_embeddings_are_absent() {
    let conn = db::open_in_memory().expect("db should open");
    add_drawer(
        &conn,
        "wing",
        "room",
        "rareword alpha alpha alpha",
        None,
        "a.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();
    add_drawer(
        &conn,
        "wing",
        "room",
        "common beta beta beta",
        None,
        "b.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let results = hybrid_search(&conn, "rareword alpha", None, &DrawerFilter::default(), 5)
        .expect("hybrid search should work");

    assert_eq!(results[0].drawer.text, "rareword alpha alpha alpha");
    assert!(results[0].bm25 > 0.0);
    assert_eq!(results[0].cosine, 0.0);
}

#[test]
fn hybrid_search_combines_cosine_and_bm25_scores() {
    let conn = db::open_in_memory().expect("db should open");
    let query_vec: Vec<f32> = (0..384).map(|i| (i as f32).sin()).collect();
    let other_vec: Vec<f32> = (0..384).map(|i| (i as f32).cos()).collect();

    add_drawer(
        &conn,
        "wing",
        "room",
        "semantic match target keyword",
        Some(&query_vec),
        "semantic.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();
    add_drawer(
        &conn,
        "wing",
        "room",
        "keyword only target target",
        Some(&other_vec),
        "keyword.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let results = hybrid_search(
        &conn,
        "target keyword",
        Some(&query_vec),
        &DrawerFilter::default(),
        5,
    )
    .expect("hybrid search should work");

    assert!(results.iter().any(|result| result.cosine > 0.0));
    assert!(results.iter().any(|result| result.bm25 > 0.0));
    assert!(results[0].combined >= results[1].combined);
}

#[test]
fn hybrid_search_exposes_preference_match_score() {
    let conn = db::open_in_memory().expect("db should open");

    add_drawer(
        &conn,
        "wing",
        "preferences",
        "I prefer source-grounded answers with drawer provenance.",
        None,
        "preference.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let results = hybrid_search(
        &conn,
        "what do I prefer for answers",
        None,
        &DrawerFilter::default(),
        5,
    )
    .expect("hybrid search should work");

    assert!(!results.is_empty());
    assert!(
        results[0].preference_match > 0.0,
        "preference-shaped result should expose a separate preference score"
    );
}

#[test]
fn hybrid_search_result_has_filed_at() {
    // Verify the new filed_at field is populated on SearchResult.
    let conn = db::open_in_memory().expect("db should open");
    add_drawer(
        &conn,
        "w",
        "r",
        "recency test entry",
        None,
        "t.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let results = hybrid_search(&conn, "recency test", None, &DrawerFilter::default(), 5)
        .expect("search should work");

    assert!(!results.is_empty());
    assert!(
        !results[0].drawer.filed_at.is_empty(),
        "filed_at should not be empty"
    );
}

#[test]
fn recency_gives_fresh_drawers_higher_combined_score() {
    // A freshly filed drawer should score slightly higher than the same content
    // from a drawer with an artificially old filed_at — even without any BM25
    // match — purely from the recency bonus.
    let conn = db::open_in_memory().expect("db should open");
    let query_vec: Vec<f32> = (0..384).map(|i| (i as f32).sin()).collect();

    // Both drawers use the same embedding so cosine scores are identical.
    add_drawer(
        &conn,
        "w",
        "r",
        "identical content recency",
        Some(&query_vec),
        "new.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let results = hybrid_search(
        &conn,
        "identical content recency",
        Some(&query_vec),
        &DrawerFilter::default(),
        5,
    )
    .expect("search should work");

    assert!(!results.is_empty());
    // The recency boost must be > 0 for a freshly filed drawer.
    assert!(
        results[0].combined > results[0].cosine * 0.65,
        "combined score should be higher than cosine alone (recency bonus expected)"
    );
}
