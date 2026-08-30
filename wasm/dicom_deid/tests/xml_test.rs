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
    for phi in ["PHI-PATIENT-0001", "Dupont", "Marie", "TECH^CLAIRE"] {
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

#[test]
fn keeps_the_sex_which_projects_analyse() {
    let hl7 = deidentify(&hl7_v3_fixture(), "REC-42");
    let philips = deidentify(&philips_fixture(), "REC-42");

    assert!(hl7.contains("<sex>F</sex>"));
    assert!(philips.contains("<sex>Female</sex>"));
}

#[test]
fn replaces_the_subject_id_and_blanks_the_race_code() {
    // The aECG schema requires trialSubject/id/@root, and the standard reserves
    // @extension for "the traditional identifier" — so the subject id is
    // replaced rather than blanked, with the record id where a reader expects it.
    let output = deidentify(
        r#"<AnnotatedECG xmlns="urn:hl7-org:v3">
              <trialSubject><id root="1.2.3.4" extension="0105883586"/></trialSubject>
              <raceCode code="2131-1"/>
              <sequence><value>1 2 3</value></sequence>
            </AnnotatedECG>"#,
        "REC-42",
    );

    assert!(!output.contains("0105883586"), "the subject id survived");
    assert!(
        !output.contains("1.2.3.4"),
        "the original uid root survived"
    );
    assert!(!output.contains("2131-1"), "the race code survived");
    assert!(output.contains(r#"extension="REC-42""#));
    assert!(
        output.contains("root="),
        "the required root attribute was removed"
    );
}

#[test]
fn identifiers_come_from_the_redcap_context() {
    // Like the DICOM side, nothing is minted at random: the root is an arc of
    // the institution's registered OID and the extension is the record, so the
    // same recording deidentifies to the same identifiers every time.
    let source = r#"<AnnotatedECG xmlns="urn:hl7-org:v3">
          <id root="755.8013991.2026623.85727" extension="annotatedEcg"/>
          <sequence><value>1 2 3</value></sequence>
        </AnnotatedECG>"#;

    let first = deidentify(source, "REC-42");
    let second = deidentify(source, "REC-42");

    // The original root encodes the acquisition date, the time and the device
    // serial, so it cannot be kept — but the schema requires a root.
    assert!(
        !first.contains("2026623.85727"),
        "the instance uid survived"
    );
    assert!(!first.contains("annotatedEcg"), "the extension survived");
    assert!(
        first.contains(r#"root="1.2.826.0.1.3680043.10.543.1""#),
        "got {first}"
    );
    assert!(first.contains(r#"extension="REC-42""#));
    assert_eq!(first, second, "deidentification must be reproducible");
}

#[test]
fn each_kind_of_entity_keeps_a_distinct_uid() {
    // The aECG, the series and the subject are different things; the standard
    // requires their UIDs to differ.
    let output = deidentify(
        r#"<AnnotatedECG xmlns="urn:hl7-org:v3">
              <id root="1.1" extension="doc"/>
              <trialSubject><id root="2.2" extension="SBJ-9"/></trialSubject>
              <clinicalTrial><id root="3.3" extension="TRIAL-7"/></clinicalTrial>
              <component><series><id root="4.4" extension="ser"/>
                <sequence><value>1 2 3</value></sequence>
              </series></component>
            </AnnotatedECG>"#,
        "REC-42",
    );

    let root = "1.2.826.0.1.3680043.10.543";
    assert!(
        output.contains(&format!(r#"root="{root}.1""#)),
        "document arc"
    );
    assert!(
        output.contains(&format!(r#"root="{root}.2""#)),
        "series arc"
    );
    assert!(
        output.contains(&format!(r#"root="{root}.3""#)),
        "subject arc"
    );
    assert!(output.contains(&format!(r#"root="{root}.4""#)), "trial arc");
    // Trial and site identifiers are site information: the extension goes.
    assert!(!output.contains("TRIAL-7"));
    assert!(!output.contains("SBJ-9"));
}

#[test]
fn rejects_a_dialect_the_allowlist_does_not_cover() {
    // An allowlist tuned to one dialect would silently empty another. That must
    // fail loudly rather than upload a gutted recording, so the error is not
    // prefixed `SKIP:` — it aborts the batch instead of dropping the file.
    let error = xml::deidentify(
        br#"<AnnotatedECG xmlns="urn:hl7-org:v3"><vendorSignal>1 2 3</vendorSignal></AnnotatedECG>"#,
        "ecg.xml",
        "REC-42",
    )
    .unwrap_err();

    assert!(!error.starts_with("SKIP:"), "got {error}");
    assert!(error.contains("no signal data"), "got {error}");
}

#[test]
fn pins_the_birth_date_to_the_epoch() {
    let hl7 = deidentify(&hl7_v3_fixture(), "REC-42");
    let philips = deidentify(&philips_fixture(), "REC-42");

    assert!(hl7.contains(r#"<birthTime value="19700101"/>"#));
    assert!(!hl7.contains("19540212"));
    // The Philips fixture writes it hyphenated; the notation is preserved.
    assert!(philips.contains("<dateofbirth>1970-01-01</dateofbirth>"));
    assert!(!philips.contains("1954-02-12"));
}

#[test]
fn shifts_the_acquisition_so_the_age_is_preserved() {
    // The fixture is born 1954-02-12 and recorded 2024-01-15. After the shift
    // the birth date is 1970-01-01, so the acquisition must sit exactly the
    // same number of days after it: the age at acquisition is unchanged.
    let output = deidentify(&hl7_v3_fixture(), "REC-42");
    let shifted = timestamp_after(&output, "effectiveTime value=\"");

    assert_eq!(
        days_between("19700101", &shifted[..8]),
        days_between("19540212", "20240115"),
        "age at acquisition changed (shifted to {shifted})"
    );
    assert!(shifted.ends_with("093000"), "the time of day must survive");
    assert!(!output.contains("20240115"), "the real date leaked");
}

#[test]
fn moves_every_date_by_the_same_offset() {
    let source = hl7_v3_fixture().replace(
        r#"<effectiveTime value="20240115093000"/>"#,
        r#"<effectiveTime value="20240115093000"/><activityTime value="20240118120000"/>"#,
    );
    let output = deidentify(&source, "REC-42");

    let first = timestamp_after(&output, "effectiveTime value=\"");
    let second = timestamp_after(&output, "activityTime value=\"");
    assert_eq!(
        days_between(&first[..8], &second[..8]),
        3,
        "the three days between the two acquisitions must survive"
    );
}

#[test]
fn blanks_dates_when_the_document_has_no_birth_date() {
    // With no birth date there is no offset to apply, and an unshifted
    // acquisition date would simply be the real one.
    let output = deidentify(
        r#"<?xml version="1.0"?>
<AnnotatedECG xmlns="urn:hl7-org:v3">
  <effectiveTime value="20240115093000"/>
  <componentOf><subject><patientId>PHI-1</patientId></subject></componentOf>
  <sequence><value>1 2 3</value></sequence>
</AnnotatedECG>"#,
        "REC-42",
    );

    assert!(
        !output.contains("20240115"),
        "the real acquisition date leaked"
    );
    assert!(output.contains("<patientId>REC-42</patientId>"));
}

#[test]
fn plain_numbers_are_not_mistaken_for_dates() {
    // An eight-digit identifier is not a date and must not be shifted.
    let source = r#"<?xml version="1.0"?>
<AnnotatedECG xmlns="urn:hl7-org:v3">
  <subjectDemographicPerson><birthTime value="19540212"/></subjectDemographicPerson>
  <component><series><sequence><value>12345678 99999999 20240115</value></sequence></series></component>
  <sequence><value>1 2 3</value></sequence>
</AnnotatedECG>"#;
    let output = deidentify(source, "REC-42");

    assert!(
        output.contains("12345678 99999999 20240115"),
        "signal samples were rewritten: {output}"
    );
}

/// The first timestamp written after `marker`.
fn timestamp_after(text: &str, marker: &str) -> String {
    let start = text.find(marker).expect("marker present") + marker.len();
    text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect()
}
