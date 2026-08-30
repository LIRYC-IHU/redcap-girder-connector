use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

use crate::dates;

/// XML ECG dialects the anonymizer knows how to recognize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlType {
    Hl7V2,
    Hl7V3,
    PhilipsEcg,
}

/// UID root registered for IHU Liryc, shared with the DICOM deidentifier.
const LIRYC_OID: &str = "1.2.826.0.1.3680043.10.543";

/// Arcs under [`LIRYC_OID`], one per kind of entity, so that the aECG, the
/// series, the subject and the trial keep distinct UIDs.
const ARC_DOCUMENT: &str = "1";
const ARC_SERIES: &str = "2";
const ARC_SUBJECT: &str = "3";
const ARC_TRIAL: &str = "4";

/// The UID root written in place of an original one.
///
/// Identifiers come from the REDCap context, exactly as they do on the DICOM
/// side: a root under the institution's registered OID, and the record id in the
/// extension. Nothing is minted at random, so deidentifying the same recording
/// twice yields the same identifiers.
fn uid_root_for(path: &str) -> String {
    let path = path.to_lowercase();
    let arc = if path.contains("trialsubject/id") {
        ARC_SUBJECT
    } else if path.contains("clinicaltrial") || path.contains("trialsite") {
        ARC_TRIAL
    } else if path.contains("series/id") {
        ARC_SERIES
    } else {
        ARC_DOCUMENT
    };

    format!("{LIRYC_OID}.{arc}")
}

/// What happens to the value carried by one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// A patient identifier: replaced by the REDCap record id.
    RecordId,
    /// The birth date: pinned to 1970-01-01, and it sets the shift for the rest.
    BirthDate,
    /// A UID root: replaced by an arc of the Liryc root, chosen by entity kind.
    UidRoot,
    /// Known, non-identifying content: kept (dates are shifted).
    Keep,
    /// An optional attribute of unknown content: removed entirely, because an
    /// emptied attribute would break the datatype's pattern.
    Drop,
    /// Element text of unknown content: emptied, keeping the element in place.
    Blank,
}

/// Node keys that are kept verbatim.
///
/// **This is an allowlist: anything not named here is dropped or emptied.** ECG
/// XML is vendor-extensible, so a denylist cannot be complete — an acquisition
/// timestamp hidden in an instance OID, a device serial number, an investigator
/// id and a free-text interpretation all reached the output while this module
/// worked the other way around.
///
/// Keys are matched against the last segment of a node's path and against the
/// last two joined, so nesting differences between vendors do not matter.
///
/// The HL7 attributes and the waveform elements below are those the aECG
/// implementation guide shows in its minimal valid document (Appendix D);
/// removing any of them produces a file that no longer validates.
const ALLOWED_KEYS: &[&str] = &[
    // -- document structure --------------------------------------------------
    "@xsi:schemalocation",
    "@xsi:type", // SLIST_PQ / GLIST_TS: discriminates the sequence datatype
    "@classcode",
    "@moodcode",
    "@typecode",
    "@determinercode",
    "@contextconductionind",
    "@negationind",
    "@representation",
    "@inclusive",
    "@type",
    "@version",
    // -- coded vocabulary ----------------------------------------------------
    "@code",
    "@codesystem",
    "@codesystemname",
    "@displayname",
    // -- measurements, their units and the time base -------------------------
    "@unit",
    "value/@value",
    "origin/@value",
    "scale/@value",
    "increment/@value",
    "head/@value",
    "low/@value",
    "high/@value",
    "center/@value",
    "width/@value",
    "effectivetime/@value",
    "activitytime/@value",
    "time/@value",
    // -- the signal itself ---------------------------------------------------
    "digits",
    "sequence/value",
    // -- acquisition context that carries no identity ------------------------
    "paced",
    "sex",
    "manufacturermodelname",
    "softwarename",
    // -- Philips `restingecgdata`; not yet checked against a real recording ---
    "amplitude",
    "duration",
    "interval",
    "leadname",
    "leadlabel",
    "parsedwaveforms",
    "waveform",
];

/// Node keys whose presence means the signal survived the allowlist.
///
/// If none is kept, the dialect is one the allowlist does not cover and the
/// output would be an empty recording; the file is rejected rather than
/// uploaded gutted.
const SIGNAL_KEYS: &[&str] = &[
    "digits",
    "sequence/value",
    "parsedwaveforms",
    "waveform",
    "amplitude",
    "value/@value",
];

/// Identity that a generic allowlist rule would otherwise let through.
///
/// Checked before the allowlist and matched anywhere in the path: `@code` has
/// to be allowed broadly (lead codes, annotation codes, control variables,
/// sex), which would otherwise carry `raceCode/@code` with it.
const DENIED_KEYS: &[&str] = &["racecode", "ethnicgroupcode"];

/// Path fragments (lowercased) naming a patient identifier.
const PATIENT_ID_FIELDS: &[&str] = &["patientid", "secondpatientid", "viperuniquepatientid"];

/// Path fragments (lowercased) holding the patient's birth date.
const BIRTH_DATE_FIELDS: &[&str] = &["birthtime", "birthdate", "dateofbirth"];

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

pub fn validate(input_bytes: &[u8], file_name: &str) -> Result<(), String> {
    detect_type(input_bytes, file_name)
        .map(|_| ())
        .map_err(|e| e.strip_prefix("SKIP: ").unwrap_or(&e).to_string())
}

pub fn deidentify(input_bytes: &[u8], file_name: &str, record_id: &str) -> Result<Vec<u8>, String> {
    detect_type(input_bytes, file_name)?;
    let offset = find_birth_date(input_bytes)
        .as_deref()
        .and_then(dates::offset_from_birth_date);
    rewrite(input_bytes, record_id.trim(), offset)
}

/// Recognize the ECG dialect from the root element name and its `xmlns`.
pub fn detect_type(input_bytes: &[u8], file_name: &str) -> Result<XmlType, String> {
    if !file_name.to_lowercase().ends_with(".xml") {
        return Err("SKIP: input is not an XML ECG file".to_string());
    }

    let mut reader = Reader::from_reader(strip_bom(input_bytes));

    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("SKIP: XML parsing failed: {e}"))?;

        let root = match event {
            Event::Start(ref e) => e.to_owned(),
            Event::Empty(ref e) => e.to_owned(),
            Event::Eof => break,
            _ => continue,
        };

        let name = String::from_utf8_lossy(root.name().as_ref()).to_string();
        let namespace = root
            .attributes()
            .flatten()
            .find(|attr| attr.key.as_ref() == b"xmlns")
            .map(|attr| attr.unescape_value().unwrap_or_default().to_string())
            .unwrap_or_default();

        // Only the root element is inspected: a nested `AnnotatedECG` is not a
        // document we know how to anonymize.
        return match (name.as_str(), namespace.as_str()) {
            ("AnnotatedECG", "urn:hl7-org:v2") => Ok(XmlType::Hl7V2),
            ("AnnotatedECG", "urn:hl7-org:v3") => Ok(XmlType::Hl7V3),
            ("restingecgdata", ns) if ns.contains("medical.philips.com") => Ok(XmlType::PhilipsEcg),
            _ => Err("SKIP: unsupported XML ECG format".to_string()),
        };
    }

    Err("SKIP: unsupported XML ECG format".to_string())
}

pub fn is_birth_date_path(path: &str) -> bool {
    let path = path.to_lowercase();
    BIRTH_DATE_FIELDS.iter().any(|field| path.contains(field))
}

fn is_patient_id_path(path: &str) -> bool {
    let path = path.to_lowercase();
    PATIENT_ID_FIELDS.iter().any(|field| path.contains(field))
}

/// The keys a node's path is matched against: its last segment, and its last
/// two joined.
fn node_keys(path: &str) -> (String, String) {
    let path = path.to_lowercase();
    let mut segments = path.rsplit('/');
    let last = segments.next().unwrap_or("").to_string();
    let parent = segments.next().unwrap_or("");
    let pair = if parent.is_empty() {
        last.clone()
    } else {
        format!("{parent}/{last}")
    };

    (last, pair)
}

/// Decide what happens to the node at `path`.
///
/// `id` elements get particular care: the aECG schema requires a `root` on the
/// document, the subject and the trial, and `root` is typed as an OID or UUID —
/// emptying it produces a document that no longer validates. Roots are
/// therefore replaced rather than blanked, and the subject's `extension`, which
/// the standard reserves for "the traditional identifier", receives the REDCap
/// record id.
pub fn policy_for(path: &str) -> Policy {
    let lowered = path.to_lowercase();
    let (last, pair) = node_keys(&lowered);
    let is_attribute = last.starts_with('@');

    if pair == "id/@root" {
        return Policy::UidRoot;
    }
    if pair == "id/@extension" {
        // The standard reserves the extension for "the traditional identifier";
        // for us that is the REDCap record. Trial and site identifiers are site
        // information and go away entirely.
        return if lowered.contains("clinicaltrial") || lowered.contains("trialsite") {
            Policy::Drop
        } else {
            Policy::RecordId
        };
    }

    if is_patient_id_path(&lowered) {
        return Policy::RecordId;
    }
    if is_birth_date_path(&lowered) {
        return Policy::BirthDate;
    }

    // Namespace declarations, whatever their prefix.
    if last.starts_with("@xmlns") {
        return Policy::Keep;
    }

    if DENIED_KEYS.iter().any(|denied| lowered.contains(denied)) {
        return if is_attribute {
            Policy::Drop
        } else {
            Policy::Blank
        };
    }
    // `…ExistFlag` attributes are presence booleans; Philips readers reject the
    // document without them.
    if is_attribute && last.ends_with("flag") {
        return Policy::Keep;
    }

    if ALLOWED_KEYS.contains(&last.as_str()) || ALLOWED_KEYS.contains(&pair.as_str()) {
        return Policy::Keep;
    }

    if is_attribute {
        Policy::Drop
    } else {
        Policy::Blank
    }
}

/// Whether a kept node counts as signal, for the "did anything survive" check.
fn is_signal(path: &str) -> bool {
    let (last, pair) = node_keys(path);
    SIGNAL_KEYS.contains(&last.as_str()) || SIGNAL_KEYS.contains(&pair.as_str())
}

/// Read the patient's birth date, which sets the offset for every other date.
///
/// A separate pass because the rewriter is a single-pass stream and the offset
/// has to be known before the first date is written.
fn find_birth_date(input_bytes: &[u8]) -> Option<String> {
    let mut reader = Reader::from_reader(strip_bom(input_bytes));
    let mut path: Vec<String> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                path.push(String::from_utf8_lossy(e.name().as_ref()).to_string());
                if let Some(found) = birth_date_attribute(&e, &path) {
                    return Some(found);
                }
            }
            Ok(Event::Empty(e)) => {
                path.push(String::from_utf8_lossy(e.name().as_ref()).to_string());
                let found = birth_date_attribute(&e, &path);
                path.pop();
                if found.is_some() {
                    return found;
                }
            }
            Ok(Event::Text(e)) => {
                if is_birth_date_path(&path.join("/")) {
                    let text = e.unescape().unwrap_or_default().trim().to_string();
                    if dates::parse(&text).is_some() {
                        return Some(text);
                    }
                }
            }
            Ok(Event::End(_)) => {
                path.pop();
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    None
}

/// A birth date carried by one of an element's attributes, if any.
fn birth_date_attribute(element: &BytesStart<'_>, path: &[String]) -> Option<String> {
    for attr in element.attributes().flatten() {
        let key = format!(
            "{}/@{}",
            path.join("/"),
            String::from_utf8_lossy(attr.key.as_ref())
        );
        if !is_birth_date_path(&key) {
            continue;
        }
        let value = attr.unescape_value().unwrap_or_default().trim().to_string();
        if dates::parse(&value).is_some() {
            return Some(value);
        }
    }

    None
}

/// Everything the rewriter needs that is constant for one document.
struct Rewriter {
    record_id: String,
    date_offset: Option<i64>,
    signal_kept: usize,
}

impl Rewriter {
    /// The value to write for one node; `None` removes the attribute.
    fn value_for(&self, path: &str, original: &str) -> Option<String> {
        let trimmed = original.trim();

        Some(match policy_for(path) {
            Policy::RecordId => self.record_id.clone(),
            Policy::UidRoot => uid_root_for(path),
            Policy::Drop => return None,
            Policy::Blank => String::new(),
            Policy::BirthDate => {
                if dates::parse(trimmed).is_some() {
                    dates::as_epoch_birth_date(trimmed)
                } else {
                    String::new()
                }
            }
            Policy::Keep => match (dates::parse(trimmed), self.date_offset) {
                // A date on a kept path still moves with the patient's offset.
                (Some(_), Some(offset)) => {
                    dates::shift(trimmed, offset).unwrap_or_else(|| original.to_string())
                }
                // A date with no birth date to anchor it: there is no age to
                // preserve, so keeping it would keep the real acquisition date.
                (Some(_), None) => String::new(),
                (None, _) => original.to_string(),
            },
        })
    }

    fn attributes(
        &mut self,
        source: &BytesStart<'_>,
        tag: &str,
        path: &[String],
    ) -> BytesStart<'static> {
        let mut element = BytesStart::new(tag.to_string());

        for attr in source.attributes().flatten() {
            let attr_name = String::from_utf8_lossy(attr.key.as_ref()).to_string();
            let key = format!("{}/@{}", path.join("/"), attr_name);
            let original = attr.unescape_value().unwrap_or_default().to_string();
            let Some(value) = self.value_for(&key, &original) else {
                continue;
            };
            if !value.is_empty() && is_signal(&key) {
                self.signal_kept += 1;
            }
            element.push_attribute((attr_name.as_str(), value.as_str()));
        }

        element
    }
}

/// Stream the document back out, keeping only allowlisted content.
///
/// The transform is event-based rather than map-based on purpose: ECG files
/// repeat the same element path many times (one per lead, per measurement…)
/// with different values, so a path-keyed value map would overwrite every
/// occurrence with the value of the last one.
fn rewrite(
    input_bytes: &[u8],
    record_id: &str,
    date_offset: Option<i64>,
) -> Result<Vec<u8>, String> {
    let body = strip_bom(input_bytes);
    let mut reader = Reader::from_reader(body);
    let mut output = Vec::new();

    if input_bytes.starts_with(UTF8_BOM) {
        output.extend_from_slice(UTF8_BOM);
    }

    let mut writer = Writer::new(output);
    let mut path: Vec<String> = Vec::new();
    let mut state = Rewriter {
        record_id: record_id.to_string(),
        date_offset,
        signal_kept: 0,
    };

    loop {
        match reader.read_event() {
            Ok(Event::Decl(e)) => {
                writer
                    .write_event(Event::Decl(e))
                    .map_err(|err| format!("SKIP: XML declaration write failed: {err}"))?;
            }
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                path.push(tag.clone());
                let element = state.attributes(&e, &tag, &path);
                writer
                    .write_event(Event::Start(element))
                    .map_err(|err| format!("SKIP: XML start-element write failed: {err}"))?;
            }
            Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let mut element_path = path.clone();
                element_path.push(tag.clone());
                let element = state.attributes(&e, &tag, &element_path);
                writer
                    .write_event(Event::Empty(element))
                    .map_err(|err| format!("SKIP: XML empty-element write failed: {err}"))?;
            }
            Ok(Event::Text(e)) => {
                let original = e.unescape().unwrap_or_default().to_string();
                let key = path.join("/");
                let text = state.value_for(&key, &original).unwrap_or_default();

                if !text.is_empty() {
                    if is_signal(&key) {
                        state.signal_kept += 1;
                    }
                    writer
                        .write_event(Event::Text(BytesText::new(&text)))
                        .map_err(|err| format!("SKIP: XML text write failed: {err}"))?;
                }
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                writer
                    .write_event(Event::End(BytesEnd::new(tag)))
                    .map_err(|err| format!("SKIP: XML end-element write failed: {err}"))?;
                path.pop();
            }
            // Comments and CDATA are free text with no schema behind them.
            Ok(Event::Comment(_)) | Ok(Event::CData(_)) => {}
            Ok(Event::DocType(e)) => {
                writer
                    .write_event(Event::DocType(e))
                    .map_err(|err| format!("SKIP: XML doctype write failed: {err}"))?;
            }
            Ok(Event::PI(e)) => {
                writer.write_event(Event::PI(e)).map_err(|err| {
                    format!("SKIP: XML processing-instruction write failed: {err}")
                })?;
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(format!("SKIP: XML serialization failed: {err}")),
        }
    }

    if state.signal_kept == 0 {
        // Not `SKIP:`: a silently emptied recording is worse than a visible
        // failure, so this aborts the upload instead of dropping the file.
        return Err(
            "XML ECG deidentification kept no signal data - this dialect is not covered by \
             the allowlist. Report the file so it can be added."
                .to_string(),
        );
    }

    Ok(writer.into_inner())
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)
}

/// The OID this module mints identifiers under, shared with the DICOM path.
pub fn uid_root() -> &'static str {
    LIRYC_OID
}
