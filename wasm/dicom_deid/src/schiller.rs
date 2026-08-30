use uuid::Uuid;

const SCHILLER_MAGIC_NUMBER: &[u8] = &[
    0x00, 0x55, 0xDA, 0xBA, 0x01, 0x00, 0x63, 0x00, 0x60, 0x43, 0x54, 0x43, 0x41, 0x43, 0x55, 0x10,
];
/// Voice annotations recorded by the technician: wiped wholesale, since they
/// routinely name the patient out loud.
const AUDIO_START: usize = 0x1800;
const AUDIO_END: usize = 0xA1800;
/// Patient demographics block, overwritten with filler.
const CHUNK_START: usize = 0x9FE;
const CHUNK_END: usize = 0xBFF;
const CHECKSUM_OFFSET: usize = 0xBFE;
const CRC_START: usize = 0xA00;
const PATIENT_ID_OFFSET: usize = 0xB2B;
const PATIENT_ID_LENGTH: usize = 28;
const UUID_OFFSET: usize = 0xBD4;
const UUID_LENGTH: usize = 38;
const FILLER_BYTE: u8 = 0xAA;
const XOR_KEY: u8 = 0x8A;

/// Cheap structural check: is this a Schiller Holter container we can rewrite?
///
/// Every offset touched by the transform sits below `AUDIO_END`, so a buffer
/// long enough to hold the audio section is long enough for all of them.
pub fn validate(input_bytes: &[u8]) -> Result<(), String> {
    if input_bytes.len() < SCHILLER_MAGIC_NUMBER.len()
        || &input_bytes[..SCHILLER_MAGIC_NUMBER.len()] != SCHILLER_MAGIC_NUMBER
    {
        return Err("input is not a Schiller Holter file".to_string());
    }

    if input_bytes.len() < AUDIO_END {
        return Err("invalid Schiller file size".to_string());
    }

    Ok(())
}

/// Strip patient identity from a Schiller Holter container.
///
/// `record_id` is written into the patient id field; when it is blank a random
/// numeric id is generated so the field never keeps its original value.
pub fn deidentify(input_bytes: &[u8], record_id: &str) -> Result<Vec<u8>, String> {
    validate(input_bytes).map_err(|e| format!("SKIP: {e}"))?;

    let mut container = Deidentified {
        buffer: input_bytes.to_vec(),
        anonymous_id: record_id.trim().to_string(),
    };
    container.anonymize()?;

    Ok(container.into_bytes())
}

struct Deidentified {
    buffer: Vec<u8>,
    anonymous_id: String,
}

impl Deidentified {
    fn anonymize(&mut self) -> Result<(), String> {
        self.buffer[AUDIO_START..AUDIO_END].fill(0x00);
        self.buffer[CHUNK_START..CHUNK_END].fill(FILLER_BYTE);
        self.anonymize_patient_info()
    }

    fn into_bytes(self) -> Vec<u8> {
        self.buffer
    }

    fn anonymize_patient_info(&mut self) -> Result<(), String> {
        self.buffer[0x09FE] = 0xCC;
        self.buffer[0x09FF] = 0x69;
        self.buffer[0x0A00] = 0x01;
        self.buffer[0x0A01] = 0x01;

        let mut numeric_id = if self.anonymous_id.is_empty() {
            random_numeric_string(PATIENT_ID_LENGTH)
        } else {
            self.anonymous_id.clone()
        };

        if numeric_id.len() > PATIENT_ID_LENGTH {
            numeric_id.truncate(PATIENT_ID_LENGTH);
        }

        write_xor_field(
            &mut self.buffer,
            PATIENT_ID_OFFSET,
            PATIENT_ID_LENGTH,
            &numeric_id,
        );
        self.anonymous_id = numeric_id;

        let new_uuid = generate_braced_uuid();
        self.buffer[UUID_OFFSET..UUID_OFFSET + UUID_LENGTH].copy_from_slice(new_uuid.as_bytes());
        self.buffer[UUID_OFFSET + UUID_LENGTH..UUID_OFFSET + UUID_LENGTH + 4].fill(0x00);

        self.update_checksum()?;
        self.verify_checksum()?;

        Ok(())
    }

    fn update_checksum(&mut self) -> Result<(), String> {
        if self.buffer.len() < CHECKSUM_OFFSET + 2 {
            return Err("invalid Schiller checksum offset".to_string());
        }

        let crc = calculate_crc16_ccitt(&self.buffer[CRC_START..CHECKSUM_OFFSET]);
        let crc_bytes = crc.to_be_bytes();
        self.buffer[CHECKSUM_OFFSET] = crc_bytes[0];
        self.buffer[CHECKSUM_OFFSET + 1] = crc_bytes[1];
        Ok(())
    }

    fn verify_checksum(&self) -> Result<(), String> {
        if self.buffer.len() < CHECKSUM_OFFSET + 2 {
            return Err("invalid Schiller checksum offset".to_string());
        }

        let stored_crc = u16::from_be_bytes([
            self.buffer[CHECKSUM_OFFSET],
            self.buffer[CHECKSUM_OFFSET + 1],
        ]);
        let calculated_crc = calculate_crc16_ccitt(&self.buffer[CRC_START..CHECKSUM_OFFSET]);

        if stored_crc != calculated_crc {
            return Err("invalid Schiller checksum".to_string());
        }

        Ok(())
    }
}

fn write_xor_field(buf: &mut [u8], offset: usize, length: usize, value: &str) {
    let mut encoded = Vec::new();
    for b in value.bytes() {
        encoded.push(b ^ XOR_KEY);
    }

    while encoded.len() < length {
        encoded.push(FILLER_BYTE);
    }

    encoded.truncate(length);
    buf[offset..offset + length].copy_from_slice(&encoded);
}

fn random_numeric_string(len: usize) -> String {
    let uuid_hex: String = Uuid::new_v4()
        .to_string()
        .chars()
        .filter(|c| *c != '-')
        .collect();

    let bytes = uuid_hex.as_bytes();
    let mut out = String::with_capacity(len);
    let mut idx = 0usize;

    while out.len() < len {
        let c = bytes[idx % bytes.len()];
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => (c - b'a') % 10,
            b'A'..=b'F' => (c - b'A') % 10,
            _ => 0,
        };
        out.push(char::from(b'0' + digit));
        idx += 1;
    }

    out
}

fn calculate_crc16_ccitt(data: &[u8]) -> u16 {
    const POLYNOMIAL: u16 = 0x1021;
    let mut crc: u16 = 0xFFFF;

    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ POLYNOMIAL;
            } else {
                crc <<= 1;
            }
        }
    }

    crc.to_be()
}

fn generate_braced_uuid() -> String {
    format!("{{{}}}", Uuid::new_v4().to_string().to_uppercase())
}
