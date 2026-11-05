mod common;

use common::*;
use dicom_core::value::DataSetSequence;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_deid::dicom;
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;

/// Deidentify with the fixed test keys; see [`patient_keys`].
fn deidentify(input: &[u8], record_id: &str, patient_name: &str) -> Result<Vec<u8>, String> {
    dicom::deidentify(input, record_id, patient_name, &patient_keys())
}

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
        let output = deidentify(&source, "REC-42", "MY PROJECT^REC-42")
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
    let error = deidentify(&dicomdir_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap_err();
    assert!(error.starts_with("SKIP:"), "got {error}");
    assert!(error.contains("DICOMDIR"), "got {error}");
}

fn with_document(object: &mut InMemDicomObject) {
    object.put(DataElement::new(
        tags::ENCAPSULATED_DOCUMENT,
        VR::OB,
        PrimitiveValue::from(b"%PDF-1.4 DOE JOHN".to_vec()),
    ));
}

#[test]
fn refuses_a_file_wrapping_a_document() {
    // A report names the patient inside the document, out of reach of any rule.
    let error = deidentify(
        &dicom_fixture_edited(with_document),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap_err();

    assert_eq!(error, dicom::ENCAPSULATED_DOCUMENT_REASON);
    assert!(!error.starts_with("SKIP:"), "the uploader must see why");
}

#[test]
fn refuses_a_document_wrapped_inside_a_sequence() {
    let input = dicom_fixture_edited(|object| {
        let mut item = InMemDicomObject::new_empty();
        with_document(&mut item);
        object.put(DataElement::new(
            tags::REFERENCED_SERIES_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![item]),
        ));
    });

    assert_eq!(
        deidentify(&input, "REC-42", "MY PROJECT^REC-42").unwrap_err(),
        dicom::ENCAPSULATED_DOCUMENT_REASON
    );
}

#[test]
fn free_text_without_a_rule_is_emptied_wherever_it_sits() {
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            put(object, tags::CONTENT_CREATOR_NAME, VR::PN, "ZZCREATOR^JANE");
            put(object, tags::REASON_FOR_VISIT, VR::UT, "ZZREASON Mme Doe");
            put(
                object,
                tags::PERSON_TELECOM_INFORMATION,
                VR::LT,
                "ZZPHONE 0556",
            );
            let mut item = InMemDicomObject::new_empty();
            put(&mut item, tags::TEXT_VALUE, VR::UT, "ZZNESTED Mme Doe");
            object.put(DataElement::new(
                tags::CONTENT_SEQUENCE,
                VR::SQ,
                DataSetSequence::from(vec![item]),
            ));
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    for marker in ["ZZCREATOR", "ZZREASON", "ZZPHONE", "ZZNESTED"] {
        assert!(!contains(&output, marker), "{marker} survived");
    }
}

#[test]
fn what_the_data_needs_stays() {
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            put(object, tags::SERIES_DESCRIPTION, VR::LO, "Coronarography");
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    assert_eq!(
        read_dicom_string(&output, tags::SERIES_DESCRIPTION).as_deref(),
        Some("Coronarography")
    );
    assert_eq!(
        read_dicom_string(&output, tags::PATIENT_NAME).as_deref(),
        Some("MY PROJECT^REC-42"),
        "the name the module writes is not free text"
    );
}

#[test]
fn the_file_meta_group_does_not_tell_who_sent_the_file() {
    let source = dicom_fixture_with_identifying_meta();
    for marker in ["ZZSOURCE", "ZZSENDER", "ZZRECEIVER", "ZZPRIVATE"] {
        assert!(
            contains(&source, marker),
            "{marker} missing from the fixture"
        );
    }

    let output = deidentify(&source, "REC-42", "MY PROJECT^REC-42").unwrap();

    for marker in ["ZZSOURCE", "ZZSENDER", "ZZRECEIVER", "ZZPRIVATE"] {
        assert!(!contains(&output, marker), "{marker} survived");
    }
    assert!(dicom::validate(&output).is_ok());
}

#[test]
fn a_date_range_is_removed_rather_than_half_shifted() {
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            put(
                object,
                tags::DATE_OF_LAST_CALIBRATION,
                VR::DA,
                "20190101-20190201",
            );
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    assert_eq!(
        read_dicom_string(&output, tags::DATE_OF_LAST_CALIBRATION),
        None
    );
    assert!(
        !contains(&output, "20190201"),
        "the end of the range leaked"
    );
}

#[test]
fn a_derived_image_is_left_out() {
    let error = deidentify(
        &dicom_fixture_edited(|object| {
            put(object, tags::IMAGE_TYPE, VR::CS, "DERIVED\\SECONDARY\\MPR");
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap_err();

    assert_eq!(error, dicom::DERIVED_IMAGE_SKIP_REASON);
    assert!(error.starts_with("SKIP:"), "out of scope, not an error");
}

#[test]
fn an_acquisition_goes_through_whether_processed_or_not() {
    // Fluoroscopy reads DERIVED\PRIMARY: processed pixels, but the examination itself.
    for image_type in [
        "ORIGINAL\\PRIMARY\\SINGLE PLANE",
        "DERIVED\\PRIMARY\\SINGLE PLANE",
        "ORIGINAL\\SECONDARY",
        "DERIVED",
    ] {
        let input = dicom_fixture_edited(|object| {
            put(object, tags::IMAGE_TYPE, VR::CS, image_type);
        });

        assert!(
            deidentify(&input, "REC-42", "MY PROJECT^REC-42").is_ok(),
            "{image_type}"
        );
    }
}

#[test]
fn an_ordinary_image_is_not_refused() {
    assert_eq!(dicom::identity(&dicom_fixture()).unwrap().refusal, None);
}

fn with_sop_class(uid: &'static str) -> impl FnOnce(&mut InMemDicomObject) {
    move |object| put(object, tags::SOP_CLASS_UID, VR::UI, uid)
}

#[test]
fn refuses_every_secondary_capture_class() {
    for uid in [
        "1.2.840.10008.5.1.4.1.1.7",
        "1.2.840.10008.5.1.4.1.1.7.1",
        "1.2.840.10008.5.1.4.1.1.7.2",
        "1.2.840.10008.5.1.4.1.1.7.3",
        "1.2.840.10008.5.1.4.1.1.7.4",
    ] {
        assert_eq!(
            deidentify(
                &dicom_fixture_edited(with_sop_class(uid)),
                "REC-42",
                "MY PROJECT^REC-42"
            )
            .unwrap_err(),
            dicom::SECONDARY_CAPTURE_REASON,
            "{uid}"
        );
    }
}

#[test]
fn refuses_a_secondary_capture_announced_by_its_file_meta_alone() {
    // The data set says otherwise: either one is enough.
    let input = dicom_fixture_with_meta_sop_class("1.2.840.10008.5.1.4.1.1.7");

    assert_eq!(
        dicom::identity(&input).unwrap().refusal,
        Some(dicom::SECONDARY_CAPTURE_REASON)
    );
}

#[test]
fn a_class_merely_starting_with_the_same_digits_is_not_a_secondary_capture() {
    let endoscopy = dicom_fixture_edited(with_sop_class("1.2.840.10008.5.1.4.1.1.77.1.1"));

    assert_eq!(dicom::identity(&endoscopy).unwrap().refusal, None);
    assert!(deidentify(&endoscopy, "REC-42", "MY PROJECT^REC-42").is_ok());
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
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42")
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
        read_dicom_string(&output, tags::DEIDENTIFICATION_METHOD),
        Some(dicom::deidentification_method())
    );
}

#[test]
fn the_deidentification_method_names_the_release() {
    // A file must say which release deidentified it: the rules, the date
    // policy among them, change between releases.
    let version = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../VERSION"))
        .expect("the repository VERSION file");
    let method = dicom::deidentification_method();

    assert_eq!(
        method,
        format!("IHU LIRYC REDCAP PLUGIN v{}", version.trim())
    );
    // `DeidentificationMethod` is a LO: 64 characters at most.
    assert!(method.len() <= 64, "{method} does not fit a LO");
}

#[test]
fn the_modified_dates_are_declared() {
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();

    assert_eq!(
        read_dicom_string(&output, tags::LONGITUDINAL_TEMPORAL_INFORMATION_MODIFIED).as_deref(),
        Some("MODIFIED")
    );
}

#[test]
fn removes_every_identifying_value_from_the_output_bytes() {
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42")
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
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42")
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
    let first = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();
    let second = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();

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
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();

    // The anonymized bytes are uploaded as-is, so they must still parse.
    assert!(dicom::validate(&output).is_ok());
    assert_eq!(
        read_dicom_string(&output, tags::MODALITY).as_deref(),
        Some("XA"),
        "non-identifying acquisition data must survive"
    );
}

// --- Dates -------------------------------------------------------------------

#[test]
fn the_birth_date_is_present_and_empty() {
    // Type 2 in the Patient module; shifted like the other dates, it would give
    // the exact age back by subtraction.
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();

    assert_eq!(
        read_dicom_string(&output, tags::PATIENT_BIRTH_DATE).as_deref(),
        Some("")
    );
}

#[test]
fn the_study_moves_by_exactly_the_offset() {
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();
    let study = read_dicom_string(&output, tags::STUDY_DATE).unwrap();

    assert_eq!(days_between(DICOM_STUDY_DATE, &study), DATE_OFFSET);
    assert!(
        study.starts_with("19"),
        "{study} should land in the 20th century"
    );
}

#[test]
fn moves_every_date_by_the_same_offset() {
    let january = deidentify(
        &dicom_fixture_with_study_date("20240115"),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();
    let february = deidentify(
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
fn series_and_acquisition_dates_move_with_the_study() {
    // The anonymizer removes these by default; the date policy keeps them,
    // shifted, so the acquisition timeline stays coherent.
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();
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

// --- Dates the anonymizer would let through --------------------------------

fn put(object: &mut InMemDicomObject, tag: Tag, vr: VR, value: &str) {
    object.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
}

/// A sequence the anonymizer keeps and descends into, holding one date.
fn put_nested_date(object: &mut InMemDicomObject, value: &str) {
    let mut item = InMemDicomObject::new_empty();
    put(&mut item, tags::CONTENT_DATE, VR::DA, value);
    object.put(DataElement::new(
        tags::REFERENCED_SERIES_SEQUENCE,
        VR::SQ,
        DataSetSequence::from(vec![item]),
    ));
}

fn read_nested_date(bytes: &[u8]) -> Option<String> {
    let object = dicom_object::from_reader(std::io::Cursor::new(&bytes[128..])).ok()?;
    let sequence = object.element(tags::REFERENCED_SERIES_SEQUENCE).ok()?;
    let item = sequence.items()?.first()?;
    Some(
        item.element(tags::CONTENT_DATE)
            .ok()?
            .to_str()
            .ok()?
            .trim()
            .to_string(),
    )
}

/// Dates the anonymizer handles three different ways on its own: hash-shifted
/// (secondary capture, procedure step end), not at all (start of acquisition),
/// and inside a sequence.
fn with_scattered_dates(object: &mut InMemDicomObject) {
    put(
        object,
        tags::DATE_OF_SECONDARY_CAPTURE,
        VR::DA,
        DICOM_STUDY_DATE,
    );
    put(
        object,
        tags::PERFORMED_PROCEDURE_STEP_START_DATE,
        VR::DA,
        DICOM_STUDY_DATE,
    );
    put(
        object,
        tags::PERFORMED_PROCEDURE_STEP_END_DATE,
        VR::DA,
        DICOM_STUDY_DATE,
    );
    put(
        object,
        tags::START_ACQUISITION_DATE_TIME,
        VR::DT,
        &format!("{DICOM_STUDY_DATE}143512"),
    );
    put_nested_date(object, DICOM_STUDY_DATE);
}

#[test]
fn every_date_moves_not_only_the_well_known_ones() {
    // Only a handful of tags used to be shifted; the rest kept the anonymizer's
    // own shift, or none, so one file mixed two calendars and the procedure
    // step ended 18 years before it began.
    let output = deidentify(
        &dicom_fixture_edited(with_scattered_dates),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();
    let study = read_dicom_string(&output, tags::STUDY_DATE).unwrap();

    for tag in [
        tags::DATE_OF_SECONDARY_CAPTURE,
        tags::PERFORMED_PROCEDURE_STEP_START_DATE,
        tags::PERFORMED_PROCEDURE_STEP_END_DATE,
    ] {
        assert_eq!(
            read_dicom_string(&output, tag).as_deref(),
            Some(study.as_str()),
            "{tag}"
        );
    }
    assert_eq!(
        read_dicom_string(&output, tags::START_ACQUISITION_DATE_TIME),
        Some(format!("{study}143512"))
    );
    assert_eq!(read_nested_date(&output), Some(study));
    assert!(!contains(&output, DICOM_STUDY_DATE), "a real date leaked");
}

#[test]
fn every_value_of_a_multi_valued_date_is_shifted() {
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            put(
                object,
                tags::DATE_OF_LAST_CALIBRATION,
                VR::DA,
                "20240115\\20230601",
            );
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();
    let calibrations = read_dicom_string(&output, tags::DATE_OF_LAST_CALIBRATION).unwrap();
    let calibrations: Vec<&str> = calibrations.split('\\').collect();

    assert_eq!(calibrations.len(), 2);
    assert_eq!(days_between("20230601", calibrations[1]), DATE_OFFSET);
    assert!(!contains(&output, "20230601"), "the second date leaked");
}

#[test]
fn dates_move_whatever_the_patient_id_and_birth_date() {
    // The offset comes from the pseudonym, not from anything left in the file:
    // a missing PatientID or birth date changes nothing about the dates.
    for (keep_patient_id, keep_birth_date) in [(true, false), (false, true), (false, false)] {
        let output = deidentify(
            &dicom_fixture_edited(|object| {
                if !keep_patient_id {
                    object.remove_element(tags::PATIENT_ID);
                }
                if !keep_birth_date {
                    object.remove_element(tags::PATIENT_BIRTH_DATE);
                }
                with_scattered_dates(object);
            }),
            "REC-42",
            "MY PROJECT^REC-42",
        )
        .unwrap();
        let case = format!("PatientID kept: {keep_patient_id}, birth date kept: {keep_birth_date}");
        let study = read_dicom_string(&output, tags::STUDY_DATE).unwrap();

        assert_eq!(
            days_between(DICOM_STUDY_DATE, &study),
            DATE_OFFSET,
            "{case}"
        );
        assert_eq!(read_nested_date(&output), Some(study), "{case}");
        assert!(
            !contains(&output, DICOM_STUDY_DATE),
            "a real date leaked ({case})"
        );
        assert!(
            !contains(&output, DICOM_BIRTH_DATE),
            "the birth date leaked ({case})"
        );
    }
}

#[test]
fn a_date_not_encoded_as_a_date_is_removed() {
    // A study date written as UN cannot be shifted; it must not reach the
    // anonymizer's hash shift either.
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            object.put(DataElement::new(
                tags::STUDY_DATE,
                VR::UN,
                PrimitiveValue::from(DICOM_STUDY_DATE.as_bytes().to_vec()),
            ));
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    assert_eq!(read_dicom_string(&output, tags::STUDY_DATE), None);
    assert!(!contains(&output, DICOM_STUDY_DATE), "a real date leaked");
}

#[test]
fn a_partial_birth_date_is_blanked_rather_than_kept_or_fatal() {
    // `1954` is not a date. The anonymizer used to keep it verbatim, or reject
    // the whole file.
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            put(object, tags::PATIENT_BIRTH_DATE, VR::DA, "1954");
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    assert_eq!(
        read_dicom_string(&output, tags::PATIENT_BIRTH_DATE).as_deref(),
        Some("")
    );
    assert_eq!(read_dicom_string(&output, tags::PATIENT_AGE), None);
}

// --- Age ---------------------------------------------------------------------

fn age_for(birth: &str, study: &str) -> Option<String> {
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            put(object, tags::PATIENT_BIRTH_DATE, VR::DA, birth);
            put(object, tags::STUDY_DATE, VR::DA, study);
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();
    read_dicom_string(&output, tags::PATIENT_AGE)
}

#[test]
fn the_age_is_written_in_years_on_the_real_dates() {
    // Born 1954-02-12, imaged 2024-01-15: the birthday is not reached yet.
    assert_eq!(
        age_for(DICOM_BIRTH_DATE, DICOM_STUDY_DATE).as_deref(),
        Some("069Y")
    );
}

#[test]
fn young_children_get_months_and_newborns_weeks() {
    assert_eq!(age_for("20220201", "20240115").as_deref(), Some("023M"));
    assert_eq!(age_for("20231001", "20240115").as_deref(), Some("003M"));
    assert_eq!(age_for("20240101", "20240115").as_deref(), Some("002W"));
}

#[test]
fn without_a_birth_date_a_valid_age_in_the_file_is_kept() {
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            object.remove_element(tags::PATIENT_BIRTH_DATE);
            put(object, tags::PATIENT_AGE, VR::AS, "070Y");
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();
    assert_eq!(
        read_dicom_string(&output, tags::PATIENT_AGE).as_deref(),
        Some("070Y")
    );

    let garbage = deidentify(
        &dicom_fixture_edited(|object| {
            object.remove_element(tags::PATIENT_BIRTH_DATE);
            put(object, tags::PATIENT_AGE, VR::AS, "seventy");
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();
    assert_eq!(read_dicom_string(&garbage, tags::PATIENT_AGE), None);
}

// --- Identity ------------------------------------------------------------------

#[test]
fn the_identity_is_read_before_deidentification() {
    let identity = dicom::identity(&dicom_fixture()).unwrap();

    assert_eq!(identity.nip.as_deref(), Some(DICOM_PATIENT_ID));
    assert_eq!(identity.name.as_deref(), Some(DICOM_PATIENT_NAME));
    assert_eq!(identity.birth_date.as_deref(), Some(DICOM_BIRTH_DATE));
}

#[test]
fn a_file_without_patient_id_has_no_nip() {
    let identity = dicom::identity(&dicom_fixture_edited(|object| {
        object.remove_element(tags::PATIENT_ID);
    }))
    .unwrap();

    assert_eq!(identity.nip, None);
}

// --- UIDs and hashed identifiers -------------------------------------------------

fn uid_with_key(uid_key: [u8; 32]) -> String {
    let keys = dicom_deid::PatientKeys {
        uid_key,
        ..patient_keys()
    };
    let output = dicom::deidentify(&dicom_fixture(), "REC-42", "P^42", &keys).unwrap();
    read_dicom_string(&output, tags::STUDY_INSTANCE_UID).unwrap()
}

#[test]
fn uids_depend_on_the_key() {
    // The anonymizer's unkeyed hash let anyone holding the original UID compute
    // the rewritten one, and so link the file back to the PACS.
    assert_eq!(uid_with_key([1; 32]), uid_with_key([1; 32]));
    assert_ne!(uid_with_key([1; 32]), uid_with_key([2; 32]));
}

#[test]
fn references_keep_pointing_at_their_targets() {
    // Inside a sequence, a reference to this very instance must follow it.
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            let mut item = InMemDicomObject::new_empty();
            put(
                &mut item,
                tags::REFERENCED_SOP_INSTANCE_UID,
                VR::UI,
                DICOM_SOP_UID,
            );
            put(
                &mut item,
                tags::REFERENCED_SOP_CLASS_UID,
                VR::UI,
                "1.2.840.10008.5.1.4.1.1.7",
            );
            object.put(DataElement::new(
                tags::REFERENCED_SERIES_SEQUENCE,
                VR::SQ,
                DataSetSequence::from(vec![item]),
            ));
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    let object = dicom_object::from_reader(std::io::Cursor::new(&output[128..])).unwrap();
    let sequence = object.element(tags::REFERENCED_SERIES_SEQUENCE).unwrap();
    let item = &sequence.items().unwrap()[0];
    let read = |tag| {
        item.element(tag)
            .unwrap()
            .to_str()
            .unwrap()
            .trim()
            .to_string()
    };

    assert_eq!(
        Some(read(tags::REFERENCED_SOP_INSTANCE_UID)),
        read_dicom_string(&output, tags::SOP_INSTANCE_UID)
    );
    assert_eq!(
        read(tags::REFERENCED_SOP_CLASS_UID),
        "1.2.840.10008.5.1.4.1.1.7"
    );
    assert!(!contains(&output, DICOM_SOP_UID), "the original UID leaked");
}

#[test]
fn the_file_meta_group_follows_the_instance_uid() {
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();
    let object = dicom_object::from_reader(std::io::Cursor::new(&output[128..])).unwrap();

    assert_eq!(
        Some(
            object
                .meta()
                .media_storage_sop_instance_uid()
                .trim_end_matches('\0')
                .to_string()
        ),
        read_dicom_string(&output, tags::SOP_INSTANCE_UID)
    );
}

#[test]
fn class_uids_are_left_alone() {
    let output = deidentify(&dicom_fixture(), "REC-42", "MY PROJECT^REC-42").unwrap();

    assert_eq!(
        read_dicom_string(&output, tags::SOP_CLASS_UID).as_deref(),
        Some("1.2.840.10008.5.1.4.1.1.12.1")
    );
}

#[test]
fn a_uid_the_anonymizer_has_no_rule_for_is_rewritten() {
    const VENDOR_UID: &str = "1.3.6.1.4.1.5962.99.1.1";
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            put(
                object,
                tags::REFERENCED_FRAME_OF_REFERENCE_UID,
                VR::UI,
                VENDOR_UID,
            );
            put(object, tags::CONCATENATION_UID, VR::UI, VENDOR_UID);
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    assert!(!contains(&output, VENDOR_UID), "the original UID leaked");
}

#[test]
fn identifiers_the_anonymizer_would_hash_are_emptied() {
    // An unkeyed hash of an accession number can be computed by anyone who has
    // the number. These attributes are type 2 at most: present and empty.
    let output = deidentify(
        &dicom_fixture_edited(|object| {
            put(object, tags::ACCESSION_NUMBER, VR::SH, "ACC-20240115-77");
            put(
                object,
                tags::PLACER_ORDER_NUMBER_IMAGING_SERVICE_REQUEST,
                VR::LO,
                "ORDER-55",
            );
            put(
                object,
                tags::FILLER_ORDER_NUMBER_IMAGING_SERVICE_REQUEST,
                VR::LO,
                "ORDER-56",
            );
        }),
        "REC-42",
        "MY PROJECT^REC-42",
    )
    .unwrap();

    for tag in [
        tags::ACCESSION_NUMBER,
        tags::PLACER_ORDER_NUMBER_IMAGING_SERVICE_REQUEST,
        tags::FILLER_ORDER_NUMBER_IMAGING_SERVICE_REQUEST,
    ] {
        assert_eq!(
            read_dicom_string(&output, tag).as_deref(),
            Some(""),
            "{tag}"
        );
    }
}
