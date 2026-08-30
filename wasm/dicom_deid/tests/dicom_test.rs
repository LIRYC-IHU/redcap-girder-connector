mod common;

use common::*;
use dicom_deid::dicom;
use dicom_dictionary_std::tags;

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn accepts_a_part10_file_with_preamble() {
    // Files exported by PACS start with the 128-byte preamble; rejecting them
    // would silently drop every real DICOM from the upload.
    assert!(dicom::validate(&dicom_fixture()).is_ok());
}

#[test]
fn accepts_a_stream_without_preamble() {
    assert!(dicom::validate(&dicom_fixture_without_preamble()).is_ok());
}

#[test]
fn rejects_non_dicom_input() {
    assert!(dicom::validate(b"").is_err());
    assert!(dicom::validate(b"not a dicom file at all").is_err());
    assert!(dicom::validate(&vec![0u8; 4096]).is_err());
}

#[test]
fn replaces_patient_identity_with_redcap_identifiers() {
    let output = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42")
        .expect("deidentification succeeds");

    assert_eq!(
        read_dicom_string(&output, tags::PATIENT_ID).as_deref(),
        Some("REC-42")
    );
    assert_eq!(
        read_dicom_string(&output, tags::PATIENT_NAME).as_deref(),
        Some("MY PROJECT^REC-42")
    );
    assert_eq!(
        read_dicom_string(&output, tags::DEIDENTIFICATION_METHOD).as_deref(),
        Some("IHU LIRYC REDCAP PLUGIN")
    );
}

#[test]
fn removes_every_identifying_value_from_the_output_bytes() {
    let output = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42")
        .expect("deidentification succeeds");

    for phi in [
        DICOM_PATIENT_NAME.trim_end_matches('^'),
        DICOM_PATIENT_ID,
        DICOM_INSTITUTION,
        DICOM_BIRTH_DATE,
        // Vendor private tags are dropped wholesale: their contents are
        // undocumented and routinely carry names or accession numbers.
        DICOM_PRIVATE_VALUE,
    ] {
        assert!(
            !contains(&output, phi),
            "identifying value {phi:?} is still present in the anonymized file"
        );
    }
}

#[test]
fn rewrites_instance_uids_under_the_configured_uid_root() {
    let output = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42")
        .expect("deidentification succeeds");

    for tag in [
        tags::STUDY_INSTANCE_UID,
        tags::SERIES_INSTANCE_UID,
        tags::SOP_INSTANCE_UID,
    ] {
        let uid = read_dicom_string(&output, tag).expect("uid is present");
        assert!(
            uid.starts_with("1.2.826.0.1.3680043.10.543"),
            "{uid} was not rewritten under the IHU Liryc UID root"
        );
    }

    for original in [DICOM_STUDY_UID, DICOM_SERIES_UID, DICOM_SOP_UID] {
        assert!(!contains(&output, original), "{original} leaked");
    }
}

#[test]
fn keeps_uids_stable_across_runs_so_series_stay_grouped() {
    let first = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();
    let second = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();

    assert_eq!(
        read_dicom_string(&first, tags::STUDY_INSTANCE_UID),
        read_dicom_string(&second, tags::STUDY_INSTANCE_UID),
        "instances of the same study must keep sharing a Study Instance UID"
    );
    assert_eq!(
        read_dicom_string(&first, tags::SERIES_INSTANCE_UID),
        read_dicom_string(&second, tags::SERIES_INSTANCE_UID)
    );
}

#[test]
fn output_is_a_readable_dicom_file() {
    let output = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();

    // The anonymized bytes are uploaded as-is, so they must still parse.
    assert!(dicom::validate(&output).is_ok());
    assert_eq!(
        read_dicom_string(&output, tags::MODALITY).as_deref(),
        Some("XA"),
        "non-identifying acquisition data must survive"
    );
}

#[test]
fn shifts_study_dates_by_a_patient_stable_offset() {
    // Dates are not kept as-is: they are shifted back by an offset derived from
    // the original patient id (up to 10 years). The offset is stable per
    // patient, so intervals between that patient's studies survive.
    let january = dicom::deidentify(
        &dicom_fixture_with_study_date("20240115"),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();
    let february = dicom::deidentify(
        &dicom_fixture_with_study_date("20240215"),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    let january_date = read_dicom_string(&january, tags::STUDY_DATE).unwrap();
    let february_date = read_dicom_string(&february, tags::STUDY_DATE).unwrap();

    assert_ne!(january_date, "20240115", "the study date was not shifted");
    assert_eq!(days_between(&january_date, &february_date), 31);
}

#[test]
fn drops_series_and_acquisition_dates() {
    let output = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();

    assert_eq!(read_dicom_string(&output, tags::SERIES_DATE), None);
    assert_eq!(read_dicom_string(&output, tags::ACQUISITION_DATE), None);
}

/// Days between two `YYYYMMDD` DICOM dates, via a day count from a fixed epoch.
fn days_between(from: &str, to: &str) -> i64 {
    fn to_days(date: &str) -> i64 {
        let year: i64 = date[0..4].parse().unwrap();
        let month: i64 = date[4..6].parse().unwrap();
        let day: i64 = date[6..8].parse().unwrap();

        // Howard Hinnant's days-from-civil algorithm.
        let year = if month <= 2 { year - 1 } else { year };
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146097 + day_of_era - 719468
    }

    to_days(to) - to_days(from)
}
