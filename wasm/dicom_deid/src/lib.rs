pub mod dates;
pub mod dicom;
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
    #[wasm_bindgen(js_name = bytes)]
    pub fn bytes_js(&self) -> Vec<u8> {
        self.inner.bytes.clone()
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

#[wasm_bindgen]
pub fn deidentify(
    input_bytes: &[u8],
    file_name: String,
    record_id: String,
    patient_name: String,
    enable_dicom: bool,
    enable_xml: bool,
    enable_schiller: bool,
) -> Result<DeidentifyResult, JsValue> {
    deidentify_bytes(
        input_bytes,
        &file_name,
        &record_id,
        &patient_name,
        enable_dicom,
        enable_xml,
        enable_schiller,
    )
    .map(|inner| DeidentifyResult { inner })
    .map_err(|e| JsValue::from_str(&e))
}

/// Detect the format of `input_bytes` and deidentify it.
///
/// Formats are probed in order (Schiller, XML ECG, DICOM); the first one that
/// recognizes the input wins. When a format is recognized but its deidentifier
/// is disabled in the project settings, the input is returned untouched: the
/// caller (REDCap) has explicitly opted out of deidentifying that format.
///
/// Unrecognized inputs yield a `SKIP:`-prefixed error, which the browser side
/// treats as "drop this file from the upload" rather than as a failure.
pub fn deidentify_bytes(
    input_bytes: &[u8],
    file_name: &str,
    record_id: &str,
    patient_name: &str,
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

    if schiller::validate(input_bytes).is_ok() {
        let bytes = if enable_schiller {
            schiller::deidentify(input_bytes, &normalized_record_id)?
        } else {
            input_bytes.to_vec()
        };

        return Ok(DeidentifiedFile {
            bytes,
            format_name: "schiller".to_string(),
            mime_type: "application/octet-stream".to_string(),
        });
    }

    if xml::validate(input_bytes, file_name).is_ok() {
        let bytes = if enable_xml {
            xml::deidentify(input_bytes, file_name, &normalized_record_id)?
        } else {
            input_bytes.to_vec()
        };

        return Ok(DeidentifiedFile {
            bytes,
            format_name: "xml".to_string(),
            mime_type: "application/xml".to_string(),
        });
    }

    if dicom::validate(input_bytes).is_ok() {
        let bytes = if enable_dicom {
            dicom::deidentify(input_bytes, &normalized_record_id, &normalized_patient_name)?
        } else {
            input_bytes.to_vec()
        };

        return Ok(DeidentifiedFile {
            bytes,
            format_name: "dicom".to_string(),
            mime_type: "application/dicom".to_string(),
        });
    }

    Err("SKIP: file is unsupported".to_string())
}
