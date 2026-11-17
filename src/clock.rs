//! Timestamps as Palace stores them.
//!
//! Stored times are RFC 3339 strings in UTC with a fixed `+00:00` offset and
//! nine fractional digits, such as `2026-11-17T10:38:08.896131500+00:00`.
//! Palace compares them as text, so every writer must use this one shape.

use jiff::{SignedDuration, Timestamp};

const FORMAT: &str = "%Y-%m-%dT%H:%M:%S.%N+00:00";

/// `at` in the stored shape.
pub fn rfc3339(at: Timestamp) -> String {
    at.strftime(FORMAT).to_string()
}

/// The current time in the stored shape.
pub fn now_rfc3339() -> String {
    rfc3339(Timestamp::now())
}

/// The time `ago` before now, in the stored shape.
pub fn ago_rfc3339(ago: SignedDuration) -> String {
    rfc3339(Timestamp::now() - ago)
}

/// Today's date in the local time zone, as `YYYY-MM-DD`.
pub fn local_date() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d").to_string()
}

/// Parse an RFC 3339 string with any offset.
pub fn parse(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

/// A span of whole days.
pub fn days(count: i64) -> SignedDuration {
    SignedDuration::from_hours(count.saturating_mul(24))
}

/// A span of whole hours.
pub fn hours(count: i64) -> SignedDuration {
    SignedDuration::from_hours(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_shape_is_fixed() {
        let at: Timestamp = "2026-11-17T10:38:08.8961315Z".parse().unwrap();
        assert_eq!(rfc3339(at), "2026-11-17T10:38:08.896131500+00:00");
        let whole: Timestamp = "2026-11-17T10:38:08Z".parse().unwrap();
        assert_eq!(rfc3339(whole), "2026-11-17T10:38:08.000000000+00:00");
    }

    #[test]
    fn parses_old_and_offset_forms() {
        assert!(parse("2023-03-25T18:00:00+00:00").is_some());
        assert!(parse("2026-11-17T10:38:08.896Z").is_some());
        assert_eq!(
            parse("2023-03-25T20:00:00+02:00"),
            parse("2023-03-25T18:00:00+00:00")
        );
        assert!(parse("not a date").is_none());
    }
}
