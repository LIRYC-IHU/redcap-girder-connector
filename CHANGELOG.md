# Changelog

## 1.4.1

### Fixed

- **One bad file no longer cancels the whole transfer.** A file the worker
  refuses — an XML ECG in an unsupported dialect, most often — aborted the
  entire batch, so a single stray `DICOMDIR`, README or viewer XML sitting in a
  DICOM archive lost a thousand-file upload. Rejections are now per file, and so
  are transfer failures: the rest of the batch goes through.
- Rejections stay visible. Each one is recorded with its file path, the stage it
  failed at and the reason, listed in the widget and stored in the field
  alongside the uploaded files, so it survives a page reload. The batch fails
  outright only when nothing could be uploaded, and the message then carries the
  first reason.
- The stored Girder reference now points at a file that actually landed; it
  could previously describe the last *attempted* upload when that one failed.

## 1.4.0

### Changed — deidentification

- **Only the HL7 Annotated ECG is accepted now.** The dialect is decided from
  the root element and namespace (`<AnnotatedECG xmlns="urn:hl7-org:v3">`), and
  every other XML ECG — Philips `restingecgdata`, GE MUSE `RestingECG`, vendor
  variants — is refused with a visible error naming the format, including when
  XML deidentification is turned off.

  The allowlist is written against the aECG schema. Applied to another dialect
  it empties the recording, and 1.3.0 would have done exactly that to a Philips
  file without tripping any guard: `parsedwaveforms` happened to be on the list,
  so the waveform survived while every measurement and interpretation was
  silently blanked. Refusing is the only outcome that neither destroys data
  quietly nor uploads identified data.

  A file that is not XML is still left to the other formats, so a DICOM named
  `.xml` reaches the DICOM deidentifier as before.

- **The record id is now guaranteed to reach every accepted ECG.** It goes into
  `trialSubject/id/@extension` — the location the standard reserves for it and
  the schema makes mandatory — and that element is created when the source lacks
  it. A document with no `trialSubject` at all is refused: an upload that cannot
  be traced back to its record is worse than no upload.
- Vendor fields duplicating the patient id, such as a `<PatientID>` under
  `subjectDemographicPerson`, are blanked instead of being filled with the
  record. `subjectDemographicPerson` has a fixed content model in the aECG
  schema — `name`, `administrativeGenderCode`, `birthTime`, `raceCode` — so
  writing a `PatientID` into it would make a conformant document invalid.

### Fixed

- An aECG that declares the HL7 namespace **through a prefix** on the root
  (`<hl7:AnnotatedECG xmlns:hl7="urn:hl7-org:v3">`) was accepted and then
  emptied, because every element carried the prefix and no allowlist key
  matched. Namespace prefixes are now stripped from element names before
  matching; attribute prefixes are kept, `xsi:type` and `xmlns:*` being
  meaningful.

## 1.3.0

### Changed — deidentification

- **Dates now follow one rule across DICOM and XML ECG**: the birth date becomes
  1970-01-01 and every other date moves by the same offset, so the age at
  acquisition is preserved to the day while the real calendar dates are
  destroyed. Previously DICOM hash-shifted the study date by an offset derived
  from the patient id (age not preserved) and removed series and acquisition
  dates, while XML ECG left every timestamp untouched — so an ECG could be used
  to undo the shift applied to that patient's DICOM.
- A file with no birth date is not shifted at all, since there is no age to
  preserve; the format's previous fallback applies.
- Sex is no longer blanked in XML ECG files. It is analysis data, and it was
  being removed from Philips documents (`<sex>`) while surviving in HL7 v3
  (`administrativeGenderCode`) — the two dialects now agree.

- **XML ECG deidentification is now an allowlist.** The waveform, coded
  vocabulary, units, the time base, sex and the structural attributes HL7
  requires are kept; everything else is dropped. It used to be a denylist, which
  cannot be complete over vendor-extensible XML — see below for what a real
  recording carried through it.
- Identifiers the aECG schema makes mandatory are replaced rather than blanked,
  from the REDCap context as on the DICOM side: each UID root becomes an arc of
  the Liryc OID (one per entity kind) and the extension receives the record id.
  Blanking them would have produced documents that no longer validate, since
  `@root` is typed as an OID or UUID. Nothing is minted at random, so the
  transform is reproducible.
- A document whose signal elements are not on the allowlist is now **refused
  with a visible error** instead of being uploaded emptied. The allowlist is
  curated against HL7 v3, the only dialect a real recording was available for.

### Fixed

- **Uploading to a record that has not been saved yet is now refused**, in the
  widget and again server-side. REDCap hands the module a placeholder instead of
  a record id until the record exists, so those uploads landed in a Girder
  folder named `external-modules-temporary-record-id-…` that no record ever
  points at, and carried `UNASSIGNED_RECORD` inside the deidentified files —
  where it cannot be corrected afterwards.

### Fixed — deidentification

- **The instance OID leaked the acquisition date, time and device serial** in
  HL7 v3 recordings (`755.<serial>.<date>.<time>`, repeated six times in the
  sample examined), which undid the date shift applied elsewhere in the file.
- The **device serial number**, the **investigator identifier** and the
  **free-text interpretation** survived, none of them covered by any pattern.
- The patient's **birth date survived in XML ECG files** (`birthTime`,
  `dateofbirth`): no pattern covered it. It is now pinned to the epoch.
- The **trial subject identifier** (`trialSubject/id`) and the **race code**
  survived for the same reason; both are now blanked.

## 1.2.0

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
