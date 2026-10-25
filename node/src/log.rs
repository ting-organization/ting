//! The node's messages (wiki: start/node#what-the-node-writes): one line
//! each on standard error, after the UTC time it was written, so that
//! `node.log` tells when each thing happened.
use std::fmt;

/// `seconds` since the Unix epoch as a UTC time, `2026-10-21T00:00:00Z`.
pub fn utc(seconds: u64) -> String {
    let (days, time) = (seconds / 86_400, seconds % 86_400);
    // The civil date of a day count from 1970-01-01, in 400-year eras that
    // begin on 1 March, so that the leap day ends each year.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        time / 60 % 60,
        time % 60
    )
}

/// A span of `seconds` in its two largest units: `45s`, `3m 20s`, `2h 5m`,
/// `3d 4h`.
pub fn duration(seconds: u64) -> String {
    let units = [(86_400, "d"), (3_600, "h"), (60, "m"), (1, "s")];
    let Some(first) = units.iter().position(|(size, _)| seconds >= *size) else {
        return "0s".into();
    };
    let (size, unit) = units[first];
    let mut text = format!("{}{unit}", seconds / size);
    if let Some(&(next, unit)) = units.get(first + 1)
        && (seconds % size) / next > 0
    {
        text += &format!(" {}{unit}", (seconds % size) / next);
    }
    text
}

/// A count with its thousands separated: `5,420`.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut text = String::new();
    for (k, digit) in digits.chars().enumerate() {
        if k > 0 && (digits.len() - k).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// Writes one message, after the time.
pub fn line(message: fmt::Arguments) {
    eprintln!("{} {message}", utc(crate::now()));
}

/// Writes one message of the node, formatted as by `format!`.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::line(format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::{count, duration, utc};

    #[test]
    fn spans_and_counts_read_as_people_write_them() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(60), "1m");
        assert_eq!(duration(200), "3m 20s");
        assert_eq!(duration(7_500), "2h 5m");
        assert_eq!(duration(7_201), "2h");
        assert_eq!(duration(273_600), "3d 4h");
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(5_420), "5,420");
        assert_eq!(count(100_000_000), "100,000,000");
    }

    #[test]
    fn times_are_utc_dates() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        // The production network's launch.
        assert_eq!(utc(1_792_540_800), "2026-10-21T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(951_868_799), "2000-02-29T23:59:59Z");
        assert_eq!(utc(4_107_542_400), "2100-03-01T00:00:00Z");
        assert_eq!(utc(1_792_933_008), "2026-10-25T12:56:48Z");
    }
}
