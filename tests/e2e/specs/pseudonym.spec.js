// The patient pseudonym end to end: NIP checks, the fingerprint stored per record, the
// date offset, and the ways an upload must be refused. See docs/design/pseudonymisation.md
// (§8 lists these scenarios).
const { test, expect } = require('@playwright/test');
const redcap = require('../helpers/redcap');
const girder = require('../helpers/girder');
const module_ = require('../helpers/module');
const { parseDicom, daysShiftedBack } = require('../helpers/dicom');
const { fixturePath, fixtureManifest, loadState } = require('../helpers/state');

// Fingerprints stay stored per record across runs: every run uses fresh record ids.
const RUN = Date.now().toString(36);
let counter = 0;
function freshRecord() {
    counter += 1;
    return `pz${RUN}${counter}`;
}

const FINGERPRINT = /^[0-9a-f]{2}$/;
const OVERRIDE_LOG = '[PSEUDONYM] fingerprint mismatch confirmed by the uploader';
const PROJECT_ALERT = 'Girder Uploader: patient signature mismatch confirmed';

/**
 * Upload fixtures to a record through its form, answer the review window if one opens,
 * and wait for the stored outcome. One field holds one upload, so a record's later
 * examinations go in `follow-up`. `review` is the window's text, null when none opened.
 */
async function upload(page, recordId, fixtures, { accept = true, followUp = false } = {}) {
    const field = followUp ? loadState().redcap.followUpField : loadState().redcap.uploadField;
    redcap.trackDialogs(page);
    await redcap.openRecordForm(page, recordId);
    await redcap.uploadFiles(page, fixtures.map(fixturePath), field);
    let review = null;
    let metadata = await redcap.waitForReviewOrOutcome(page, recordId, { field });
    if (metadata === 'review') {
        review = await redcap.answerReview(page, { accept });
        metadata = accept ? await redcap.waitForUploadOutcome(recordId, { field }) : await redcap.readUploadMetadata(recordId, field);
    }
    page.removeAllListeners('dialog');
    return { metadata, review };
}

async function datesInGirder(recordId) {
    const files = await girder.listRecordFiles(recordId);
    const dates = [];
    for (const entry of files) {
        dates.push(parseDicom(await girder.downloadFile(entry.file._id)).get('StudyDate'));
    }
    return dates.sort();
}

function expectCompleted(metadata) {
    expect(metadata.uploadState, JSON.stringify(metadata.uploadState)).toMatchObject({ status: 'completed' });
}

function expectFailed(metadata, message) {
    expect(metadata.uploadState.status, JSON.stringify(metadata.uploadState)).toBe('failed');
    expect(metadata.uploadState.error).toMatch(message);
}

test.describe.configure({ mode: 'serial' });

test.describe('patient pseudonym', () => {
    test.beforeAll(async () => {
        // Records of earlier runs: a development project holds at most 50.
        await redcap.deleteRecordsWithPrefix('pz');
        module_.forgetFingerprints('pz');
        await redcap.warmRecordListCache();
        module_.setFingerprintCheck(true);
        module_.setDailyLimit(null);
    });

    test('1. a file without NIP is refused, the patient\'s files go through', async ({ page }) => {
        const recordId = freshRecord();
        const { metadata } = await upload(page, recordId, ['noid_20240115.dcm', 'p1000001_20240115.dcm']);

        expectCompleted(metadata);
        expect(await girder.listRecordFiles(recordId)).toHaveLength(1);
        expect(metadata.rejectedFiles.map((entry) => entry.name)).toEqual(['noid_20240115.dcm']);
        expect(metadata.rejectedFiles[0].reason).toMatch(/no patient identifier \(NIP\)/);
    });

    test('1b. a batch whose files all lack a NIP uploads nothing', async ({ page }) => {
        const recordId = freshRecord();
        const { metadata } = await upload(page, recordId, ['noid_20240115.dcm', 'emptyid_20240115.dcm']);

        expectFailed(metadata, /NIP/);
        expect(await girder.listRecordFiles(recordId)).toHaveLength(0);
        expect(module_.storedFingerprint(recordId)).toBeNull();
    });

    test('2-3. the first upload stores the fingerprint; the same patient later is accepted, intervals exact', async ({ page }) => {
        const recordId = freshRecord();
        const first = await upload(page, recordId, ['p1000001_20240115.dcm']);
        expectCompleted(first.metadata);
        // The uploader saw the patient, the content and the check before confirming.
        for (const line of [/1000001/, /ZZTEST/, /ALICE/, /12\/02\/1954/, /DICOM\s+OT/, /First upload for this record/]) {
            expect(first.review).toMatch(line);
        }
        const fingerprint = module_.storedFingerprint(recordId);
        expect(fingerprint).toMatch(FINGERPRINT);

        const second = await upload(page, recordId, ['p1000001_20240215.dcm'], { followUp: true });
        expectCompleted(second.metadata);
        expect(second.review).toMatch(/Same patient signature/);
        expect(second.review).toMatch(/1 earlier upload of this record/);
        expect(module_.storedFingerprint(recordId)).toBe(fingerprint);

        // Both studies shifted by the same offset: 31 days apart, as in reality.
        const dates = await datesInGirder(recordId);
        expect(dates).toHaveLength(2);
        expect(daysShiftedBack(dates[1], dates[0])).toBe(31);
        const original = fixtureManifest()['p1000001_20240115.dcm'];
        const shift = daysShiftedBack(original.studyDate, dates[0]);
        expect(shift).toBeGreaterThanOrEqual(18262);
        expect(shift).toBeLessThanOrEqual(36524);
    });

    test('the offset belongs to the patient, whatever the record; UIDs to the record', async ({ page }) => {
        const [a, b] = [freshRecord(), freshRecord()];
        expectCompleted((await upload(page, a, ['p1000001_20240115.dcm'])).metadata);
        expectCompleted((await upload(page, b, ['p1000001_20240115.dcm'])).metadata);

        expect(await datesInGirder(a)).toEqual(await datesInGirder(b));
        expect(module_.storedFingerprint(a)).toBe(module_.storedFingerprint(b));

        // The same study in two records cannot be linked through its UID.
        const uid = async (recordId) => {
            const [entry] = await girder.listRecordFiles(recordId);
            return parseDicom(await girder.downloadFile(entry.file._id)).get('StudyInstanceUID');
        };
        const original = fixtureManifest()['p1000001_20240115.dcm'].studyInstanceUID;
        expect(await uid(a)).not.toBe(original);
        expect(await uid(a)).not.toBe(await uid(b));
    });

    test('4a. another patient on the same record: cancelling sends nothing and leaves no trace', async ({ page }) => {
        const recordId = freshRecord();
        expectCompleted((await upload(page, recordId, ['p1000001_20240115.dcm'])).metadata);
        const fingerprint = module_.storedFingerprint(recordId);
        const overridesBefore = module_.countLogs(OVERRIDE_LOG);

        const { metadata, review } = await upload(page, recordId, ['p1000002_20240115.dcm'], { followUp: true, accept: false });
        expect(review).toMatch(/Patient signature mismatch/);
        expect(metadata, 'the field stays empty').toBeNull();
        expect(await girder.listRecordFiles(recordId)).toHaveLength(1);
        expect(module_.storedFingerprint(recordId)).toBe(fingerprint);
        expect(module_.countLogs(OVERRIDE_LOG)).toBe(overridesBefore);
    });

    test('4b. another patient on the same record: confirming sends it and raises the alert', async ({ page }) => {
        const recordId = freshRecord();
        const followUp = loadState().redcap.followUpField;
        expectCompleted((await upload(page, recordId, ['p1000001_20240115.dcm'])).metadata);
        const fingerprint = module_.storedFingerprint(recordId);
        const overridesBefore = module_.countLogs(OVERRIDE_LOG);
        const projectLogsBefore = module_.countProjectLogs(PROJECT_ALERT);

        const { metadata, review } = await upload(page, recordId, ['p1000002_20240115.dcm'], { followUp: true });
        expect(review).toMatch(/Patient signature mismatch/);
        expect(review).toMatch(/Upload anyway/);
        expectCompleted(metadata);
        expect(await girder.listRecordFiles(recordId)).toHaveLength(2);
        expect(module_.storedFingerprint(recordId), 'the record keeps its signature').toBe(fingerprint);

        // The alert: in the module's log, the project's, and on the field.
        expect(module_.countLogs(OVERRIDE_LOG)).toBe(overridesBefore + 1);
        expect(module_.countProjectLogs(PROJECT_ALERT)).toBe(projectLogsBefore + 1);
        expect(metadata.patientAlert).toMatchObject({ type: 'fingerprint-mismatch' });

        // It survives the widget's refresh from Girder.
        await redcap.openRecordForm(page, recordId, { create: false });
        const alert = redcap.widget(page, followUp).locator('.girder-file-alert');
        await expect(alert).toContainText('Patient signature mismatch');
        await expect(redcap.widget(page, followUp).locator('.girder-upload-status')).toContainText('Girder metadata refreshed');
        await expect(alert).toContainText('Patient signature mismatch');
        await expect(redcap.widget(page).locator('.girder-file-alert'), 'the first upload raised none').toHaveCount(0);
    });

    test('4c. what a file says is shown as text, never run', async ({ page }) => {
        const recordId = freshRecord();
        redcap.trackDialogs(page);
        await redcap.openRecordForm(page, recordId);
        await redcap.uploadFiles(page, [fixturePath('html_20240115.dcm')]);
        const review = redcap.review(page);
        await expect(review).toBeVisible({ timeout: 60_000 });

        await expect(review.locator('.ui-dialog-title')).toHaveText('Review this transfer');
        await expect(review.locator('.girder-review-patient')).toContainText('ZZTEST<img src=x onerror=window.__xss=1>');
        await expect(review.locator('.girder-review-content')).toContainText('<img src=x onerror=window.__xss=2>');
        await expect(review.locator('img')).toHaveCount(0);
        expect(await page.evaluate(() => window.__xss)).toBeUndefined();
        await review.getByRole('button', { name: 'Cancel' }).click();
        page.removeAllListeners('dialog');
    });

    test('4d. cancelling takes the identity off the page and leaves the widget ready', async ({ page }) => {
        const recordId = freshRecord();
        redcap.trackDialogs(page);
        await redcap.openRecordForm(page, recordId);
        await redcap.uploadFiles(page, [fixturePath('p1000001_20240115.dcm')]);
        await expect(redcap.review(page)).toBeVisible({ timeout: 60_000 });
        // Enter must not send anything: the focus starts on Cancel.
        await expect(redcap.review(page).getByRole('button', { name: 'Cancel' })).toBeFocused();
        await page.keyboard.press('Enter');

        await expect(redcap.review(page)).toHaveCount(0);
        expect(await page.content()).not.toContain('ZZTEST');
        await expect(redcap.widget(page).locator('.girder-upload-status')).toHaveText('Upload cancelled. Nothing was sent.');

        // The same file again, and the Escape key is a cancel too.
        await redcap.uploadFiles(page, [fixturePath('p1000001_20240115.dcm')]);
        await expect(redcap.review(page)).toBeVisible({ timeout: 60_000 });
        await page.keyboard.press('Escape');
        await expect(redcap.review(page)).toHaveCount(0);
        expect(await redcap.readUploadMetadata(recordId)).toBeNull();
        page.removeAllListeners('dialog');
    });

    test('5. a batch of two patients is refused as a whole', async ({ page }) => {
        const recordId = freshRecord();
        const { metadata } = await upload(page, recordId, ['p1000001_20240115.dcm', 'p1000002_20240115.dcm']);

        expectFailed(metadata, /2 different patients/);
        // Refused or not, no identity may reach REDCap.
        const fixtures = fixtureManifest();
        for (const name of ['p1000001_20240115.dcm', 'p1000002_20240115.dcm']) {
            const [family, given] = fixtures[name].patientName.split('^');
            for (const value of [family, given, fixtures[name].patientId, fixtures[name].birthDate.slice(0, 4)]) {
                expect(JSON.stringify(metadata)).not.toContain(value);
            }
        }
        expect(await girder.listRecordFiles(recordId)).toHaveLength(0);
        expect(module_.storedFingerprint(recordId)).toBeNull();
    });

    test('6. two simultaneous first uploads of different patients: one wins', async ({ browser }) => {
        const recordId = freshRecord();
        await redcap.ensureRecord(recordId);
        const mismatchesBefore = module_.countLogs('[PSEUDONYM] fingerprint mismatch');
        const contexts = [await browser.newContext(), await browser.newContext()];
        const pages = await Promise.all(contexts.map((context) => context.newPage()));
        try {
            for (const page of pages) {
                redcap.trackDialogs(page);
                await redcap.openRecordForm(page, recordId, { create: false });
            }
            await Promise.all([
                redcap.uploadFiles(pages[0], [fixturePath('p1000001_20240115.dcm')]),
                redcap.uploadFiles(pages[1], [fixturePath('p1000002_20240115.dcm')]),
            ]);
            // Both were told it is the record's first upload, then both confirm.
            for (const page of pages) {
                await expect(redcap.review(page)).toContainText('First upload for this record', { timeout: 60_000 });
            }
            await Promise.all(pages.map((page) => redcap.review(page).getByRole('button', { name: /^Upload/ }).click()));

            // One upload is refused for the other patient, logged by the module (its
            // status line does not survive the winner's save and reload).
            await expect.poll(() => module_.countLogs('[PSEUDONYM] fingerprint mismatch') - mismatchesBefore, { timeout: 90_000 }).toBe(1);
            await expect.poll(async () => (await girder.listRecordFiles(recordId)).length, { timeout: 90_000 }).toBe(1);
            expect(module_.storedFingerprint(recordId)).toMatch(FINGERPRINT);
        } finally {
            await Promise.all(contexts.map((context) => context.close()));
        }
    });

    test('7a. REDCap\'s daily limit stops the evaluation', async ({ page }) => {
        const recordId = freshRecord();
        module_.setDailyLimit(module_.recentEvaluations());
        try {
            const { metadata } = await upload(page, recordId, ['p1000001_20240115.dcm']);
            expectFailed(metadata, /pseudonym limit/);
            expect(await girder.listRecordFiles(recordId)).toHaveLength(0);
        } finally {
            module_.setDailyLimit(null);
        }
    });

    test('8. deleting the record\'s content clears its fingerprint', async ({ page }) => {
        test.skip(!module_.compose(['ps', '-q', 'worker']).trim(), 'Girder 5 deletes folders through its worker: run ./run.sh up --worker');
        const recordId = freshRecord();
        expectCompleted((await upload(page, recordId, ['p1000001_20240115.dcm'])).metadata);
        expect(module_.storedFingerprint(recordId)).toMatch(FINGERPRINT);

        redcap.trackDialogs(page);
        await redcap.openRecordForm(page, recordId, { create: false });
        await redcap.widget(page).locator('.girder-delete-button').click();
        await expect(redcap.widget(page).locator('.girder-upload-status')).toContainText('Uploaded contents deleted', { timeout: 60_000 });
        page.removeAllListeners('dialog');
        expect(module_.storedFingerprint(recordId)).toBeNull();

        // Another patient can now be sent to the record.
        expectCompleted((await upload(page, recordId, ['p1000002_20240115.dcm'])).metadata);
    });

    test('9. with the check unticked, another patient is accepted and nothing is stored', async ({ page }) => {
        const recordId = freshRecord();
        module_.setFingerprintCheck(false);
        try {
            expectCompleted((await upload(page, recordId, ['p1000001_20240115.dcm'])).metadata);
            expectCompleted((await upload(page, recordId, ['p1000002_20240115.dcm'], { followUp: true })).metadata);
            expect(module_.storedFingerprint(recordId)).toBeNull();
            // Dates are still shifted.
            for (const date of await datesInGirder(recordId)) {
                expect(daysShiftedBack('20240115', date)).toBeGreaterThanOrEqual(18262);
            }
        } finally {
            module_.setFingerprintCheck(true);
        }
    });

    test('declining the confirmation stores and sends nothing', async ({ page }) => {
        const recordId = freshRecord();
        const { metadata, review } = await upload(page, recordId, ['p1000001_20240115.dcm'], { accept: false });

        expect(review).toMatch(/First upload for this record/);
        expect(metadata, 'cancelling leaves the field as it was').toBeNull();
        expect(module_.storedFingerprint(recordId), 'the fingerprint is stored only once confirmed').toBeNull();
        expect(await girder.listRecordFiles(recordId)).toHaveLength(0);
    });

    // Last: it stops Girder, which takes a while to come back.
    test('7b. with Girder down, nothing is stored or uploaded', async ({ page }) => {
        const recordId = freshRecord();
        await redcap.ensureRecord(recordId);
        module_.compose(['stop', 'girder']);
        try {
            const { metadata, review } = await upload(page, recordId, ['p1000001_20240115.dcm']);
            expectFailed(metadata, /Girder is unavailable/);
            expect(review, 'refused before the uploader is asked anything').toBeNull();
            expect(module_.storedFingerprint(recordId)).toBeNull();
        } finally {
            module_.compose(['start', 'girder']);
            await expect.poll(async () => {
                try {
                    return (await fetch('http://localhost:18080/api/v1/system/version')).ok;
                } catch {
                    return false;
                }
            }, { timeout: 180_000, intervals: [2_000] }).toBe(true);
        }
        expect(await girder.listRecordFiles(recordId)).toHaveLength(0);
    });
});
