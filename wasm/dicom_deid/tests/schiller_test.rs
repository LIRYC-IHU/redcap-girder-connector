mod common;

use common::*;
use dicom_deid::schiller;

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn recognizes_a_schiller_container() {
    assert!(schiller::validate(&schiller_fixture()).is_ok());
}

#[test]
fn rejects_files_without_the_magic_number() {
    assert!(schiller::validate(b"").is_err());
    assert!(schiller::validate(&vec![0u8; 0x2000]).is_err());
}

#[test]
fn rejects_a_truncated_container_instead_of_panicking() {
    // A file that only carries the magic number used to be sliced blindly, so
    // a truncated (or spoofed) upload aborted the whole WASM worker.
    let mut truncated = vec![0u8; 0x2000];
    truncated[..SCHILLER_MAGIC_NUMBER.len()].copy_from_slice(&SCHILLER_MAGIC_NUMBER);

    assert!(schiller::validate(&truncated).is_err());
    assert!(schiller::deidentify(&truncated, "REC-42").is_err());
}

#[test]
fn writes_the_record_id_into_the_patient_id_field() {
    let output = schiller::deidentify(&schiller_fixture(), "REC-42").expect("deidentified");

    let stored = schiller_decode_field(
        &output,
        SCHILLER_PATIENT_ID_OFFSET,
        SCHILLER_PATIENT_ID_LENGTH,
    );
    assert_eq!(stored, "REC-42");
}

#[test]
fn truncates_a_record_id_longer_than_the_field() {
    let long_id = "A".repeat(40);
    let output = schiller::deidentify(&schiller_fixture(), &long_id).expect("deidentified");

    let stored = schiller_decode_field(
        &output,
        SCHILLER_PATIENT_ID_OFFSET,
        SCHILLER_PATIENT_ID_LENGTH,
    );
    assert_eq!(stored.len(), SCHILLER_PATIENT_ID_LENGTH);
    assert_eq!(stored, "A".repeat(SCHILLER_PATIENT_ID_LENGTH));
}

#[test]
fn wipes_the_cleartext_patient_block() {
    let output = schiller::deidentify(&schiller_fixture(), "REC-42").expect("deidentified");

    assert!(
        !contains(&output, SCHILLER_PATIENT_NAME),
        "the cleartext patient name is still in the file"
    );

    let original_id_encoded: Vec<u8> = SCHILLER_PATIENT_ID
        .bytes()
        .map(|b| b ^ SCHILLER_XOR_KEY)
        .collect();
    assert!(
        !contains(&output, &original_id_encoded),
        "the original obfuscated patient id is still in the file"
    );
}

#[test]
fn wipes_the_voice_annotation_section() {
    let output = schiller::deidentify(&schiller_fixture(), "REC-42").expect("deidentified");

    // Voice annotations are recorded by the technician and routinely name the
    // patient, so the whole section is zeroed rather than filtered.
    assert!(
        output[SCHILLER_AUDIO_START..SCHILLER_AUDIO_END]
            .iter()
            .all(|b| *b == 0),
        "the audio section was not wiped"
    );
    assert!(!contains(&output, &[SCHILLER_AUDIO_MARKER; 64]));
}

#[test]
fn regenerates_the_device_uuid() {
    let output = schiller::deidentify(&schiller_fixture(), "REC-42").expect("deidentified");

    let uuid = String::from_utf8_lossy(&output[SCHILLER_UUID_OFFSET..SCHILLER_UUID_OFFSET + 38]);
    assert!(uuid.starts_with('{') && uuid.ends_with('}'), "got {uuid}");
    assert_ne!(uuid, "{AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE}");

    let mut second = schiller::deidentify(&schiller_fixture(), "REC-42").expect("deidentified");
    second.truncate(SCHILLER_UUID_OFFSET + 38);
    assert_ne!(
        uuid,
        String::from_utf8_lossy(&second[SCHILLER_UUID_OFFSET..SCHILLER_UUID_OFFSET + 38]),
        "each run must mint a fresh uuid"
    );
}

#[test]
fn keeps_the_container_structure_intact() {
    let input = schiller_fixture();
    let output = schiller::deidentify(&input, "REC-42").expect("deidentified");

    assert_eq!(output.len(), input.len(), "file size must not change");
    assert_eq!(
        &output[..SCHILLER_MAGIC_NUMBER.len()],
        &SCHILLER_MAGIC_NUMBER,
        "the magic number must survive"
    );
    // The anonymized file is re-parsable by the deidentifier itself, which is
    // the closest we can get to "the device can still read it".
    assert!(schiller::validate(&output).is_ok());
}

#[test]
fn falls_back_to_a_synthetic_id_when_no_record_id_is_given() {
    let output = schiller::deidentify(&schiller_fixture(), "   ").expect("deidentified");

    let stored = schiller_decode_field(
        &output,
        SCHILLER_PATIENT_ID_OFFSET,
        SCHILLER_PATIENT_ID_LENGTH,
    );
    assert_eq!(stored.len(), SCHILLER_PATIENT_ID_LENGTH);
    assert!(stored.chars().all(|c| c.is_ascii_digit()));
    assert_ne!(stored, SCHILLER_PATIENT_ID);
}
