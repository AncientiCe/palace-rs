use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Baseline {
    sample_seed: u64,
    sample_size: usize,
    single_session_preference: PreferenceBaseline,
}

#[derive(Debug, Deserialize)]
struct PreferenceBaseline {
    recall_at_1_floor: f64,
    recall_at_5_floor: f64,
}

#[test]
fn sampled_longmemeval_baseline_guard_is_configured() {
    let baseline: Baseline =
        serde_json::from_str(include_str!("baselines/longmemeval.json")).expect("baseline JSON");

    assert_eq!(baseline.sample_seed, 4040);
    assert_eq!(baseline.sample_size, 100);
    assert!(baseline.single_session_preference.recall_at_1_floor >= 0.78);
    assert!(baseline.single_session_preference.recall_at_5_floor >= 0.95);

    if std::env::var("PALACE_RUN_LONGMEMEVAL").ok().as_deref() != Some("1") {
        eprintln!("PALACE_RUN_LONGMEMEVAL is not set; sampled LongMemEval execution is skipped.");
    }
}
