pub mod dates;
pub mod dicom;
pub use liryc_pseudonym as pseudonym;
pub mod schiller;
pub mod xml;

use wasm_bindgen::prelude::*;

/// Outcome of a deidentification run: the bytes that should be uploaded, plus
/// the detected format and the MIME type to store them under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeidentifiedFile {
    pub bytes: Vec<u8>,
    pub format_name: String,
    pub mime_type: String,
}

#[wasm_bindgen]
pub struct DeidentifyResult {
    inner: DeidentifiedFile,
}

#[wasm_bindgen]
impl DeidentifyResult {
    /// Hands the bytes over: a Holter runs to hundreds of megabytes, too much to
    /// hold twice in WASM memory. A second call returns nothing.
    #[wasm_bindgen(js_name = bytes)]
    pub fn bytes_js(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.inner.bytes)
    }

    #[wasm_bindgen(js_name = formatName)]
    pub fn format_name_js(&self) -> String {
        self.inner.format_name.clone()
    }

    #[wasm_bindgen(js_name = mimeType)]
    pub fn mime_type_js(&self) -> String {
        self.inner.mime_type.clone()
    }
}

/// What deidentifying a patient's files takes from their pseudonym.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatientKeys {
    /// Days added to every date.
    pub date_offset_days: i64,
    /// Keys the rewriting of DICOM UIDs.
    pub uid_key: [u8; 32],
}

#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn deidentify(
    input_bytes: Vec<u8>,
    file_name: String,
    record_id: String,
    patient_name: String,
    date_offset_days: i32,
    uid_key: String,
    enable_dicom: bool,
    enable_xml: bool,
    enable_schiller: bool,
) -> Result<DeidentifyResult, JsValue> {
    // Without a key UIDs could not be rewritten; an empty one is only acceptable
    // when DICOM files are sent as they are.
    let uid_key = if uid_key.is_empty() && !enable_dicom {
        [0; 32]
    } else {
        pseudonym::key_from_hex(&uid_key).map_err(|e| JsValue::from_str(&e.to_string()))?
    };
    deidentify_bytes(
        input_bytes,
        &file_name,
        &record_id,
        &patient_name,
        &PatientKeys {
            date_offset_days: i64::from(date_offset_days),
            uid_key,
        },
        enable_dicom,
        enable_xml,
        enable_schiller,
    )
    .map(|inner| DeidentifyResult { inner })
    .map_err(|e| JsValue::from_str(&e))
}

/// What the browser learns about a file before deidentifying it: what to
/// compute the pseudonym from, and what to show the uploader in the transfer
/// summary. None of it leaves the browser except as a blinded element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inspection {
    pub format_name: String,
    /// Whether the file will be deidentified, so needs the patient's pseudonym.
    pub deidentified: bool,
    /// Normalized, empty when the file carries none.
    pub nip: String,
    /// The NIP as written in the file, for the summary.
    pub raw_nip: String,
    pub family_name: String,
    pub given_name: String,
    /// `12/02/1954`.
    pub birth_date: String,
    /// DICOM only: the series the file belongs to.
    pub series_uid: String,
    pub modality: String,
    pub series_description: String,
}

#[wasm_bindgen]
pub struct InspectResult {
    inner: Inspection,
}

#[wasm_bindgen]
impl InspectResult {
    #[wasm_bindgen(js_name = formatName)]
    pub fn format_name_js(&self) -> String {
        self.inner.format_name.clone()
    }

    #[wasm_bindgen(js_name = deidentified)]
    pub fn deidentified_js(&self) -> bool {
        self.inner.deidentified
    }

    #[wasm_bindgen(js_name = nip)]
    pub fn nip_js(&self) -> String {
        self.inner.nip.clone()
    }

    #[wasm_bindgen(js_name = rawNip)]
    pub fn raw_nip_js(&self) -> String {
        self.inner.raw_nip.clone()
    }

    #[wasm_bindgen(js_name = familyName)]
    pub fn family_name_js(&self) -> String {
        self.inner.family_name.clone()
    }

    #[wasm_bindgen(js_name = givenName)]
    pub fn given_name_js(&self) -> String {
        self.inner.given_name.clone()
    }

    #[wasm_bindgen(js_name = birthDate)]
    pub fn birth_date_js(&self) -> String {
        self.inner.birth_date.clone()
    }

    #[wasm_bindgen(js_name = seriesUid)]
    pub fn series_uid_js(&self) -> String {
        self.inner.series_uid.clone()
    }

    #[wasm_bindgen(js_name = modality)]
    pub fn modality_js(&self) -> String {
        self.inner.modality.clone()
    }

    #[wasm_bindgen(js_name = seriesDescription)]
    pub fn series_description_js(&self) -> String {
        self.inner.series_description.clone()
    }
}

#[wasm_bindgen]
pub fn inspect(
    input_bytes: &[u8],
    file_name: String,
    enable_dicom: bool,
    enable_xml: bool,
    enable_schiller: bool,
) -> Result<InspectResult, JsValue> {
    inspect_bytes(
        input_bytes,
        &file_name,
        enable_dicom,
        enable_xml,
        enable_schiller,
    )
    .map(|inner| InspectResult { inner })
    .map_err(|e| JsValue::from_str(&e))
}

/// Recognize a file the way [`deidentify_bytes`] will, and read who it belongs
/// to. The same `SKIP:` and refusal errors come out, so a file is set aside
/// before any pseudonym is computed for it.
pub fn inspect_bytes(
    input_bytes: &[u8],
    file_name: &str,
    enable_dicom: bool,
    enable_xml: bool,
    enable_schiller: bool,
) -> Result<Inspection, String> {
    let (format_name, deidentified, identity) = if schiller::validate(input_bytes).is_ok() {
        (
            "schiller",
            enable_schiller,
            schiller::identity(input_bytes)?,
        )
    } else {
        match xml::classify(input_bytes, file_name) {
            xml::XmlOutcome::AnnotatedEcg => ("xml", enable_xml, xml::identity(input_bytes)),
            xml::XmlOutcome::Unsupported(reason) => return Err(reason),
            xml::XmlOutcome::NotXml => match dicom::validate(input_bytes) {
                Ok(dicom::DicomKind::Directory) => {
                    return Err(dicom::DICOMDIR_SKIP_REASON.to_string())
                }
                Ok(dicom::DicomKind::Object) => {
                    let identity = dicom::identity(input_bytes)?;
                    match identity.refusal {
                        Some(reason) if enable_dicom => return Err(reason.to_string()),
                        _ => ("dicom", enable_dicom, identity),
                    }
                }
                Err(_) => return Err("SKIP: file is unsupported".to_string()),
            },
        }
    };

    let (family_name, given_name) = identity.name.as_deref().map(split_name).unwrap_or_default();
    let series = identity.series.unwrap_or_default();
    Ok(Inspection {
        format_name: format_name.to_string(),
        deidentified,
        nip: identity
            .nip
            .as_deref()
            .and_then(pseudonym::normalize_nip)
            .unwrap_or_default(),
        raw_nip: identity.nip.unwrap_or_default().trim().to_string(),
        family_name,
        given_name,
        birth_date: identity
            .birth_date
            .as_deref()
            .and_then(display_date)
            .unwrap_or_default(),
        series_uid: series.uid,
        modality: series.modality,
        series_description: series.description,
    })
}

/// `DUPONT^MARIE^^^` → (`DUPONT`, `MARIE`); a name without `^` is all family name.
fn split_name(person_name: &str) -> (String, String) {
    let mut parts = person_name.split('^').map(str::trim);
    let family = parts.next().unwrap_or("").to_string();
    let given = parts.next().unwrap_or("").to_string();
    (family, given)
}

/// `19540212` → `12/02/1954`
fn display_date(value: &str) -> Option<String> {
    let parsed = dates::parse(value)?;
    let compact = dates::render(parsed.days, dates::DateFormat::Compact, "");
    Some(format!(
        "{}/{}/{}",
        &compact[6..8],
        &compact[4..6],
        &compact[0..4]
    ))
}

/// The browser's half of a pseudonym computation, kept between the blinding
/// and the servers' answers. The blind never leaves WASM memory.
#[wasm_bindgen]
pub struct PseudonymSession {
    request: pseudonym::Request,
}

#[wasm_bindgen]
impl PseudonymSession {
    #[wasm_bindgen(constructor)]
    pub fn new(domain: String, nip: String) -> Result<PseudonymSession, JsValue> {
        pseudonym::start(&domain, &nip)
            .map(|request| PseudonymSession { request })
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// For files that carry no NIP: the pseudonym then belongs to the record.
    #[wasm_bindgen(js_name = forRecord)]
    pub fn for_record(domain: String, record_id: String) -> Result<PseudonymSession, JsValue> {
        pseudonym::start_without_nip(&domain, &record_id)
            .map(|request| PseudonymSession { request })
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    #[wasm_bindgen(js_name = blindedElement)]
    pub fn blinded_element(&self) -> String {
        pseudonym::element_to_hex(&self.request.blinded_element())
    }

    /// Verify the server's proof, then derive the pseudonym.
    pub fn finish(
        &self,
        evaluated_element: String,
        proof: String,
        public_key: String,
        record_id: String,
    ) -> Result<PseudonymResult, JsValue> {
        let to_js = |e: pseudonym::PseudonymError| JsValue::from_str(&e.to_string());
        let evaluation = pseudonym::Evaluation::from_hex(&evaluated_element, &proof, &public_key)
            .map_err(to_js)?;
        pseudonym::finish(&self.request, &evaluation, &record_id)
            .map(|inner| PseudonymResult { inner })
            .map_err(to_js)
    }
}

#[wasm_bindgen]
pub struct PseudonymResult {
    inner: pseudonym::Pseudonym,
}

#[wasm_bindgen]
impl PseudonymResult {
    pub fn fingerprint(&self) -> String {
        self.inner.fingerprint.clone()
    }

    #[wasm_bindgen(js_name = dateOffsetDays)]
    pub fn date_offset_days(&self) -> i32 {
        i32::try_from(self.inner.date_offset_days).expect("the offset fits in an i32")
    }

    #[wasm_bindgen(js_name = uidKey)]
    pub fn uid_key(&self) -> String {
        pseudonym::to_hex(&self.inner.uid_key)
    }
}

/// Detect the format of `input_bytes` and deidentify it.
///
/// Formats are probed in order (Schiller, XML ECG, DICOM); the first one that
/// recognizes the input wins. When a format is recognized but its deidentifier
/// is disabled in the project settings, the input is returned untouched: the
/// caller (REDCap) has explicitly opted out of deidentifying that format.
///
/// Unrecognized inputs yield a `SKIP:`-prefixed error, which the browser side
/// treats as "drop this file from the upload" rather than as a failure. So
/// does a DICOMDIR, recognized by its SOP class rather than its name.
///
/// `keys` come from the patient's pseudonym; see [`pseudonym`].
#[allow(clippy::too_many_arguments)]
pub fn deidentify_bytes(
    input_bytes: Vec<u8>,
    file_name: &str,
    record_id: &str,
    patient_name: &str,
    keys: &PatientKeys,
    enable_dicom: bool,
    enable_xml: bool,
    enable_schiller: bool,
) -> Result<DeidentifiedFile, String> {
    let normalized_record_id = if record_id.trim().is_empty() {
        "UNASSIGNED_RECORD".to_string()
    } else {
        record_id.trim().to_string()
    };
    let normalized_patient_name = if patient_name.trim().is_empty() {
        format!("REDCAP_PROJECT^{}", normalized_record_id)
    } else {
        patient_name.trim().to_string()
    };

    if schiller::validate(&input_bytes).is_ok() {
        let bytes = if enable_schiller {
            schiller::deidentify(input_bytes, &normalized_record_id)?
        } else {
            input_bytes
        };

        return Ok(DeidentifiedFile {
            bytes,
            format_name: "schiller".to_string(),
            mime_type: "application/octet-stream".to_string(),
        });
    }

    match xml::classify(&input_bytes, file_name) {
        xml::XmlOutcome::AnnotatedEcg => {
            let bytes = if enable_xml {
                xml::deidentify(
                    &input_bytes,
                    file_name,
                    &normalized_record_id,
                    keys.date_offset_days,
                )?
            } else {
                input_bytes
            };

            return Ok(DeidentifiedFile {
                bytes,
                format_name: "xml".to_string(),
                mime_type: "application/xml".to_string(),
            });
        }
        // An XML ECG in a dialect we cannot certify is refused outright, and the
        // error carries no `SKIP:` prefix so the upload stops instead of
        // quietly dropping the file.
        xml::XmlOutcome::Unsupported(reason) => return Err(reason),
        xml::XmlOutcome::NotXml => {}
    }

    if let Ok(kind) = dicom::validate(&input_bytes) {
        // A DICOMDIR is skipped whether or not DICOM deidentification is
        // enabled: it is the index of the original media, it names every
        // patient on it in clear, and anonymizing it leaves a stale index.
        // See `dicom::DicomKind`.
        if kind == dicom::DicomKind::Directory {
            return Err(dicom::DICOMDIR_SKIP_REASON.to_string());
        }

        let bytes = if enable_dicom {
            dicom::deidentify(
                &input_bytes,
                &normalized_record_id,
                &normalized_patient_name,
                keys,
            )?
        } else {
            input_bytes
        };

        return Ok(DeidentifiedFile {
            bytes,
            format_name: "dicom".to_string(),
            mime_type: "application/dicom".to_string(),
        });
    }

    Err("SKIP: file is unsupported".to_string())
}
