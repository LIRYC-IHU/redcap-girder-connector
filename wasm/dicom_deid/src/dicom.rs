use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::Cursor;

use dicom_anonymization::actions::Action;
use dicom_anonymization::config::builder::ConfigBuilder;
use dicom_anonymization::config::uid_root::UidRoot;
use dicom_anonymization::config::Config;
use dicom_anonymization::processor::DefaultProcessor;
use dicom_anonymization::tags;
use dicom_anonymization::Anonymizer;
use dicom_core::header::Header;
use dicom_core::value::DataSetSequence;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::mem::InMemElement;
use dicom_object::{DefaultDicomObject, FileMetaTableBuilder, InMemDicomObject};
use dicom_transfer_syntax_registry::entries::{
    EXPLICIT_VR_LITTLE_ENDIAN, IMPLICIT_VR_LITTLE_ENDIAN,
};

use crate::{dates, pseudonym, PatientKeys};

/// UID root registered for IHU Liryc, used to derive the anonymized UIDs.
const UID_ROOT: &str = "1.2.826.0.1.3680043.10.543";

/// Root of the UIDs the DICOM standard itself defines (SOP classes, transfer
/// syntaxes): they say nothing about a patient.
const DICOM_STANDARD_UID_ROOT: &str = "1.2.840.10008.";

/// Value written into `DeidentificationMethod` (0012,0063), followed by the
/// release version.
const DEIDENTIFICATION_METHOD: &str = "IHU LIRYC REDCAP PLUGIN";

/// Release of the module this deidentifier is built for, read from the
/// repository's `VERSION` file at compile time. The packaging script builds
/// the WASM from the same file, so a deidentified file names the rules it went
/// through: the date policy, for one, has changed between releases.
const RELEASE_VERSION: &str = include_str!("../../../VERSION");

/// `IHU LIRYC REDCAP PLUGIN v2.0.0`, for instance.
pub fn deidentification_method() -> String {
    format!("{DEIDENTIFICATION_METHOD} v{}", RELEASE_VERSION.trim())
}

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

/// Who a file belongs to, read before deidentification: the NIP feeds the
/// pseudonym, the name and birth date are shown to the uploader for
/// confirmation and never leave the browser.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub nip: Option<String>,
    /// A DICOM person name: `FAMILY^GIVEN^…`, or the whole name as text.
    pub name: Option<String>,
    pub birth_date: Option<String>,
    /// The DICOM series the file belongs to, for the transfer summary.
    pub series: Option<Series>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Series {
    pub uid: String,
    pub modality: String,
    pub description: String,
}

/// Read up to the pixel data only: a batch can hold thousands of images, and
/// identity and series sit in the header.
pub fn identity(input_bytes: &[u8]) -> Result<Identity, String> {
    let stream = dicom_stream(input_bytes)?;
    let object = dicom_object::OpenFileOptions::new()
        .read_until(tags::PIXEL_DATA)
        .from_reader(Cursor::new(stream.as_ref()))
        .map_err(|e| format!("input is not a valid DICOM file: {e}"))?;
    let series = read_string(&object, tags::SERIES_INSTANCE_UID).map(|uid| Series {
        uid,
        modality: read_string(&object, tags::MODALITY).unwrap_or_default(),
        description: read_string(&object, tags::SERIES_DESCRIPTION).unwrap_or_default(),
    });
    Ok(Identity {
        nip: read_string(&object, tags::PATIENT_ID),
        name: read_string(&object, tags::PATIENT_NAME),
        birth_date: read_string(&object, tags::PATIENT_BIRTH_DATE),
        series,
    })
}

/// `keys` come from the patient's pseudonym: every date of the file moves by the
/// offset and every UID is rewritten under the key, see [`rewrite_elements`].
pub fn deidentify(
    input_bytes: &[u8],
    record_id: &str,
    patient_name: &str,
    keys: &PatientKeys,
) -> Result<Vec<u8>, String> {
    let stream = dicom_stream(input_bytes).map_err(|e| format!("SKIP: {e}"))?;
    let mut source = dicom_object::from_reader(Cursor::new(stream.as_ref()))
        .map_err(|e| format!("SKIP: input is not a valid DICOM file: {e}"))?;
    if kind_of(&source) == DicomKind::Directory {
        return Err(DICOMDIR_SKIP_REASON.to_string());
    }

    // Computed on the real dates, before they move.
    let patient_age = age_at_acquisition(&source);

    // Dates and UIDs are settled before the anonymizer runs. Every rewritten
    // element is then marked `Keep`, so the anonymizer neither changes it a
    // second time nor removes it.
    let defaults = anonymizer_config().build();
    let mut kept_tags = BTreeSet::new();
    rewrite_elements(&mut source, keys, &defaults, &mut kept_tags);

    let mut bytes = Vec::with_capacity(stream.len());
    source
        .write_all(&mut bytes)
        .map_err(|e| format!("SKIP: DICOM write failed: {e}"))?;
    // `write_all` emits the preamble as well; the anonymizer expects `DICM` first.
    let prepared = bytes.split_off(PREAMBLE_LENGTH);

    // The anonymizer only visits elements that already exist in the source, so
    // the REDCap identity is written explicitly afterwards: a file missing
    // `PatientID` (or the deidentification stamp) would otherwise come out
    // without either.
    let config = kept_tags
        .into_iter()
        .fold(anonymizer_config(), |builder, tag| {
            builder.tag_action(tag, Action::Keep)
        })
        .build();

    let processor = DefaultProcessor::new(config);
    let anonymizer = Anonymizer::new(processor);

    let mut anonymized = anonymizer
        .anonymize(Cursor::new(prepared.as_slice()))
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
    // Type 2 in the Patient module: present, and empty.
    anonymized.put(DataElement::new(
        tags::PATIENT_BIRTH_DATE,
        VR::DA,
        PrimitiveValue::Empty,
    ));
    match patient_age {
        Some(age) => {
            anonymized.put(DataElement::new(
                tags::PATIENT_AGE,
                VR::AS,
                PrimitiveValue::from(age),
            ));
        }
        None => {
            anonymized.remove_element(tags::PATIENT_AGE);
        }
    }
    anonymized.put(DataElement::new(
        tags::DEIDENTIFICATION_METHOD,
        VR::LO,
        PrimitiveValue::from(deidentification_method()),
    ));
    anonymized.put(DataElement::new(
        tags::PATIENT_IDENTITY_REMOVED,
        VR::CS,
        PrimitiveValue::from("YES"),
    ));
    // PS3.15, Retain Longitudinal Temporal Information with Modified Dates.
    anonymized.put(DataElement::new(
        tags::LONGITUDINAL_TEMPORAL_INFORMATION_MODIFIED,
        VR::CS,
        PrimitiveValue::from("MODIFIED"),
    ));

    let mut out = Vec::new();
    anonymized
        .write_all(&mut out)
        .map_err(|e| format!("SKIP: DICOM write failed: {e}"))?;

    Ok(out)
}

fn anonymizer_config() -> ConfigBuilder {
    ConfigBuilder::default().uid_root(UidRoot(UID_ROOT.into()))
}

/// Dates the age is computed at, best first: the study, then the acquisition.
const ACQUISITION_DATE_TAGS: &[Tag] = &[
    tags::STUDY_DATE,
    tags::SERIES_DATE,
    tags::ACQUISITION_DATE,
    tags::CONTENT_DATE,
    tags::ACQUISITION_DATE_TIME,
];

/// `PatientAge` (an `AS`): years from two years old, months below, weeks below
/// two months. Days are never given: for a newborn they would be nearly as
/// precise as the dates that were removed.
///
/// Without a birth date, a well-formed `PatientAge` already in the file is kept.
fn age_at_acquisition(source: &DefaultDicomObject) -> Option<String> {
    let birth = read_string(source, tags::PATIENT_BIRTH_DATE);
    let at = ACQUISITION_DATE_TAGS
        .iter()
        .find_map(|tag| read_string(source, *tag));

    if let (Some(birth), Some(at)) = (birth, at) {
        if let Some(age) = dates::age(&birth, &at) {
            return Some(if age.years >= 2 {
                format!("{:03}Y", age.years.min(999))
            } else if age.days >= 8 * 7 {
                format!("{:03}M", age.months)
            } else {
                format!("{:03}W", age.days / 7)
            });
        }
    }

    read_string(source, tags::PATIENT_AGE).filter(|age| is_age_string(age))
}

fn is_age_string(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 4
        && bytes[..3].iter().all(u8::is_ascii_digit)
        && matches!(bytes[3], b'D' | b'W' | b'M' | b'Y')
}

/// What happens to one date element before the anonymizer sees it.
enum DateEdit {
    Leave,
    Replace(PrimitiveValue),
    Remove,
}

fn date_edit(tag: Tag, value: &str, offset: i64) -> DateEdit {
    // Shifted like the others, the birth date would give the exact age back.
    if tag == tags::PATIENT_BIRTH_DATE {
        return DateEdit::Replace(PrimitiveValue::Empty);
    }

    let values: Vec<&str> = value
        .split('\\')
        .map(|part| part.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
        .collect();
    if values.iter().all(|part| part.is_empty()) {
        return DateEdit::Leave;
    }

    values
        .iter()
        .map(|part| dates::shift(part, offset))
        .collect::<Option<Vec<_>>>()
        .map_or(DateEdit::Remove, |shifted| {
            DateEdit::Replace(PrimitiveValue::from(shifted.join("\\")))
        })
}

/// The value of a `UI` element under the patient's UID key, or `None` to leave it.
///
/// What the anonymizer would hash is rewritten, and so is any UID it has no rule
/// for, unless the standard defines it. Elements it explicitly leaves alone are
/// class UIDs.
fn rewritten_uids(value: &str, action: &Action, uid_key: &[u8; 32]) -> Option<String> {
    let hashed = matches!(action, Action::HashUID);
    if !hashed && !matches!(action, Action::Keep) {
        return None;
    }

    let mut changed = false;
    let uids: Vec<String> = value
        .split('\\')
        .map(|part| part.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
        .map(|uid| {
            if uid.is_empty() || (!hashed && uid.starts_with(DICOM_STANDARD_UID_ROOT)) {
                uid.to_string()
            } else {
                changed = true;
                pseudonym::pseudonymize_uid(uid_key, uid, UID_ROOT)
            }
        })
        .collect();

    changed.then(|| uids.join("\\"))
}

/// Settle every date, UID and hashed identifier of `object`, descending into
/// sequences, and collect the tags rewritten into `kept_tags`.
///
/// The anonymizer's own handling of these is not used. Its date shift is keyed
/// on a public hash of `PatientID` and leaves the date untouched when that is
/// missing; any date tag it has no rule for is kept as-is; and a value it cannot
/// parse aborts the whole file. Its UIDs and identifiers (`AccessionNumber`,
/// order numbers) are an unkeyed hash: anyone holding the original can compute
/// the result and link the file back to its source.
///
/// So dates move by the offset, UIDs are rewritten under a key, and identifiers
/// are emptied. A value that is not a date is removed, and so is a tag the
/// anonymizer would hash-shift but that is not encoded as a date (an `UN`
/// element, typically). Private elements are skipped: the anonymizer removes them
/// all, and marking one `Keep` would bring it back.
fn rewrite_elements(
    object: &mut InMemDicomObject,
    keys: &PatientKeys,
    defaults: &Config,
    kept_tags: &mut BTreeSet<Tag>,
) {
    let mut edits: Vec<(Tag, Option<InMemElement>)> = Vec::new();

    for elem in object.iter() {
        let tag = elem.tag();
        if tag.group() % 2 == 1 {
            continue;
        }
        let action = defaults.get_action(&tag);

        match elem.vr() {
            VR::DA | VR::DT => {
                let edit = match elem.to_str() {
                    Ok(value) => date_edit(tag, &value, keys.date_offset_days),
                    Err(_) => DateEdit::Remove,
                };
                match edit {
                    DateEdit::Leave => {}
                    DateEdit::Replace(value) => {
                        kept_tags.insert(tag);
                        edits.push((tag, Some(DataElement::new(tag, elem.vr(), value))));
                    }
                    DateEdit::Remove => edits.push((tag, None)),
                }
            }
            VR::UI => match elem.to_str() {
                Ok(value) => {
                    if let Some(uids) = rewritten_uids(&value, action, &keys.uid_key) {
                        kept_tags.insert(tag);
                        edits.push((
                            tag,
                            Some(DataElement::new(tag, VR::UI, PrimitiveValue::from(uids))),
                        ));
                    }
                }
                Err(_) => edits.push((tag, None)),
            },
            VR::SQ => {
                let Some(items) = elem.items() else {
                    continue;
                };
                let mut items = items.to_vec();
                for item in &mut items {
                    rewrite_elements(item, keys, defaults, kept_tags);
                }
                edits.push((
                    tag,
                    Some(DataElement::new(tag, VR::SQ, DataSetSequence::from(items))),
                ));
            }
            _ => match action {
                Action::HashDate => edits.push((tag, None)),
                Action::Hash { .. } => {
                    kept_tags.insert(tag);
                    edits.push((
                        tag,
                        Some(DataElement::new(tag, elem.vr(), PrimitiveValue::Empty)),
                    ));
                }
                _ => {}
            },
        }
    }

    for (tag, elem) in edits {
        match elem {
            Some(elem) => {
                object.put(elem);
            }
            None => {
                object.remove_element(tag);
            }
        }
    }
}

fn read_string(object: &DefaultDicomObject, tag: Tag) -> Option<String> {
    let value = object.element(tag).ok()?.to_str().ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}
