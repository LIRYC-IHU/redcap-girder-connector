//! The date arithmetic every deidentifier shares.

use dicom_deid::dates;

fn days(date: &str) -> i64 {
    dates::parse(date).expect("a date").days.0
}

#[test]
fn a_range_still_counts_as_holding_a_date() {
    assert!(dates::begins_with_date("20190101-20190201"));
    assert!(dates::begins_with_date("20190101"));
    assert!(!dates::begins_with_date("12.5"));
}

#[test]
fn only_a_time_of_day_may_trail_a_date() {
    for value in [
        "20240115",
        "20240115143512",
        "20240115143512.123456",
        "20240115143512.000+0100",
        "20240115143512-0500",
        "20240115-0500",
        "202401151435",
        "2024-01-15",
        "2024-01-15T14:35:12",
        "2024-01-15T14:35:12.5Z",
        "2024-01-15T14:35:12+01:00",
        "2024-01-15 14:35",
    ] {
        assert!(dates::parse(value).is_some(), "{value}");
    }

    for value in [
        "20190101-20190201",
        "20190101120000-20190201120000",
        "20190101-",
        "20190101 born 19540212",
        "2019-01-01/2019-02-01",
        "20190101143512999",
    ] {
        assert!(dates::parse(value).is_none(), "{value}");
    }
}

#[test]
fn a_date_moves_by_exactly_the_offset() {
    for offset in [-18_262, -27_000, -36_524] {
        let shifted = dates::shift("20240115", offset).unwrap();
        assert_eq!(days(&shifted) - days("20240115"), offset, "{shifted}");
    }
}

#[test]
fn the_interval_between_two_dates_is_preserved() {
    // 2024 is a leap year, so this crosses a 29 February.
    let first = dates::shift("20240115", -20_000).unwrap();
    let second = dates::shift("20240315", -20_000).unwrap();

    assert_eq!(days(&second) - days(&first), 60);
}

#[test]
fn the_time_of_day_and_notation_survive() {
    assert_eq!(dates::shift("20240115093000", 0).unwrap(), "20240115093000");
    assert_eq!(dates::shift("2024-01-15", 0).unwrap(), "2024-01-15");
    assert_eq!(
        dates::shift("20240115093000.500", 1).unwrap(),
        "20240116093000.500"
    );
}

#[test]
fn non_dates_are_not_shifted() {
    // Identifiers, measurements and signal samples must pass through untouched.
    for value in [
        "0105883586", // subject id
        "12345678",   // eight digits, but not a calendar date
        "20241315",   // month 13
        "20240230",   // 30 February
        "18000101",   // outside the plausible range
        "",
        "REC-42",
        "1.2.840.10008",
    ] {
        assert!(
            dates::parse(value).is_none(),
            "{value:?} was mistaken for a date"
        );
        assert!(dates::shift(value, 100).is_none());
    }
}

#[test]
fn real_calendar_dates_are_recognized() {
    for value in ["19540212", "20240229", "2024-02-29", "20240115093000"] {
        assert!(
            dates::parse(value).is_some(),
            "{value:?} was not recognized"
        );
    }
}

#[test]
fn leap_days_round_trip() {
    // 2024-02-29 shifted by a whole number of days must stay a real date.
    let shifted = dates::shift("20240229", -20_000).unwrap();

    assert!(dates::parse(&shifted).is_some(), "{shifted} is not a date");
    assert_eq!(shifted.len(), 8);
}

// --- Age ---------------------------------------------------------------------

#[test]
fn a_birthday_not_yet_reached_does_not_count() {
    let before = dates::age("19540212", "20240115").unwrap();
    let after = dates::age("19540212", "20240212").unwrap();

    assert_eq!(before.years, 69);
    assert_eq!(after.years, 70);
}

#[test]
fn months_and_days_are_counted_in_total() {
    let age = dates::age("20231001", "20240115").unwrap();

    assert_eq!(age.years, 0);
    assert_eq!(age.months, 3);
    assert_eq!(age.days, 106);
}

#[test]
fn a_birth_on_29_february_ages_on_1_march() {
    assert_eq!(dates::age("20000229", "20230228").unwrap().years, 22);
    assert_eq!(dates::age("20000229", "20230301").unwrap().years, 23);
}

#[test]
fn there_is_no_age_before_birth_or_without_dates() {
    assert_eq!(dates::age("20240115", "20230101"), None);
    assert_eq!(dates::age("UNKNOWN", "20240115"), None);
    assert_eq!(dates::age("19540212", ""), None);
}
