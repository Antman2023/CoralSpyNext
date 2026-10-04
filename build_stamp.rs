//! Dependency-free UTC build timestamp formatting for build.rs and tests.
//! Only whole Unix seconds in the unambiguous 1970..=9999 range are accepted.
const MAX_UNIX_SECONDS: u64 = 253_402_300_799;

pub fn resolve_epoch(source_date_epoch: Option<&str>, now: u64) -> Result<u64, String> {
    let seconds = match source_date_epoch {
        Some(value) if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => value
            .parse::<u64>()
            .map_err(|_| "SOURCE_DATE_EPOCH exceeds the supported range".to_string())?,
        Some(_) => {
            return Err("SOURCE_DATE_EPOCH must contain nonnegative whole Unix seconds".into())
        }
        None => now,
    };
    if seconds > MAX_UNIX_SECONDS {
        return Err("Build time must fall between 1970-01-01 and 9999-12-31 UTC".into());
    }
    Ok(seconds)
}

pub fn format_utc(seconds: u64) -> Result<String, String> {
    let seconds = resolve_epoch(None, seconds)?;
    let days = (seconds / 86_400) as i64;
    // Gregorian civil date from Unix days. The shift starts the computational
    // year in March, making leap-day handling part of the 400-year cycle.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = march_month + if march_month < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let time = seconds % 86_400;
    Ok(format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        time / 3600,
        time / 60 % 60,
        time % 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_day_boundary() {
        assert_eq!(format_utc(0).unwrap(), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_utc(86_399).unwrap(), "1970-01-01 23:59:59 UTC");
        assert_eq!(format_utc(86_400).unwrap(), "1970-01-02 00:00:00 UTC");
    }

    #[test]
    fn leap_day_in_a_400_year_cycle() {
        assert_eq!(format_utc(951_782_400).unwrap(), "2000-02-29 00:00:00 UTC");
        assert_eq!(format_utc(951_868_800).unwrap(), "2000-03-01 00:00:00 UTC");
    }

    #[test]
    fn a_non_leap_century() {
        assert_eq!(
            format_utc(4_107_542_400).unwrap(),
            "2100-03-01 00:00:00 UTC"
        );
    }

    #[test]
    fn final_supported_second() {
        assert_eq!(
            format_utc(MAX_UNIX_SECONDS).unwrap(),
            "9999-12-31 23:59:59 UTC"
        );
        assert!(format_utc(MAX_UNIX_SECONDS + 1).is_err());
    }

    #[test]
    fn reproducible_override_wins_over_the_clock() {
        assert_eq!(
            resolve_epoch(Some("1234567890"), 9_999).unwrap(),
            1_234_567_890
        );
        assert_eq!(resolve_epoch(None, 9_999).unwrap(), 9_999);
        assert_eq!(resolve_epoch(Some("0"), 9_999).unwrap(), 0);
    }

    #[test]
    fn invalid_override_is_not_silently_replaced() {
        for value in [
            "",
            "-1",
            "+1",
            "1.5",
            " 1",
            "1 ",
            "not-a-date",
            "18446744073709551616",
            "253402300800",
        ] {
            assert!(resolve_epoch(Some(value), 0).is_err(), "accepted {value:?}");
        }
    }
}
