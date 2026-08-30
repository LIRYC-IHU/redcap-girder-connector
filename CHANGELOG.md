# Changelog

## Unreleased

### Fixed — deidentification

- **DICOM files with a Part 10 preamble were never deidentified.** The reader
  expects a stream starting at the `DICM` magic code, but files exported by PACS
  start with a 128-byte preamble, so they failed detection and were dropped from
  the batch as "unsupported". The preamble is now skipped when present.
- **`PatientID`, `PatientName` and the deidentification stamp were only written
  when the tag already existed.** The anonymizer walks existing elements only,
  so `Action::Replace` on an absent tag did nothing — in particular
  `DeidentificationMethod` was never written. The REDCap identity and the
  `PatientIdentityRemoved`/`DeidentificationMethod` stamp are now set explicitly
  after anonymization.
- **The record id never reached Schiller Holter files.** `set_patient_id()` ran
  after the patient block had already been written, so every file got a random
  id instead of the record id.
- **A truncated or spoofed Schiller file crashed the worker.** Only the magic
  number was checked before slicing at fixed offsets, so a short file panicked
  out of WASM instead of being skipped. Size is now validated up front.
- **XML ECG values were duplicated across repeated element paths.** Values were
  collected into a path-keyed map, so every `<value>` of a multi-lead recording
  was rewritten with the last lead's samples. The document is now transformed as
  a stream, leaving repeated paths untouched.
- `clinicaltrialprotocolid` and `clinicaltrialprotocolname` are blanked in XML
  ECG files, matching the reference deidentifier.

### Added

- Test suites for all three deidentifiers (Rust), the browser-side upload logic
  (Node) and the REDCap module (PHP), all on synthesized fixtures.
- Property-based checks against real recordings dropped into an untracked
  `test_data/` directory, skipped when it is absent.
- `scripts/build.sh`, `scripts/package.sh` and `scripts/bump-version.sh`, plus
  GitHub Actions for CI and tagged releases.
- README covering layout, configuration, upload flow and deidentification.

### Changed

- The repository is now laid out as `src/` (module), `wasm/` (deidentifiers),
  `tests/` and `scripts/`, instead of a single `girder_uploader_v1.1.0` folder;
  that folder is now produced by `scripts/package.sh` from the `VERSION` file.
- Pure browser logic moved to `js/girder-uploader-core.js` so it can be tested
  outside a browser; the widget consumes it through `window.GirderUploaderCore`.
- The `dicom-object` dependency moved from 0.6 to 0.8, matching the version
  `dicom-anonymization` uses, so the crate no longer links two copies.

## 1.1.0

Initial packaged release of the module.
