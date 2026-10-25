//! Timestamp formatting shared by the plugins that list times.
//!
//! Logs stay in UTC so events from different zones line up; a file listing
//! shows the wall clock, which is what the reference manager does.

pub use wassh_core::time::{
    civil_from_days, format_local_seconds, format_millis, format_seconds,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_seconds_read_as_a_date() {
        // 2023-11-14T22:13:20Z
        assert_eq!(format_seconds(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_seconds(0), "1970-01-01 00:00");
    }

    #[test]
    fn milliseconds_keep_the_seconds() {
        assert_eq!(format_millis(1_700_000_000_000), "11-14 22:13:20");
        // Sub-second parts are dropped, they do not round up.
        assert_eq!(format_millis(1_700_000_000_999), "11-14 22:13:20");
        assert_eq!(format_millis(0), "01-01 00:00:00");
    }

    #[test]
    fn leap_days_line_up() {
        // 2024-02-29T00:00:00Z
        assert_eq!(format_seconds(1_709_164_800), "2024-02-29 00:00");
    }
}
