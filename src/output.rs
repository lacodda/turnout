//! What a reading command prints: text for a person, or JSON for a program.
//!
//! Both come from one value. A command builds what it knows into a view - a
//! plain struct that derives `Serialize` - and hands it here with the function
//! that writes it as text. `--json` prints the view as it is; without it the
//! text function reads the same fields. A script, a status bar and the future
//! MCP server therefore see exactly what `status` and `show` show, and a field
//! cannot reach one of them without reaching the other.
//!
//! The views are not the catalog files. Those are storage, with migrations of
//! their own; the views are the contract for whoever reads turnout's output
//! (ADR 0020), so every key is always present - `null` when there is nothing -
//! and times are ISO 8601 in UTC.

use anyhow::Result;
use serde::Serialize;

use crate::cli::Format;

/// Print `view` as JSON when `format` asks for it, otherwise as `text` writes it.
pub fn emit<T: Serialize>(format: Format, view: &T, text: impl FnOnce(&T) -> String) -> Result<()> {
    let out = if format.json { json(view)? } else { text(view) };
    print!("{out}");
    if !out.is_empty() && !out.ends_with('\n') {
        println!();
    }
    Ok(())
}

/// The JSON form of a view: pretty-printed, the shape `jq` and people both read.
pub fn json<T: Serialize>(view: &T) -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(view)?))
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso8601(seconds: u64) -> String {
    let (days, rem) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = crate::utils::civil_from_days(days as i64);
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `YYYY-MM-DDTHH:MM:SSZ` back to seconds since the epoch, for the journal's
/// timestamps; `None` for anything else.
pub fn parse_iso8601(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() != 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' || bytes[19] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // Hinnant's days_from_civil, the inverse of `civil_from_days`.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_print_as_iso_8601_in_utc() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601(1_790_000_000), "2026-09-21T14:13:20Z");
    }

    /// The parser is the printer's inverse, and turns down whatever is not
    /// one of its timestamps rather than guessing.
    #[test]
    fn timestamps_read_back_as_the_seconds_they_came_from() {
        for seconds in [0, 59, 86_399, 951_782_400, 1_790_000_000, 4_102_444_800] {
            assert_eq!(parse_iso8601(&iso8601(seconds)), Some(seconds), "{}", iso8601(seconds));
        }
        assert_eq!(parse_iso8601("2026-09-21 14:13:20Z"), None);
        assert_eq!(parse_iso8601("2026-13-21T14:13:20Z"), None);
        assert_eq!(parse_iso8601("yesterday"), None);
    }
}
