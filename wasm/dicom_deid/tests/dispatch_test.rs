//! Format routing: which deidentifier handles a file, and what happens when a
//! format is recognized but disabled in the REDCap project settings.

mod common;

use common::*;
use dicom_deid::deidentify_bytes;

const ALL: (bool, bool, bool) = (true, true, true);

fn run(
    bytes: &[u8],
    file_name: &str,
    record_id: &str,
    (dicom, xml, schiller): (bool, bool, bool),
) -> Result<dicom_deid::DeidentifiedFile, String> {
    deidentify_bytes(
        bytes,
        file_name,
        record_id,
        "MY PROJECT^REC-42",
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
        hl7_v3_fixture().as_bytes(),
        "ecg.xml",
        "  ",
        "",
        true,
        true,
        true,
    )
    .unwrap();

    let text = String::from_utf8(output.bytes).unwrap();
    assert!(text.contains("<patientId>UNASSIGNED_RECORD</patientId>"));
    assert!(!text.contains("PHI-PATIENT-0001"));
}

#[test]
fn a_dicom_file_is_not_mistaken_for_an_xml_ecg() {
    let result = run(&dicom_fixture(), "weird-name.xml", "REC-42", ALL).unwrap();
    assert_eq!(result.format_name, "dicom");
}
