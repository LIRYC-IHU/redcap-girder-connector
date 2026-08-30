# Girder Uploader — REDCap External Module

Turns a REDCap `textarea` field into a drag-and-drop uploader that streams files
to a [Girder](https://girder.readthedocs.io/) collection, **deidentifying them in
the browser before anything leaves the machine**.

Imaging and signal files are matched against their format, stripped of patient
identity by a Rust/WASM worker, and only then handed to REDCap for chunked
upload to Girder. The original file never crosses the network.

Supported formats:

| Format | Detection | What is removed |
| --- | --- | --- |
| DICOM | `DICM` magic code | Patient identity, institution, private tags; UIDs rehashed; dates shifted |
| XML ECG (HL7 v2/v3, Philips) | root element + namespace | Patient identity, staff names, demographics, trial identifiers |
| Schiller Holter | file magic number | Patient block, voice annotations, device UUID |

Anything else is dropped from the batch rather than uploaded.

## Repository layout

```
src/                     the REDCap module, exactly as deployed
  GirderUploaderModule.php     server side: Girder API, upload plans, metadata
  config.json                  module manifest and project settings
  js/girder-uploader.js        the widget
  js/girder-uploader-core.js   pure browser logic (tested)
  js/deidentify-worker.js      Web Worker bridging the widget and the WASM
wasm/dicom_deid/         Rust crate: the deidentifiers, plus their tests
tests/js/                Node tests for the browser logic
tests/php/               PHP tests for the module
scripts/                 build, package and version-bump helpers
VERSION                  the single source of truth for the module version
```

`src/wasm/` and `dist/` are build output and are not tracked.

## Getting started

Requirements: [Rust](https://rustup.rs) with
[wasm-pack](https://rustwasm.github.io/wasm-pack/installer/), Node 20+, PHP 8.1+.

```bash
scripts/build.sh
```

This compiles the deidentifiers to WebAssembly and installs the package under
`src/wasm/dicom_deid/pkg`, which is the path the worker imports at runtime. The
module is unusable without it, so run it once after cloning and again after any
change under `wasm/`.

To deploy a working copy into a local REDCap, symlink `src/` into
`redcap/modules/` under a versioned name:

```bash
ln -s "$PWD/src" /path/to/redcap/modules/girder_uploader_v1.2.0
```

## Tests

```bash
cd wasm/dicom_deid && cargo test    # deidentification (DICOM, XML ECG, Schiller)
node --test "tests/js/*.test.js"    # file selection, worker contract, metadata
php tests/php/run.php               # paths, settings, upload-plan signatures
```

The PHP suite is dependency-free: it stubs the REDCap base class and calls the
module directly, so `php` is all it needs. Deidentification fixtures are
synthesized in code — no patient data lives in this repository.

### Checking against real recordings

Drop real `.dcm` / `.xml` files into an untracked `test_data/` directory and
`cargo test` picks them up (`tests/real_data_test.rs`); without it those tests
skip, so CI and fresh clones are unaffected. The assertions are properties, not
fixed values, so they hold for any recording:

- the file is recognized, and still parses after deidentification;
- every value the deidentifier itself considers identifying is gone from the
  output, and patient identity does not resurface anywhere in the bytes;
- every other value comes back byte-for-byte — the recording is untouched.

**Never commit files placed there.** `test_data/` is in `.gitignore`; keep it
that way.

## Releasing

REDCap identifies a module version by its **directory name**, so a release is a
zip containing a single `girder_uploader_v<VERSION>` folder.

```bash
scripts/bump-version.sh 1.2.0
git commit -am "Release v1.2.0"
git tag v1.2.0 && git push --follow-tags
```

The tag triggers `.github/workflows/release.yml`, which re-runs the suites,
builds the WASM, and publishes `girder_uploader_v1.2.0.zip` on the GitHub
release. That zip is what you feed to REDCap's *Upload module ZIP*.

To build one locally:

```bash
scripts/package.sh          # -> dist/girder_uploader_v<VERSION>.zip
```

The release workflow refuses to run if the tag and `VERSION` disagree.

## Configuration

Add `@GIRDER_UPLOAD` to the field annotation of a `textarea` field, then fill in
the project settings:

| Setting | Purpose |
| --- | --- |
| `girder-backchannel-api-url` | Girder API used by the server (must end in `/api/v1`) |
| `girder-frontchannel-base-url` | Girder base URL used for the links shown to users |
| `api-key` | Girder API key; also the secret signing upload plans |
| `root-collection-id` | Collection uploads are stored under |
| `chunk-size`, `max-retries`, `retry-delay` | Upload transport tuning |
| `deidentify-dicom` | Deidentify DICOM files (on by default) |
| `deidentify-xml-ecg` | Deidentify XML ECG files |
| `deidentify-schiller-holter` | Deidentify Schiller Holter files |
| `preserve-upload-folder-architecture` | Keep the user's folder names, or store flat under neutral names |

A format whose checkbox is off is still recognized but uploaded **untouched** —
that is an explicit opt-out, not an oversight.

## How an upload works

1. The browser expands a single top-level ZIP if one was dropped, filters out OS
   noise (`.DS_Store`, dotfiles, `~*`) and nested archives, and sorts the batch
   by path. Batches are capped at 9999 files.
2. Every file goes through the WASM worker. Recognized formats are deidentified
   (or passed through when disabled); unrecognized files are skipped.
3. The server builds a signed upload plan per file — target folder, item, stored
   name — and the browser sends chunks back against that plan. The signature is
   an HMAC over the plan keyed by the Girder API key, so the browser cannot
   redirect an upload to another collection or record.
4. Files land under `{DAG}/{record_id}/{field_name}` (with ` -{instance}`
   appended for repeating instances beyond the first).
5. A JSON summary — file sample, counts, Girder folder link, upload state — is
   written back into the tagged field. Failed uploads are recorded and can be
   retried.

With `preserve-upload-folder-architecture` disabled, files are stored flat under
neutral names (`0001.ext` … `9999.ext`), so the file names themselves cannot
leak identity.

## Deidentification details

Everything runs client-side, in a Web Worker, through
[`wasm/dicom_deid`](wasm/dicom_deid). Formats are probed in order — Schiller,
XML ECG, DICOM — and the first that recognizes the file wins.

**DICOM** uses [`dicom-anonymization`](https://crates.io/crates/dicom-anonymization)
with a Liryc UID root (`1.2.826.0.1.3680043.10.543`). On top of its defaults the
module writes:

- `PatientID` ← the REDCap record id
- `PatientName` ← `{project title}^{record id}`
- `DeidentificationMethod` ← `IHU LIRYC REDCAP PLUGIN`, `PatientIdentityRemoved` ← `YES`

UIDs are rehashed deterministically, so instances of one study stay grouped.
Study dates are **shifted back** by an offset derived from the original patient
id — intervals between that patient's studies survive, absolute dates do not.
Series and acquisition dates are removed outright.

**XML ECG** recognizes HL7 v2, HL7 v3 (`AnnotatedECG`) and Philips
(`restingecgdata`) documents by root element and namespace. Element paths
matching a known identity fragment (`patientid`, `lastname`, `age`, `room`,
`technician`, `clinicaltrialprotocolid`, …) are blanked; paths containing
`patientid` receive the record id instead. `*ExistFlag` attributes are left
alone, since they describe structure rather than identity. The document is
rewritten as a stream, so repeated paths — one per lead, per measurement — keep
their own values.

The identity fragments are matched as substrings of the whole element path,
which is deliberately blunt: on an HL7 v3 recording it also blanks vocabulary
attributes such as `codeSystemName` and `displayName` (they contain `name`).
The codes themselves and their `codeSystem` OIDs are preserved, so the document
stays machine-readable, but it loses the human-readable labels. Widening the
rule is safe; narrowing it is not, so it is left as it is.

**Schiller Holter** zeroes the voice-annotation section (technicians name the
patient out loud), fills the demographics block, writes the record id into the
patient id field, mints a fresh device UUID, and repairs the CRC.

A file the worker cannot place is reported as `SKIP:` and dropped from the
batch; a genuine failure aborts the upload instead.

## Authors

IHU Liryc — University of Bordeaux.
