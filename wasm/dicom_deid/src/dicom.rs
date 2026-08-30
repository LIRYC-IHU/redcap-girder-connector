use std::io::Cursor;

use dicom_anonymization::config::builder::ConfigBuilder;
use dicom_anonymization::config::uid_root::UidRoot;
use dicom_anonymization::processor::DefaultProcessor;
use dicom_anonymization::tags;
use dicom_anonymization::Anonymizer;
use dicom_core::{DataElement, PrimitiveValue, VR};

/// UID root registered for IHU Liryc, used to derive the anonymized UIDs.
const UID_ROOT: &str = "1.2.826.0.1.3680043.10.543";

/// Value written into `DeidentificationMethod` (0012,0063).
const DEIDENTIFICATION_METHOD: &str = "IHU LIRYC REDCAP PLUGIN";

const PREAMBLE_LENGTH: usize = 128;
const DICM_MAGIC_CODE: &[u8] = b"DICM";

/// Return the DICOM stream starting at the `DICM` magic code.
///
/// DICOM Part 10 files start with a 128-byte preamble followed by `DICM`, but
/// both `dicom_object::from_reader` and the anonymizer expect a stream that
/// starts at the magic code, so the preamble has to be skipped explicitly.
/// Streams already positioned at `DICM` are accepted as-is.
fn dicom_stream(input_bytes: &[u8]) -> Result<&[u8], String> {
    if input_bytes.is_empty() {
        return Err("input file is empty".to_string());
    }

    if input_bytes.starts_with(DICM_MAGIC_CODE) {
        return Ok(input_bytes);
    }

    let magic_end = PREAMBLE_LENGTH + DICM_MAGIC_CODE.len();
    if input_bytes.len() > magic_end && &input_bytes[PREAMBLE_LENGTH..magic_end] == DICM_MAGIC_CODE
    {
        return Ok(&input_bytes[PREAMBLE_LENGTH..]);
    }

    Err("input is not a valid DICOM file: DICM magic code not found".to_string())
}

pub fn validate(input_bytes: &[u8]) -> Result<(), String> {
    let stream = dicom_stream(input_bytes)?;

    let mut verify_cursor = Cursor::new(stream);
    dicom_object::from_reader(&mut verify_cursor)
        .map(|_| ())
        .map_err(|e| format!("input is not a valid DICOM file: {e}"))
}

pub fn deidentify(
    input_bytes: &[u8],
    record_id: &str,
    patient_name: &str,
) -> Result<Vec<u8>, String> {
    validate(input_bytes).map_err(|e| format!("SKIP: {e}"))?;
    let stream = dicom_stream(input_bytes).map_err(|e| format!("SKIP: {e}"))?;

    // The anonymizer only visits elements that already exist in the source, so
    // the REDCap identity is written explicitly afterwards: a file missing
    // `PatientID` (or the deidentification stamp) would otherwise come out
    // without either.
    let config = ConfigBuilder::default()
        .uid_root(UidRoot(UID_ROOT.into()))
        .build();

    let processor = DefaultProcessor::new(config);
    let anonymizer = Anonymizer::new(processor);

    let mut anonymized = anonymizer
        .anonymize(Cursor::new(stream))
        .map_err(|e| format!("SKIP: DICOM anonymization failed: {e}"))?
        .anonymized;

    anonymized.put(DataElement::new(
        tags::PATIENT_ID,
        VR::LO,
        PrimitiveValue::from(record_id),
    ));
    anonymized.put(DataElement::new(
        tags::PATIENT_NAME,
        VR::PN,
        PrimitiveValue::from(patient_name),
    ));
    anonymized.put(DataElement::new(
        tags::DEIDENTIFICATION_METHOD,
        VR::LO,
        PrimitiveValue::from(DEIDENTIFICATION_METHOD),
    ));
    anonymized.put(DataElement::new(
        tags::PATIENT_IDENTITY_REMOVED,
        VR::CS,
        PrimitiveValue::from("YES"),
    ));

    let mut out = Vec::new();
    anonymized
        .write_all(&mut out)
        .map_err(|e| format!("SKIP: DICOM write failed: {e}"))?;

    Ok(out)
}
