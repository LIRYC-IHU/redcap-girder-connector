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
fn accepts_a_file_meta_group_without_the_magic_code() {
    assert!(dicom::validate(&dicom_fixture_without_magic_code()).is_ok());
}

#[test]
fn accepts_a_bare_data_set_in_explicit_vr() {
    // Some exporters write the data set alone, with no Part 10 header at all,
    // typically under a `.vim` extension or none. The bytes still say DICOM.
    assert!(dicom::validate(&dicom_fixture_bare_explicit_vr()).is_ok());
}

#[test]
fn accepts_a_bare_data_set_in_implicit_vr() {
    assert!(dicom::validate(&dicom_fixture_bare_implicit_vr()).is_ok());
}

#[test]
fn deidentifies_a_bare_data_set_into_a_part10_file() {
    for (label, source) in [
        ("explicit VR", dicom_fixture_bare_explicit_vr()),
        ("implicit VR", dicom_fixture_bare_implicit_vr()),
        ("meta without DICM", dicom_fixture_without_magic_code()),
    ] {
        let output = dicom::deidentify(&source, "REC-42", "MY PROJECT^REC-42")
            .unwrap_or_else(|e| panic!("{label}: {e}"));

        // The output gains the header the input lacked, so any viewer opens it.
        assert_eq!(&output[128..132], b"DICM", "{label}");
        assert!(dicom::validate(&output).is_ok(), "{label}");
        assert_eq!(
            read_dicom_string(&output, tags::PATIENT_ID).as_deref(),
            Some("REC-42"),
            "{label}"
        );
        assert_eq!(
            read_dicom_string(&output, tags::MODALITY).as_deref(),
            Some("XA"),
            "{label}"
        );
        assert!(!contains(&output, DICOM_PATIENT_NAME), "{label}");
        assert!(!contains(&output, DICOM_PATIENT_ID), "{label}");
        assert!(!contains(&output, DICOM_INSTITUTION), "{label}");
        assert!(!contains(&output, DICOM_PRIVATE_VALUE), "{label}");
    }
}

#[test]
fn tells_a_dicomdir_apart_from_a_data_object() {
    assert_eq!(
        dicom::validate(&dicom_fixture()),
        Ok(dicom::DicomKind::Object)
    );
    assert_eq!(
        dicom::validate(&dicomdir_fixture()),
        Ok(dicom::DicomKind::Directory)
    );
}

#[test]
fn a_dicomdir_is_recognized_from_the_data_set_too() {
    // Exporters sometimes leave the meta group announcing an image class; the
    // data set is then the one telling the truth.
    assert_eq!(
        dicom::validate(&dicomdir_fixture_with_misleading_meta()),
        Ok(dicom::DicomKind::Directory)
    );
}

#[test]
fn refuses_to_deidentify_a_dicomdir() {
    // The index names every patient of the media and points at files and UIDs
    // that the upload renames and rehashes: nothing useful survives, so it is
    // skipped rather than uploaded.
    let error = dicom::deidentify(&dicomdir_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap_err();
    assert!(error.starts_with("SKIP:"), "got {error}");
    assert!(error.contains("DICOMDIR"), "got {error}");
}

#[test]
fn rejects_non_dicom_input() {
    assert!(dicom::validate(b"").is_err());
    assert!(dicom::validate(b"not a dicom file at all").is_err());
    assert!(dicom::validate(&vec![0u8; 4096]).is_err());
    // Looks like the start of a data set but is not one: (0008,0005) with an
    // explicit VR, followed by garbage; then the implicit flavour with a value
    // length pointing past the end of the file.
    assert!(dicom::validate(b"\x08\x00\x05\x00CS\xff\xffgarbage garbage").is_err());
    assert!(dicom::validate(b"\x08\x00\x05\x00\xff\xff\xff\x7fgarbage").is_err());
    // A different first group is not sniffed at all.
    assert!(dicom::validate(b"\x10\x00\x10\x00PN\x04\x00DOE^").is_err());
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
fn pins_the_birth_date_to_the_epoch() {
    let output = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();

    assert_eq!(
        read_dicom_string(&output, tags::PATIENT_BIRTH_DATE).as_deref(),
        Some("19700101")
    );
}

#[test]
fn shifts_the_study_so_the_age_is_preserved() {
    // Same rule as the XML deidentifier: birth lands on 1970-01-01 and every
    // other date moves with it, so age at acquisition survives and the real
    // calendar date does not.
    let output = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();
    let study = read_dicom_string(&output, tags::STUDY_DATE).unwrap();

    assert_eq!(
        days_between("19700101", &study),
        days_between(DICOM_BIRTH_DATE, DICOM_STUDY_DATE),
        "age at acquisition changed (study shifted to {study})"
    );
    assert_ne!(study, DICOM_STUDY_DATE, "the real study date leaked");
}

#[test]
fn moves_every_date_by_the_same_offset() {
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

    assert_eq!(
        days_between(
            &read_dicom_string(&january, tags::STUDY_DATE).unwrap(),
            &read_dicom_string(&february, tags::STUDY_DATE).unwrap()
        ),
        31,
        "the interval between two studies of one patient must survive"
    );
}

#[test]
fn the_shift_differs_between_patients() {
    // The offset comes from each patient's own birth date, so two patients
    // imaged the same day do not land on the same anonymized date.
    let older =
        dicom::deidentify(&dicom_fixture_with_birth_date("19400301"), "REC-1", "P^1").unwrap();
    let younger =
        dicom::deidentify(&dicom_fixture_with_birth_date("19800301"), "REC-2", "P^2").unwrap();

    assert_ne!(
        read_dicom_string(&older, tags::STUDY_DATE),
        read_dicom_string(&younger, tags::STUDY_DATE)
    );
}

#[test]
fn falls_back_to_the_library_default_without_a_birth_date() {
    // No birth date means no age to preserve; the study date must still not be
    // the real one.
    let output = dicom::deidentify(
        &dicom_fixture_without_birth_date(),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    assert_eq!(read_dicom_string(&output, tags::PATIENT_BIRTH_DATE), None);
    assert_ne!(
        read_dicom_string(&output, tags::STUDY_DATE).as_deref(),
        Some(DICOM_STUDY_DATE),
        "the real study date leaked"
    );
}

#[test]
fn series_and_acquisition_dates_move_with_the_study() {
    // The anonymizer removes these by default; the date policy restores them
    // from the source, shifted, so the acquisition timeline stays coherent.
    let output = dicom::deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();
    let study = read_dicom_string(&output, tags::STUDY_DATE).unwrap();

    assert_eq!(
        read_dicom_string(&output, tags::SERIES_DATE).as_deref(),
        Some(study.as_str())
    );
    assert_eq!(
        read_dicom_string(&output, tags::ACQUISITION_DATE).as_deref(),
        Some(study.as_str())
    );
}
