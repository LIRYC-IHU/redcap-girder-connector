//! Synthetic fixtures for the deidentification tests.
//!
//! Everything here is generated from scratch: the repository must never carry
//! real patient files, and a hand-built fixture makes the identifying values
//! explicit so a test can assert they are gone.

#![allow(dead_code)]

use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};

/// Identifying values planted in the DICOM fixture.
pub const DICOM_PATIENT_NAME: &str = "DOE^JOHN^^^";
pub const DICOM_PATIENT_ID: &str = "PHI-PATIENT-0001";
pub const DICOM_BIRTH_DATE: &str = "19540212";
pub const DICOM_INSTITUTION: &str = "HOPITAL HAUT LEVEQUE";
pub const DICOM_STUDY_DATE: &str = "20240115";
pub const DICOM_STUDY_UID: &str = "1.2.840.113619.2.55.3.604688119.868.1234567890.1";
pub const DICOM_SERIES_UID: &str = "1.2.840.113619.2.55.3.604688119.868.1234567890.2";
pub const DICOM_SOP_UID: &str = "1.2.840.113619.2.55.3.604688119.868.1234567890.3";
/// A vendor private tag, of the kind that routinely smuggles identity.
pub const DICOM_PRIVATE_TAG: dicom_core::Tag = dicom_core::Tag(0x0033, 0x1001);
pub const DICOM_PRIVATE_VALUE: &str = "OPERATOR DUPONT MARIE";

const EXPLICIT_VR_LITTLE_ENDIAN: &str = "1.2.840.10008.1.2.1";
const SECONDARY_CAPTURE_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.7";

/// A minimal but standards-shaped DICOM Part 10 file (128-byte preamble +
/// `DICM` + file meta group + data set) carrying the PHI constants above.
pub fn dicom_fixture() -> Vec<u8> {
    dicom_fixture_with(DICOM_STUDY_DATE, Some(DICOM_BIRTH_DATE))
}

/// Same fixture with a caller-chosen `StudyDate`, to observe the date shift
/// across two studies of one patient.
pub fn dicom_fixture_with_study_date(study_date: &str) -> Vec<u8> {
    dicom_fixture_with(study_date, Some(DICOM_BIRTH_DATE))
}

/// Same fixture with a caller-chosen `PatientBirthDate`, which is what sets the
/// offset every other date moves by.
pub fn dicom_fixture_with_birth_date(birth_date: &str) -> Vec<u8> {
    dicom_fixture_with(DICOM_STUDY_DATE, Some(birth_date))
}

/// Same fixture with no birth date at all: there is then no age to preserve.
pub fn dicom_fixture_without_birth_date() -> Vec<u8> {
    dicom_fixture_with(DICOM_STUDY_DATE, None)
}

fn dicom_fixture_with(study_date: &str, birth_date: Option<&str>) -> Vec<u8> {
    let mut object = InMemDicomObject::new_empty();

    object.put(DataElement::new(
        tags::SPECIFIC_CHARACTER_SET,
        VR::CS,
        PrimitiveValue::from("ISO_IR 100"),
    ));
    object.put(DataElement::new(
        tags::PATIENT_NAME,
        VR::PN,
        PrimitiveValue::from(DICOM_PATIENT_NAME),
    ));
    object.put(DataElement::new(
        tags::PATIENT_ID,
        VR::LO,
        PrimitiveValue::from(DICOM_PATIENT_ID),
    ));
    if let Some(birth_date) = birth_date {
        object.put(DataElement::new(
            tags::PATIENT_BIRTH_DATE,
            VR::DA,
            PrimitiveValue::from(birth_date),
        ));
    }
    object.put(DataElement::new(
        tags::PATIENT_SEX,
        VR::CS,
        PrimitiveValue::from("M"),
    ));
    object.put(DataElement::new(
        tags::INSTITUTION_NAME,
        VR::LO,
        PrimitiveValue::from(DICOM_INSTITUTION),
    ));
    object.put(DataElement::new(
        tags::STUDY_DATE,
        VR::DA,
        PrimitiveValue::from(study_date),
    ));
    object.put(DataElement::new(
        tags::SERIES_DATE,
        VR::DA,
        PrimitiveValue::from(study_date),
    ));
    object.put(DataElement::new(
        tags::ACQUISITION_DATE,
        VR::DA,
        PrimitiveValue::from(study_date),
    ));
    object.put(DataElement::new(
        tags::MODALITY,
        VR::CS,
        PrimitiveValue::from("XA"),
    ));
    object.put(DataElement::new(
        tags::STUDY_INSTANCE_UID,
        VR::UI,
        PrimitiveValue::from(DICOM_STUDY_UID),
    ));
    object.put(DataElement::new(
        tags::SERIES_INSTANCE_UID,
        VR::UI,
        PrimitiveValue::from(DICOM_SERIES_UID),
    ));
    object.put(DataElement::new(
        tags::SOP_INSTANCE_UID,
        VR::UI,
        PrimitiveValue::from(DICOM_SOP_UID),
    ));
    object.put(DataElement::new(
        DICOM_PRIVATE_TAG,
        VR::LO,
        PrimitiveValue::from(DICOM_PRIVATE_VALUE),
    ));
    object.put(DataElement::new(
        tags::SOP_CLASS_UID,
        VR::UI,
        PrimitiveValue::from(SECONDARY_CAPTURE_STORAGE),
    ));

    let meta_builder = FileMetaTableBuilder::new()
        .media_storage_sop_class_uid(SECONDARY_CAPTURE_STORAGE)
        .media_storage_sop_instance_uid(DICOM_SOP_UID)
        .transfer_syntax(EXPLICIT_VR_LITTLE_ENDIAN)
        .implementation_class_uid("1.2.826.0.1.3680043.10.543.1");

    let file_object = object.with_meta(meta_builder).expect("object with meta");

    let mut bytes = Vec::new();
    file_object
        .write_all(&mut bytes)
        .expect("serialize fixture");
    bytes
}

/// The same file without the 128-byte preamble, as produced by some exporters.
pub fn dicom_fixture_without_preamble() -> Vec<u8> {
    let bytes = dicom_fixture();
    assert_eq!(&bytes[128..132], b"DICM");
    bytes[128..].to_vec()
}

/// Read a string element from a serialized DICOM file.
pub fn read_dicom_string(bytes: &[u8], tag: dicom_core::Tag) -> Option<String> {
    let stream = if bytes.starts_with(b"DICM") {
        bytes
    } else {
        &bytes[128..]
    };

    let object = dicom_object::from_reader(std::io::Cursor::new(stream)).ok()?;
    let element = object.element(tag).ok()?;
    Some(element.to_str().ok()?.trim().to_string())
}

// --- Schiller ---------------------------------------------------------------

pub const SCHILLER_MAGIC_NUMBER: [u8; 16] = [
    0x00, 0x55, 0xDA, 0xBA, 0x01, 0x00, 0x63, 0x00, 0x60, 0x43, 0x54, 0x43, 0x41, 0x43, 0x55, 0x10,
];
pub const SCHILLER_AUDIO_START: usize = 0x1800;
pub const SCHILLER_AUDIO_END: usize = 0xA1800;
pub const SCHILLER_PATIENT_BLOCK_START: usize = 0x9FE;
pub const SCHILLER_PATIENT_BLOCK_END: usize = 0xBFF;
pub const SCHILLER_PATIENT_ID_OFFSET: usize = 0xB2B;
pub const SCHILLER_PATIENT_ID_LENGTH: usize = 28;
pub const SCHILLER_UUID_OFFSET: usize = 0xBD4;
pub const SCHILLER_XOR_KEY: u8 = 0x8A;

pub const SCHILLER_PATIENT_ID: &str = "9876543210987654321098765";
/// Cleartext planted in the patient block; it must not survive the transform.
pub const SCHILLER_PATIENT_NAME: &[u8] = b"DUPONT^MARIE";
/// Marker filling the voice-annotation section, which is wiped wholesale.
pub const SCHILLER_AUDIO_MARKER: u8 = 0x5C;

/// A synthetic Schiller Holter container: correct magic number, a full-size
/// audio section, and a patient block holding both a cleartext name and an
/// XOR-obfuscated patient id.
pub fn schiller_fixture() -> Vec<u8> {
    let mut buffer = vec![0u8; SCHILLER_AUDIO_END + 0x1000];
    buffer[..SCHILLER_MAGIC_NUMBER.len()].copy_from_slice(&SCHILLER_MAGIC_NUMBER);

    buffer[SCHILLER_AUDIO_START..SCHILLER_AUDIO_END].fill(SCHILLER_AUDIO_MARKER);

    // Cleartext PHI somewhere in the patient block.
    let name_offset = SCHILLER_PATIENT_BLOCK_START + 0x40;
    buffer[name_offset..name_offset + SCHILLER_PATIENT_NAME.len()]
        .copy_from_slice(SCHILLER_PATIENT_NAME);

    // Original (obfuscated) patient id in its dedicated field.
    let encoded: Vec<u8> = SCHILLER_PATIENT_ID
        .bytes()
        .map(|b| b ^ SCHILLER_XOR_KEY)
        .collect();
    buffer[SCHILLER_PATIENT_ID_OFFSET..SCHILLER_PATIENT_ID_OFFSET + encoded.len()]
        .copy_from_slice(&encoded);

    // A device UUID that must be regenerated.
    let original_uuid = b"{AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE}";
    buffer[SCHILLER_UUID_OFFSET..SCHILLER_UUID_OFFSET + original_uuid.len()]
        .copy_from_slice(original_uuid);

    buffer
}

/// Decode an XOR-obfuscated Schiller string field.
pub fn schiller_decode_field(buffer: &[u8], offset: usize, length: usize) -> String {
    buffer[offset..offset + length]
        .iter()
        .map(|b| b ^ SCHILLER_XOR_KEY)
        .take_while(|b| *b != (0xAA ^ SCHILLER_XOR_KEY) && *b != 0x00)
        .map(char::from)
        .collect()
}

// --- XML ECG ----------------------------------------------------------------

/// HL7 v3 `AnnotatedECG` fixture with identity fields and two leads that share
/// the same element path but hold different values.
pub fn hl7_v3_fixture() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<AnnotatedECG xmlns="urn:hl7-org:v3" type="Observation">
  <id root="1.2.840.113619.2.999" extension="STUDY-1"/>
  <componentOf>
    <subject>
      <patientId>PHI-PATIENT-0001</patientId>
      <secondPatientId>SSN-123-456-789</secondPatientId>
      <firstName>Marie</firstName>
      <lastName>Dupont</lastName>
      <middleName>Anne</middleName>
      <age>67</age>
      <sex>F</sex>
      <room>412</room>
      <bed>B</bed>
      <pointOfCare>CARDIOLOGY WARD 3</pointOfCare>
      <subjectDemographicPerson>
        <birthTime value="19540212"/>
      </subjectDemographicPerson>
      <technician>NURSE^ALICE</technician>
      <doctor>PROF^BERNARD</doctor>
      <birthTime value="19540212"/>
    </subject>
  </componentOf>
  <component>
    <series>
      <effectiveTime value="20240115093000"/>
      <sequence>
        <code code="MDC_ECG_LEAD_I"/>
        <value>111 112 113</value>
      </sequence>
      <sequence>
        <code code="MDC_ECG_LEAD_II"/>
        <value>221 222 223</value>
      </sequence>
    </series>
  </component>
</AnnotatedECG>
"#
    .to_string()
}

/// Philips `restingecgdata` fixture, including a `*ExistFlag` attribute that
/// must survive and a repeated measurement path.
pub fn philips_fixture() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<restingecgdata xmlns="http://www3.medical.philips.com" version="1.03">
  <patient>
    <generalpatientdata>
      <patientid>PHI-PATIENT-0001</patientid>
      <name nameExistFlag="true">
        <lastname>Dupont</lastname>
        <firstname>Marie</firstname>
      </name>
      <age>
        <years>67</years>
      </age>
      <sex>Female</sex>
      <dateofbirth>1954-02-12</dateofbirth>
    </generalpatientdata>
    <clinicaltrialdata>
      <clinicaltrialprotocolid>PROTO-2024-007</clinicaltrialprotocolid>
      <clinicaltrialprotocolname>HAUT LEVEQUE ABLATION TRIAL</clinicaltrialprotocolname>
    </clinicaltrialdata>
  </patient>
  <dataacquisition>
    <acquirer>
      <operator>TECH^CLAIRE</operator>
    </acquirer>
  </dataacquisition>
  <measurements>
    <measurement>
      <amplitude>101</amplitude>
    </measurement>
    <measurement>
      <amplitude>202</amplitude>
    </measurement>
  </measurements>
</restingecgdata>
"#
    .to_string()
}

/// Days between two `YYYYMMDD` DICOM dates, via a day count from a fixed epoch.
pub fn days_between(from: &str, to: &str) -> i64 {
    fn to_days(date: &str) -> i64 {
        let year: i64 = date[0..4].parse().unwrap();
        let month: i64 = date[4..6].parse().unwrap();
        let day: i64 = date[6..8].parse().unwrap();

        // Howard Hinnant's days-from-civil algorithm.
        let year = if month <= 2 { year - 1 } else { year };
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146097 + day_of_era - 719468
    }

    to_days(to) - to_days(from)
}
