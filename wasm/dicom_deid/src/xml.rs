use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

/// XML ECG dialects the anonymizer knows how to recognize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlType {
    Hl7V2,
    Hl7V3,
    PhilipsEcg,
}

/// Path fragments (lowercased) whose values carry patient or staff identity.
const FIELDS_TO_ANONYMIZE: &[&str] = &[
    "patientid",
    "secondpatientid",
    "firstname",
    "lastname",
    "name",
    "age",
    "bed",
    "room",
    "sexe",
    "sex",
    "pointofcare",
    "patientname",
    "surname",
    "givenname",
    "technician",
    "doctor",
    "operator",
    "clinicaltrialprotocolid",
    "clinicaltrialprotocolname",
    "middlename",
    "viperuniquepatientid",
];

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

pub fn validate(input_bytes: &[u8], file_name: &str) -> Result<(), String> {
    detect_type(input_bytes, file_name)
        .map(|_| ())
        .map_err(|e| e.strip_prefix("SKIP: ").unwrap_or(&e).to_string())
}

pub fn deidentify(input_bytes: &[u8], file_name: &str, record_id: &str) -> Result<Vec<u8>, String> {
    detect_type(input_bytes, file_name)?;
    rewrite(input_bytes, record_id.trim())
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

/// Decide what a node located at `path` should be replaced with.
///
/// `None` means "leave the value untouched". `path` is the `/`-joined element
/// path, with `/@attribute` appended for attributes.
///
/// Public so tests can walk a real recording and check that every value this
/// predicate claims is identifying has actually left the output.
pub fn replacement_for(path: &str, record_id: &str) -> Option<String> {
    let path_lower = path.to_lowercase();
    let last_part = path_lower.rsplit('/').next().unwrap_or("");

    let matches = FIELDS_TO_ANONYMIZE.iter().any(|pattern| {
        if !path_lower.contains(pattern) {
            return false;
        }

        // `...NameExistFlag` style attributes are presence booleans, not
        // identity: blanking them would corrupt the document structure.
        if let Some(attr_name) = last_part.strip_prefix('@') {
            return !attr_name.ends_with("flag");
        }

        true
    });

    if !matches {
        return None;
    }

    if path_lower.contains("patientid") {
        Some(record_id.to_string())
    } else {
        Some(String::new())
    }
}

/// Stream the document back out, replacing identifying values on the fly.
///
/// The transform is event-based rather than map-based on purpose: ECG files
/// repeat the same element path many times (one per lead, per measurement…)
/// with different values, so a path-keyed value map would overwrite every
/// occurrence with the value of the last one.
fn rewrite(input_bytes: &[u8], record_id: &str) -> Result<Vec<u8>, String> {
    let body = strip_bom(input_bytes);
    let mut reader = Reader::from_reader(body);
    let mut output = Vec::new();

    if input_bytes.starts_with(UTF8_BOM) {
        output.extend_from_slice(UTF8_BOM);
    }

    let mut writer = Writer::new(output);
    let mut path: Vec<String> = Vec::new();

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
                let element = anonymize_attributes(&e, &tag, &path, record_id);
                writer
                    .write_event(Event::Start(element))
                    .map_err(|err| format!("SKIP: XML start-element write failed: {err}"))?;
            }
            Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let mut element_path = path.clone();
                element_path.push(tag.clone());
                let element = anonymize_attributes(&e, &tag, &element_path, record_id);
                writer
                    .write_event(Event::Empty(element))
                    .map_err(|err| format!("SKIP: XML empty-element write failed: {err}"))?;
            }
            Ok(Event::Text(e)) => {
                let original = e.unescape().unwrap_or_default().to_string();
                let text = replacement_for(&path.join("/"), record_id).unwrap_or(original);

                if !text.is_empty() {
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
            Ok(Event::Comment(e)) => {
                writer
                    .write_event(Event::Comment(e))
                    .map_err(|err| format!("SKIP: XML comment write failed: {err}"))?;
            }
            Ok(Event::CData(e)) => {
                writer
                    .write_event(Event::CData(e))
                    .map_err(|err| format!("SKIP: XML CDATA write failed: {err}"))?;
            }
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

    Ok(writer.into_inner())
}

fn anonymize_attributes(
    source: &BytesStart<'_>,
    tag: &str,
    path: &[String],
    record_id: &str,
) -> BytesStart<'static> {
    let mut element = BytesStart::new(tag.to_string());

    for attr in source.attributes().flatten() {
        let attr_name = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let key = format!("{}/@{}", path.join("/"), attr_name);
        let value = replacement_for(&key, record_id)
            .unwrap_or_else(|| attr.unescape_value().unwrap_or_default().to_string());
        element.push_attribute((attr_name.as_str(), value.as_str()));
    }

    element
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)
}
