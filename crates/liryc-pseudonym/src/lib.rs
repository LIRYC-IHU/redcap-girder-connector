//! Pseudonym of a patient: a short fingerprint of the NIP for quality control, a
//! date offset, and a key that rewrites the DICOM UIDs of the record, all derived
//! from a verifiable OPRF.
//!
//! The browser blinds the NIP, REDCap evaluates it with its key and proves it
//! used the key it publishes. The NIP never reaches the server and the key never
//! leaves it. See `docs/design/pseudonymisation.md` and `protocol/README.md`.
//!
//! The primitive is RFC 9497, ciphersuite ristretto255-SHA512, VOPRF mode. The
//! server half lives here too, for the tests: REDCap implements it in PHP on
//! libsodium and checks itself against the vectors this crate produces.

use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};

/// `"OPRFV1-" ‖ I2OSP(mode = VOPRF, 1) ‖ "-" ‖ identifier`.
const CONTEXT_STRING: &[u8] = b"OPRFV1-\x01-ristretto255-SHA512";

const INPUT_PREFIX: &[u8] = b"liryc-pseudonym-v1";
const FINGERPRINT_LABEL: &[u8] = b"liryc-pseudonym-fingerprint-v1";
const DATE_OFFSET_LABEL: &[u8] = b"liryc-pseudonym-date-offset-v1";
const UID_KEY_LABEL: &[u8] = b"liryc-pseudonym-uid-key-v1";

/// Dates move back by 50 to 100 years: far enough to land in the 20th century,
/// where they are plainly not real, and to hide the era, the season and the
/// weekday of every examination.
const MIN_OFFSET_DAYS: i64 = 18_262;
const OFFSET_SPAN_DAYS: u64 = 18_263;

/// One byte, 256 values: it tells apart two patients 255 times out of 256, which
/// is what catching an upload on the wrong record takes, and identifies nobody:
/// even with the keys, millions of NIPs share each value.
const FINGERPRINT_BYTES: usize = 1;

/// Bits of a rewritten UID: a UID is 64 characters at most, and the Liryc root
/// plus its dot leave 37 digits.
const UID_BYTES: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PseudonymError {
    /// The file carries no usable patient identifier.
    MissingNip,
    /// An element, scalar or proof that is malformed or not on the group.
    Malformed(&'static str),
    /// The server's proof does not hold: it did not use the key it publishes.
    InvalidProof,
    /// The platform could not supply randomness.
    Randomness,
}

impl std::fmt::Display for PseudonymError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingNip => write!(f, "the file carries no patient identifier (NIP)"),
            Self::Malformed(what) => write!(f, "malformed {what}"),
            Self::InvalidProof => write!(
                f,
                "the server's proof does not verify: it did not evaluate with the key it publishes"
            ),
            Self::Randomness => write!(f, "no randomness available"),
        }
    }
}

/// The pseudonym of one patient within one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pseudonym {
    /// Two hex characters: stored by REDCap and compared across uploads.
    pub fingerprint: String,
    /// Days added to every date of the patient's files.
    pub date_offset_days: i64,
    /// Keys the rewriting of the record's DICOM UIDs; see [`pseudonymize_uid`].
    pub uid_key: [u8; 32],
}

/// The server's answer: the evaluated element, its proof, and the public key it
/// claims to have used.
#[derive(Debug, Clone)]
pub struct Evaluation {
    pub element: RistrettoPoint,
    pub proof: Proof,
    pub public_key: RistrettoPoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proof {
    pub c: Scalar,
    pub s: Scalar,
}

/// The browser's state between blinding and finalization. The blind never
/// leaves it.
pub struct Request {
    input: Vec<u8>,
    blind: Scalar,
    blinded: RistrettoPoint,
}

impl Request {
    pub fn blinded_element(&self) -> RistrettoPoint {
        self.blinded
    }
}

// --- NIP --------------------------------------------------------------------

/// Spaces and case are noise; leading zeros of a purely numeric NIP are an
/// artefact of how some systems pad it. An empty result means no NIP.
pub fn normalize_nip(nip: &str) -> Option<String> {
    let compact: String = nip
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\0')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let normalized = if !compact.is_empty() && compact.bytes().all(|b| b.is_ascii_digit()) {
        compact.trim_start_matches('0').to_string()
    } else {
        compact
    };
    (!normalized.is_empty()).then_some(normalized)
}

/// `"liryc-pseudonym-v1" ‖ 0x00 ‖ domain ‖ 0x00 ‖ nip`.
pub fn input_for(domain: &str, normalized_nip: &str) -> Vec<u8> {
    let mut input =
        Vec::with_capacity(INPUT_PREFIX.len() + domain.len() + normalized_nip.len() + 2);
    input.extend_from_slice(INPUT_PREFIX);
    input.push(0);
    input.extend_from_slice(domain.as_bytes());
    input.push(0);
    input.extend_from_slice(normalized_nip.as_bytes());
    input
}

// --- Client -----------------------------------------------------------------

/// Blind a NIP with fresh randomness.
pub fn start(domain: &str, nip: &str) -> Result<Request, PseudonymError> {
    let nip = normalize_nip(nip).ok_or(PseudonymError::MissingNip)?;
    let blind = random_scalar()?;
    Ok(start_with_blind(input_for(domain, &nip), blind))
}

/// Blind with a given scalar; only the tests choose it.
pub fn start_with_blind(input: Vec<u8>, blind: Scalar) -> Request {
    let blinded = hash_to_group(&input) * blind;
    Request {
        input,
        blind,
        blinded,
    }
}

/// Check the proof, unblind, and derive the pseudonym.
pub fn finish(
    request: &Request,
    evaluation: &Evaluation,
    record_id: &str,
) -> Result<Pseudonym, PseudonymError> {
    if !verify_proof(
        &evaluation.public_key,
        &request.blinded,
        &evaluation.element,
        &evaluation.proof,
    ) {
        return Err(PseudonymError::InvalidProof);
    }
    let output = finalize(&request.input, &request.blind, &evaluation.element);
    Ok(derive(&output, record_id))
}

/// RFC 9497 `Finalize`: unblind and hash with the input.
pub fn finalize(input: &[u8], blind: &Scalar, evaluated: &RistrettoPoint) -> [u8; 64] {
    let unblinded = evaluated * blind.invert();
    let mut hasher = Sha512::new();
    hasher.update(i2osp2(input.len()));
    hasher.update(input);
    hasher.update(i2osp2(32));
    hasher.update(unblinded.compress().as_bytes());
    hasher.update(b"Finalize");
    hasher.finalize().into()
}

/// Independent values from one OPRF output: the fingerprint REDCap stores cannot
/// be turned into the offset or the UID key, which never leave the browser. The
/// offset belongs to the patient, so their dates stay on one calendar; the UID
/// key also depends on the record, so their records cannot be linked through
/// UIDs.
pub fn derive(output: &[u8; 64], record_id: &str) -> Pseudonym {
    let labelled = |label: &[u8]| Sha512::new().chain_update(label).chain_update(output);

    let fingerprint = labelled(FINGERPRINT_LABEL).finalize();
    let seed = labelled(DATE_OFFSET_LABEL).finalize();
    let seed = u64::from_be_bytes(seed[..8].try_into().expect("8 bytes"));
    let uid_key = labelled(UID_KEY_LABEL)
        .chain_update(record_id.trim().as_bytes())
        .finalize();

    Pseudonym {
        fingerprint: to_hex(&fingerprint[..FINGERPRINT_BYTES]),
        date_offset_days: -(MIN_OFFSET_DAYS + (seed % OFFSET_SPAN_DAYS) as i64),
        uid_key: uid_key[..32].try_into().expect("32 bytes"),
    }
}

/// `{root}.{120 bits of HMAC-SHA256(uid_key, uid)}`: the same UID always maps to
/// the same one within a record, so series stay grouped and references keep
/// pointing at their targets, and the original cannot be linked to it without
/// the key.
pub fn pseudonymize_uid(uid_key: &[u8; 32], uid: &str, root: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(uid_key).expect("HMAC takes any key length");
    mac.update(uid.as_bytes());
    let digest = mac.finalize().into_bytes();

    let mut value = [0u8; 16];
    value[16 - UID_BYTES..].copy_from_slice(&digest[..UID_BYTES]);
    format!("{root}.{}", u128::from_be_bytes(value))
}

// --- Server -----------------------------------------------------------------

/// RFC 9497 `BlindEvaluate` with its proof, for a given proof randomness.
pub fn blind_evaluate(
    key: &Scalar,
    blinded: &RistrettoPoint,
    proof_random_scalar: &Scalar,
) -> (RistrettoPoint, Proof) {
    let evaluated = blinded * key;
    let public_key = RISTRETTO_BASEPOINT_POINT * key;
    let proof = generate_proof(key, &public_key, blinded, &evaluated, proof_random_scalar);
    (evaluated, proof)
}

pub fn public_key(key: &Scalar) -> RistrettoPoint {
    RISTRETTO_BASEPOINT_POINT * key
}

fn generate_proof(
    key: &Scalar,
    public_key: &RistrettoPoint,
    blinded: &RistrettoPoint,
    evaluated: &RistrettoPoint,
    r: &Scalar,
) -> Proof {
    let (m, _) = compute_composites(public_key, blinded, evaluated);
    let z = m * key;
    let t2 = RISTRETTO_BASEPOINT_POINT * r;
    let t3 = m * r;
    let c = challenge(public_key, &m, &z, &t2, &t3);
    Proof { c, s: r - c * key }
}

// --- Proofs -----------------------------------------------------------------

/// RFC 9497 `VerifyProof` for a batch of one, with `A` the generator.
pub fn verify_proof(
    public_key: &RistrettoPoint,
    blinded: &RistrettoPoint,
    evaluated: &RistrettoPoint,
    proof: &Proof,
) -> bool {
    let (m, z) = compute_composites(public_key, blinded, evaluated);
    let t2 = RISTRETTO_BASEPOINT_POINT * proof.s + public_key * proof.c;
    let t3 = m * proof.s + z * proof.c;
    challenge(public_key, &m, &z, &t2, &t3) == proof.c
}

/// `ComputeComposites` for one element pair. The server's fast variant computes
/// `Z = k·M`; with a single pair, `d·D` is the same point.
fn compute_composites(
    public_key: &RistrettoPoint,
    c: &RistrettoPoint,
    d: &RistrettoPoint,
) -> (RistrettoPoint, RistrettoPoint) {
    let bm = public_key.compress();
    let mut seed_dst = b"Seed-".to_vec();
    seed_dst.extend_from_slice(CONTEXT_STRING);
    let seed = Sha512::new()
        .chain_update(i2osp2(32))
        .chain_update(bm.as_bytes())
        .chain_update(i2osp2(seed_dst.len()))
        .chain_update(&seed_dst)
        .finalize();

    let mut transcript = Vec::with_capacity(64 + 2 + 2 + 2 + 32 + 2 + 32 + 9);
    transcript.extend_from_slice(&i2osp2(seed.len()));
    transcript.extend_from_slice(&seed);
    transcript.extend_from_slice(&i2osp2(0));
    transcript.extend_from_slice(&i2osp2(32));
    transcript.extend_from_slice(c.compress().as_bytes());
    transcript.extend_from_slice(&i2osp2(32));
    transcript.extend_from_slice(d.compress().as_bytes());
    transcript.extend_from_slice(b"Composite");
    let di = hash_to_scalar(&transcript);

    (c * di, d * di)
}

fn challenge(
    public_key: &RistrettoPoint,
    m: &RistrettoPoint,
    z: &RistrettoPoint,
    t2: &RistrettoPoint,
    t3: &RistrettoPoint,
) -> Scalar {
    let mut transcript = Vec::with_capacity(5 * 34 + 9);
    for point in [public_key, m, z, t2, t3] {
        transcript.extend_from_slice(&i2osp2(32));
        transcript.extend_from_slice(point.compress().as_bytes());
    }
    transcript.extend_from_slice(b"Challenge");
    hash_to_scalar(&transcript)
}

// --- Hashing ----------------------------------------------------------------

pub fn hash_to_group(input: &[u8]) -> RistrettoPoint {
    let mut dst = b"HashToGroup-".to_vec();
    dst.extend_from_slice(CONTEXT_STRING);
    let uniform: [u8; 64] = expand_message_xmd(input, &dst);
    RistrettoPoint::from_uniform_bytes(&uniform)
}

fn hash_to_scalar(input: &[u8]) -> Scalar {
    let mut dst = b"HashToScalar-".to_vec();
    dst.extend_from_slice(CONTEXT_STRING);
    let uniform: [u8; 64] = expand_message_xmd(input, &dst);
    Scalar::from_bytes_mod_order_wide(&uniform)
}

/// RFC 9380 §5.3.1 with SHA-512, for an output of exactly one block (64 bytes),
/// which is all the ciphersuite asks for.
fn expand_message_xmd(msg: &[u8], dst: &[u8]) -> [u8; 64] {
    assert!(dst.len() <= 255, "DST too long");
    let mut dst_prime = dst.to_vec();
    dst_prime.push(dst.len() as u8);

    let b0 = Sha512::new()
        .chain_update([0u8; 128])
        .chain_update(msg)
        .chain_update(i2osp2(64))
        .chain_update([0u8])
        .chain_update(&dst_prime)
        .finalize();
    let b1 = Sha512::new()
        .chain_update(b0)
        .chain_update([1u8])
        .chain_update(&dst_prime)
        .finalize();
    b1.into()
}

fn i2osp2(n: usize) -> [u8; 2] {
    u16::try_from(n)
        .expect("length fits in two bytes")
        .to_be_bytes()
}

fn random_scalar() -> Result<Scalar, PseudonymError> {
    loop {
        let mut bytes = [0u8; 64];
        getrandom::fill(&mut bytes).map_err(|_| PseudonymError::Randomness)?;
        let scalar = Scalar::from_bytes_mod_order_wide(&bytes);
        if scalar != Scalar::ZERO {
            return Ok(scalar);
        }
    }
}

// --- Encoding ---------------------------------------------------------------

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex<const N: usize>(hex: &str, what: &'static str) -> Result<[u8; N], PseudonymError> {
    let hex = hex.trim();
    if hex.len() != 2 * N || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PseudonymError::Malformed(what));
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
            .map_err(|_| PseudonymError::Malformed(what))?;
    }
    Ok(out)
}

/// A group element other than the identity.
pub fn element_from_hex(hex: &str, what: &'static str) -> Result<RistrettoPoint, PseudonymError> {
    let point = CompressedRistretto(from_hex::<32>(hex, what)?)
        .decompress()
        .ok_or(PseudonymError::Malformed(what))?;
    if point == RistrettoPoint::identity() {
        return Err(PseudonymError::Malformed(what));
    }
    Ok(point)
}

pub fn element_to_hex(point: &RistrettoPoint) -> String {
    to_hex(point.compress().as_bytes())
}

/// A key as handed around in hex, such as [`Pseudonym::uid_key`].
pub fn key_from_hex(hex: &str) -> Result<[u8; 32], PseudonymError> {
    from_hex::<32>(hex, "key")
}

/// A canonical scalar.
pub fn scalar_from_hex(hex: &str, what: &'static str) -> Result<Scalar, PseudonymError> {
    Option::from(Scalar::from_canonical_bytes(from_hex::<32>(hex, what)?))
        .ok_or(PseudonymError::Malformed(what))
}

pub fn proof_from_hex(hex: &str) -> Result<Proof, PseudonymError> {
    let bytes = from_hex::<64>(hex, "proof")?;
    let c = Option::from(Scalar::from_canonical_bytes(
        bytes[..32].try_into().unwrap(),
    ))
    .ok_or(PseudonymError::Malformed("proof"))?;
    let s = Option::from(Scalar::from_canonical_bytes(
        bytes[32..].try_into().unwrap(),
    ))
    .ok_or(PseudonymError::Malformed("proof"))?;
    Ok(Proof { c, s })
}

pub fn proof_to_hex(proof: &Proof) -> String {
    let mut bytes = proof.c.to_bytes().to_vec();
    bytes.extend_from_slice(&proof.s.to_bytes());
    to_hex(&bytes)
}

impl Evaluation {
    pub fn from_hex(element: &str, proof: &str, public_key: &str) -> Result<Self, PseudonymError> {
        Ok(Self {
            element: element_from_hex(element, "evaluated element")?,
            proof: proof_from_hex(proof)?,
            public_key: element_from_hex(public_key, "public key")?,
        })
    }
}
