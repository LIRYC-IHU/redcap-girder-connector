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

use dicom_deid::{deidentify_bytes, dicom, xml};
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
        eprintln!(
            "{name}: {} identifying elements removed, {} -> {} bytes",
            identifiers.len(),
            source.len(),
            output.len()
        );
    }
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

        assert!(
            xml::validate(&source, &name).is_ok(),
            "{name}: not recognized as an XML ECG ({:?})",
            xml::detect_type(&source, &name)
        );

        let source_nodes = xml_nodes(&source);
        let identifying: Vec<_> = source_nodes
            .iter()
            .filter(|(node_path, _)| xml::replacement_for(node_path, "X").is_some())
            .collect();
        assert!(
            !identifying.is_empty(),
            "{name}: no identifying value found — is this really a patient file?"
        );

        let output =
            xml::deidentify(&source, &name, RECORD_ID).unwrap_or_else(|e| panic!("{name}: {e}"));
        let output_nodes = xml_nodes(&output);

        // Every identifying node still present in the output must hold the
        // record id; the ones that are blanked disappear entirely.
        for (node_path, value) in &output_nodes {
            if xml::replacement_for(node_path, RECORD_ID).is_some() {
                assert_eq!(
                    value, RECORD_ID,
                    "{name}: {node_path} still holds {value:?}"
                );
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

        // Everything else — the recording itself — must come back untouched.
        let kept = |nodes: &[(String, String)]| -> Vec<(String, String)> {
            nodes
                .iter()
                .filter(|(node_path, _)| xml::replacement_for(node_path, "X").is_none())
                .cloned()
                .collect()
        };
        assert_eq!(
            kept(&source_nodes),
            kept(&output_nodes),
            "{name}: non-identifying content was altered"
        );

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
