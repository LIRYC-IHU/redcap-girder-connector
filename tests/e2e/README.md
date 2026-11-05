# End-to-end tests

A local Docker stack (REDCap + MariaDB, Girder 5 + MongoDB) provisioned without manual
steps, and Playwright scenarios that drive the module's widget in headless Chromium.

All credentials in this directory are throwaway values for this local stack. The
generated ones (Girder passwords, API key, REDCap tokens) live in `.state/state.json`,
which is gitignored.

## Quick start

```bash
scripts/build.sh            # from the repo root: WASM package into src/wasm/dicom_deid/pkg
cd tests/e2e
./run.sh up                 # build images, start the stack, wait until healthy
./run.sh provision          # install + configure REDCap and Girder (idempotent)
./run.sh fixtures           # synthetic DICOM files into fixtures/out/
./run.sh test               # Playwright (extra args are passed through)
```

`./run.sh all` chains the four. Other commands:

| Command | Effect |
| --- | --- |
| `./run.sh up --worker` | also start redis, rabbitmq and a Celery worker (see below) |
| `./run.sh rebuild-girder` | rebuild the Girder image, restart, re-provision |
| `./run.sh ps` / `./run.sh logs [service]` | status / last 200 log lines |
| `./run.sh down` | stop and remove the containers; volumes (data) are kept |
| `./run.sh reset` | `down` + delete the volumes and `.state/`: next `up` + `provision` is a fresh install |

Requirements: Docker Desktop, `jq`, `curl`, Node 20+. Playwright is pinned to
`@playwright/test` **1.59.1**, whose Chromium (build 1217) must already be in
`~/Library/Caches/ms-playwright`; `run.sh` never runs `playwright install`.

## Ports and services

Compose project `rgc-e2e`, bound to 127.0.0.1 only.

| Service | Host URL | Notes |
| --- | --- | --- |
| `redcap` | http://localhost:18081 | REDCap 15.9.3 from `docker-redcap-php:latest`, auth "None (Public)" (you are `site_admin`) |
| `girder` | http://localhost:18080 | Girder 5.0.9 (`girder/girder:v5.0.9-py3`, amd64, emulated on Apple silicon) |
| `mariadb`, `mongo` | internal | |
| `redis`, `rabbitmq`, `worker` | internal | profile `worker` only |

The module is the repository's `src/`, bind-mounted **read-only** at
`modules/girder_uploader_v<VERSION>` (REDCap identifies module versions by directory name;
`VERSION` is read by `run.sh`). PHP and JS edits are live; after a `VERSION` bump run
`./run.sh up && ./run.sh provision`. To test another copy of the module (a release, a
clean `git archive`), set `MODULE_SRC=/abs/path/to/src ./run.sh up`.

### Why the worker is optional

The upload flow only calls synchronous Girder endpoints (`POST /folder`, `/item`,
`/file`, `/file/chunk`, listings). Girder 5 hands `DELETE /folder`, folder copy and
collection deletion to Celery, so the module's *Delete uploaded contents* action needs the
worker (`./run.sh up --worker`); without it that request fails or hangs on the broker.
Redis only carries the Girder web UI's notifications. Once started, `run.sh` keeps the
worker profile on for `up` and `rebuild-girder`.

## What `provision.sh` sets up

**Girder** (REST, as admin):

- `admin`: the first user, so site administrator;
- a filesystem assetstore at `/var/girder/assetstore` (named volume);
- `redcap-module`: a **non-admin** user with an API key named `redcap-e2e` (full scope);
- collection `REDCap E2E`, owned by the admin, WRITE for `redcap-module`.

**REDCap**:

- fresh install through REDCap's own installer, `install.php?auto=1`, when the database
  has no `redcap_config` table yet. Auto mode applies the installer's defaults:
  authentication "None (Public)" (every request is `site_admin`, a super user), API on,
  demo projects;
- `redcap_base_url = http://localhost:18081/`, stats reporting off;
- a super API token for `site_admin` (SQL), used once to create the project;
- the module enabled system-wide (`redcap_external_modules` + system setting
  `version = v<VERSION>`; the framework expects the `v`);
- project **Girder Uploader E2E** created through the API (`content=project`), so that
  REDCap's own code creates the arm, event and creator rights. A raw-SQL project would
  have to reproduce a dozen tables and break on REDCap upgrades. Its data dictionary is
  then imported through the API: instrument `uploads`, fields `record_id` and
  `girder_files` (`notes`, i.e. textarea, annotated `@GIRDER_UPLOAD`);
- the module enabled on the project with these settings: backchannel
  `http://girder:8080/api/v1`, frontchannel `http://localhost:18080`, the
  `redcap-module` API key, the collection id, `deidentify-dicom`, `deidentify-xml-ecg` and
  `deidentify-schiller-holter` on, folder architecture preserved. Settings are written to
  `redcap_external_module_settings` in the format the EM framework uses;
- the project's record-list cache is built (see Troubleshooting).

`.state/state.json` then holds everything the tests need: URLs, project id, event id,
project API token, Girder ids, and the passwords and key for both Girder users.

## Fixtures

`fixtures/make_dicoms.py` runs in a throwaway `python:3.12-slim` + pydicom image (compose
service `fixtures`, profile `tools`) and writes to `fixtures/out/` (gitignored):

| File | PatientID | PatientName | BirthDate |
| --- | --- | --- | --- |
| `p1000001_{20240115,20240215}.dcm` | `1000001` | `ZZTEST^ALICE` | 19540212 |
| `p1000002_{20240115,20240215}.dcm` | `1000002` | `ZZTEST^BRUNO` | 19601105 |
| `noid_{…}.dcm` | *element absent* | `ZZTEST^CHARLES` | 19720330 |
| `emptyid_{…}.dcm` | *present, empty* | `ZZTEST^DIANE` | 19810719 |
| `html_{…}.dcm` | `1000003` | active HTML, also in SeriesDescription | 19900101 |
| `report_{…}.dcm` | `1000001` | `ZZTEST^ALICE`, wrapping a PDF | 19540212 |
| `capture_{…}.dcm` | `1000001` | `ZZTEST^ALICE`, a Secondary Capture | 19540212 |
| `derived_{…}.dcm` | `1000001` | `ZZTEST^ALICE`, `DERIVED\SECONDARY\MPR` | 19540212 |
| `fluoro_{…}.dcm` | `1000001` | `ZZTEST^ALICE`, `DERIVED\PRIMARY\SINGLE PLANE` | 19540212 |

Each is an 8x8 MONOCHROME2 X-ray angiography image, explicit VR little endian, with deterministic
UIDs. StudyDate/SeriesDate/ContentDate are 20240115 or 20240215. Institution, referring
physician and accession number are also `ZZTEST…` values. `manifest.json` describes every
file, so tests read expectations from it instead of hard-coding them.

## Test helpers

- `helpers/state.js`: `loadState()`, `fixturePath(name)`, `fixtureManifest()`.
- `helpers/dicom.js`: `parseDicom(buffer)` (top-level elements; `get(name)` is `null` when
  absent and `''` when present but empty), `daysShiftedBack(originalDA, shiftedDA)`.
- `helpers/redcap.js`: `ensureRecord(id, fields)`, `resetRecord(id)` (empty field + Girder
  folder moved away + record-list cache warmed), `openRecordForm(page, id)`,
  `uploadFiles(page, paths)` (the widget's hidden `<input type=file>`),
  `waitForUploadOutcome(id)` (polls the field the widget saves until
  `uploadState.status` is `completed` or `failed`, so it does not depend on the reload the
  widget triggers), `review(page)` (the transfer review window), `answerReview(page,
  {accept})` (confirms or cancels it, resolves with its text),
  `waitForReviewOrOutcome(page, id)` (for an upload that may be refused before any
  review), `trackDialogs(page, {accept})` (answers the delete button's `window.confirm()`
  and REDCap's "leave this page?" prompt; call it before acting), `readWidgetStatus(page)`,
  `readUploadMetadata(id)`, `api(params)`.
- `helpers/girder.js` (as admin): `recordFolder(id)`, `listRecordFiles(id)` (files under
  `{collection}/NO_DAG/{record}/…`, so paths start with the event folder),
  `downloadFile(fileId)`, `moveRecordFolderToTrash(id)`,
  `request(method, path, {params})`.

`resetRecord` moves the old record folder into an admin-only `E2E Trash` collection rather
than deleting it, because a move is synchronous and needs no worker.

## Security

`specs/security.spec.js` acts as real users through REDCap's "View project as user":
`e2e_read_only` (form right 129) must be refused every action from the console, and
`e2e_editor` (130) must be able to upload without being an administrator. It also puts
active HTML in the project title and in `pseudonym-domain`, asks to delete and refresh
folders outside the field, and sends a DICOM file wrapping a PDF and a Secondary Capture.

## Updating from a release

`./run.sh upgrade [tag]` (default `v1.5.0`) checks that this version can replace a release
on an installation that already holds data. It builds the tag once under
`.state/legacy/` (its WASM is not in git), then `specs/upgrade.spec.js`:

1. copies the release next to the current version in REDCap's `modules/` and makes the
   database look as it left it: that version enabled, nothing about pseudonyms;
2. uploads with the release, writes the field's oldest format and a failed upload into
   other records, and leaves a form open in a second browser;
3. updates through `manager/ajax/enable-module.php`, the Control Center's own route;
4. checks the settings, the earlier uploads, the open form, a new upload on a record
   with earlier data, and deleting. The last steps need the worker profile.

Records use the prefix `up`. The stack ends on the current version with its REDCap key,
but without the fingerprints it had. `./run.sh test` skips this spec.

## The patient pseudonym

Nothing pseudonym-specific is provisioned: the module generates
`pseudonym-redcap-key` and `pseudonym-domain` on first use, and
`pseudonym-fingerprint-check` defaults to on. Fingerprints are stored per record in the
module's settings and are not touched by `resetRecord`. A scenario that needs a fresh
record should use a new record id, or the module's delete action (worker profile).

## Troubleshooting

- **Upload fails with `Unexpected token '<' … is not valid JSON`.** REDCap quirk: while a
  project's record-list cache is `NOT_STARTED`, the first request into the project is
  redirected to `DataEntryController:buildRecordListCache`. If that request is the
  module's AJAX POST, it gets a 302 to HTML. Provisioning and `resetRecord` build the
  cache first. REDCap's cron drops the cache of idle projects, so a long-idle stack can
  show this again: re-run `./run.sh provision`. The same can happen in production.
- **`cannot log in as Girder admin`.** The volumes predate `.state/` (it was deleted).
  `./run.sh reset`, then `up` + `provision`.
- **Widget missing on the form.** Check that the module is mounted
  (`./run.sh logs redcap`, `docker compose -p rgc-e2e exec redcap ls /var/www/html/modules`)
  and that `provision` ran after the last `VERSION` change.
- **Debugging a failure.** Failed tests keep a trace, a screenshot and the browser
  console (`test-results/`, `npx playwright show-report`, `npx playwright show-trace …`).
  The module's server-side log is under REDCap → project → External Modules → View Logs.
- **Girder is slow to start.** It runs under amd64 emulation on Apple silicon; `up`
  waits for its healthcheck.

## Tear down

```bash
./run.sh down     # keep data
./run.sh reset    # wipe data and .state/
```
