//! Checks run against real recordings placed in `test_data/` at the repository
//! root.
//!
//! That directory is deliberately untracked: real files carry patient data and
//! must not enter the repository. These tests therefore skip when it is absent
//! (CI, a fresh clone) and run for anyone who has dropped files in it.
//!
//! Drop any `.dcm` / `.xml` in `test_data/` and it gets picked up. The
//! assertions are property-based rather than value-based, so they hold for any
//! recording: nothing the deidentifier itself considers identifying may survive
//! into the output.

use std::path::{Path, PathBuf};

use dicom_deid::xml::Policy;
use dicom_deid::{dates, deidentify_bytes, dicom, xml};
use quick_xml::events::Event;
use quick_xml::Reader;

const RECORD_ID: &str = "REC-42";
const PATIENT_NAME: &str = "TEST PROJECT^REC-42";

/// Short values (a sex code, an age, two digits of a room number) reappear by
/// chance inside waveform data or UIDs, so only longer ones are searched for.
const MIN_SEARCHABLE_LENGTH: usize = 6;

fn test_data_dir() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_data");
    dir.is_dir().then_some(dir)
}

fn files_with_extension(extension: &str) -> Vec<PathBuf> {
    let Some(dir) = test_data_dir() else {
        return Vec::new();
    };

    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("test_data is readable")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        })
        .collect();
    files.sort();
    files
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn real_dicom_files_are_deidentified() {
    let files = files_with_extension("dcm");
    if files.is_empty() {
        eprintln!("no .dcm in test_data/ — skipping");
        return;
    }

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read(&path).expect("readable");

        assert!(
            dicom::validate(&source).is_ok(),
            "{name}: not recognized as DICOM (a 128-byte preamble must be tolerated)"
        );

        let identifiers = dicom_identifiers(&source);
        assert!(
            !identifiers.is_empty(),
            "{name}: no identifying element found — is this really a patient file?"
        );

        let output = dicom::deidentify(&source, RECORD_ID, PATIENT_NAME)
            .unwrap_or_else(|e| panic!("{name}: {e}"));

        for (tag, value) in &identifiers {
            if value.len() >= MIN_SEARCHABLE_LENGTH {
                assert!(
                    !contains(&output, value),
                    "{name}: {tag} ({value:?}) survived deidentification"
                );
            }
        }

        assert!(
            dicom::validate(&output).is_ok(),
            "{name}: the deidentified file no longer parses"
        );

        if let Some(birth) = source_birth_date(&source) {
            let offset = dates::offset_from_birth_date(&birth).expect("birth date parses");
            assert_eq!(
                read_string(&output, dicom_dictionary_std::tags::PATIENT_BIRTH_DATE).as_deref(),
                Some("19700101"),
                "{name}: the birth date was not pinned to the epoch"
            );
            if let Some(study) = read_string(&source, dicom_dictionary_std::tags::STUDY_DATE) {
                assert_eq!(
                    read_string(&output, dicom_dictionary_std::tags::STUDY_DATE),
                    dates::shift(&study, offset),
                    "{name}: the study date was not moved by the patient offset"
                );
            }
            eprintln!("{name}: birth date pinned to the epoch, study date moved with it");
        }
        eprintln!(
            "{name}: {} identifying elements removed, {} -> {} bytes",
            identifiers.len(),
            source.len(),
            output.len()
        );
    }
}

fn source_birth_date(bytes: &[u8]) -> Option<String> {
    read_string(bytes, dicom_dictionary_std::tags::PATIENT_BIRTH_DATE)
}

/// One string element of a serialized DICOM file.
fn read_string(bytes: &[u8], tag: dicom_core::Tag) -> Option<String> {
    let stream = if bytes.starts_with(b"DICM") {
        bytes
    } else {
        &bytes[128..]
    };
    let object = dicom_object::from_reader(std::io::Cursor::new(stream)).ok()?;
    let value = object.element(tag).ok()?.to_str().ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Elements of a real file that must not survive, read back as strings.
fn dicom_identifiers(bytes: &[u8]) -> Vec<(&'static str, String)> {
    use dicom_dictionary_std::tags;

    let stream = if bytes.starts_with(b"DICM") {
        bytes
    } else {
        &bytes[128..]
    };
    let object = dicom_object::from_reader(std::io::Cursor::new(stream)).expect("parses");

    [
        ("PatientName", tags::PATIENT_NAME),
        ("PatientID", tags::PATIENT_ID),
        ("PatientBirthDate", tags::PATIENT_BIRTH_DATE),
        ("OtherPatientIDs", tags::OTHER_PATIENT_I_DS_SEQUENCE),
        ("AccessionNumber", tags::ACCESSION_NUMBER),
        ("InstitutionName", tags::INSTITUTION_NAME),
        ("InstitutionAddress", tags::INSTITUTION_ADDRESS),
        ("ReferringPhysicianName", tags::REFERRING_PHYSICIAN_NAME),
        ("PerformingPhysicianName", tags::PERFORMING_PHYSICIAN_NAME),
        ("OperatorsName", tags::OPERATORS_NAME),
        ("StationName", tags::STATION_NAME),
        ("StudyInstanceUID", tags::STUDY_INSTANCE_UID),
        ("SeriesInstanceUID", tags::SERIES_INSTANCE_UID),
        ("SOPInstanceUID", tags::SOP_INSTANCE_UID),
    ]
    .into_iter()
    .filter_map(|(name, tag)| {
        let value = object.element(tag).ok()?.to_str().ok()?.trim().to_string();
        (!value.is_empty()).then_some((name, value))
    })
    .collect()
}

#[test]
fn real_xml_ecg_files_are_deidentified() {
    let files = files_with_extension("xml");
    if files.is_empty() {
        eprintln!("no .xml in test_data/ — skipping");
        return;
    }

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read(&path).expect("readable");

        assert_eq!(
            xml::classify(&source, &name),
            xml::XmlOutcome::AnnotatedEcg,
            "{name}: not recognized as an HL7 Annotated ECG"
        );

        let source_nodes = xml_nodes(&source);
        let identifying: Vec<_> = source_nodes
            .iter()
            .filter(|(node_path, _)| !is_kept(node_path))
            .collect();
        assert!(
            !identifying.is_empty(),
            "{name}: no identifying value found — is this really a patient file?"
        );

        let output =
            xml::deidentify(&source, &name, RECORD_ID).unwrap_or_else(|e| panic!("{name}: {e}"));
        let output_nodes = xml_nodes(&output);

        // Nothing outside the allowlist may keep its original value.
        for (node_path, value) in &output_nodes {
            match xml::policy_for(node_path) {
                Policy::Keep | Policy::BirthDate => {}
                Policy::RecordId => {
                    assert_eq!(value, RECORD_ID, "{name}: {node_path} holds {value:?}")
                }
                Policy::UidRoot => assert!(
                    value.starts_with(xml::uid_root()),
                    "{name}: {node_path} kept a real uid ({value:?})"
                ),
                Policy::Drop | Policy::Blank => {
                    panic!("{name}: {node_path} should be gone but holds {value:?}")
                }
            }
        }

        // Patient identity must not resurface anywhere in the bytes.
        for (node_path, value) in &identifying {
            if is_person_identity(node_path) && value.len() >= MIN_SEARCHABLE_LENGTH {
                assert!(
                    !contains(&output, value),
                    "{name}: {node_path} ({value:?}) survived deidentification"
                );
            }
        }

        // Dates have their own policy, checked below; everything else — the
        // recording itself — must come back untouched.
        let governed = |path: &str, value: &str| !is_kept(path) || dates::parse(value).is_some();
        let kept = |nodes: &[(String, String)]| -> Vec<(String, String)> {
            nodes
                .iter()
                .filter(|(path, value)| !governed(path, value))
                .cloned()
                .collect()
        };
        assert_eq!(
            kept(&source_nodes),
            kept(&output_nodes),
            "{name}: non-identifying content was altered"
        );

        check_dates(&name, &source_nodes, &output_nodes);

        assert!(
            xml::validate(&output, &name).is_ok(),
            "{name}: the deidentified file is no longer a recognized ECG"
        );
        eprintln!(
            "{name}: {} identifying values removed, {} values preserved",
            identifying.len(),
            kept(&output_nodes).len()
        );
    }
}

/// Every date must move by one offset, the one that puts the birth date on
/// 1970-01-01, so that the age at acquisition is preserved and the real
/// calendar dates are gone.
fn check_dates(name: &str, source: &[(String, String)], output: &[(String, String)]) {
    let dated = |nodes: &[(String, String)]| -> Vec<(String, String)> {
        nodes
            .iter()
            .filter(|(path, value)| is_kept(path) && dates::parse(value).is_some())
            .cloned()
            .collect()
    };
    let (src, out) = (dated(source), dated(output));
    assert_eq!(src.len(), out.len(), "{name}: dates appeared or vanished");

    let birth = src
        .iter()
        .find(|(path, _)| xml::is_birth_date_path(path))
        .map(|(_, value)| value.clone());
    let Some(birth) = birth else {
        eprintln!("{name}: no birth date, dates left as they are");
        return;
    };

    let offset = dates::offset_from_birth_date(&birth).expect("birth date parses");
    let mut shifted = 0;

    for ((src_path, src_value), (out_path, out_value)) in src.iter().zip(out.iter()) {
        assert_eq!(
            src_path, out_path,
            "{name}: dates came back in another order"
        );
        let expected = if xml::is_birth_date_path(src_path) {
            dates::as_epoch_birth_date(src_value)
        } else {
            dates::shift(src_value, offset).expect("shiftable")
        };
        assert_eq!(
            out_value, &expected,
            "{name}: {src_path} was not moved by the patient offset"
        );
        assert_ne!(
            out_value, src_value,
            "{name}: {src_path} kept its real value"
        );
        shifted += 1;
    }

    eprintln!("{name}: birth date pinned to the epoch, {shifted} dates moved with it");
}

/// A node the allowlist lets through with its own value (dates included).
fn is_kept(path: &str) -> bool {
    matches!(xml::policy_for(path), Policy::Keep | Policy::BirthDate)
}

/// Paths naming the patient or the staff, as opposed to the wider set the
/// anonymizer blanks — which also catches vocabulary attributes such as
/// `codeSystemName`, matched only because they contain "name".
fn is_person_identity(path: &str) -> bool {
    const PERSON_FIELDS: &[&str] = &[
        "patientid",
        "patientname",
        "firstname",
        "lastname",
        "middlename",
        "givenname",
        "surname",
        "technician",
        "doctor",
        "operator",
    ];

    let path = path.to_lowercase();
    PERSON_FIELDS.iter().any(|field| path.contains(field))
}

/// Every non-empty text node and attribute of a document, keyed by path.
fn xml_nodes(bytes: &[u8]) -> Vec<(String, String)> {
    let mut reader = Reader::from_reader(bytes);
    let mut path: Vec<String> = Vec::new();
    let mut nodes = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                path.push(String::from_utf8_lossy(e.name().as_ref()).to_string());
                collect_attributes(&e, &path, &mut nodes);
            }
            Ok(Event::Empty(e)) => {
                path.push(String::from_utf8_lossy(e.name().as_ref()).to_string());
                collect_attributes(&e, &path, &mut nodes);
                path.pop();
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().trim().to_string();
                if !text.is_empty() {
                    nodes.push((path.join("/"), text));
                }
            }
            Ok(Event::End(_)) => {
                path.pop();
            }
            Ok(Event::Eof) => break,
            Err(e) => panic!("XML parsing failed: {e}"),
            _ => {}
        }
    }

    nodes
}

/// Collect one element's attributes as `path/@name` entries.
fn collect_attributes(
    element: &quick_xml::events::BytesStart<'_>,
    path: &[String],
    nodes: &mut Vec<(String, String)>,
) {
    for attr in element.attributes().flatten() {
        let key = format!(
            "{}/@{}",
            path.join("/"),
            String::from_utf8_lossy(attr.key.as_ref())
        );
        let value = attr.unescape_value().unwrap_or_default().to_string();
        if !value.trim().is_empty() {
            nodes.push((key, value));
        }
    }
}

#[test]
fn real_files_are_routed_to_the_right_deidentifier() {
    if test_data_dir().is_none() {
        eprintln!("no test_data/ — skipping");
        return;
    }

    for (extension, expected) in [("dcm", "dicom"), ("xml", "xml")] {
        for path in files_with_extension(extension) {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let source = std::fs::read(&path).expect("readable");

            let result =
                deidentify_bytes(&source, &name, RECORD_ID, PATIENT_NAME, true, true, true)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));

            assert_eq!(result.format_name, expected, "{name} was routed wrong");
            assert_ne!(result.bytes, source, "{name} came back untouched");
        }
    }
}

#[test]
fn the_leaks_found_in_the_real_recording_are_closed() {
    let Some(dir) = test_data_dir() else {
        eprintln!("no test_data/ — skipping");
        return;
    };
    let path = dir.join("ECG.xml");
    if !path.is_file() {
        eprintln!("no ECG.xml — skipping");
        return;
    }

    let source = std::fs::read(&path).expect("readable");
    let output = xml::deidentify(&source, "ECG.xml", RECORD_ID).expect("deidentified");
    let text = String::from_utf8(output).expect("utf-8");

    // Values a denylist let through, each found in this recording.
    for (what, needle) in [
        (
            "the instance OID encoding the acquisition date and time",
            "2026623.85727",
        ),
        ("the device serial number", "FN-8B013991"),
        ("the trial identifier", "26060670914"),
        ("the free-text interpretation", "Fibrillation"),
    ] {
        assert!(!text.contains(needle), "{what} survived");
    }

    // …while the recording itself is intact.
    assert!(text.contains("MDC_ECG_LEAD_I"), "lead codes were lost");
    assert!(text.contains("SLIST_PQ"), "the sequence datatype was lost");
    assert!(text.contains("<digits>"), "the waveform was lost");
    assert!(text.contains(r#"code="F""#), "sex was lost");
}
