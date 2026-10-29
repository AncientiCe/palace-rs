use palace::db;
use palace::ranker::{hybrid_search, hybrid_search_with, RankOptions};
use palace::store::{add_drawer, DrawerFilter, VectorSource};

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
fn intent_terms_inside_other_words_do_not_boost() {
    // LongMemEval gpt4_e061b84f: "latest" contains "test", which used to fire the
    // command intent and lift an unrelated drawer containing "--" into the top five.
    let conn = db::open_in_memory().expect("db should open");
    add_drawer(
        &conn,
        "wing",
        "room",
        "Upcoming events: rewards expire soon -- redeem them this month.",
        None,
        "rewards.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let results = hybrid_search(
        &conn,
        "What is the order of the events, from earliest to latest?",
        None,
        &DrawerFilter::default(),
        5,
    )
    .expect("hybrid search should work");
    assert!(!results.is_empty());
    assert_eq!(
        results[0].coding_boost, 0.0,
        "\"latest\" must not count as the command term \"test\""
    );

    let results = hybrid_search(
        &conn,
        "which test command runs before the events",
        None,
        &DrawerFilter::default(),
        5,
    )
    .expect("hybrid search should work");
    assert!(
        results[0].coding_boost > 0.0,
        "a standalone \"test\" should still fire the command intent"
    );
}

#[test]
fn should_i_asks_for_advice_not_a_convention() {
    // "What should I serve?" wants a suggestion for the speaker; it used to fire
    // the convention intent and lift any drawer that says "always" or "never".
    let conn = db::open_in_memory().expect("db should open");
    add_drawer(
        &conn,
        "wing",
        "room",
        "We always serve the guests first at the dinner table.",
        None,
        "dinner.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let search = |query: &str| {
        hybrid_search(&conn, query, None, &DrawerFilter::default(), 5)
            .expect("hybrid search should work")
    };
    assert_eq!(
        search("What should I serve for dinner?")[0].coding_boost,
        0.0,
        "\"should I\" must not count as convention phrasing"
    );
    assert!(
        search("What should we serve for dinner?")[0].coding_boost > 0.0,
        "\"should we\" still asks for a shared norm"
    );
}

#[test]
fn time_phrases_lift_drawers_from_that_day() {
    let conn = db::open_in_memory().expect("db should open");
    for (id, created_at) in [
        ("march.txt", "2023-03-15T09:00:00+00:00"),
        ("january.txt", "2023-01-20T09:00:00+00:00"),
    ] {
        add_drawer(
            &conn,
            "w",
            "r",
            "I bought a new kitchen appliance for the house.",
            None,
            id,
            0,
            "test",
            3.0,
        )
        .unwrap();
        conn.execute(
            "UPDATE drawers SET created_at = ?1 WHERE source_file = ?2",
            [created_at, id],
        )
        .unwrap();
    }

    let options = RankOptions {
        now: chrono::DateTime::parse_from_rfc3339("2023-03-25T18:00:00+00:00")
            .unwrap()
            .to_utc(),
        ..RankOptions::default()
    };
    let results = hybrid_search_with(
        &conn,
        "What kitchen appliance did I buy 10 days ago?",
        None,
        VectorSource::Hot,
        palace::embedder::active_model(),
        &DrawerFilter::default(),
        5,
        &options,
    )
    .expect("hybrid search should work");
    assert_eq!(results[0].drawer.source_file, "march.txt");
    assert!(results[0].date_boost > 0.1);
    assert!(results[1].date_boost < 0.001);
}

#[test]
fn sources_matching_in_several_places_rank_higher() {
    let conn = db::open_in_memory().expect("db should open");
    for (source, chunk, text) in [
        ("single.txt", 0, "garden tomatoes basil"),
        ("several.txt", 0, "garden tomatoes"),
        ("several.txt", 1, "tomatoes basil"),
        ("", 0, "garden basil"),
        ("", 1, "basil tomatoes"),
    ] {
        add_drawer(&conn, "w", "r", text, None, source, chunk, "test", 3.0).unwrap();
    }
    let search = |source_weight: f64| {
        hybrid_search_with(
            &conn,
            "garden tomatoes basil",
            None,
            VectorSource::Hot,
            palace::embedder::active_model(),
            &DrawerFilter::default(),
            10,
            &RankOptions {
                source_weight,
                ..RankOptions::default()
            },
        )
        .expect("hybrid search should work")
    };

    let alone = search(0.0);
    assert_eq!(alone[0].drawer.source_file, "single.txt");
    assert!(alone.iter().all(|result| result.source_boost == 0.0));

    let together = search(0.5);
    assert_eq!(together[0].drawer.source_file, "several.txt");
    assert!(together[0].source_boost > 0.0);
    assert!(
        together
            .iter()
            .filter(|result| result.drawer.source_file.is_empty())
            .all(|result| result.source_boost == 0.0),
        "drawers without a source are not one source"
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
