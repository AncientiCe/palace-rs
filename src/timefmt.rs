//! Timestamps as stored text.
//!
//! Timestamps are kept as RFC 3339 text and compared as text (`created_at >=
//! cutoff`), so the form written must not drift: a numeric offset (`+00:00`,
//! never `Z`) and a fraction of 3, 6 or 9 digits, or none for whole seconds.
//! That is the form every earlier release wrote, and palaces on disk hold it.

use jiff::civil::DateTime;
use jiff::tz::{Offset, TimeZone};
use jiff::{SignedDuration, Timestamp, ZonedDateTime};

/// `timestamp` as stored text, at UTC.
pub fn rfc3339(timestamp: Timestamp) -> String {
    format_at(TimeZone::UTC.to_datetime(timestamp), Offset::UTC)
}

/// The current moment as stored text, at UTC.
pub fn now_rfc3339() -> String {
    rfc3339(Timestamp::now())
}

/// The moment `seconds` before now as stored text, at UTC. A span reaching
/// past the earliest representable moment gives that moment.
pub fn rfc3339_ago(seconds: i64) -> String {
    let now = Timestamp::now();
    rfc3339(
        now.checked_sub(SignedDuration::from_secs(seconds))
            .unwrap_or(Timestamp::MIN),
    )
}

/// The current moment as stored text, at the machine's local offset.
pub fn local_now_rfc3339() -> String {
    let now = ZonedDateTime::now();
    format_at(now.datetime(), now.offset())
}

/// Today's date in the machine's local time zone, as `YYYY-MM-DD`.
pub fn local_today() -> String {
    ZonedDateTime::now().date().strftime("%Y-%m-%d").to_string()
}

/// The calendar day of `timestamp` at UTC, as `YYYY-MM-DD`.
pub fn utc_date(timestamp: Timestamp) -> String {
    TimeZone::UTC
        .to_datetime(timestamp)
        .date()
        .strftime("%Y-%m-%d")
        .to_string()
}

/// The moment an RFC 3339 timestamp names, whatever offset it was written
/// with. `None` when the text is not one.
pub fn parse_rfc3339(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

fn format_at(datetime: DateTime, offset: Offset) -> String {
    let nanos = datetime.subsec_nanosecond();
    let fraction = if nanos == 0 {
        String::new()
    } else if nanos % 1_000_000 == 0 {
        format!(".{:03}", nanos / 1_000_000)
    } else if nanos % 1_000 == 0 {
        format!(".{:06}", nanos / 1_000)
    } else {
        format!(".{nanos:09}")
    };
    let seconds = offset.seconds();
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{fraction}{sign}{:02}:{:02}",
        datetime.year(),
        datetime.month(),
        datetime.day(),
        datetime.hour(),
        datetime.minute(),
        datetime.second(),
        minutes / 60,
        minutes % 60,
    )
}
