// Smoke test of the upload flow: one synthetic DICOM with a PatientID, on record 1.
const { test, expect } = require('@playwright/test');
const redcap = require('../helpers/redcap');
const girder = require('../helpers/girder');
const { parseDicom, daysShiftedBack } = require('../helpers/dicom');
const module_ = require('../helpers/module');
const { fixturePath, fixtureManifest, loadState } = require('../helpers/state');

// Design §3.5: offset = -(18 262 + (seed mod 18 263)) days, i.e. 50 to 100 years back.
const MIN_SHIFT_DAYS = 18262;
const MAX_SHIFT_DAYS = 18262 + 18262;

test.describe('upload flow', () => {
    let consoleLines = [];

    test.beforeEach(async ({ page }) => {
        consoleLines = [];
        page.on('console', (msg) => consoleLines.push(`[${msg.type()}] ${msg.text()}`));
        page.on('pageerror', (error) => consoleLines.push(`[pageerror] ${error.message}`));
    });

    test.afterEach(async ({}, testInfo) => {
        if (testInfo.status !== testInfo.expectedStatus) {
            await testInfo.attach('browser-console', { body: consoleLines.join('\n'), contentType: 'text/plain' });
        }
    });

    test('a DICOM uploaded on record 1 lands in Girder, pseudonymised', async ({ page }) => {
        const fixture = 'p1000001_20240115.dcm';
        const original = fixtureManifest()[fixture];
        const recordId = '1';
        const field = loadState().redcap.uploadField;

        await redcap.resetRecord(recordId);
        redcap.trackDialogs(page);
        await redcap.openRecordForm(page, recordId);
        await redcap.uploadFiles(page, [fixturePath(fixture)]);

        // The widget has the patient reviewed before sending anything.
        await expect(redcap.review(page)).toBeVisible({ timeout: 60_000 });
        await redcap.answerReview(page);

        const metadata = await redcap.waitForUploadOutcome(recordId);
        expect(metadata.uploadState, JSON.stringify(metadata.uploadState)).toMatchObject({ status: 'completed' });

        // Stored under {collection}/NO_DAG/{record}/{event}/{field}/...
        const files = await girder.listRecordFiles(recordId);
        expect(files.map((f) => f.path)).toHaveLength(1);
        expect(files[0].path.startsWith(`${module_.eventFolderName()}/${field}/`), files[0].path).toBe(true);

        const bytes = await girder.downloadFile(files[0].file._id);
        const text = bytes.toString('latin1');
        const [familyName, givenName] = original.patientName.split('^');
        expect(text).not.toContain(familyName);
        expect(text).not.toContain(givenName);

        const ds = parseDicom(bytes);
        expect(ds.get('PatientID')).not.toBe(original.patientId);
        // Birth date: present and empty (type 2), so age cannot be recomputed.
        expect(ds.has('PatientBirthDate')).toBe(true);
        expect(ds.get('PatientBirthDate')).toBe('');
        // Study date moved 50 to 100 years back.
        const shift = daysShiftedBack(original.studyDate, ds.get('StudyDate'));
        expect(shift).toBeGreaterThanOrEqual(MIN_SHIFT_DAYS);
        expect(shift).toBeLessThanOrEqual(MAX_SHIFT_DAYS);
        expect(text).not.toContain(original.studyDate);
        expect(text).not.toContain(original.birthDate);

        // After its save & reload, the widget shows the completed upload.
        await redcap.openRecordForm(page, recordId, { create: false });
        await expect(redcap.widget(page).locator('.girder-file-info h5')).toHaveText('Uploaded file');
    });
});
