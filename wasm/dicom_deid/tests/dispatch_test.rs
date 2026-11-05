//! Format routing: which deidentifier handles a file, and what happens when a
//! format is recognized but disabled in the REDCap project settings.

mod common;

use common::*;
use dicom_deid::{deidentify_bytes, inspect_bytes};

const ALL: (bool, bool, bool) = (true, true, true);

fn run(
    bytes: &[u8],
    file_name: &str,
    record_id: &str,
    (dicom, xml, schiller): (bool, bool, bool),
) -> Result<dicom_deid::DeidentifiedFile, String> {
    deidentify_bytes(
        bytes.to_vec(),
        file_name,
        record_id,
        "MY PROJECT^REC-42",
        &patient_keys(),
        dicom,
        xml,
        schiller,
    )
}

#[test]
fn routes_each_format_to_its_deidentifier() {
    assert_eq!(
        run(&dicom_fixture(), "image.dcm", "REC-42", ALL)
            .unwrap()
            .format_name,
        "dicom"
    );
    assert_eq!(
        run(hl7_v3_fixture().as_bytes(), "ecg.xml", "REC-42", ALL)
            .unwrap()
            .format_name,
        "xml"
    );
    assert_eq!(
        run(&schiller_fixture(), "holter.dat", "REC-42", ALL)
            .unwrap()
            .format_name,
        "schiller"
    );
}

#[test]
fn reports_the_mime_type_the_file_is_stored_under() {
    assert_eq!(
        run(&dicom_fixture(), "image.dcm", "REC-42", ALL)
            .unwrap()
            .mime_type,
        "application/dicom"
    );
    assert_eq!(
        run(hl7_v3_fixture().as_bytes(), "ecg.xml", "REC-42", ALL)
            .unwrap()
            .mime_type,
        "application/xml"
    );
}

#[test]
fn skips_unsupported_files() {
    // `SKIP:` tells the browser to drop the file instead of failing the batch.
    let error = run(b"just a text file", "notes.txt", "REC-42", ALL).unwrap_err();
    assert!(error.starts_with("SKIP:"), "got {error}");

    let error = run(b"", "empty.bin", "REC-42", ALL).unwrap_err();
    assert!(error.starts_with("SKIP:"), "got {error}");
}

#[test]
fn passes_a_recognized_format_through_untouched_when_it_is_disabled() {
    // Disabling a format in the project settings is an explicit opt-out: the
    // file is still uploaded, unmodified.
    let source = hl7_v3_fixture();
    let result = run(source.as_bytes(), "ecg.xml", "REC-42", (true, false, true)).unwrap();

    assert_eq!(result.format_name, "xml");
    assert_eq!(result.bytes, source.as_bytes());

    let schiller = schiller_fixture();
    let result = run(&schiller, "holter.dat", "REC-42", (true, true, false)).unwrap();
    assert_eq!(result.bytes, schiller);

    let dicom = dicom_fixture();
    let result = run(&dicom, "image.dcm", "REC-42", (false, true, true)).unwrap();
    assert_eq!(result.bytes, dicom);
}

#[test]
fn falls_back_to_a_placeholder_when_the_record_has_no_id() {
    // Data entry forms can be open on an unsaved record; the file must still be
    // deidentified rather than carrying its original identifiers.
    let output = deidentify_bytes(
        hl7_v3_fixture().into_bytes(),
        "ecg.xml",
        "  ",
        "",
        &patient_keys(),
        true,
        true,
        true,
    )
    .unwrap();

    let text = String::from_utf8(output.bytes).unwrap();
    assert!(
        text.contains(r#"extension="UNASSIGNED_RECORD""#),
        "got {text}"
    );
    assert!(!text.contains("PHI-PATIENT-0001"));
}

#[test]
fn a_header_less_dicom_is_routed_by_its_bytes_whatever_its_name() {
    // `.vim`, no extension: the exporter's naming says nothing, the bytes do.
    for (source, name) in [
        (dicom_fixture_bare_explicit_vr(), "IMG0001.vim"),
        (dicom_fixture_bare_implicit_vr(), "IMG0002"),
        (dicom_fixture_without_magic_code(), "IMG0003.vim"),
    ] {
        let result = run(&source, name, "REC-42", ALL).unwrap();
        assert_eq!(result.format_name, "dicom", "{name}");
        assert_eq!(result.mime_type, "application/dicom", "{name}");
    }
}

#[test]
fn a_dicomdir_is_skipped_by_its_bytes_not_its_name() {
    // The DICOMDIR itself is dropped, and so it is even when DICOM
    // deidentification is off: passing it through would upload every patient
    // name on the media.
    for settings in [ALL, (false, true, true)] {
        let error = run(&dicomdir_fixture(), "DICOMDIR", "REC-42", settings).unwrap_err();
        assert!(error.starts_with("SKIP:"), "got {error}");
        assert!(error.contains("DICOMDIR"), "got {error}");

        let error = run(&dicomdir_fixture(), "index.dcm", "REC-42", settings).unwrap_err();
        assert!(error.starts_with("SKIP:"), "got {error}");
    }

    // Conversely an image that happens to be called DICOMDIR is still an image.
    let result = run(&dicom_fixture(), "DICOMDIR", "REC-42", ALL).unwrap();
    assert_eq!(result.format_name, "dicom");
}

#[test]
fn a_dicom_file_is_not_mistaken_for_an_xml_ecg() {
    let result = run(&dicom_fixture(), "weird-name.xml", "REC-42", ALL).unwrap();
    assert_eq!(result.format_name, "dicom");
}

#[test]
fn an_unsupported_xml_ecg_stops_the_upload() {
    // Not `SKIP:`: dropping the file silently would let a clinician believe the
    // ECG was uploaded, and passing it through would upload identified data.
    let error = run(philips_fixture().as_bytes(), "ecg.xml", "REC-42", ALL).unwrap_err();

    assert!(!error.starts_with("SKIP:"), "got {error}");
    assert!(error.contains("Philips"), "got {error}");
}

#[test]
fn an_unsupported_xml_is_refused_even_with_deidentification_disabled() {
    // Turning XML deidentification off is an opt-out for the formats we can
    // certify, not a licence to upload a dialect we cannot read.
    let error = run(
        philips_fixture().as_bytes(),
        "ecg.xml",
        "REC-42",
        (true, false, true),
    )
    .unwrap_err();

    assert!(!error.starts_with("SKIP:"), "got {error}");
}

// --- Inspection --------------------------------------------------------------

#[test]
fn inspection_reads_the_nip_and_what_to_show_the_uploader() {
    let inspection = inspect_bytes(&dicom_fixture(), "image.dcm", true, true, true).unwrap();

    assert_eq!(inspection.format_name, "dicom");
    assert!(inspection.deidentified);
    assert_eq!(inspection.nip, "PHI-PATIENT-0001");
    assert_eq!(inspection.raw_nip, "PHI-PATIENT-0001");
    assert_eq!(inspection.family_name, "DOE");
    assert_eq!(inspection.given_name, "JOHN");
    assert_eq!(inspection.birth_date, "12/02/1954");
}

#[test]
fn inspection_reads_the_dicom_series_for_the_summary() {
    let inspection = inspect_bytes(&dicom_fixture(), "image.dcm", true, true, true).unwrap();

    assert_eq!(inspection.series_uid, DICOM_SERIES_UID);
    assert_eq!(inspection.modality, "XA");
}

#[test]
fn inspection_keeps_the_nip_as_written_and_normalized() {
    let bytes = dicom_fixture_edited(|object| {
        object.put(dicom_core::DataElement::new(
            dicom_dictionary_std::tags::PATIENT_ID,
            dicom_core::VR::LO,
            dicom_core::PrimitiveValue::from("0001234567"),
        ));
    });
    let inspection = inspect_bytes(&bytes, "image.dcm", true, true, true).unwrap();

    assert_eq!(inspection.raw_nip, "0001234567");
    assert_eq!(inspection.nip, "1234567");
}

#[test]
fn an_ecg_has_no_series() {
    let inspection =
        inspect_bytes(hl7_v3_fixture().as_bytes(), "ecg.xml", true, true, true).unwrap();

    assert_eq!(inspection.format_name, "xml");
    assert_eq!(inspection.series_uid, "");
    assert_eq!(inspection.family_name, "Dupont");
    assert_eq!(inspection.given_name, "Marie");
}

#[test]
fn inspection_normalizes_the_nip_the_way_the_pseudonym_does() {
    let inspection = inspect_bytes(&schiller_fixture(), "holter.bin", true, true, true).unwrap();

    assert_eq!(inspection.format_name, "schiller");
    assert_eq!(
        Some(inspection.nip),
        dicom_deid::pseudonym::normalize_nip(SCHILLER_PATIENT_ID)
    );
}

#[test]
fn inspection_says_when_a_format_will_not_be_deidentified() {
    // A file that cannot be deidentified is an opt-out's business too.
    let capture = dicom_fixture_edited(|object| {
        object.put(dicom_core::DataElement::new(
            dicom_dictionary_std::tags::SOP_CLASS_UID,
            dicom_core::VR::UI,
            dicom_core::PrimitiveValue::from("1.2.840.10008.5.1.4.1.1.7"),
        ));
    });
    assert_eq!(
        inspect_bytes(&capture, "capture.dcm", true, true, true).unwrap_err(),
        dicom_deid::dicom::SECONDARY_CAPTURE_REASON
    );
    assert!(inspect_bytes(&capture, "capture.dcm", false, true, true).is_ok());

    let inspection = inspect_bytes(&dicom_fixture(), "image.dcm", false, true, true).unwrap();

    assert!(
        !inspection.deidentified,
        "an opted-out format needs no pseudonym"
    );
}

#[test]
fn inspection_sets_aside_what_deidentification_would() {
    assert!(
        inspect_bytes(b"not a medical file", "notes.txt", true, true, true)
            .unwrap_err()
            .starts_with("SKIP:")
    );
    assert_eq!(
        inspect_bytes(&dicomdir_fixture(), "DICOMDIR", true, true, true).unwrap_err(),
        dicom_deid::dicom::DICOMDIR_SKIP_REASON
    );
}

#[test]
fn a_file_without_patient_id_has_an_empty_nip() {
    let bytes = dicom_fixture_edited(|object| {
        object.remove_element(dicom_dictionary_std::tags::PATIENT_ID);
    });

    assert_eq!(
        inspect_bytes(&bytes, "image.dcm", true, true, true)
            .unwrap()
            .nip,
        ""
    );
}
