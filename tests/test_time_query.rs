use chrono::{DateTime, Duration, TimeZone, Utc};
use palace::time_query::{time_match, time_targets, TimeTarget};

/// Saturday 2023-03-25, 18:00 UTC.
fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2023, 3, 25, 18, 0, 0).unwrap()
}

fn days_back(at: DateTime<Utc>) -> f64 {
    (now() - at).num_seconds() as f64 / 86_400.0
}

fn only(query: &str) -> TimeTarget {
    let targets = time_targets(query, now());
    assert_eq!(targets.len(), 1, "{query:?} gave {targets:?}");
    targets[0]
}

/// A point target's distance back from now, in days.
fn point(query: &str) -> f64 {
    let target = only(query);
    assert_eq!(target.start, target.end, "{query:?} should be a point");
    days_back(target.start)
}

#[test]
fn counts_back_from_ago_phrases() {
    assert_eq!(point("What kitchen appliance did I buy 10 days ago?"), 10.0);
    assert_eq!(point("the event two weeks ago"), 14.0);
    assert_eq!(point("what I said a couple of days ago"), 2.0);
    assert_eq!(point("a few weeks back"), 21.0);
    assert_eq!(point("a month ago"), 30.0);
}

#[test]
fn spread_grows_with_distance() {
    assert_eq!(only("yesterday").spread_days, 1.0);
    assert_eq!(only("a week ago").spread_days, 1.75);
    assert_eq!(only("10 days ago").spread_days, 2.5);
    assert_eq!(only("two months ago").spread_days, 15.0);
}

#[test]
fn yesterday_and_the_day_before() {
    assert_eq!(point("what did I cook yesterday?"), 1.0);
    assert_eq!(point("the day before yesterday"), 2.0);
}

#[test]
fn spans_run_up_to_now() {
    let target = only("how many dinner parties did I attend in the past month?");
    assert_eq!(days_back(target.start), 30.0);
    assert_eq!(target.end, now());

    let target = only("what did I buy last week?");
    assert_eq!(days_back(target.start), 7.0);
    assert_eq!(target.end, now());

    assert_eq!(days_back(only("in the past two weeks").start), 14.0);
}

#[test]
fn this_week_starts_on_monday() {
    let target = only("what did I cook this week?");
    assert_eq!(
        target.start,
        Utc.with_ymd_and_hms(2023, 3, 20, 0, 0, 0).unwrap()
    );
    assert_eq!(
        only("this month").start,
        Utc.with_ymd_and_hms(2023, 3, 1, 0, 0, 0).unwrap()
    );
}

#[test]
fn last_weekday_is_the_most_recent_one_before_today() {
    // `now` is a Saturday.
    assert_eq!(point("last Tuesday"), 4.0);
    assert_eq!(point("last Saturday"), 7.0);
    assert_eq!(point("this past Friday"), 1.0);
}

#[test]
fn on_weekday_is_not_a_date() {
    assert!(time_targets("What time do I wake up on Saturday mornings?", now()).is_empty());
}

#[test]
fn months_cover_the_latest_one_begun() {
    let march = only("the trip in March");
    assert_eq!(
        march.start,
        Utc.with_ymd_and_hms(2023, 3, 1, 0, 0, 0).unwrap()
    );
    assert_eq!(march.end, now(), "the month under way ends now");

    let february = only("during February");
    assert_eq!(
        february.end,
        Utc.with_ymd_and_hms(2023, 3, 1, 0, 0, 0).unwrap()
    );

    let june = only("early June");
    assert_eq!(
        june.start,
        Utc.with_ymd_and_hms(2022, 6, 1, 0, 0, 0).unwrap()
    );
    assert_eq!(
        june.end,
        Utc.with_ymd_and_hms(2022, 6, 11, 0, 0, 0).unwrap()
    );
}

#[test]
fn month_and_day_is_one_date() {
    let target = only("the concert on March 15th");
    assert_eq!(
        target.start,
        Utc.with_ymd_and_hms(2023, 3, 15, 12, 0, 0).unwrap()
    );
    assert_eq!(target.start, target.end);
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
fn points_fall_off_with_spread() {
    let targets = time_targets("10 days ago", now());
    let at = targets[0].start;
    assert_eq!(time_match(&targets, at), 1.0);
    let one_spread = time_match(&targets, at - Duration::hours(60));
    assert!((one_spread - (-0.5f64).exp()).abs() < 1e-9);
    assert!(time_match(&targets, now()) < 0.001);
    assert_eq!(time_match(&[], at), 0.0);
}

#[test]
fn spans_match_fully_anywhere_inside() {
    let targets = time_targets("in the past month", now());
    for days in [1, 15, 29] {
        assert_eq!(time_match(&targets, now() - Duration::days(days)), 1.0);
    }
    assert!(time_match(&targets, now() - Duration::days(60)) < 0.01);
}
