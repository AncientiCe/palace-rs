use mempalace::db;
use mempalace::ranker::hybrid_search;
use mempalace::store::{add_drawer, DrawerFilter};

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
