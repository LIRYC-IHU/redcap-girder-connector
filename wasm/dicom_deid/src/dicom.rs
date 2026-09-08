use std::borrow::Cow;
use std::io::Cursor;

use dicom_anonymization::config::builder::ConfigBuilder;
use dicom_anonymization::config::uid_root::UidRoot;
use dicom_anonymization::processor::DefaultProcessor;
use dicom_anonymization::tags;
use dicom_anonymization::Anonymizer;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, FileDicomObject, FileMetaTableBuilder, InMemDicomObject};
use dicom_transfer_syntax_registry::entries::{
    EXPLICIT_VR_LITTLE_ENDIAN, IMPLICIT_VR_LITTLE_ENDIAN,
};

use crate::dates;

/// UID root registered for IHU Liryc, used to derive the anonymized UIDs.
const UID_ROOT: &str = "1.2.826.0.1.3680043.10.543";

/// Value written into `DeidentificationMethod` (0012,0063).
const DEIDENTIFICATION_METHOD: &str = "IHU LIRYC REDCAP PLUGIN";

const PREAMBLE_LENGTH: usize = 128;
const DICM_MAGIC_CODE: &[u8] = b"DICM";

/// Implementation class UID stamped into the file meta group that is minted
/// for header-less data sets.
const IMPLEMENTATION_CLASS_UID: &str = "1.2.826.0.1.3680043.10.543.1";

/// SOP class of a DICOMDIR (Media Storage Directory Storage).
const MEDIA_STORAGE_DIRECTORY_STORAGE: &str = "1.2.840.10008.1.3.10";

/// What a valid DICOM stream turns out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DicomKind {
    /// An image, waveform or any other data object: deidentified and uploaded.
    Object,
    /// A DICOMDIR: the index of the original media, which lists every patient
    /// on it in clear and points at the media's own file names.
    ///
    /// Anonymizing one is neither useful nor safe. Its directory records live
    /// in a sequence, so the identity inside them is cleaned but the study
    /// date is not shifted, and every reference it holds — file names, and the
    /// UIDs the upload rehashes — is dangling by the time the files land in
    /// Girder. The result is a stale index carrying an unshifted date, so it
    /// is dropped from the upload instead.
    Directory,
}

fn kind_of(object: &DefaultDicomObject) -> DicomKind {
    let is_directory =
        |uid: &str| uid.trim_end_matches('\0').trim() == MEDIA_STORAGE_DIRECTORY_STORAGE;

    let from_meta = is_directory(object.meta().media_storage_sop_class_uid());
    let from_dataset = object
        .element(tags::SOP_CLASS_UID)
        .ok()
        .and_then(|elem| elem.to_str().ok().map(|uid| is_directory(&uid)))
        .unwrap_or(false);

    if from_meta || from_dataset {
        DicomKind::Directory
    } else {
        DicomKind::Object
    }
}

/// Return the DICOM stream starting at the `DICM` magic code.
///
/// DICOM Part 10 files start with a 128-byte preamble followed by `DICM`, but
/// both `dicom_object::from_reader` and the anonymizer expect a stream that
/// starts at the magic code, so the preamble has to be skipped explicitly.
/// Streams already positioned at `DICM` are accepted as-is.
///
/// The file name is never consulted: exporters hand out DICOM under `.dcm`,
/// `.vim`, `.ima` or no extension at all, so the decision rests on the bytes.
/// Files that lack the Part 10 header altogether — a bare data set, with or
/// without the file meta group in front — are recognized by their first
/// elements and wrapped in a freshly minted header, see [`wrap_headerless`].
fn dicom_stream(input_bytes: &[u8]) -> Result<Cow<'_, [u8]>, String> {
    if input_bytes.is_empty() {
        return Err("input file is empty".to_string());
    }

    if input_bytes.starts_with(DICM_MAGIC_CODE) {
        return Ok(Cow::Borrowed(input_bytes));
    }

    let magic_end = PREAMBLE_LENGTH + DICM_MAGIC_CODE.len();
    if input_bytes.len() > magic_end && &input_bytes[PREAMBLE_LENGTH..magic_end] == DICM_MAGIC_CODE
    {
        return Ok(Cow::Borrowed(&input_bytes[PREAMBLE_LENGTH..]));
    }

    wrap_headerless(input_bytes).map(Cow::Owned)
}

/// Which header-less layout the first bytes of a file announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderlessLayout {
    /// The file meta group (0002,xxxx) is present but the preamble and the
    /// `DICM` magic code in front of it are not.
    MetaWithoutMagicCode,
    /// A bare data set, in explicit or implicit VR little endian.
    DataSet { explicit_vr: bool },
}

/// Sniff the first element of a file for one of the header-less layouts.
///
/// The check is deliberately cheap and strict, since it runs on every file
/// that is not Schiller, XML or Part 10 DICOM: the element must belong to
/// group 0002 (file meta) or 0008 (the first group of any data set), and
/// its encoding must be self-consistent — a valid two-letter VR for explicit
/// VR, a value length that fits in the file for implicit VR. Only little
/// endian is considered: the big endian syntax has been retired for years
/// and never appears in header-less exports.
fn sniff_headerless_layout(input_bytes: &[u8]) -> Option<HeaderlessLayout> {
    // Tag (4 bytes) plus either VR + length (4) or a plain length (4).
    if input_bytes.len() < 8 {
        return None;
    }

    let group = u16::from_le_bytes([input_bytes[0], input_bytes[1]]);
    let has_explicit_vr = input_bytes[4..6]
        .iter()
        .all(|byte| byte.is_ascii_uppercase());

    match group {
        // The file meta group is always explicit VR little endian.
        0x0002 if has_explicit_vr => Some(HeaderlessLayout::MetaWithoutMagicCode),
        0x0008 if has_explicit_vr => Some(HeaderlessLayout::DataSet { explicit_vr: true }),
        0x0008 => {
            let length = u32::from_le_bytes([
                input_bytes[4],
                input_bytes[5],
                input_bytes[6],
                input_bytes[7],
            ]) as usize;
            let remaining = input_bytes.len() - 8;
            (length <= remaining).then_some(HeaderlessLayout::DataSet { explicit_vr: false })
        }
        _ => None,
    }
}

/// Turn a header-less DICOM export into a stream starting at `DICM`.
///
/// Some systems write the data set alone, without the Part 10 header the
/// standard requires for files; others keep the file meta group but drop the
/// preamble and magic code. Both are common enough in clinical archives (often
/// under a `.vim` extension, or none) that refusing them would silently lose
/// images. The bytes are parsed as the layout they announce and, when they
/// really are a DICOM data set, re-serialized behind a minted file meta group
/// so the rest of the pipeline sees an ordinary Part 10 stream.
fn wrap_headerless(input_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let layout = sniff_headerless_layout(input_bytes)
        .ok_or_else(|| "input is not a valid DICOM file: DICM magic code not found".to_string())?;

    match layout {
        HeaderlessLayout::MetaWithoutMagicCode => {
            let mut stream = Vec::with_capacity(DICM_MAGIC_CODE.len() + input_bytes.len());
            stream.extend_from_slice(DICM_MAGIC_CODE);
            stream.extend_from_slice(input_bytes);
            Ok(stream)
        }
        HeaderlessLayout::DataSet { explicit_vr } => {
            let ts = if explicit_vr {
                EXPLICIT_VR_LITTLE_ENDIAN.erased()
            } else {
                IMPLICIT_VR_LITTLE_ENDIAN.erased()
            };
            let dataset = InMemDicomObject::read_dataset_with_ts(Cursor::new(input_bytes), &ts)
                .map_err(|e| {
                    format!("input is not a valid DICOM file: no DICM magic code, and not a bare data set either: {e}")
                })?;

            // `with_meta` copies the SOP class and instance UIDs from the data
            // set into the meta group, which is what makes the header valid.
            let meta = FileMetaTableBuilder::new()
                .transfer_syntax(ts.uid())
                .implementation_class_uid(IMPLEMENTATION_CLASS_UID);
            let file_object = dataset.with_meta(meta).map_err(|e| {
                format!("input is a bare DICOM data set that cannot be given a file header: {e}")
            })?;

            let mut bytes = Vec::with_capacity(input_bytes.len() + 256);
            file_object.write_all(&mut bytes).map_err(|e| {
                format!("input is a bare DICOM data set that cannot be re-serialized: {e}")
            })?;
            // `write_all` emits the preamble as well; the callers expect `DICM` first.
            Ok(bytes.split_off(PREAMBLE_LENGTH))
        }
    }
}

/// Check that the bytes are DICOM, and say whether they are a data object or
/// a DICOMDIR. The decision rests on the bytes alone, never on the file name.
pub fn validate(input_bytes: &[u8]) -> Result<DicomKind, String> {
    let stream = dicom_stream(input_bytes)?;

    let mut verify_cursor = Cursor::new(stream.as_ref());
    dicom_object::from_reader(&mut verify_cursor)
        .map(|object| kind_of(&object))
        .map_err(|e| format!("input is not a valid DICOM file: {e}"))
}

/// The reason a DICOMDIR is left out, as reported to the browser.
pub const DICOMDIR_SKIP_REASON: &str =
    "SKIP: DICOMDIR is the index of the original media, not data; it is not uploaded";

pub fn deidentify(
    input_bytes: &[u8],
    record_id: &str,
    patient_name: &str,
) -> Result<Vec<u8>, String> {
    let stream = dicom_stream(input_bytes).map_err(|e| format!("SKIP: {e}"))?;
    let source = dicom_object::from_reader(Cursor::new(stream.as_ref()))
        .map_err(|e| format!("SKIP: input is not a valid DICOM file: {e}"))?;
    if kind_of(&source) == DicomKind::Directory {
        return Err(DICOMDIR_SKIP_REASON.to_string());
    }

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
        .anonymize(Cursor::new(stream.as_ref()))
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
