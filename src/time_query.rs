//! Relative time in queries: "two weeks ago", "last Tuesday", "in March".
//!
//! Each time phrase becomes the moment it points at, counted back from when
//! the query is asked, plus a spread: how far either side still counts. The
//! spread grows with distance, because "yesterday" means one day while "two
//! months ago" means roughly that month.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc, Weekday};
use once_cell::sync::Lazy;
use regex::Regex;

/// The moment a time phrase points at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeTarget {
    pub center: DateTime<Utc>,
    /// Days either side of `center` that still match well.
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
static LAST_WEEKDAY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(?:last|this\s+past|on)\s+(monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b",
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
    let back = |days: f64, spread: f64| TimeTarget {
        center: now - Duration::seconds((days * 86_400.0) as i64),
        spread_days: spread,
    };

    for caps in AGO.captures_iter(query) {
        let (Some(count), Some(unit)) = (number(&caps[1]), unit_days(&caps[2])) else {
            continue;
        };
        let days = count * unit;
        targets.push(back(days, point_spread(days)));
    }
    for caps in PAST_SPAN.captures_iter(query) {
        let count = caps.get(1).map_or(Some(1.0), |m| number(m.as_str()));
        let (Some(count), Some(unit)) = (count, unit_days(&caps[2])) else {
            continue;
        };
        // "last week" is a stretch of time, not a point: anywhere in it counts.
        let days = count * unit;
        targets.push(back(days / 2.0, (days / 2.0).max(1.0)));
    }
    for caps in THIS_SPAN.captures_iter(query) {
        let Some(unit) = unit_days(&caps[1]) else {
            continue;
        };
        targets.push(back(unit / 4.0, (unit / 4.0).max(1.0)));
    }
    for caps in YESTERDAY.captures_iter(query) {
        let days = if caps.get(1).is_some() { 2.0 } else { 1.0 };
        targets.push(back(days, 1.0));
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
        targets.push(back(days as f64, 1.0));
    }
    for caps in MONTH_DAY.captures_iter(query) {
        let Some(month) = month(&caps[1]) else {
            continue;
        };
        let Ok(day) = caps[2].parse::<u32>() else {
            continue;
        };
        if let Some(center) = latest_date(now, month, day) {
            targets.push(TimeTarget {
                center,
                spread_days: 1.0,
            });
        }
    }
    for caps in IN_MONTH.captures_iter(query) {
        let Some(month) = month(&caps[2]) else {
            continue;
        };
        let (day, spread) = match caps[1].to_ascii_lowercase().as_str() {
            "early" => (5, 5.0),
            "mid" => (15, 5.0),
            "late" => (25, 5.0),
            _ => (15, 15.0),
        };
        // The latest such month that has begun; within it, never past `now`.
        let Some(start) = latest_date(now, month, 1) else {
            continue;
        };
        if let Some(center) = start.with_day(day) {
            targets.push(TimeTarget {
                center: center.min(now),
                spread_days: spread,
            });
        }
    }
    targets
}

/// How well `date` matches the closest of `targets`: 1.0 on target, falling
/// off as a bell curve whose width is the target's spread.
pub fn time_match(targets: &[TimeTarget], date: DateTime<Utc>) -> f64 {
    targets
        .iter()
        .map(|target| {
            let offset = (date - target.center).num_seconds() as f64 / 86_400.0;
            let z = offset / target.spread_days;
            (-0.5 * z * z).exp()
        })
        .fold(0.0, f64::max)
}

/// A point in time "N units ago" is fuzzier the further back it is.
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
