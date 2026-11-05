# Changelog

## 2.0.1

**Install this version rather than 2.0.0, which never went into service.** Pseudonyms
differ between the two, see "Security" below.

### Security

- **Each project has its own pseudonym key**, derived from the module's and from the
  project's domain, which the server reads from its settings. The browser's input is
  blinded, so the domain it claimed could not be checked: a user of any project who
  knew a NIP could compute its offset and signature for another project.
  `pseudonym-domain` can now only be set by an administrator. **Signatures, date
  offsets and UIDs all differ from 2.0.0.**
- **Free text without a rule is emptied in DICOM files.** The anonymizer keeps some
  two hundred person-name and text elements it has no rule for, `ReasonForVisit`,
  `PersonTelecomInformation`, `ContentCreatorName` and `TextValue` among them, in
  sequences too. Every public PN, LT, ST, UT and UC element it would keep is now
  emptied. LO and SH stay: they hold `SeriesDescription` and what the data needs.
- **The DICOM file meta group no longer tells who sent the file.** Application entity
  titles and private information were copied as they were.
- **A date range no longer leaves a real date behind.** In `20190101-20190201`, the end
  was written back unshifted. Only a time of day may follow a date now: anything else
  is removed, in DICOM and in XML ECGs.
- **XML ECGs lose their `DOCTYPE` and processing instructions**, which were copied
  verbatim, and could carry text or declare entities for whoever parses the file next.
- **Rejected files keep their path out of the REDCap field**, where it could carry the
  patient's name. The review window names them before the upload, and the field stores
  `File 1 (.dcm)`, unless the project keeps its folder and file names anyway. The
  refusal of an XML dialect no longer names the file either.

### Changed

- **Derived DICOM images are left out of the upload**: `DERIVED` in `ImageType`, as in
  dicom-rust-wasm-deid. They are out of scope rather than wrong, so they are counted
  in the review window and not listed as rejections. With DICOM deidentification off,
  they are sent as they are.

### Fixed

- **An upload no longer fails with `Unexpected token '<'` on a project whose
  record-list cache is not built.** That is a new project, or one whose cache REDCap's
  cron dropped after a few idle weeks. REDCap then redirects the first request into
  the project to its cache builder, and the widget got an HTML page instead of JSON.
  The widget now recognizes a response that is not JSON, builds the cache itself, and
  sends the request once more.

## 2.0.0

Design and threat model:
[`docs/design/pseudonymisation.md`](docs/design/pseudonymisation.md).

### Added — patient pseudonym

- **Every deidentified file is tied to its patient by a pseudonym computed from
  its NIP**, through a verifiable OPRF (RFC 9497, ristretto255-SHA512) keyed on
  REDCap. The browser blinds the NIP, so it never reaches the server; the key
  never leaves the server; and the server proves it used the key it publishes.
- **The record's earlier uploads are checked against the new one.** Its first
  upload stores a two-character signature of the patient (one byte of a keyed
  hash: it says two uploads are *a priori* the same patient, misses a wrong one
  1 time in 256, and cannot be traced back to a NIP); a later upload that does
  not match raises an alert before anything is sent. The check is a project
  checkbox, `pseudonym-fingerprint-check`, ticked by default. Deleting a record's content
  clears its fingerprint once nothing is left under the record in Girder.
- **The uploader reviews the transfer before anything is sent**, in a REDCap
  window like the Online Designer's: last and first name, date of birth, NIP,
  each DICOM series (modality, description, file count), ECG and Holter counts,
  and the result of the check. It stays in the browser, and the fingerprint is
  stored only once the uploader has confirmed. Cancelling leaves the field and
  the form as they were.
- **A mismatch can be confirmed.** The window turns to a red alert and offers
  **Upload anyway**. The record keeps its fingerprint, and the override is written
  to the project's Logging page, to the module's log and to the field, where the
  widget keeps showing it. A mismatch that appears only after the uploader
  confirmed, because of a simultaneous upload, is refused: nobody saw it.
- A file to deidentify **without a NIP** is set aside with its reason; a batch
  spanning **several patients** is refused as a whole.

### Added — event folder

- **Files are stored under `{record}/{event_id} ({event_name})/{field}`.** The same
  field on two events of a longitudinal project used to share one folder. Uploads
  sent before stay where they are.

### Fixed — security

- **Uploads and deletions follow the user's rights on the instrument.** The check
  stopped at a `method_exists()` that is false in production, so it let every user
  through, read-only ones included; behind it, it read form rights from a key the
  framework does not return, with fallback values that were the wrong way round. It
  now reads the right REDCap enforces on the request, or else the project's
  `data_entry` rights, and understands both encodings: legacy `0`–`3` and REDCap
  15's bitmask. Anything it cannot establish is a refusal.
- **Refreshing or deleting an upload is confined to the field.** Both took a folder
  id from the browser and acted on it with the project's Girder key, so a user who
  knew an id could read or delete any folder that key reaches, in another record
  or DAG. The server now checks that it is this field's folder, on this record, in
  the project's collection, and logs a refusal. An upload whose record was renamed
  since no longer matches: the field keeps showing what it stored.
- **A DICOM file wrapping a document is refused.** The anonymizer has no rule for
  `EncapsulatedDocument`, so a PDF or CDA report went through with the patient's
  identity inside it. The file is set aside with its reason, and the rest of the
  batch goes on. With DICOM deidentification off, it is sent as it is, like any
  file of that format.
- **A DICOM Secondary Capture is refused**, in all five classes, on the same terms:
  an image of a screen or a document can show the patient's identity in its pixels,
  which the module does not read. Other classes of images are not concerned.
- **Free text can no longer run in the form.** The page's configuration was printed
  in a `<script>` with `</script>` left intact, so a user with design rights could
  run code as anyone opening the form, through the project title. It is now escaped
  for a script block, and `pseudonym-domain` is restricted to a plain identifier.

### Updating from 1.x

Tested with a real update, from v1.2.0 and from v1.5.0: see "Updating from 1.x" in
the README, which lists what stays different for data sent before.

- **Needs PHP 8.1 or later with sodium**, declared in `config.json`. On an older
  PHP, REDCap refuses this version and disables the module across the instance,
  until the previous version is enabled again: check PHP before updating.
- **A form opened before the update can no longer upload.** It would have sent
  files deidentified the old way, past the fingerprint check; it now has to be
  reloaded.
- **Earlier uploads are not called a first upload.** With no fingerprint on a
  record that already holds uploads, the review window says they cannot be
  compared.
- A file without a NIP is refused, where 1.x accepted it.

### Changed — dates

- **Every date moves back by the patient's offset, 50 to 100 years**, derived
  from the pseudonym and applied in the browser: intervals between a patient's
  examinations are exact across uploads and formats, and the calendar — era,
  season, weekday — is gone. Dates no longer depend on the birth date: a file
  without one keeps its dates, shifted, where 1.5.1 removed them.
- **The birth date is blanked**, no longer pinned to 1970-01-01: the age at a
  procedure, exact to the day, all but identified the patient. DICOM files get
  `PatientAge` instead, in years (months under two, weeks under two months),
  and `LongitudinalTemporalInformationModified` = `MODIFIED`.
- `DeidentificationMethod` names the release: `IHU LIRYC REDCAP PLUGIN v2.0.0`.
  Files deidentified before sit on another calendar; this tells them apart.

### Changed — identifiers (CNIL MR-CNS-03/04)

- **DICOM UIDs are rewritten under a key**, HMAC-SHA256 with a key derived from
  the patient's pseudonym and the record id. The anonymizer's unkeyed hash let
  anyone with the original UID compute the new one, and so link a file back to
  the PACS. Series stay grouped, references stay valid, and one study in two
  records cannot be linked through its UIDs.
- **`AccessionNumber` and imaging order numbers are emptied**, no longer hashed.

### Changed — defaults (CNIL MR-SEC-05/06)

- **XML ECG and Schiller Holter deidentification are on by default**, so a new
  project no longer uploads them identified. An existing project keeps what it
  has stored, and uploads them identified until someone ticks the boxes.
- **Files are stored under neutral names by default**: folder and file names can
  carry the patient's identity. `preserve-upload-folder-architecture` keeps the
  previous behaviour.

### Fixed — Schiller Holter

- **Recordings whose header has `0x01` in byte 5 are accepted.** That byte varies
  between recordings with the same layout; such a Holter was dropped from the
  upload as an unsupported format.
- **A Holter is deidentified in place**, without copying it: a 340 MB recording
  no longer needs several times that in WASM memory.

### Settings

- Project: `pseudonym-fingerprint-check`, `pseudonym-domain` (filled in as
  `redcap:<project id>`).
- System: `pseudonym-redcap-key` (generated when empty), `pseudonym-daily-limit`.
- **Never change the key or the domain once files have been uploaded**, and back
  the key up.

## 1.5.1

### Fixed — deidentification

- **No DICOM date survives in clear any more.** Only seven well-known dates
  were moved by the birth-date offset; every other one was left to the
  anonymizer, which got three cases wrong:
  - its hash shift is keyed on `PatientID`, and a file *without* one kept its
    dates as they were — next to a rebased study date, a real date gives the
    birth date away by subtraction. A partial birth date (`1954`) was kept
    verbatim the same way;
  - it keeps any date it has no rule for, some 70 standard tags among them
    (`StartAcquisitionDateTime`, `RadiopharmaceuticalStartDateTime`, …);
  - a partial birth date in a file *with* a `PatientID` made it reject the
    whole file.

  Every `DA` and `DT` element, sequences included, is now settled before the
  anonymizer runs, and its own date shift is no longer used:
  - **with a birth date**, every date moves by the same offset, whatever the
    `PatientID`, so a file no longer mixes two calendars (a procedure step
    used to end 18 years before it began);
  - **without one** — absent, or not a full calendar date — every date is
    removed. Such files used to keep a study date hash-shifted by up to ten
    years.

## 1.5.0

### Fixed

- **DICOM files are recognized from their bytes, whatever their name.** Some
  exporters write DICOM under a `.vim` extension or with no extension, and
  without the Part 10 header: either the file meta group with no `DICM` in
  front, or the bare data set alone (explicit or implicit VR little endian).
  Those files were dropped as unsupported. The deidentifier now sniffs the
  first element of such files, parses them, and mints the missing file meta
  group, so the deidentified output is a regular Part 10 file. The widget then
  lays them out as DICOM in Girder based on the deidentifier's verdict rather
  than the file name.

### Changed — deidentification

- **A DICOMDIR is no longer uploaded.** It is recognized by its SOP class
  rather than its name, and skipped whether or not DICOM deidentification is
  enabled. It indexes the original media rather than holding data: it names
  every patient on the media in clear, and anonymizing it leaves a stale index
  — its directory records sit in a sequence, so the study date inside them is
  not shifted, while the file names and UIDs it points at are renamed and
  rehashed by the upload. On one real export the anonymized DICOMDIR still
  carried the original study date and 340 dangling file references. As with any
  skipped file, the rest of the batch goes through. An image merely *named*
  `DICOMDIR` is still uploaded.

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
