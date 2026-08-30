//! The date arithmetic every deidentifier shares.

use dicom_deid::dates;

fn shift(value: &str, birth: &str) -> String {
    let offset = dates::offset_from_birth_date(birth).expect("birth date parses");
    dates::shift(value, offset).expect("value is a date")
}

#[test]
fn a_birth_date_lands_on_the_epoch() {
    for birth in ["19540212", "19700101", "20240229", "1954-02-12"] {
        assert_eq!(
            shift(birth, birth).replace('-', ""),
            "19700101",
            "{birth} did not land on the epoch"
        );
    }
}

#[test]
fn the_interval_between_two_dates_is_preserved() {
    let birth = "19540212";
    let offset = dates::offset_from_birth_date(birth).unwrap();

    // 2024 is a leap year, so this crosses a 29 February.
    let first = dates::shift("20240115", offset).unwrap();
    let second = dates::shift("20240315", offset).unwrap();

    assert_eq!(
        dates::parse(&second).unwrap().days.0 - dates::parse(&first).unwrap().days.0,
        60
    );
}

#[test]
fn the_time_of_day_and_notation_survive() {
    assert_eq!(shift("20240115093000", "19700101"), "20240115093000");
    assert_eq!(shift("2024-01-15", "19700101"), "2024-01-15");
    assert_eq!(
        shift("20240115093000.500", "19700101"),
        "20240115093000.500"
    );
}

#[test]
fn a_patient_born_on_the_epoch_is_not_shifted() {
    assert_eq!(dates::offset_from_birth_date("19700101"), Some(0));
}

#[test]
fn two_patients_get_different_offsets() {
    assert_ne!(
        dates::offset_from_birth_date("19400301"),
        dates::offset_from_birth_date("19800301")
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
    let offset = dates::offset_from_birth_date("19540212").unwrap();
    let shifted = dates::shift("20240229", offset).unwrap();

    assert!(dates::parse(&shifted).is_some(), "{shifted} is not a date");
    assert_eq!(shifted.len(), 8);
}

#[test]
fn a_missing_birth_date_yields_no_offset() {
    assert_eq!(dates::offset_from_birth_date(""), None);
    assert_eq!(dates::offset_from_birth_date("UNKNOWN"), None);
}
