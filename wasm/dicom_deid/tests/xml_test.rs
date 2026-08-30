mod common;

use common::*;
use dicom_deid::xml::{self, XmlType};

fn deidentify(source: &str, record_id: &str) -> String {
    let output = xml::deidentify(source.as_bytes(), "ecg.xml", record_id).expect("deidentified");
    String::from_utf8(output).expect("output is utf-8")
}

#[test]
fn recognizes_supported_dialects() {
    assert_eq!(
        xml::detect_type(hl7_v3_fixture().as_bytes(), "ecg.xml"),
        Ok(XmlType::Hl7V3)
    );
    assert_eq!(
        xml::detect_type(philips_fixture().as_bytes(), "ecg.XML"),
        Ok(XmlType::PhilipsEcg)
    );
}

#[test]
fn rejects_files_that_are_not_xml_ecgs() {
    // Extension gate: a DICOM must not be routed through the XML branch.
    assert!(xml::validate(hl7_v3_fixture().as_bytes(), "ecg.dcm").is_err());
    // Well-formed XML in an unknown dialect.
    assert!(xml::validate(
        br#"<?xml version="1.0"?><report><x>1</x></report>"#,
        "r.xml"
    )
    .is_err());
    // Right root name, wrong namespace.
    assert!(xml::validate(
        br#"<AnnotatedECG xmlns="urn:example:other"><id/></AnnotatedECG>"#,
        "r.xml"
    )
    .is_err());
    assert!(xml::validate(b"not xml at all", "r.xml").is_err());
}

#[test]
fn replaces_patient_ids_with_the_record_id() {
    let output = deidentify(&hl7_v3_fixture(), "REC-42");

    assert!(output.contains("<patientId>REC-42</patientId>"));
    assert!(output.contains("<secondPatientId>REC-42</secondPatientId>"));
    assert!(!output.contains("PHI-PATIENT-0001"));
    assert!(!output.contains("SSN-123-456-789"));
}

#[test]
fn blanks_names_and_demographics() {
    let output = deidentify(&hl7_v3_fixture(), "REC-42");

    for phi in [
        "Marie",
        "Dupont",
        "Anne",
        "67",
        "412",
        "CARDIOLOGY WARD 3",
        "NURSE^ALICE",
        "PROF^BERNARD",
    ] {
        assert!(!output.contains(phi), "{phi:?} survived deidentification");
    }

    // The elements themselves stay in place so the document stays valid.
    assert!(output.contains("<lastName></lastName>"));
    assert!(output.contains("<technician></technician>"));
}

#[test]
fn keeps_acquisition_data() {
    let output = deidentify(&hl7_v3_fixture(), "REC-42");

    assert!(output.contains(r#"<effectiveTime value="20240115093000"/>"#));
    assert!(output.contains("MDC_ECG_LEAD_I"));
    assert!(output.contains(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
}

#[test]
fn keeps_repeated_elements_distinct() {
    // Signal data repeats the same element path once per lead. Keying values by
    // path would give every occurrence the value of the last one, silently
    // corrupting the recording.
    let output = deidentify(&hl7_v3_fixture(), "REC-42");

    assert!(output.contains("111 112 113"), "first lead was overwritten");
    assert!(
        output.contains("221 222 223"),
        "second lead was overwritten"
    );

    let philips = deidentify(&philips_fixture(), "REC-42");
    assert!(philips.contains("<amplitude>101</amplitude>"));
    assert!(philips.contains("<amplitude>202</amplitude>"));
}

#[test]
fn anonymizes_philips_records() {
    let output = deidentify(&philips_fixture(), "REC-42");

    assert!(output.contains("<patientid>REC-42</patientid>"));
    for phi in ["PHI-PATIENT-0001", "Dupont", "Marie", "TECH^CLAIRE", "67"] {
        assert!(!output.contains(phi), "{phi:?} survived deidentification");
    }
}

#[test]
fn preserves_presence_flags() {
    // `*ExistFlag` attributes describe structure, not identity: blanking them
    // makes downstream Philips readers reject the file.
    let output = deidentify(&philips_fixture(), "REC-42");

    assert!(output.contains(r#"nameExistFlag="true""#));
    assert!(output.contains(r#"version="1.03""#));
}

#[test]
fn preserves_a_utf8_byte_order_mark() {
    let mut source = Vec::from(b"\xef\xbb\xbf".as_slice());
    source.extend_from_slice(hl7_v3_fixture().as_bytes());

    let output = xml::deidentify(&source, "ecg.xml", "REC-42").expect("deidentified");

    assert!(output.starts_with(b"\xef\xbb\xbf"));
    assert!(xml::validate(&output, "ecg.xml").is_ok());
}

#[test]
fn output_is_still_a_recognized_ecg() {
    let output = deidentify(&hl7_v3_fixture(), "REC-42");
    assert!(xml::validate(output.as_bytes(), "ecg.xml").is_ok());
}

#[test]
fn escapes_special_characters_in_the_record_id() {
    let output = deidentify(&hl7_v3_fixture(), "REC&<42>");

    assert!(output.contains("<patientId>REC&amp;&lt;42&gt;</patientId>"));
    assert!(xml::validate(output.as_bytes(), "ecg.xml").is_ok());
}

#[test]
fn blanks_clinical_trial_identifiers() {
    // The acquisition device stamps the trial it was configured for; that is
    // site information the upload must not carry over.
    let output = deidentify(&philips_fixture(), "REC-42");

    assert!(!output.contains("PROTO-2024-007"));
    assert!(!output.contains("HAUT LEVEQUE ABLATION TRIAL"));
    assert!(output.contains("<clinicaltrialprotocolid></clinicaltrialprotocolid>"));
}
