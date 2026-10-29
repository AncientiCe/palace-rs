use chrono::{DateTime, TimeZone, Utc};
use palace::time_query::{time_match, time_targets, TimeTarget};

/// Saturday 2023-03-25, 18:00 UTC.
fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2023, 3, 25, 18, 0, 0).unwrap()
}

fn days_back(target: &TimeTarget) -> f64 {
    (now() - target.center).num_seconds() as f64 / 86_400.0
}

fn only(query: &str) -> TimeTarget {
    let targets = time_targets(query, now());
    assert_eq!(targets.len(), 1, "{query:?} gave {targets:?}");
    targets[0]
}

#[test]
fn counts_back_from_ago_phrases() {
    let target = only("What kitchen appliance did I buy 10 days ago?");
    assert_eq!(days_back(&target), 10.0);
    assert_eq!(target.spread_days, 2.5);

    assert_eq!(days_back(&only("the event two weeks ago")), 14.0);
    assert_eq!(days_back(&only("what I said a couple of days ago")), 2.0);
    assert_eq!(days_back(&only("a few weeks back")), 21.0);
    assert_eq!(days_back(&only("a month ago")), 30.0);
}

#[test]
fn spread_grows_with_distance() {
    assert_eq!(only("yesterday").spread_days, 1.0);
    assert_eq!(only("a week ago").spread_days, 1.75);
    assert_eq!(only("two months ago").spread_days, 15.0);
}

#[test]
fn yesterday_and_the_day_before() {
    assert_eq!(days_back(&only("what did I cook yesterday?")), 1.0);
    assert_eq!(days_back(&only("the day before yesterday")), 2.0);
}

#[test]
fn spans_cover_the_whole_stretch() {
    let target = only("what did I buy last week?");
    assert_eq!(days_back(&target), 3.5);
    assert_eq!(target.spread_days, 3.5);

    let target = only("in the past two weeks");
    assert_eq!(days_back(&target), 7.0);
    assert_eq!(target.spread_days, 7.0);
}

#[test]
fn last_weekday_is_the_most_recent_one_before_today() {
    // `now` is a Saturday.
    assert_eq!(days_back(&only("last Tuesday")), 4.0);
    assert_eq!(days_back(&only("last Saturday")), 7.0);
    assert_eq!(days_back(&only("this past Friday")), 1.0);
}

#[test]
fn months_resolve_to_the_latest_one_begun() {
    let march = only("the trip in March");
    assert_eq!(
        march.center,
        Utc.with_ymd_and_hms(2023, 3, 15, 12, 0, 0).unwrap(),
        "the month under way counts"
    );
    assert_eq!(march.spread_days, 15.0);

    let february = only("during February");
    assert_eq!(
        february.center,
        Utc.with_ymd_and_hms(2023, 2, 15, 12, 0, 0).unwrap()
    );

    let june = only("early June");
    assert_eq!(
        june.center,
        Utc.with_ymd_and_hms(2022, 6, 5, 12, 0, 0).unwrap()
    );
    assert_eq!(june.spread_days, 5.0);
}

#[test]
fn month_and_day_is_one_date() {
    let target = only("the concert on March 15th");
    assert_eq!(
        target.center,
        Utc.with_ymd_and_hms(2023, 3, 15, 12, 0, 0).unwrap()
    );
    assert_eq!(target.spread_days, 1.0);
}

#[test]
fn ignores_queries_without_time_phrases() {
    assert!(time_targets("May I ask which camera I bought?", now()).is_empty());
    assert!(time_targets("what is the TDD convention?", now()).is_empty());
    assert!(time_targets("when was the last time we fixed the build", now()).is_empty());
}

#[test]
fn several_phrases_give_several_targets() {
    let targets = time_targets("did I go hiking two weeks ago or last month?", now());
    assert_eq!(targets.len(), 2);
}

#[test]
fn match_is_one_on_target_and_falls_off_with_spread() {
    let targets = time_targets("10 days ago", now());
    let center = targets[0].center;
    assert_eq!(time_match(&targets, center), 1.0);
    let one_spread = time_match(&targets, center - chrono::Duration::hours(60));
    assert!((one_spread - (-0.5f64).exp()).abs() < 1e-9);
    assert!(time_match(&targets, now()) < 0.001);
    assert_eq!(time_match(&[], center), 0.0);
}
