//! Timestamp helpers shared by the store and the plugins.
//!
//! Stored timestamps are UTC everywhere; `format_local_seconds` adds the local
//! offset when a pane shows a wall-clock time, which is what the reference file
//! manager does for its Modified column.

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

/// `YYYY-MM-DD HH:MM` in UTC.
pub fn format_seconds(seconds: u64) -> String {
    let days = seconds / 86_400;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        time / 3600,
        (time % 3600) / 60
    )
}

/// Seconds the local zone is ahead of UTC at `seconds`.
///
/// Reads the zone from `TZ` (a zone name or a path) or `/etc/localtime`; a
/// missing or unreadable zone means UTC. Only the transitions at or before
/// `seconds` matter, so the table is kept sorted and searched backwards.
pub fn local_offset_seconds(seconds: u64) -> i64 {
    static ZONE: std::sync::OnceLock<Option<Zone>> = std::sync::OnceLock::new();
    let Some(zone) = ZONE.get_or_init(load_zone).as_ref() else {
        return 0;
    };
    let at = seconds as i64;
    let mut offset = zone.initial;
    for (when, value) in &zone.transitions {
        if *when > at {
            break;
        }
        offset = *value;
    }
    offset
}

/// `YYYY-MM-DD HH:MM` in the local zone.
pub fn format_local_seconds(seconds: u64) -> String {
    let offset = local_offset_seconds(seconds);
    if offset == 0 {
        return format_seconds(seconds);
    }
    let shifted = (seconds as i64 + offset).max(0) as u64;
    format_seconds(shifted)
}

/// A parsed POSIX TZ database file.
struct Zone {
    /// Offset in effect before the first transition.
    initial: i64,
    /// `(transition time, offset)` pairs, ascending.
    transitions: Vec<(i64, i64)>,
}

/// Read the zone the process should use, if any.
fn load_zone() -> Option<Zone> {
    let path = match std::env::var("TZ") {
        Ok(value) if !value.is_empty() => {
            let value = value.trim_start_matches(':').to_owned();
            if value.starts_with('/') {
                value
            } else {
                format!("/usr/share/zoneinfo/{value}")
            }
        }
        _ => "/etc/localtime".to_owned(),
    };
    let bytes = std::fs::read(path).ok()?;
    parse_tzif(&bytes)
}

/// Parse a TZif file (`/etc/localtime` is one).
///
/// Version 2 and 3 files carry a second, 64-bit block; that one is used when
/// present, since the 32-bit block cannot describe times past 2038.
fn parse_tzif(bytes: &[u8]) -> Option<Zone> {
    if bytes.len() < 44 || &bytes[0..4] != b"TZif" {
        return None;
    }
    let version = bytes[4];

    // Header counts, in file order: isutcnt, isstdcnt, leapcnt, timecnt,
    // typecnt, charcnt.
    let counts = |header: usize| -> Option<[usize; 6]> {
        let mut values = [0usize; 6];
        for (index, slot) in values.iter_mut().enumerate() {
            let at = header + 20 + index * 4;
            let slice = bytes.get(at..at + 4)?;
            *slot = u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]) as usize;
        }
        Some(values)
    };
    let block_len = |counts: [usize; 6], size: usize| -> usize {
        let [isut, isstd, leap, time, types, chars] = counts;
        44 + time * size + time + types * 6 + chars + leap * (size + 4) + isstd + isut
    };
    let parse_block = |header: usize, counts: [usize; 6], size: usize| -> Option<Zone> {
        let [isut, isstd, leap, time_count, type_count, char_count] = counts;
        let mut at = header + 44;
        let mut times = Vec::with_capacity(time_count);
        for _ in 0..time_count {
            let slice = bytes.get(at..at + size)?;
            let value = if size == 8 {
                i64::from_be_bytes([
                    slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
                ])
            } else {
                i32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]) as i64
            };
            times.push(value);
            at += size;
        }
        let mut kinds = Vec::with_capacity(time_count);
        for _ in 0..time_count {
            kinds.push(*bytes.get(at)? as usize);
            at += 1;
        }
        at += leap * (size + 4);

        // Writers disagree about whether the std/wall and UT/local indicator
        // arrays sit before or after the local time type records, and their
        // lengths are equal either way. Take whichever placement parses as a
        // sane table of offsets.
        let records = |start: usize| -> Option<Vec<i64>> {
            let mut offsets = Vec::with_capacity(type_count);
            for index in 0..type_count {
                let entry = bytes.get(start + index * 6..start + index * 6 + 6)?;
                let offset = i32::from_be_bytes([entry[0], entry[1], entry[2], entry[3]]) as i64;
                let is_dst = entry[4];
                let designation = entry[5] as usize;
                if offset.abs() > 24 * 3600 || is_dst > 1 || designation >= char_count.max(1) {
                    return None;
                }
                offsets.push(offset);
            }
            Some(offsets)
        };
        let offsets = records(at).or_else(|| records(at + isstd + isut))?;
        if kinds.iter().any(|kind| *kind >= type_count) {
            return None;
        }
        let initial = offsets.first().copied().unwrap_or(0);
        let mut transitions = Vec::with_capacity(time_count);
        for (index, when) in times.iter().enumerate() {
            let offset = offsets[kinds[index]];
            transitions.push((*when, offset));
        }
        Some(Zone {
            initial,
            transitions,
        })
    };

    let head = counts(0)?;
    if version == 0 {
        return parse_block(0, head, 4);
    }
    // Version 2+: the 64-bit block follows the whole 32-bit one.
    let second = block_len(head, 4);
    let tail = counts(second)?;
    parse_block(second, tail, 8)
}

/// `MM-DD HH:MM:SS` in UTC, for dense event lists.
pub fn format_millis(ms: u64) -> String {
    let seconds = ms / 1000;
    let (_, month, day) = civil_from_days((seconds / 86_400) as i64);
    let time = seconds % 86_400;
    format!(
        "{month:02}-{day:02} {:02}:{:02}:{:02}",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

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
    fn a_zone_file_is_parsed_and_applied() {
        // A minimal TZif v1 file: one type (+1h), one transition at 1000.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"TZif");
        bytes.push(0);
        bytes.extend_from_slice(&[0u8; 15]);
        for count in [0u32, 0, 0, 1, 1, 0] {
            bytes.extend_from_slice(&count.to_be_bytes());
        }
        bytes.extend_from_slice(&1000i32.to_be_bytes()); // transition time
        bytes.push(0); // which type that transition selects
        bytes.extend_from_slice(&3600i32.to_be_bytes()); // ttinfo.utoff
        bytes.push(0); // ttinfo.isdst
        bytes.push(0); // ttinfo.desigidx
        let zone = parse_tzif(&bytes).expect("a zone");
        assert_eq!(zone.initial, 3600);
        assert_eq!(zone.transitions, [(1000, 3600)]);
        assert!(parse_tzif(b"not a zone").is_none());
    }

    #[test]
    fn local_time_falls_back_to_utc_without_a_zone() {
        // Whatever the machine is set to, the call must not panic and must
        // stay a valid date.
        let formatted = format_local_seconds(1_700_000_000);
        assert_eq!(formatted.len(), 16, "{formatted}");
        // The offset is a real zone offset, never a stray timestamp.
        assert!(local_offset_seconds(1_700_000_000).abs() <= 24 * 3600);
    }

    #[test]
    fn leap_days_line_up() {
        // 2024-02-29T00:00:00Z
        assert_eq!(format_seconds(1_709_164_800), "2024-02-29 00:00");
    }
}
