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
| DICOM | `DICM` magic code, or a bare data set / file meta group without one | Patient identity, institution, private tags; UIDs rehashed; dates shifted |
| DICOMDIR | Media Storage Directory SOP class | Not uploaded at all — see below |
| XML ECG (HL7 aECG only) | root element + namespace | Everything outside an allowlist; UIDs replaced, dates shifted |
| Schiller Holter | file magic number | Patient block, voice annotations, device UUID |

A file in none of these formats is dropped from the batch. An XML ECG in a
dialect other than HL7 aECG is **refused** — see below. Either way the rest of
the batch is uploaded, and what was left behind is reported.

## Repository layout

```
src/                     the REDCap module, exactly as deployed
  GirderUploaderModule.php     server side: Girder API, upload plans, metadata
  config.json                  module manifest and project settings
  js/girder-uploader.js        the widget
  js/girder-uploader-core.js   pure browser logic (tested)
  js/deidentify-worker.js      Web Worker bridging the widget and the WASM
wasm/dicom_deid/         Rust crate: the deidentifiers, plus their tests
  src/dates.rs                 the date policy shared by DICOM and XML ECG
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
ln -s "$PWD/src" /path/to/redcap/modules/girder_uploader_v1.5.0
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

Drop real `.dcm` / `.vim` / extension-less DICOM and `.xml` files into an untracked `test_data/` directory and
`cargo test` picks them up (`tests/real_data_test.rs`); without it those tests
skip, so CI and fresh clones are unaffected. The assertions are properties, not
fixed values, so they hold for any recording:

- the file is recognized, and still parses after deidentification;
- every value the deidentifier itself considers identifying is gone from the
  output, and patient identity does not resurface anywhere in the bytes;
- every other value comes back byte-for-byte — the recording is untouched —
  dates aside, which are checked separately against the shift described below.

**Never commit files placed there.** `test_data/` is in `.gitignore`; keep it
that way.

## Releasing

REDCap identifies a module version by its **directory name**, so a release is a
zip containing a single `girder_uploader_v<VERSION>` folder.

```bash
scripts/bump-version.sh 1.5.0
git commit -am "Release v1.5.0"
git tag v1.5.0 && git push --follow-tags
```

The tag triggers `.github/workflows/release.yml`, which re-runs the suites,
builds the WASM, and publishes `girder_uploader_v1.5.0.zip` on the GitHub
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
that is an explicit opt-out, not an oversight. One exception: an XML ECG in a
dialect we cannot certify is refused whichever way `deidentify-xml-ecg` is set.
Opting out covers the formats we can read, not one we cannot. The refusal
concerns that file alone; the rest of the batch goes through.

## How an upload works

1. The record must already exist. Until it is saved REDCap has no id for it,
   and the record id is both the Girder folder name and the identity written
   into the deidentified files — so the widget stays locked, and the server
   refuses the upload independently.
2. The browser expands a single top-level ZIP if one was dropped, filters out OS
   noise (`.DS_Store`, dotfiles, `~*`) and nested archives, and sorts the batch
   by path. Batches are capped at 9999 files.
3. Every file goes through the WASM worker. Recognized formats are deidentified
   (or passed through when disabled); a file in none of them is skipped, and a
   file the worker refuses — an XML ECG in an unsupported dialect, say — is set
   aside. **Neither cancels the batch**: one stray `DICOMDIR` or viewer XML in a
   DICOM archive must not lose a thousand-file upload. The same holds during the
   transfer itself: a file that fails to reach Girder is recorded and the others
   carry on.
4. The server builds a signed upload plan per file — target folder, item, stored
   name — and the browser sends chunks back against that plan. The signature is
   an HMAC over the plan keyed by the Girder API key, so the browser cannot
   redirect an upload to another collection or record.
5. Files land under `{DAG}/{record_id}/{field_name}` (with ` -{instance}`
   appended for repeating instances beyond the first).
6. A JSON summary — file sample, counts, Girder folder link, upload state — is
   written back into the tagged field, **including the files that were left
   behind and why**. They are listed in the widget and stored with the rest, so
   a rejection is never silent: it survives a page reload and stays auditable.
   The batch only fails outright when *nothing* could be uploaded.

With `preserve-upload-folder-architecture` disabled, files are stored flat under
neutral names (`0001.ext` … `9999.ext`), so the file names themselves cannot
leak identity.

## Deidentification details

Everything runs client-side, in a Web Worker, through
[`wasm/dicom_deid`](wasm/dicom_deid). Formats are probed in order — Schiller,
XML ECG, DICOM — and the first that recognizes the file wins.

**DICOM** is recognized from the bytes, never from the name: exporters hand
out DICOM as `.dcm`, `.vim` or with no extension at all. A Part 10 file is
spotted by its `DICM` magic code, with or without the 128-byte preamble. Files
that lack the Part 10 header — the file meta group without `DICM` in front, or
a bare data set in explicit or implicit VR little endian — are recognized by
their first element (group 0002 or 0008, self-consistent encoding), parsed, and
given a minted file meta group, so the deidentified output is always a regular
Part 10 file that any viewer opens. The anonymizer is
[`dicom-anonymization`](https://crates.io/crates/dicom-anonymization)
with a Liryc UID root (`1.2.826.0.1.3680043.10.543`). On top of its defaults the
module writes:

- `PatientID` ← the REDCap record id
- `PatientName` ← `{project title}^{record id}`
- `DeidentificationMethod` ← `IHU LIRYC REDCAP PLUGIN`, `PatientIdentityRemoved` ← `YES`

UIDs are rehashed deterministically, so instances of one study stay grouped.
Dates follow the shared policy below.

A **DICOMDIR** is recognized by its SOP class (`1.2.840.10008.1.3.10`), never by
its name, and is dropped from the batch — with DICOM deidentification on or off.
It is the index of the original media, not data. It names every patient on the
media in clear, and anonymizing it does not make it safe or useful: its
directory records sit in a sequence, so the study date inside them is not
shifted, while every file name and UID it points at is renamed or rehashed by
the upload. On one real export, an anonymized DICOMDIR still carried the
original study date and 340 dangling file references. An image that happens to
be *named* `DICOMDIR` is still uploaded as an image.

**XML ECG** accepts one dialect: the HL7 Annotated ECG the FDA takes, recognized
by its root element and namespace (`<AnnotatedECG xmlns="urn:hl7-org:v3">`, a
namespace prefix on the root being equally valid). Every other XML ECG — Philips
`restingecgdata`, GE MUSE `RestingECG`, any vendor variant — is **refused with a
visible error that names the format**, and the upload stops.

That refusal is deliberate and applies even when XML deidentification is turned
off. The allowlist below is written against the aECG schema: applied to another
dialect it would empty the recording, and passing such a file through untouched
would upload identified data. Refusing is the only option that neither destroys
data silently nor leaks it. A file that is not XML at all is left to the other
formats, so a DICOM that happens to be named `.xml` still reaches the DICOM
deidentifier.

Accepted documents are rewritten against an **allowlist**: the waveform, coded
vocabulary, units, the time base, sex and the structural attributes HL7 requires
are kept; *everything else is dropped*. Unknown attributes are removed rather
than emptied, since an empty value breaks the datatype's pattern; unknown
element text is emptied, keeping
the element in place. Comments and CDATA are dropped outright — free text with
no schema behind it.

An allowlist is the only workable direction here. ECG XML is vendor-extensible,
and while this module used a denylist a real recording carried through: the
patient's birth date, the trial subject id, the race code, the device serial
number, a free-text interpretation, and — six times over — an instance OID that
spelled out the acquisition date and time, which quietly undid the date shift.

Identifiers required by the schema are **replaced, not blanked**. The aECG
implementation guide (Appendix D) makes `AnnotatedECG/id/@root`,
`trialSubject/id/@root` and `clinicalTrial/id/@root` mandatory, and `@root` is
typed as an OID or UUID, so emptying it yields a document that no longer
validates.

The pseudonym lives in exactly one place: `trialSubject/id/@extension`, which
the standard reserves for "the traditional identifier" and the schema makes
mandatory. If the source has no `trialSubject/id`, one is **created**; if it has
no `trialSubject` at all the file is refused, since an upload that cannot be
traced back to its record is worse than no upload. Vendor fields duplicating the
patient id — a `<PatientID>` under `subjectDemographicPerson`, say, which is not
part of the aECG content model — are blanked rather than filled in, so there is
one place to look and nothing to keep in step.

The replacements come from the REDCap context, exactly as on the DICOM side:
each `@root` becomes an arc of the Liryc OID `1.2.826.0.1.3680043.10.543` —
`.1` for the document, `.2` for a series, `.3` for the subject, `.4` for the
trial, so the entities keep distinct UIDs — and `@extension`, which the standard
reserves for "the traditional identifier", receives the record id. Trial and
site extensions are dropped instead, being site information. Nothing is minted
at random, so deidentifying a recording twice yields the same identifiers.

Sex is deliberately kept; it is analysis data. `*ExistFlag` attributes are kept
too, since they describe structure rather than identity. The document is
rewritten as a stream, so repeated paths — one per lead, per measurement — keep
their own values.

As a second guard, a document that comes out with no signal at all — an aECG
variant the allowlist does not fully cover — is refused rather than stored
gutted.

Vendor extensions outside the aECG schema are dropped like anything else. On one
real recording that meant a `<channel name1="I"…>` element duplicating the lead
labels; the leads stayed identified where the standard puts them, in
`sequence/code/@code` (`MDC_ECG_LEAD_*`). Extending the allowlist for a vendor
element is a deliberate decision, not the default.

**Schiller Holter** zeroes the voice-annotation section (technicians name the
patient out loud), fills the demographics block, writes the record id into the
patient id field, mints a fresh device UUID, and repairs the CRC.

### Dates

DICOM and XML ECG share one rule, so a patient's files stay consistent with each
other:

- the birth date becomes **1970-01-01**;
- every other date moves by that same offset, `1970-01-01 − birth date`.

`exam − birth` is therefore preserved to the day: **age at acquisition is
exact**, intervals between a patient's studies are exact, and the real calendar
dates are gone. Reversing a shifted date needs the birth date, which the file no
longer carries. The offset is per patient, so two patients imaged the same day
do not land on the same anonymized date.

Two consequences worth knowing. The shift is a whole number of days, so the
acquisition's day-of-year moves by the birth date's day-of-year: for a patient
recorded as born on 1 January — a common placeholder for an unknown date — the
acquisition keeps its real day and month. And a file with **no** birth date gets
no shift, because there is no age to preserve; its dates are then handled by the
format's own default, which for DICOM means a hash-shifted study date and
removed series and acquisition dates.

Times of day are kept, and each date is written back in the notation it used
(`YYYYMMDD` or `YYYY-MM-DD`). A value is only treated as a date if it parses as
a real calendar date in a plausible year, so identifiers and signal samples that
happen to be eight digits are left alone.

A file the worker cannot place is reported as `SKIP:` and dropped quietly, being
a format we never claimed to handle. Any other error rejects that one file, with
its reason recorded and shown. Only a failure that leaves nothing to upload —
or a worker that will not start at all — ends the batch.

## Authors

Josselin Duchateau @ IHU Liryc


## Funding

This work was funded by the following grants:
IHU Liryc ANR-10-IAHU-0004
RHU TALENT ANR 23-RHUS-0015
MEDITWIN consortium (France 2030)
