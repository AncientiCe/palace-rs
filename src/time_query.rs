//! Relative time in queries: "two weeks ago", "last Tuesday", "in March".
//!
//! Each time phrase becomes the stretch of time it points at, counted back
//! from when the query is asked, plus a spread: how far outside that stretch
//! still counts. A point in time ("10 days ago") is a stretch of zero length
//! whose spread grows with distance, because "yesterday" means one day while
//! "two months ago" means roughly that month. A span ("in the past month")
//! counts in full anywhere inside it.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc, Weekday};
use once_cell::sync::Lazy;
use regex::Regex;

/// The stretch of time a time phrase points at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeTarget {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
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
pub fn time_targets(query: &str, now: DateTime<Utc>) -> Vec<TimeTarget> {
    let mut targets = Vec::new();
    let days_back = |days: f64| now - Duration::seconds((days * 86_400.0) as i64);
    let point = |at: DateTime<Utc>, spread_days: f64| TimeTarget {
        start: at,
        end: at,
        spread_days,
    };

    for caps in AGO.captures_iter(query) {
        let (Some(count), Some(unit)) = (number(&caps[1]), unit_days(&caps[2])) else {
            continue;
        };
        let days = count * unit;
        targets.push(point(days_back(days), point_spread(days)));
    }
    for caps in PAST_SPAN.captures_iter(query) {
        let count = caps.get(1).map_or(Some(1.0), |m| number(m.as_str()));
        let (Some(count), Some(unit)) = (count, unit_days(&caps[2])) else {
            continue;
        };
        targets.push(TimeTarget {
            start: days_back(count * unit),
            end: now,
            spread_days: point_spread(unit),
        });
    }
    for caps in THIS_SPAN.captures_iter(query) {
        let today = now.date_naive();
        let start = match caps[1].to_ascii_lowercase().as_str() {
            "week" => today - Duration::days(now.weekday().num_days_from_monday().into()),
            "month" => today.with_day(1).unwrap_or(today),
            _ => today.with_ordinal(1).unwrap_or(today),
        };
        let Some(start) = start.and_hms_opt(0, 0, 0) else {
            continue;
        };
        targets.push(TimeTarget {
            start: Utc.from_utc_datetime(&start),
            end: now,
            spread_days: 1.0,
        });
    }
    for caps in YESTERDAY.captures_iter(query) {
        let days = if caps.get(1).is_some() { 2.0 } else { 1.0 };
        targets.push(point(days_back(days), 1.0));
    }
    for caps in LAST_WEEKDAY.captures_iter(query) {
        let Some(weekday) = weekday(&caps[1]) else {
            continue;
        };
        // The most recent such day before today.
        let today = now.weekday().num_days_from_monday() as i64;
        let wanted = weekday.num_days_from_monday() as i64;
        let days = match (today - wanted).rem_euclid(7) {
            0 => 7,
            days => days,
        };
        targets.push(point(days_back(days as f64), 1.0));
    }
    for caps in MONTH_DAY.captures_iter(query) {
        let Some(month) = month(&caps[1]) else {
            continue;
        };
        let Ok(day) = caps[2].parse::<u32>() else {
            continue;
        };
        if let Some(at) = latest_date(now, month, day) {
            targets.push(point(at, 1.0));
        }
    }
    for caps in IN_MONTH.captures_iter(query) {
        let Some(month) = month(&caps[2]) else {
            continue;
        };
        // The latest such month that has begun.
        let Some(first) = latest_date(now, month, 1) else {
            continue;
        };
        let last = days_in_month(first);
        let (from, to) = match caps[1].to_ascii_lowercase().as_str() {
            "early" => (1, 10),
            "mid" => (11, 20),
            "late" => (21, last),
            _ => (1, last),
        };
        let (Some(start), Some(end)) = (first.with_day(from), first.with_day(to)) else {
            continue;
        };
        // Whole days: from the first day's start to the last day's end.
        targets.push(TimeTarget {
            start: start - Duration::hours(12),
            end: (end + Duration::hours(12)).min(now),
            spread_days: 3.0,
        });
    }
    targets
}

/// How well `date` matches the closest of `targets`: 1.0 inside a target,
/// falling off outside it as a bell curve whose width is the target's spread.
pub fn time_match(targets: &[TimeTarget], date: DateTime<Utc>) -> f64 {
    targets
        .iter()
        .map(|target| {
            let outside = if date < target.start {
                target.start - date
            } else if date > target.end {
                date - target.end
            } else {
                Duration::zero()
            };
            let z = outside.num_seconds() as f64 / 86_400.0 / target.spread_days;
            (-0.5 * z * z).exp()
        })
        .fold(0.0, f64::max)
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
    name.to_ascii_lowercase().parse().ok()
}

fn month(name: &str) -> Option<u32> {
    let name = name.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|month| *month == name)
        .map(|index| index as u32 + 1)
}

fn days_in_month(first: DateTime<Utc>) -> u32 {
    (28..=31)
        .rev()
        .find(|&day| first.with_day(day).is_some())
        .unwrap_or(28)
}

/// The latest `month`/`day` that is not after `now`, at noon.
fn latest_date(now: DateTime<Utc>, month: u32, day: u32) -> Option<DateTime<Utc>> {
    let at = |year: i32| {
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_opt(12, 0, 0))
            .map(|naive| Utc.from_utc_datetime(&naive))
    };
    let this_year = at(now.year())?;
    if this_year <= now {
        Some(this_year)
    } else {
        at(now.year() - 1)
    }
}
