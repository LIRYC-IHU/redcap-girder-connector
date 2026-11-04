# Pseudonym protocol — wire specification

Implementer's reference for the VOPRF described in
[`docs/design/pseudonymisation.md`](../docs/design/pseudonymisation.md). Two
implementations exist and must agree byte for byte:

| Side | Code | Role |
| --- | --- | --- |
| Browser (Rust → WASM) | `crates/liryc-pseudonym`, via `wasm/dicom_deid` | blind, verify, finalize, derive |
| REDCap (PHP, libsodium) | `src/Voprf.php` | key `k`, blind-evaluate + proof |

## Primitive

RFC 9497, ciphersuite **ristretto255-SHA512**, **VOPRF mode** (mode byte `0x01`):

- `contextString = "OPRFV1-" ‖ 0x01 ‖ "-ristretto255-SHA512"`
- `HashToGroup(x)`: `expand_message_xmd(SHA-512, x, "HashToGroup-" ‖ contextString, 64)`,
  then the ristretto255 one-way map (`crypto_core_ristretto255_from_hash`).
- `HashToScalar(x)`: `expand_message_xmd(SHA-512, x, "HashToScalar-" ‖ contextString, 64)`,
  reduced modulo the group order (`crypto_core_ristretto255_scalar_reduce`).
- `BlindEvaluate(k, B)`: `Z = k·B`, `proof = GenerateProof(k, G, k·G, [B], [Z])`.
- `GenerateProof` / `VerifyProof` / `ComputeComposites(Fast)`: RFC 9497 §2.2, batch size 1.
- `Finalize(x, N)`: `SHA-512(I2OSP(len(x), 2) ‖ x ‖ I2OSP(32, 2) ‖ Ser(N) ‖ "Finalize")`.

Validated against `test-vectors/rfc9497-ristretto255-sha512-voprf.json` (RFC 9497
Appendix A.1.2), and element decoding against `test-vectors/ristretto255-decoding.json`
(625 encodings with their RFC 9496 verdict; libsodium 1.0.18 wrongly accepts the identity
and encodings with bit 255 set). `test-vectors/pseudonym.json` fixes `k`, `r` and the proof
scalar, and records every value of the protocol below.

## Encoding

Every element, scalar and proof is **lowercase hex**:

- element (compressed ristretto255): 32 bytes → 64 hex characters;
- scalar (little-endian): 32 bytes → 64 hex characters;
- proof: `c ‖ s`, 64 bytes → 128 hex characters.

The server **must reject** a blinded element that is not 32 bytes, does not decode to a
valid ristretto255 point, has bit 255 set, or is the identity (32 zero bytes) — checked
explicitly rather than left to the library.

## Evaluation

```
B     = r · HashToGroup(x)              browser
Z, π  = BlindEvaluate(k, B)             REDCap
VerifyProof(G, K, [B], [Z], π)          browser
N     = r⁻¹ · Z                         browser
y     = Finalize(x, N)                  browser, 64 bytes
```

## Input and derived values

```
x = "liryc-pseudonym-v1" ‖ 0x00 ‖ domain ‖ 0x00 ‖ nip      (UTF-8)
fingerprint = hex(SHA-512("liryc-pseudonym-fingerprint-v1" ‖ y)[0..1])    two hex characters
seed        = big-endian u64 of SHA-512("liryc-pseudonym-date-offset-v1" ‖ y)[0..8]
offset_days = −(18262 + seed mod 18263)                    ∈ [−36524, −18262]
uid_key     = SHA-512("liryc-pseudonym-uid-key-v1" ‖ y ‖ record_id)[0..32]
new_uid     = root "." decimal(HMAC-SHA256(uid_key, uid)[0..15])
```

`root` is `1.2.826.0.1.3680043.10.543`; 120 bits fit the 64 characters of a UID. Only the
fingerprint leaves the browser. `record_id` is trimmed.

NIP normalization: remove every whitespace character, ASCII-uppercase, and when the
result is all ASCII digits strip leading zeros. An empty result means "no NIP".

## REDCap module AJAX actions

`pseudonym-evaluate` — payload `{ "fieldName", "blindedElement" }`

```json
{ "ok": true, "evaluatedElement": "<hex Z>", "proof": "<hex>", "publicKey": "<hex K>" }
```

`pseudonym-check` — payload `{ "fieldName", "fingerprint" }`. Called **before** the
uploader confirms; stores nothing. Refused when Girder does not answer.
`existingUploads` counts the record's field folders in Girder that hold something:
`new` with a non-zero count means uploads that predate fingerprints.

```json
{ "ok": true, "status": "new" | "matched" | "mismatch" | "unchecked", "existingUploads": 0 }
```

`pseudonym-register` — payload `{ "fieldName", "fingerprint", "confirmedMismatch" }`.
Called **once the uploader has confirmed**; stores the fingerprint when the record has
none (comparing again under a per-record lock) and issues the ticket. `confirmedMismatch`
is `true` when the uploader was shown a mismatch and chose to go on: the record keeps its
fingerprint and the answer is `overridden`. A mismatch without it was never shown, and is
refused.

```json
{ "ok": true, "status": "registered" | "matched" | "overridden", "ticket": "<opaque>" }
{ "ok": false, "code": "fingerprint-mismatch", "error": "<message>" }
```

`init-batch` gains `pseudonymTicket`, required whenever the batch carries deidentified
files **and** the project checkbox `pseudonym-fingerprint-check` is ticked (the default).
The key itself must always be present, empty if need be: a batch without it comes from a
page older than the module, and is refused.
With the box unticked, `pseudonym-register` answers `{ "ok": true, "status": "unchecked" }`
and `pseudonym-check` answers `unchecked`, neither storing nor comparing anything;
`pseudonym-evaluate` is unaffected, since it also yields the date offset and the UID key.
