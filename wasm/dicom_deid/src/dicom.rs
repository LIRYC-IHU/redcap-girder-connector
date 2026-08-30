use std::io::Cursor;

use dicom_anonymization::config::builder::ConfigBuilder;
use dicom_anonymization::config::uid_root::UidRoot;
use dicom_anonymization::processor::DefaultProcessor;
use dicom_anonymization::tags;
use dicom_anonymization::Anonymizer;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, FileDicomObject, InMemDicomObject};

use crate::dates;

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
    let stream = dicom_stream(input_bytes).map_err(|e| format!("SKIP: {e}"))?;
    let source = dicom_object::from_reader(Cursor::new(stream))
        .map_err(|e| format!("SKIP: input is not a valid DICOM file: {e}"))?;

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

    apply_date_policy(&source, &mut anonymized);

    let mut out = Vec::new();
    anonymized
        .write_all(&mut out)
        .map_err(|e| format!("SKIP: DICOM write failed: {e}"))?;

    Ok(out)
}

/// Date tags rewritten by the age-preserving shift.
///
/// The anonymizer removes or hash-shifts these on its own; they are restored
/// here from the source, moved by the patient's offset, so the interval between
/// the birth date and the acquisition survives.
const SHIFTED_DATE_TAGS: &[(Tag, VR)] = &[
    (tags::STUDY_DATE, VR::DA),
    (tags::SERIES_DATE, VR::DA),
    (tags::ACQUISITION_DATE, VR::DA),
    (tags::CONTENT_DATE, VR::DA),
    (tags::INSTANCE_CREATION_DATE, VR::DA),
    (tags::PERFORMED_PROCEDURE_STEP_START_DATE, VR::DA),
    (tags::ACQUISITION_DATE_TIME, VR::DT),
];

/// Pin the birth date to 1970-01-01 and move every other date by the same
/// offset. Without a birth date there is no age to preserve, so the anonymizer's
/// own handling (hash-shifted study date, removed series/acquisition dates) is
/// left in place.
fn apply_date_policy(
    source: &DefaultDicomObject,
    anonymized: &mut FileDicomObject<InMemDicomObject>,
) {
    let Some(birth_date) = read_string(source, tags::PATIENT_BIRTH_DATE) else {
        return;
    };
    let Some(offset) = dates::offset_from_birth_date(&birth_date) else {
        return;
    };

    anonymized.put(DataElement::new(
        tags::PATIENT_BIRTH_DATE,
        VR::DA,
        PrimitiveValue::from(dates::as_epoch_birth_date(&birth_date)),
    ));

    for (tag, vr) in SHIFTED_DATE_TAGS {
        let Some(original) = read_string(source, *tag) else {
            continue;
        };
        let Some(shifted) = dates::shift(&original, offset) else {
            continue;
        };
        anonymized.put(DataElement::new(*tag, *vr, PrimitiveValue::from(shifted)));
    }
}

fn read_string(object: &DefaultDicomObject, tag: Tag) -> Option<String> {
    let value = object.element(tag).ok()?.to_str().ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}
