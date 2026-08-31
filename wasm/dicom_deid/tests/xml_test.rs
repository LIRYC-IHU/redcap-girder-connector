mod common;

use common::*;
use dicom_deid::xml::{self, XmlOutcome};

fn deidentify(source: &str, record_id: &str) -> String {
    let output = xml::deidentify(source.as_bytes(), "ecg.xml", record_id).expect("deidentified");
    String::from_utf8(output).expect("output is utf-8")
}

#[test]
fn recognizes_an_hl7_annotated_ecg() {
    assert_eq!(
        xml::classify(hl7_v3_fixture().as_bytes(), "ecg.xml"),
        XmlOutcome::AnnotatedEcg
    );
    assert_eq!(
        xml::classify(hl7_v3_fixture().as_bytes(), "ECG.XML"),
        XmlOutcome::AnnotatedEcg
    );
}

#[test]
fn accepts_a_namespace_prefix_on_the_root() {
    // aECG files usually declare the namespace as the default, but a prefixed
    // root is just as valid and must not be refused.
    let prefixed = r#"<hl7:AnnotatedECG xmlns:hl7="urn:hl7-org:v3"><hl7:trialSubject/>
          <hl7:sequence><hl7:value>1 2 3</hl7:value></hl7:sequence>
        </hl7:AnnotatedECG>"#;

    assert_eq!(
        xml::classify(prefixed.as_bytes(), "ecg.xml"),
        XmlOutcome::AnnotatedEcg
    );
}

#[test]
fn refuses_vendor_ecg_formats_by_name() {
    // Philips and GE MUSE are not HL7 aECG. The allowlist is written against
    // the aECG schema, so applying it to them would empty the recording — and
    // passing them through would upload identified data.
    let philips = xml::classify(philips_fixture().as_bytes(), "ecg.xml");
    let muse = xml::classify(
        br#"<RestingECG><PatientDemographics><PatientID>7</PatientID></PatientDemographics></RestingECG>"#,
        "muse.xml",
    );

    for (outcome, expected) in [(philips, "Philips"), (muse, "GE MUSE")] {
        match outcome {
            XmlOutcome::Unsupported(reason) => {
                assert!(reason.contains(expected), "got {reason}");
                assert!(!reason.starts_with("SKIP:"), "the refusal must be visible");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}

#[test]
fn refuses_an_annotated_ecg_in_the_wrong_namespace() {
    // The root name alone is not enough: the HL7 v3 namespace is what marks the
    // FDA format.
    let outcome = xml::classify(
        br#"<AnnotatedECG xmlns="urn:hl7-org:v2"><trialSubject/><id/></AnnotatedECG>"#,
        "ecg.xml",
    );
    assert!(
        matches!(outcome, XmlOutcome::Unsupported(_)),
        "got {outcome:?}"
    );
}

#[test]
fn a_file_that_is_not_xml_is_left_to_the_other_formats() {
    // A DICOM that happens to be named `.xml` must still reach the DICOM
    // deidentifier rather than being refused here.
    assert_eq!(
        xml::classify(b"not xml at all", "weird.xml"),
        XmlOutcome::NotXml
    );
    assert_eq!(
        xml::classify(hl7_v3_fixture().as_bytes(), "ecg.dcm"),
        XmlOutcome::NotXml
    );
}

#[test]
fn the_pseudonym_lives_only_in_the_standard_place() {
    // `trialSubject/id/@extension` is where the standard puts the subject's
    // identifier. A vendor `PatientID` duplicating it is removed rather than
    // filled in, so there is exactly one place to look.
    let output = deidentify(&hl7_v3_fixture(), "REC-42");

    assert!(output.contains(r#"extension="REC-42""#));
    assert!(!output.contains("<patientId>REC-42</patientId>"));
    assert!(!output.contains("PHI-PATIENT-0001"));
    assert!(!output.contains("SSN-123-456-789"));
}

#[test]
fn a_missing_subject_id_is_created() {
    // The schema requires trialSubject/id, and without it the upload would
    // carry nothing tying it back to the REDCap record.
    let output = deidentify(
        r#"<AnnotatedECG xmlns="urn:hl7-org:v3">
              <componentOf><subject><trialSubject>
                <code code="ENROLLED"/>
              </trialSubject></subject></componentOf>
              <sequence><value>1 2 3</value></sequence>
            </AnnotatedECG>"#,
        "REC-42",
    );

    assert!(
        output.contains(r#"<id root="1.2.826.0.1.3680043.10.543.3" extension="REC-42"/>"#),
        "got {output}"
    );
    // `id` comes first in the schema's content model.
    assert!(output.find("<id ").unwrap() < output.find("<code ").unwrap());
}

#[test]
fn a_self_closing_trial_subject_is_opened_up() {
    let output = deidentify(
        r#"<AnnotatedECG xmlns="urn:hl7-org:v3">
              <subject><trialSubject/></subject>
              <sequence><value>1 2 3</value></sequence>
            </AnnotatedECG>"#,
        "REC-42",
    );

    assert!(output.contains("<trialSubject><id "), "got {output}");
    assert!(output.contains("</trialSubject>"), "got {output}");
    assert!(output.contains(r#"extension="REC-42""#));
}

#[test]
fn an_existing_subject_id_is_not_duplicated() {
    let output = deidentify(
        r#"<AnnotatedECG xmlns="urn:hl7-org:v3">
              <trialSubject><id root="1.2.3" extension="SBJ-9"/></trialSubject>
              <sequence><value>1 2 3</value></sequence>
            </AnnotatedECG>"#,
        "REC-42",
    );

    assert_eq!(output.matches("<id ").count(), 1, "got {output}");
    assert!(!output.contains("SBJ-9"));
    assert!(output.contains(r#"extension="REC-42""#));
}

#[test]
fn the_created_subject_id_follows_the_document_prefix() {
    let output = deidentify(
        r#"<hl7:AnnotatedECG xmlns:hl7="urn:hl7-org:v3">
              <hl7:trialSubject/>
              <hl7:sequence><hl7:value>1 2 3</hl7:value></hl7:sequence>
            </hl7:AnnotatedECG>"#,
        "REC-42",
    );

    assert!(output.contains("<hl7:id "), "got {output}");
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
}

#[test]
fn preserves_presence_flags() {
    // `*ExistFlag` attributes are presence booleans, not identity; blanking
    // them changes the document's meaning.
    let output = deidentify(
        r#"<AnnotatedECG xmlns="urn:hl7-org:v3"><trialSubject/>
              <subject><name nameExistFlag="true"><lastName>Dupont</lastName></name></subject>
              <sequence><value>1 2 3</value></sequence>
            </AnnotatedECG>"#,
        "REC-42",
    );

    assert!(output.contains(r#"nameExistFlag="true""#));
    assert!(!output.contains("Dupont"));
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

    assert!(
        output.contains(r#"extension="REC&amp;&lt;42&gt;""#),
        "got {output}"
    );
    assert!(xml::validate(output.as_bytes(), "ecg.xml").is_ok());
}

#[test]
fn blanks_clinical_trial_identifiers() {
    // The acquisition device stamps the trial it was configured for; that is
    // site information the upload must not carry over.
    let output = deidentify(
        r#"<AnnotatedECG xmlns="urn:hl7-org:v3"><trialSubject/>
              <clinicalTrial><id root="1.2.3" extension="PROTO-2024-007"/>
                <title>HAUT LEVEQUE ABLATION TRIAL</title></clinicalTrial>
              <sequence><value>1 2 3</value></sequence>
            </AnnotatedECG>"#,
        "REC-42",
    );

    assert!(!output.contains("PROTO-2024-007"));
    assert!(!output.contains("HAUT LEVEQUE ABLATION TRIAL"));
}

#[test]
fn keeps_the_sex_which_projects_analyse() {
    let output = deidentify(&hl7_v3_fixture(), "REC-42");

    assert!(output.contains("<sex>F</sex>"));
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
    let source = r#"<AnnotatedECG xmlns="urn:hl7-org:v3"><trialSubject/>
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
        br#"<AnnotatedECG xmlns="urn:hl7-org:v3"><trialSubject/><vendorSignal>1 2 3</vendorSignal></AnnotatedECG>"#,
        "ecg.xml",
        "REC-42",
    )
    .unwrap_err();

    assert!(!error.starts_with("SKIP:"), "got {error}");
    assert!(error.contains("no signal data"), "got {error}");
}

#[test]
fn pins_the_birth_date_to_the_epoch() {
    let output = deidentify(&hl7_v3_fixture(), "REC-42");

    assert!(output.contains(r#"<birthTime value="19700101"/>"#));
    assert!(!output.contains("19540212"));
}

#[test]
fn a_hyphenated_birth_date_keeps_its_notation() {
    let output = deidentify(
        r#"<AnnotatedECG xmlns="urn:hl7-org:v3"><trialSubject/>
              <subject><dateofbirth>1954-02-12</dateofbirth></subject>
              <sequence><value>1 2 3</value></sequence>
            </AnnotatedECG>"#,
        "REC-42",
    );

    assert!(
        output.contains("<dateofbirth>1970-01-01</dateofbirth>"),
        "got {output}"
    );
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
  <componentOf><subject><trialSubject><patientId>PHI-1</patientId></trialSubject></subject></componentOf>
  <sequence><value>1 2 3</value></sequence>
</AnnotatedECG>"#,
        "REC-42",
    );

    assert!(
        !output.contains("20240115"),
        "the real acquisition date leaked"
    );
    assert!(output.contains(r#"extension="REC-42""#), "got {output}");
}

#[test]
fn plain_numbers_are_not_mistaken_for_dates() {
    // An eight-digit identifier is not a date and must not be shifted.
    let source = r#"<?xml version="1.0"?>
<AnnotatedECG xmlns="urn:hl7-org:v3"><trialSubject/>
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

#[test]
fn a_prefixed_document_is_processed_like_any_other() {
    // Accepting a prefixed root is not enough: every element then carries the
    // prefix, and the allowlist has to see through it or the recording comes
    // out empty.
    let output = deidentify(
        r#"<hl7:AnnotatedECG xmlns:hl7="urn:hl7-org:v3">
              <hl7:componentOf><hl7:subject><hl7:trialSubject>
                <hl7:patientId>PHI-1</hl7:patientId>
                <hl7:lastName>Dupont</hl7:lastName>
              </hl7:trialSubject></hl7:subject></hl7:componentOf>
              <hl7:sequence><hl7:value>111 112 113</hl7:value></hl7:sequence>
            </hl7:AnnotatedECG>"#,
        "REC-42",
    );

    assert!(
        output.contains("111 112 113"),
        "the signal was lost: {output}"
    );
    assert!(
        output.contains(r#"<hl7:id root="1.2.826.0.1.3680043.10.543.3" extension="REC-42"/>"#),
        "got {output}"
    );
    assert!(!output.contains("Dupont"), "the name survived");
    assert!(
        output.contains("xmlns:hl7"),
        "the namespace declaration was dropped"
    );
}

#[test]
fn a_document_with_no_subject_element_is_refused() {
    // Without trialSubject there is nowhere to attach the record id, and an
    // upload that cannot be traced back to its record is worse than no upload.
    let error = xml::deidentify(
        br#"<AnnotatedECG xmlns="urn:hl7-org:v3">
              <sequence><value>1 2 3</value></sequence>
            </AnnotatedECG>"#,
        "ecg.xml",
        "REC-42",
    )
    .unwrap_err();

    assert!(!error.starts_with("SKIP:"), "got {error}");
    assert!(error.contains("trialSubject"), "got {error}");
}
