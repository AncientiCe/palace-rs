//! Relative time in queries: "two weeks ago", "last Tuesday", "in March".
//!
//! Each time phrase becomes the stretch of time it points at, counted back
//! from when the query is asked, plus a spread: how far outside that stretch
//! still counts. A point in time ("10 days ago") is a stretch of zero length
//! whose spread grows with distance, because "yesterday" means one day while
//! "two months ago" means roughly that month. A span ("in the past month")
//! counts in full anywhere inside it.

use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span, Timestamp};
use once_cell::sync::Lazy;
use regex::Regex;

/// The stretch of time a time phrase points at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeTarget {
    pub start: Timestamp,
    pub end: Timestamp,
    /// Days outside `start..=end` that still match well.
    pub spread_days: f64,
}

const NUMBER: &str = r"\d+|an?|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|(?:a\s+)?couple(?:\s+of)?|(?:a\s+)?few";
const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

static AGO: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)\b({NUMBER})\s+(day|week|month|year)s?\s+(?:ago|back|before)\b"
    ))
    .unwrap()
});
static PAST_SPAN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:past|last|previous)\s+(?:({NUMBER})\s+)?(day|week|month|year)s?\b"
    ))
    .unwrap()
});
static THIS_SPAN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bthis\s+(week|month|year)\b").unwrap());
static YESTERDAY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(the\s+day\s+before\s+)?yesterday\b").unwrap());
// "on Saturday" is left out: it as often means a habit ("on Saturday
// mornings") or a plan as a day that has passed.
static LAST_WEEKDAY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(?:last|this\s+past)\s+(monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b",
    )
    .unwrap()
});
static MONTH_DAY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)\b({})\s+(\d{{1,2}})(?:st|nd|rd|th)?\b",
        MONTHS.join("|")
    ))
    .unwrap()
});
// A bare month name needs a lead-in word, so "may" the verb never matches.
static IN_MONTH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)\b(in|during|early|mid|late|last)[\s-]+({})\b",
        MONTHS.join("|")
    ))
    .unwrap()
});

/// Every time phrase in `query`, resolved against `now`.
pub fn time_targets(query: &str, now: Timestamp) -> Vec<TimeTarget> {
    let mut targets = Vec::new();
    let days_back =
        |days: f64| now.checked_sub(SignedDuration::from_secs((days * 86_400.0) as i64));
    let today = utc_date(now);

    for caps in AGO.captures_iter(query) {
        let (Some(count), Some(unit)) = (number(&caps[1]), unit_days(&caps[2])) else {
            continue;
        };
        let days = count * unit;
        let Ok(at) = days_back(days) else {
            continue;
        };
        targets.push(point(at, point_spread(days)));
    }
    for caps in PAST_SPAN.captures_iter(query) {
        let count = caps.get(1).map_or(Some(1.0), |m| number(m.as_str()));
        let (Some(count), Some(unit)) = (count, unit_days(&caps[2])) else {
            continue;
        };
        let Ok(start) = days_back(count * unit) else {
            continue;
        };
        targets.push(TimeTarget {
            start,
            end: now,
            spread_days: point_spread(unit),
        });
    }
    for caps in THIS_SPAN.captures_iter(query) {
        let start = match caps[1].to_ascii_lowercase().as_str() {
            "week" => today
                .checked_sub(Span::new().days(today.weekday().to_monday_zero_offset()))
                .unwrap_or(today),
            "month" => today.first_of_month(),
            _ => today.first_of_year(),
        };
        let Some(start) = midnight(start) else {
            continue;
        };
        targets.push(TimeTarget {
            start,
            end: now,
            spread_days: 1.0,
        });
    }
    for caps in YESTERDAY.captures_iter(query) {
        let days = if caps.get(1).is_some() { 2.0 } else { 1.0 };
        if let Ok(at) = days_back(days) {
            targets.push(point(at, 1.0));
        }
    }
    for caps in LAST_WEEKDAY.captures_iter(query) {
        let Some(weekday) = weekday(&caps[1]) else {
            continue;
        };
        // The most recent such day before today.
        let current = i64::from(today.weekday().to_monday_zero_offset());
        let wanted = i64::from(weekday.to_monday_zero_offset());
        let days = match (current - wanted).rem_euclid(7) {
            0 => 7,
            days => days,
        };
        if let Ok(at) = days_back(days as f64) {
            targets.push(point(at, 1.0));
        }
    }
    for caps in MONTH_DAY.captures_iter(query) {
        let Some(month) = month(&caps[1]) else {
            continue;
        };
        let Ok(day) = caps[2].parse::<i8>() else {
            continue;
        };
        if let Some(at) = latest_date(today, now, month, day).and_then(noon) {
            targets.push(point(at, 1.0));
        }
    }
    for caps in IN_MONTH.captures_iter(query) {
        let Some(month) = month(&caps[2]) else {
            continue;
        };
        // The latest such month that has begun.
        let Some(first) = latest_date(today, now, month, 1) else {
            continue;
        };
        let last = first.days_in_month();
        let (from, to) = match caps[1].to_ascii_lowercase().as_str() {
            "early" => (1, 10),
            "mid" => (11, 20),
            "late" => (21, last),
            _ => (1, last),
        };
        // Whole days: from the first day's start to the last day's end.
        let (Ok(start), Ok(end)) = (
            first.with().day(from).build().map(midnight),
            first.with().day(to).build().map(midnight),
        ) else {
            continue;
        };
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        let Ok(end) = end.checked_add(SignedDuration::from_hours(24)) else {
            continue;
        };
        targets.push(TimeTarget {
            start,
            end: end.min(now),
            spread_days: 3.0,
        });
    }
    targets
}

/// How well `date` matches the closest of `targets`: 1.0 inside a target,
/// falling off outside it as a bell curve whose width is the target's spread.
pub fn time_match(targets: &[TimeTarget], date: Timestamp) -> f64 {
    targets
        .iter()
        .map(|target| {
            let outside = if date < target.start {
                target.start.duration_since(date)
            } else if date > target.end {
                date.duration_since(target.end)
            } else {
                SignedDuration::ZERO
            };
            let z = outside.as_secs() as f64 / 86_400.0 / target.spread_days;
            (-0.5 * z * z).exp()
        })
        .fold(0.0, f64::max)
}

fn point(at: Timestamp, spread_days: f64) -> TimeTarget {
    TimeTarget {
        start: at,
        end: at,
        spread_days,
    }
}

fn utc_date(at: Timestamp) -> Date {
    TimeZone::UTC.to_datetime(at).date()
}

fn midnight(date: Date) -> Option<Timestamp> {
    TimeZone::UTC.to_timestamp(date.at(0, 0, 0, 0)).ok()
}

fn noon(date: Date) -> Option<Timestamp> {
    TimeZone::UTC.to_timestamp(date.at(12, 0, 0, 0)).ok()
}

/// A point in time "N days ago" is fuzzier the further back it is.
fn point_spread(days: f64) -> f64 {
    (days / 4.0).max(1.0)
}

fn number(text: &str) -> Option<f64> {
    let text = text.to_ascii_lowercase();
    let word = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let value = match word.as_str() {
        "a" | "an" | "one" => 1.0,
        "two" => 2.0,
        "three" => 3.0,
        "four" => 4.0,
        "five" => 5.0,
        "six" => 6.0,
        "seven" => 7.0,
        "eight" => 8.0,
        "nine" => 9.0,
        "ten" => 10.0,
        "eleven" => 11.0,
        "twelve" => 12.0,
        _ if word.contains("couple") => 2.0,
        _ if word.contains("few") => 3.0,
        _ => word.parse::<u32>().ok()?.into(),
    };
    Some(value)
}

fn unit_days(unit: &str) -> Option<f64> {
    match unit.to_ascii_lowercase().as_str() {
        "day" => Some(1.0),
        "week" => Some(7.0),
        "month" => Some(30.0),
        "year" => Some(365.0),
        _ => None,
    }
}

fn weekday(name: &str) -> Option<Weekday> {
    match name.to_ascii_lowercase().as_str() {
        "monday" => Some(Weekday::Monday),
        "tuesday" => Some(Weekday::Tuesday),
        "wednesday" => Some(Weekday::Wednesday),
        "thursday" => Some(Weekday::Thursday),
        "friday" => Some(Weekday::Friday),
        "saturday" => Some(Weekday::Saturday),
        "sunday" => Some(Weekday::Sunday),
        _ => None,
    }
}

fn month(name: &str) -> Option<i8> {
    let name = name.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|month| *month == name)
        .and_then(|index| i8::try_from(index + 1).ok())
}

/// The latest `month`/`day` that is not after `now` (at noon). `today` is
/// `now`'s date. A date missing from this year, like 29 February, gives none.
fn latest_date(today: Date, now: Timestamp, month: i8, day: i8) -> Option<Date> {
    let this_year = Date::new(today.year(), month, day).ok()?;
    if noon(this_year)? <= now {
        Some(this_year)
    } else {
        Date::new(today.year().checked_sub(1)?, month, day).ok()
    }
}
