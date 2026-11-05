// Updating an installation that already holds data: uploads made with a released
// version, the update through the Control Center's own route, then everything a user
// does with those data. Run with ./run.sh upgrade [tag], which builds that release.
const crypto = require('node:crypto');
const { test, expect } = require('@playwright/test');
const redcap = require('../helpers/redcap');
const girder = require('../helpers/girder');
const module_ = require('../helpers/module');
const { parseDicom, daysShiftedBack } = require('../helpers/dicom');
const { fixturePath, fixtureManifest, loadState } = require('../helpers/state');

const FROM = process.env.UPGRADE_FROM;
const SOURCE = process.env.UPGRADE_SRC;
const RUN = Date.now().toString(36);
const RECORD = {
    legacy: `up${RUN}a`,
    otherPatient: `up${RUN}b`,
    oldFormat: `up${RUN}c`,
    failed: `up${RUN}d`,
    stalePage: `up${RUN}e`,
};

const sha256 = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');

async function snapshot(recordId) {
    const files = await girder.listRecordFiles(recordId);
    return Promise.all(files.map(async (entry) => ({
        path: entry.path,
        id: entry.file._id,
        hash: sha256(await girder.downloadFile(entry.file._id)),
    })));
}

/** An upload as the released version does it: no review window. */
async function uploadWithRelease(page, recordId, fixture) {
    redcap.trackDialogs(page);
    await redcap.openRecordForm(page, recordId);
    await redcap.uploadFiles(page, [fixturePath(fixture)]);
    const metadata = await redcap.waitForUploadOutcome(recordId);
    expect(metadata.uploadState, JSON.stringify(metadata.uploadState)).toMatchObject({ status: 'completed' });
    page.removeAllListeners('dialog');
    return metadata;
}

async function uploadWithReview(page, recordId, fixture, field) {
    redcap.trackDialogs(page);
    await redcap.openRecordForm(page, recordId, { create: false });
    await redcap.uploadFiles(page, [fixturePath(fixture)], field);
    await expect(redcap.review(page)).toBeVisible({ timeout: 60_000 });
    const review = await redcap.answerReview(page);
    const metadata = await redcap.waitForUploadOutcome(recordId, { field });
    page.removeAllListeners('dialog');
    return { review, metadata };
}

test.describe.configure({ mode: 'serial' });

test.describe(`update from ${FROM}`, () => {
    test.skip(!FROM, 'run through ./run.sh upgrade');

    const current = () => `v${loadState().module.version}`;
    const before = {};
    let stalePage = null;
    let staleContext = null;
    let key = null;
    const pageErrors = [];

    test.beforeAll(async ({ browser }) => {
        await redcap.deleteRecordsWithPrefix('up');
        await redcap.deleteRecordsWithPrefix('pz');
        await redcap.warmRecordListCache();
        key = module_.getSystemSetting('pseudonym-redcap-key');
        module_.installVersion(FROM, SOURCE);
        module_.rollBackTo(FROM);
        // A project that left these two as they came under 1.x, and kept the folders.
        const pid = Number(loadState().redcap.projectId);
        module_.setSetting(pid, 'deidentify-xml-ecg', 'boolean', 'false');
        module_.setSetting(pid, 'deidentify-schiller-holter', 'boolean', 'false');
        module_.setSetting(pid, 'preserve-upload-folder-architecture', 'boolean', 'true');
        staleContext = await browser.newContext();
    });

    test.afterAll(async () => {
        await staleContext.close();
        const pid = Number(loadState().redcap.projectId);
        module_.setSetting(null, 'version', 'string', current());
        module_.setSetting(null, 'pseudonym-redcap-key', 'string', key);
        module_.setSetting(pid, 'deidentify-xml-ecg', 'boolean', 'true');
        module_.setSetting(pid, 'deidentify-schiller-holter', 'boolean', 'true');
        module_.removeVersion(FROM);
    });

    test('the release uploads data', async ({ page }) => {
        expect(module_.getSystemSetting('version')).toBe(FROM);

        await uploadWithRelease(page, RECORD.legacy, 'p1000001_20240115.dcm');
        await uploadWithRelease(page, RECORD.otherPatient, 'p1000002_20240115.dcm');
        await uploadWithRelease(page, RECORD.oldFormat, 'p1000001_20240215.dcm');
        before.legacy = await snapshot(RECORD.legacy);
        expect(before.legacy).toHaveLength(1);
        // Really the release's work: it stamps no version on its method.
        const dataset = parseDicom(await girder.downloadFile(before.legacy[0].id));
        expect(dataset.get('DeidentificationMethod')).toBe('IHU LIRYC REDCAP PLUGIN');
        before.studyDate = dataset.get('StudyDate');

        // The oldest format of the field: the full upload, no summary, no rejections.
        const stored = await redcap.readUploadMetadata(RECORD.oldFormat);
        await redcap.ensureRecord(RECORD.oldFormat, {
            [loadState().redcap.uploadField]: JSON.stringify({
                version: 1,
                uploadedAt: stored.uploadedAt,
                uploadedFiles: stored.uploadedFiles.map(({ name, size, mimeType }) => ({ name, size, mimeType })),
                totalSizeBytes: stored.totalSizeBytes,
                uploadState: stored.uploadState,
                girder: { baseUrl: stored.girder.baseUrl, parentFolderId: stored.girder.parentFolderId },
            }),
        });
        await redcap.ensureRecord(RECORD.failed, {
            [loadState().redcap.uploadField]: JSON.stringify({
                version: 1,
                uploadedAt: null,
                uploadedFiles: [],
                uploadState: { status: 'failed', stage: 'error', updatedAt: stored.uploadedAt, error: 'Girder request failed (500)' },
                girder: {},
            }),
        });

        // A user who has the form open while the module is updated.
        stalePage = await staleContext.newPage();
        redcap.trackDialogs(stalePage);
        await redcap.openRecordForm(stalePage, RECORD.stalePage);
    });

    test('the update goes through the Control Center, and creates the key', async ({ page }) => {
        const { version } = loadState().redcap;
        await page.goto(`/redcap_v${version}/ExternalModules/manager/control_center.php`);
        const answer = await page.evaluate(async ([prefix, target]) => {
            return window.jQuery.post(`${window.app_path_webroot}ExternalModules/manager/ajax/enable-module.php`, { prefix, version: target });
        }, [loadState().module.prefix, current()]);
        const result = typeof answer === 'string' ? JSON.parse(answer) : answer;
        expect(result.error_message || '').toBe('');
        expect(result.message).toBe('success');

        expect(module_.getSystemSetting('version')).toBe(current());
        expect(module_.getSystemSetting('pseudonym-redcap-key')).toMatch(/^[0-9a-f]{64}$/);
        expect(module_.getSystemSetting('pseudonym-daily-limit')).toBe('500');

        // Back to the stack's key: under a fresh one, the two test patients would share
        // their signature once in 256 runs, and the mismatch below would not show.
        if (key) {
            module_.setSetting(null, 'pseudonym-redcap-key', 'string', key);
        }
    });

    test('the project keeps its settings, and the check is on', async () => {
        expect(module_.getProjectSetting('enabled')).toBe('true');
        expect(module_.getProjectSetting('deidentify-xml-ecg')).toBe('false');
        expect(module_.getProjectSetting('deidentify-schiller-holter')).toBe('false');
        expect(module_.getProjectSetting('preserve-upload-folder-architecture')).toBe('true');
        expect(module_.getProjectSetting('pseudonym-fingerprint-check'), 'never stored: the default applies').toBeNull();
    });

    test('earlier uploads still show, refresh and link to Girder', async ({ page }) => {
        page.on('pageerror', (error) => pageErrors.push(error.message));
        for (const recordId of [RECORD.legacy, RECORD.oldFormat]) {
            redcap.trackDialogs(page);
            await redcap.openRecordForm(page, recordId, { create: false });
            const widget = redcap.widget(page);
            await expect(widget.locator('.girder-file-info h5')).toHaveText('Uploaded file');
            await expect(widget.locator('.girder-upload-status')).toContainText('Girder metadata refreshed', { timeout: 30_000 });
            await expect(widget.locator('.girder-file-info')).toContainText('Files: 1');
            await expect(widget.locator('.girder-file-info a')).toHaveAttribute('href', /#folder\//);
            await expect(widget.locator('.girder-file-alert')).toHaveCount(0);
            page.removeAllListeners('dialog');
        }
        expect(pageErrors).toEqual([]);
    });

    test('an upload that had failed can be sent again', async ({ page }) => {
        redcap.trackDialogs(page);
        await redcap.openRecordForm(page, RECORD.failed, { create: false });
        await expect(redcap.widget(page).locator('.girder-upload-status')).toContainText('Previous upload failed');
        await redcap.uploadFiles(page, [fixturePath('p1000001_20240115.dcm')]);
        await expect(redcap.review(page)).toContainText('First upload for this record', { timeout: 60_000 });
        await redcap.answerReview(page);
        await expect.poll(async () => (await redcap.readUploadMetadata(RECORD.failed)).uploadState.status, { timeout: 60_000 }).toBe('completed');
        page.removeAllListeners('dialog');
    });

    test('a page opened before the update uploads nothing', async () => {
        await redcap.uploadFiles(stalePage, [fixturePath('p1000001_20240115.dcm')]);
        const metadata = await redcap.waitForUploadOutcome(RECORD.stalePage);

        expect(metadata.uploadState.status).toBe('failed');
        expect(metadata.uploadState.error).toMatch(/opened before the module was updated/);
        expect(await girder.listRecordFiles(RECORD.stalePage)).toHaveLength(0);
    });

    test('a new upload on a record with earlier data is not called a first upload', async ({ page }) => {
        const followUp = loadState().redcap.followUpField;
        const { review, metadata } = await uploadWithReview(page, RECORD.legacy, 'p1000001_20240215.dcm', followUp);

        expect(review).toMatch(/Earlier uploads cannot be compared/);
        expect(review).toMatch(/1 earlier upload from a version/);
        expect(metadata.uploadState.status).toBe('completed');
        expect(module_.storedFingerprint(RECORD.legacy)).toMatch(/^[0-9a-f]{2}$/);

        // The release's file is untouched, the new one sits next to it, under the new rules.
        const after = await snapshot(RECORD.legacy);
        expect(after).toHaveLength(2);
        expect(after).toEqual(expect.arrayContaining(before.legacy));
        const added = after.find((entry) => entry.id !== before.legacy[0].id);
        expect(added.path.startsWith(`${module_.eventFolderName()}/${followUp}/`), added.path).toBe(true);
        expect(before.legacy[0].path.startsWith(`${loadState().redcap.uploadField}/`), 'the release\'s path stays').toBe(true);
        const dataset = parseDicom(await girder.downloadFile(added.id));
        const shift = daysShiftedBack(fixtureManifest()['p1000001_20240215.dcm'].studyDate, dataset.get('StudyDate'));
        expect(shift).toBeGreaterThanOrEqual(18262);
        expect(shift).toBeLessThanOrEqual(36524);
        expect(dataset.get('DeidentificationMethod')).toContain(current());

        // The limit of an update: the release's dates follow another calendar, so an
        // interval across the two means nothing. These studies are 31 days apart.
        const interval = daysShiftedBack(before.studyDate, dataset.get('StudyDate'));
        console.log(`interval across versions: ${interval} days, 31 in reality`);
        expect(interval).not.toBe(-31);
        expect(interval).not.toBe(31);
    });

    test('from then on the record is checked', async ({ page }) => {
        // The first field is taken: free it through the widget, as a user would.
        test.skip(!module_.compose(['ps', '-q', 'worker']).trim(), 'deleting needs ./run.sh up --worker');
        redcap.trackDialogs(page);
        await redcap.openRecordForm(page, RECORD.legacy, { create: false });
        await redcap.widget(page).locator('.girder-delete-button').first().click();
        await expect(redcap.widget(page).locator('.girder-upload-status')).toContainText('Uploaded contents deleted', { timeout: 60_000 });
        page.removeAllListeners('dialog');
        expect(module_.storedFingerprint(RECORD.legacy), 'kept: the follow-up upload is still there').toMatch(/^[0-9a-f]{2}$/);

        const { review } = await uploadWithReview(page, RECORD.legacy, 'p1000002_20240115.dcm', loadState().redcap.uploadField);
        expect(review).toMatch(/Patient signature mismatch/);
    });

    test('a record whose only upload came from the release can be emptied', async ({ page }) => {
        test.skip(!module_.compose(['ps', '-q', 'worker']).trim(), 'deleting needs ./run.sh up --worker');
        redcap.trackDialogs(page);
        await redcap.openRecordForm(page, RECORD.otherPatient, { create: false });
        await redcap.widget(page).locator('.girder-delete-button').click();
        await expect(redcap.widget(page).locator('.girder-upload-status')).toContainText('Uploaded contents deleted', { timeout: 60_000 });
        page.removeAllListeners('dialog');
        expect(await girder.listRecordFiles(RECORD.otherPatient)).toHaveLength(0);
    });
});
