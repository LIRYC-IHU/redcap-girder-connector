//! The VOPRF: RFC 9497 conformance, agreement with the reference implementation,
//! and the vectors the REDCap server checks against.

use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use liryc_pseudonym::{self as pseudonym, Evaluation, PseudonymError};
use serde_json::{json, Value};

const RFC_VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../protocol/test-vectors/rfc9497-ristretto255-sha512-voprf.json"
);
const PSEUDONYM_VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../protocol/test-vectors/pseudonym.json"
);

fn hex(value: &Value) -> Vec<u8> {
    let s = value.as_str().expect("hex string");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn scalar(value: &Value) -> Scalar {
    pseudonym::scalar_from_hex(value.as_str().unwrap(), "scalar").unwrap()
}

fn element(value: &Value) -> RistrettoPoint {
    pseudonym::element_from_hex(value.as_str().unwrap(), "element").unwrap()
}

fn rfc_vectors() -> Value {
    serde_json::from_str(&std::fs::read_to_string(RFC_VECTORS).unwrap()).unwrap()
}

// --- RFC 9497, Appendix A.1.2 ------------------------------------------------

#[test]
fn the_public_key_matches_the_rfc() {
    let vectors = rfc_vectors();
    assert_eq!(
        pseudonym::element_to_hex(&pseudonym::public_key(&scalar(&vectors["skSm"]))),
        vectors["pkSm"].as_str().unwrap()
    );
}

#[test]
fn every_step_matches_the_rfc_vectors() {
    let vectors = rfc_vectors();
    let key = scalar(&vectors["skSm"]);
    let public_key = element(&vectors["pkSm"]);

    for vector in vectors["vectors"].as_array().unwrap() {
        let request = pseudonym::start_with_blind(hex(&vector["input"]), scalar(&vector["blind"]));
        assert_eq!(
            pseudonym::element_to_hex(&request.blinded_element()),
            vector["blindedElement"].as_str().unwrap(),
            "blind"
        );

        let (evaluated, proof) = pseudonym::blind_evaluate(
            &key,
            &request.blinded_element(),
            &scalar(&vector["proofRandomScalar"]),
        );
        assert_eq!(
            pseudonym::element_to_hex(&evaluated),
            vector["evaluationElement"].as_str().unwrap(),
            "blind evaluate"
        );
        assert_eq!(
            pseudonym::proof_to_hex(&proof),
            vector["proof"].as_str().unwrap(),
            "proof"
        );
        assert!(pseudonym::verify_proof(
            &public_key,
            &request.blinded_element(),
            &evaluated,
            &proof
        ));

        let output = pseudonym::finalize(
            &hex(&vector["input"]),
            &scalar(&vector["blind"]),
            &evaluated,
        );
        assert_eq!(
            pseudonym::to_hex(&output),
            vector["output"].as_str().unwrap(),
            "finalize"
        );
    }
}

#[test]
fn a_proof_for_another_key_or_element_is_rejected() {
    let vectors = rfc_vectors();
    let vector = &vectors["vectors"][0];
    let key = scalar(&vectors["skSm"]);
    let blinded = element(&vector["blindedElement"]);
    let (evaluated, proof) =
        pseudonym::blind_evaluate(&key, &blinded, &scalar(&vector["proofRandomScalar"]));

    let other_key = pseudonym::public_key(&(key + Scalar::ONE));
    assert!(!pseudonym::verify_proof(
        &other_key, &blinded, &evaluated, &proof
    ));

    let other_evaluated = evaluated + pseudonym::public_key(&Scalar::ONE);
    let public_key = pseudonym::public_key(&key);
    assert!(!pseudonym::verify_proof(
        &public_key,
        &blinded,
        &other_evaluated,
        &proof
    ));
}

// --- Reference implementation -------------------------------------------------

#[test]
fn our_client_agrees_with_the_reference_server() {
    use voprf::{BlindedElement, Ristretto255, VoprfServer};

    let mut rng = rand::rngs::OsRng;
    for nip in ["1234567", "A-998877", "42"] {
        let server = VoprfServer::<Ristretto255>::new(&mut rng).unwrap();
        let public_key = pseudonym::element_from_hex(
            &pseudonym::to_hex(&server.get_public_key().compress().to_bytes()),
            "pk",
        )
        .unwrap();

        let request = pseudonym::start("redcap:12", nip).unwrap();
        let blinded = BlindedElement::<Ristretto255>::deserialize(
            request.blinded_element().compress().as_bytes(),
        )
        .unwrap();
        let answer = server.blind_evaluate(&mut rng, &blinded);

        let evaluated =
            pseudonym::element_from_hex(&pseudonym::to_hex(&answer.message.serialize()), "z")
                .unwrap();
        let proof =
            pseudonym::proof_from_hex(&pseudonym::to_hex(&answer.proof.serialize())).unwrap();
        assert!(
            pseudonym::verify_proof(&public_key, &request.blinded_element(), &evaluated, &proof),
            "the reference server's proof must verify with our code"
        );

        let input = pseudonym::input_for("redcap:12", &pseudonym::normalize_nip(nip).unwrap());
        let expected = server.evaluate(&input).unwrap();
        // `finalize` needs the blind, which `Request` keeps private: go through
        // a fixed blind instead.
        let blind = Scalar::from(7u64);
        let fixed = pseudonym::start_with_blind(input.clone(), blind);
        let fixed_blinded = BlindedElement::<Ristretto255>::deserialize(
            fixed.blinded_element().compress().as_bytes(),
        )
        .unwrap();
        let fixed_answer = server.blind_evaluate(&mut rng, &fixed_blinded);
        let fixed_evaluated =
            pseudonym::element_from_hex(&pseudonym::to_hex(&fixed_answer.message.serialize()), "z")
                .unwrap();
        assert_eq!(
            pseudonym::finalize(&input, &blind, &fixed_evaluated).to_vec(),
            expected.to_vec(),
            "the unblinded output must equal the server's direct evaluation"
        );
    }
}

// --- The protocol ---------------------------------------------------------------

struct Fixture {
    key: Scalar,
    domain: &'static str,
    record_id: &'static str,
    nip: &'static str,
    blind: Scalar,
    proof_scalar: Scalar,
}

fn fixture() -> Fixture {
    Fixture {
        key: Scalar::from_bytes_mod_order([0x11; 32]),
        domain: "redcap:12",
        record_id: "REC-1",
        nip: " 0001234567 ",
        blind: Scalar::from_bytes_mod_order([0x33; 32]),
        proof_scalar: Scalar::from_bytes_mod_order([0x44; 32]),
    }
}

fn run(fixture: &Fixture) -> Value {
    let normalized = pseudonym::normalize_nip(fixture.nip).unwrap();
    let input = pseudonym::input_for(fixture.domain, &normalized);
    let request = pseudonym::start_with_blind(input.clone(), fixture.blind);
    let (evaluated, proof) = pseudonym::blind_evaluate(
        &fixture.key,
        &request.blinded_element(),
        &fixture.proof_scalar,
    );

    let pseudonym = pseudonym::finish(
        &request,
        &Evaluation {
            element: evaluated,
            proof,
            public_key: pseudonym::public_key(&fixture.key),
        },
        fixture.record_id,
    )
    .unwrap();
    let output = pseudonym::finalize(&input, &fixture.blind, &evaluated);

    json!({
        "description": "The VOPRF of protocol/README.md end to end, produced by crates/liryc-pseudonym (pseudonym_test.rs). Given its key, the blinded element and the proof scalar, the server must return exactly the evaluated element and proof.",
        "domain": fixture.domain,
        "recordId": fixture.record_id,
        "nip": fixture.nip,
        "normalizedNip": normalized,
        "input": pseudonym::to_hex(&input),
        "blind": pseudonym::to_hex(fixture.blind.as_bytes()),
        "blindedElement": pseudonym::element_to_hex(&request.blinded_element()),
        "key": pseudonym::to_hex(fixture.key.as_bytes()),
        "publicKey": pseudonym::element_to_hex(&pseudonym::public_key(&fixture.key)),
        "proofRandomScalar": pseudonym::to_hex(fixture.proof_scalar.as_bytes()),
        "evaluatedElement": pseudonym::element_to_hex(&evaluated),
        "proof": pseudonym::proof_to_hex(&proof),
        "output": pseudonym::to_hex(&output),
        "fingerprint": pseudonym.fingerprint,
        "dateOffsetDays": pseudonym.date_offset_days,
        "uidKey": pseudonym::to_hex(&pseudonym.uid_key),
    })
}

#[test]
fn the_vectors_are_up_to_date() {
    let produced = run(&fixture());
    if std::env::var_os("UPDATE_VECTORS").is_some() {
        std::fs::write(
            PSEUDONYM_VECTORS,
            serde_json::to_string_pretty(&produced).unwrap() + "\n",
        )
        .unwrap();
    }
    let recorded: Value = serde_json::from_str(
        &std::fs::read_to_string(PSEUDONYM_VECTORS)
            .expect("run once with UPDATE_VECTORS=1 to create pseudonym.json"),
    )
    .unwrap();
    assert_eq!(
        recorded, produced,
        "pseudonym.json is stale: rerun with UPDATE_VECTORS=1"
    );
}

#[test]
fn the_pseudonym_depends_on_the_key() {
    let reference = run(&fixture());
    let other = run(&Fixture {
        key: fixture().key + Scalar::ONE,
        ..fixture()
    });

    assert_ne!(reference["output"], other["output"]);
    assert_ne!(reference["uidKey"], other["uidKey"]);
}

#[test]
fn the_blind_does_not_change_the_pseudonym() {
    let a = run(&fixture());
    let b = run(&Fixture {
        blind: Scalar::from(987_654_321u64),
        ..fixture()
    });

    assert_ne!(
        a["blindedElement"], b["blindedElement"],
        "the server sees unrelated elements"
    );
    assert_eq!(a["fingerprint"], b["fingerprint"]);
    assert_eq!(a["dateOffsetDays"], b["dateOffsetDays"]);
    assert_eq!(a["uidKey"], b["uidKey"]);
}

#[test]
fn projects_get_unrelated_pseudonyms() {
    let a = run(&fixture());
    let b = run(&Fixture {
        domain: "redcap:13",
        ..fixture()
    });

    assert_ne!(a["output"], b["output"]);
}

#[test]
fn a_tampered_answer_is_refused() {
    let fixture = fixture();
    let input = pseudonym::input_for(fixture.domain, "1234567");
    let request = pseudonym::start_with_blind(input, fixture.blind);
    let (_, proof) = pseudonym::blind_evaluate(
        &fixture.key,
        &request.blinded_element(),
        &fixture.proof_scalar,
    );
    let public_key = pseudonym::public_key(&fixture.key);

    // The server answering with another key than the one it publishes.
    let (other, other_proof) = pseudonym::blind_evaluate(
        &(fixture.key + Scalar::ONE),
        &request.blinded_element(),
        &fixture.proof_scalar,
    );
    let lying = Evaluation {
        element: other,
        proof: other_proof,
        public_key,
    };
    assert_eq!(
        pseudonym::finish(&request, &lying, "REC-1"),
        Err(PseudonymError::InvalidProof)
    );

    // The answer swapped for another element.
    let swapped = Evaluation {
        element: other,
        proof,
        public_key,
    };
    assert_eq!(
        pseudonym::finish(&request, &swapped, "REC-1"),
        Err(PseudonymError::InvalidProof)
    );
}

// --- Inputs and derived values -------------------------------------------------

#[test]
fn nips_are_normalized() {
    assert_eq!(
        pseudonym::normalize_nip(" 000123 4567\n").as_deref(),
        Some("1234567")
    );
    assert_eq!(
        pseudonym::normalize_nip("ab-0012").as_deref(),
        Some("AB-0012"),
        "leading zeros only go from all-digit NIPs"
    );
    assert_eq!(pseudonym::normalize_nip("   "), None);
    assert_eq!(pseudonym::normalize_nip("0000"), None);
    assert_eq!(
        pseudonym::start("redcap:12", "  ").err(),
        Some(PseudonymError::MissingNip)
    );
}

#[test]
fn offsets_stay_within_50_to_100_years_back() {
    for i in 0u8..=255 {
        let offset = pseudonym::derive(&[i; 64], "REC-1").date_offset_days;
        assert!((-36_524..=-18_262).contains(&offset), "{offset}");
    }
}

#[test]
fn the_offset_belongs_to_the_patient_whatever_the_record() {
    assert_eq!(
        pseudonym::derive(&[7; 64], "REC-1").date_offset_days,
        pseudonym::derive(&[7; 64], "REC-2").date_offset_days
    );
    assert_ne!(
        pseudonym::derive(&[7; 64], "REC-1").date_offset_days,
        pseudonym::derive(&[8; 64], "REC-1").date_offset_days
    );
}

#[test]
fn malformed_elements_are_rejected() {
    assert!(
        pseudonym::element_from_hex(&"00".repeat(32), "e").is_err(),
        "identity"
    );
    assert!(
        pseudonym::element_from_hex(&"ff".repeat(32), "e").is_err(),
        "not on the group"
    );
    assert!(
        pseudonym::element_from_hex("abcd", "e").is_err(),
        "wrong length"
    );
    assert!(
        pseudonym::proof_from_hex(&"ff".repeat(64)).is_err(),
        "non-canonical scalars"
    );
}

#[test]
fn element_decoding_follows_rfc_9496() {
    // Shared with the REDCap and Girder implementations: libsodium 1.0.18
    // accepts the identity and encodings with bit 255 set, which RFC 9496
    // rejects; every implementation must give the RFC's answer.
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/test-vectors/ristretto255-decoding.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let key = scalar(&fixture["key"]);

    for case in fixture["cases"].as_array().unwrap() {
        let encoding = case["element"].as_str().unwrap();
        let decoded = pseudonym::element_from_hex(encoding, "element");
        assert_eq!(
            decoded.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{encoding}"
        );
        if let (Ok(point), Some(expected)) = (decoded, case["kTimesElement"].as_str()) {
            assert_eq!(
                pseudonym::element_to_hex(&(point * key)),
                expected,
                "{encoding}"
            );
        }
    }
}

#[test]
fn the_fingerprint_is_two_hex_characters() {
    for i in 0u8..=255 {
        let fingerprint = pseudonym::derive(&[i; 64], "REC-1").fingerprint;
        assert_eq!(fingerprint.len(), 2);
        assert!(fingerprint.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}

// --- UIDs ----------------------------------------------------------------------

const ROOT: &str = "1.2.826.0.1.3680043.10.543";
const UID: &str = "1.2.840.113619.2.55.3.604688119.868.1234567890.1";

#[test]
fn a_uid_maps_to_the_same_one_within_a_record() {
    let key = pseudonym::derive(&[7; 64], "REC-1").uid_key;

    assert_eq!(
        pseudonym::pseudonymize_uid(&key, UID, ROOT),
        pseudonym::pseudonymize_uid(&key, UID, ROOT)
    );
    assert_ne!(
        pseudonym::pseudonymize_uid(&key, UID, ROOT),
        pseudonym::pseudonymize_uid(&key, "1.2.3", ROOT)
    );
}

#[test]
fn a_uid_cannot_be_followed_across_records_or_patients() {
    let key = pseudonym::derive(&[7; 64], "REC-1").uid_key;
    let other_record = pseudonym::derive(&[7; 64], "REC-2").uid_key;
    let other_patient = pseudonym::derive(&[8; 64], "REC-1").uid_key;

    assert_ne!(key, other_record);
    assert_ne!(key, other_patient);
}

#[test]
fn rewritten_uids_are_valid_dicom_uids() {
    for i in 0u8..=255 {
        let key = pseudonym::derive(&[i; 64], "REC-1").uid_key;
        let uid = pseudonym::pseudonymize_uid(&key, UID, ROOT);

        assert!(uid.len() <= 64, "{uid}");
        assert!(uid.starts_with(ROOT), "{uid}");
        let last = uid.rsplit('.').next().unwrap();
        assert!(last.bytes().all(|b| b.is_ascii_digit()), "{uid}");
        assert!(
            last == "0" || !last.starts_with('0'),
            "no leading zero: {uid}"
        );
    }
}

#[test]
fn files_without_nip_have_an_input_of_their_own() {
    let input = liryc_pseudonym::input_without_nip("redcap:12", " REC-1 ");

    assert_eq!(input, b"liryc-pseudonym-no-nip-v1\0redcap:12\0REC-1");
    assert_ne!(
        input,
        liryc_pseudonym::input_without_nip("redcap:12", "REC-2"),
        "one offset per record"
    );
    assert_ne!(
        input,
        liryc_pseudonym::input_without_nip("redcap:13", "REC-1")
    );
    assert_ne!(
        input,
        liryc_pseudonym::input_for("redcap:12", "REC-1"),
        "no NIP can reach it"
    );
}
