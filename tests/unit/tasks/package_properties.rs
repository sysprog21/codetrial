//! Properties of the close date of a task package.

use super::*;

fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days from 1970-01-01 to a civil date, by integer arithmetic alone.
fn days_from_epoch(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468
}

/// The packager's rule written down without chrono: a real calendar day, a
/// time of day with seconds 0 to 59 (no leap second), and not before 1970.
fn model(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> Option<u64> {
    let february = if leap(year) { 29 } else { 28 };
    let length = [31, february, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let real_day = (1..=12).contains(&month)
        && (1..=length[usize::try_from(month - 1).expect("month in range")]).contains(&day);
    if !real_day || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let seconds = days_from_epoch(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(seconds).ok()
}

/// Years where the calendar rules bite.
const YEARS: [i64; 10] = [1969, 1970, 1999, 2000, 2024, 2100, 2400, 9999, 1900, 2038];

/// One of those years, or a free draw.
fn year_of(pick: u8, free: u16) -> i64 {
    if pick.is_multiple_of(2) {
        YEARS[usize::from(pick / 2) % YEARS.len()]
    } else {
        i64::from(free % 10_000)
    }
}

#[ephyra::test(tests = 256, seed = 5)]
fn close_date_matches_the_calendar_model(
    pick: u8,
    free: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
) {
    let year = year_of(pick, free);
    let (month, day) = (i64::from(month % 14), i64::from(day % 33));
    let (hour, minute, second) = (
        i64::from(hour % 25),
        i64::from(minute % 61),
        i64::from(second % 62),
    );
    let text = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z");
    assert_eq!(
        parse_close(&text).ok(),
        model(year, month, day, hour, minute, second),
        "{text}"
    );
}

/// The end of February in each year that decides it.
#[test]
fn close_date_agrees_on_the_end_of_february() {
    for year in YEARS {
        for day in 27..=30 {
            let text = format!("{year:04}-02-{day:02}T00:00:00Z");
            assert_eq!(
                parse_close(&text).ok(),
                model(year, 2, day, 0, 0, 0),
                "{text}"
            );
        }
    }
}

/// Every one-byte substitution of a lowercase letter into the accepted shape.
#[test]
fn close_date_refuses_every_noncanonical_character() {
    let canonical = *b"2026-10-06T12:34:56Z";
    for slot in 0..canonical.len() {
        for letter in b'a'..=b'z' {
            let mut text = canonical;
            text[slot] = letter;
            let text = std::str::from_utf8(&text).expect("ASCII date");
            assert!(parse_close(text).is_err(), "{text}");
        }
    }
}
