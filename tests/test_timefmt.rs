//! Timestamps are stored as text and compared as text, so the written format
//! must stay byte-for-byte what earlier releases wrote, and every timestamp
//! an earlier release wrote must still be read.

use jiff::Timestamp;
use palace::timefmt::{now_rfc3339, parse_rfc3339, rfc3339, rfc3339_ago, utc_date};

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

#[test]
fn whole_seconds_have_no_fraction_and_a_numeric_offset() {
    assert_eq!(
        rfc3339(at("2023-03-25T18:00:00Z")),
        "2023-03-25T18:00:00+00:00"
    );
}

#[test]
fn fraction_is_three_six_or_nine_digits_like_earlier_releases() {
    assert_eq!(
        rfc3339(at("2023-03-25T18:00:00.123Z")),
        "2023-03-25T18:00:00.123+00:00"
    );
    assert_eq!(
        rfc3339(at("2023-03-25T18:00:00.120Z")),
        "2023-03-25T18:00:00.120+00:00"
    );
    assert_eq!(
        rfc3339(at("2023-03-25T18:00:00.123456Z")),
        "2023-03-25T18:00:00.123456+00:00"
    );
    assert_eq!(
        rfc3339(at("2023-03-25T18:00:00.123450Z")),
        "2023-03-25T18:00:00.123450+00:00"
    );
    assert_eq!(
        rfc3339(at("2023-03-25T18:00:00.123456789Z")),
        "2023-03-25T18:00:00.123456789+00:00"
    );
    assert_eq!(
        rfc3339(at("2023-03-25T18:00:00.000000001Z")),
        "2023-03-25T18:00:00.000000001+00:00"
    );
}

#[test]
fn now_is_written_in_the_same_format() {
    let text = now_rfc3339();
    assert!(text.ends_with("+00:00"), "{text}");
    assert_eq!(rfc3339(parse_rfc3339(&text).unwrap()), text);
}

#[test]
fn reads_timestamps_written_by_earlier_releases() {
    let written = [
        ("2023-03-25T18:00:00+00:00", "2023-03-25T18:00:00Z"),
        ("2023-03-25T18:00:00.123+00:00", "2023-03-25T18:00:00.123Z"),
        (
            "2023-03-25T18:00:00.123456+00:00",
            "2023-03-25T18:00:00.123456Z",
        ),
        (
            "2023-03-25T18:00:00.123456789+00:00",
            "2023-03-25T18:00:00.123456789Z",
        ),
        // Facts and hashes were stamped with the local offset.
        ("2023-03-25T19:00:00+01:00", "2023-03-25T18:00:00Z"),
        ("2023-03-25T13:30:00.500-04:30", "2023-03-25T18:00:00.500Z"),
        ("2023-03-25T18:00:00Z", "2023-03-25T18:00:00Z"),
    ];
    for (text, expected) in written {
        assert_eq!(parse_rfc3339(text), Some(at(expected)), "{text}");
    }
}

#[test]
fn text_that_is_not_a_timestamp_is_not_read() {
    for text in [
        "",
        "yesterday",
        "2023-03-25",
        "2023-03-25T18:00:00",
        "1679767200",
    ] {
        assert_eq!(parse_rfc3339(text), None, "{text:?}");
    }
}

#[test]
fn written_text_orders_like_the_moments_it_names() {
    let moments = [
        at("2023-03-25T17:59:59Z"),
        at("2023-03-25T18:00:00Z"),
        at("2023-03-25T18:00:00.000001001Z"),
        at("2023-03-25T18:00:00.001Z"),
        at("2023-03-25T18:00:01Z"),
    ];
    let texts: Vec<String> = moments.iter().map(|m| rfc3339(*m)).collect();
    let mut sorted = texts.clone();
    sorted.sort();
    assert_eq!(texts, sorted);
}

#[test]
fn ago_counts_back_from_now_and_never_overflows() {
    let hour = parse_rfc3339(&rfc3339_ago(3_600)).unwrap();
    let gap = Timestamp::now().as_second() - hour.as_second();
    assert!((3_599..=3_601).contains(&gap), "{gap}");
    assert_eq!(rfc3339_ago(i64::MAX), rfc3339(Timestamp::MIN));
}

#[test]
fn utc_date_is_the_calendar_day_in_utc() {
    assert_eq!(utc_date(at("2023-03-25T23:59:59Z")), "2023-03-25");
    assert_eq!(utc_date(at("2023-03-26T00:00:00Z")), "2023-03-26");
}
