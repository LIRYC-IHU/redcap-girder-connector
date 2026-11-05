// What the security audit found, kept from coming back: who may act on a form, and what
// free text can do to the page. See the 2.0.0 entry of the changelog.
const { test, expect } = require('@playwright/test');
const redcap = require('../helpers/redcap');
const girder = require('../helpers/girder');
const module_ = require('../helpers/module');
const { fixturePath, loadState } = require('../helpers/state');

const RUN = Date.now().toString(36);
const READ_ONLY = 'e2e_read_only';
const EDITOR = 'e2e_editor';

test.describe.configure({ mode: 'serial' });

test.describe('security', () => {
    const field = () => loadState().redcap.uploadField;

    test.beforeAll(async () => {
        await redcap.deleteRecordsWithPrefix('sx');
        module_.forgetFingerprints('sx');
        await redcap.warmRecordListCache();
        module_.setFingerprintCheck(true);
        module_.ensureProjectUser(READ_ONLY, 129);
        module_.ensureProjectUser(EDITOR, 130);
    });

    async function asUser(page, username, recordId, body) {
        redcap.trackDialogs(page);
        await redcap.openRecordForm(page, recordId);
        await redcap.viewProjectAs(page, username);
        try {
            await redcap.openRecordForm(page, recordId, { create: false });
            await body();
        } finally {
            await redcap.viewProjectAs(page, '');
            page.removeAllListeners('dialog');
        }
    }

    test('a read-only user cannot act, even from the console', async ({ page }) => {
        const recordId = `sx${RUN}a`;
        await asUser(page, READ_ONLY, recordId, async () => {
            const fileEntry = { relativePath: 'a.dcm', fileName: 'a.dcm', fileSize: 10, mimeType: 'application/dicom' };
            const attempts = [
                ['pseudonym-evaluate', { fieldName: field(), blindedElement: '00'.repeat(32) }],
                ['pseudonym-check', { fieldName: field(), fingerprint: '00' }],
                ['pseudonym-register', { fieldName: field(), fingerprint: '00' }],
                ['init-batch', { fieldName: field(), pseudonymTicket: '', files: [fileEntry] }],
                ['init-file-upload', { fieldName: field(), uploadPlan: {} }],
                ['upload-file', { fieldName: field() }],
                ['delete-upload-contents', { fieldName: field(), folderId: 'a'.repeat(24) }],
            ];
            for (const [action, payload] of attempts) {
                const answer = await redcap.callModule(page, action, payload);
                expect(answer.ok, action).toBe(false);
                expect(answer.error, action).toMatch(/read-only/);
            }
        });
        expect(module_.storedFingerprint(recordId)).toBeNull();
        expect(await girder.listRecordFiles(recordId)).toHaveLength(0);
    });

    test('a user with view & edit rights, not an administrator, uploads', async ({ page }) => {
        const recordId = `sx${RUN}b`;
        await asUser(page, EDITOR, recordId, async () => {
            await redcap.uploadFiles(page, [fixturePath('p1000001_20240115.dcm')]);
            await expect(redcap.review(page)).toBeVisible({ timeout: 60_000 });
            await redcap.answerReview(page);
            const metadata = await redcap.waitForUploadOutcome(recordId);
            expect(metadata.uploadState, JSON.stringify(metadata.uploadState)).toMatchObject({ status: 'completed' });
        });
        expect(module_.storedFingerprint(recordId)).toMatch(/^[0-9a-f]{2}$/);
        expect(await girder.listRecordFiles(recordId)).toHaveLength(1);
    });

    test('a folder of another record, or the record itself, cannot be deleted or read', async ({ page }) => {
        const [mine, other] = [`sx${RUN}d`, `sx${RUN}e`];
        redcap.trackDialogs(page);
        for (const recordId of [mine, other]) {
            await redcap.openRecordForm(page, recordId);
            await redcap.uploadFiles(page, [fixturePath('p1000001_20240115.dcm')]);
            await expect(redcap.review(page)).toBeVisible({ timeout: 60_000 });
            await redcap.answerReview(page);
            await redcap.waitForUploadOutcome(recordId);
            await page.waitForLoadState('load');
        }
        const otherUpload = (await redcap.readUploadMetadata(other)).girder.parentFolderId;
        const myRecordFolder = (await girder.recordFolder(mine))._id;

        await redcap.openRecordForm(page, mine, { create: false });
        for (const folderId of [otherUpload, myRecordFolder]) {
            for (const action of ['delete-upload-contents', 'refresh-upload-metadata']) {
                const answer = await redcap.callModule(page, action, { fieldName: field(), folderId });
                expect(answer.ok, `${action} ${folderId}`).toBe(false);
                expect(answer.error).toMatch(/does not belong/);
            }
        }
        page.removeAllListeners('dialog');
        expect(await girder.listRecordFiles(other)).toHaveLength(1);
        expect(await girder.listRecordFiles(mine)).toHaveLength(1);
    });

    test('a wrapped report and a Secondary Capture are set aside with their reason, the images go through', async ({ page }) => {
        const recordId = `sx${RUN}f`;
        redcap.trackDialogs(page);
        await redcap.openRecordForm(page, recordId);
        await redcap.uploadFiles(page, ['report_20240115.dcm', 'capture_20240115.dcm', 'p1000001_20240115.dcm'].map(fixturePath));
        await expect(redcap.review(page)).toContainText('2 files set aside', { timeout: 60_000 });
        await redcap.answerReview(page);
        const metadata = await redcap.waitForUploadOutcome(recordId);
        page.removeAllListeners('dialog');

        expect(metadata.uploadState.status).toBe('completed');
        const reasons = metadata.rejectedFiles.map((entry) => entry.reason).join();
        expect(reasons).toMatch(/wraps a document/);
        expect(reasons).toMatch(/Secondary Capture/);
        const files = await girder.listRecordFiles(recordId);
        expect(files).toHaveLength(1);
        expect((await girder.downloadFile(files[0].file._id)).toString('latin1')).not.toContain('%PDF');
    });

    test('free text in the project title or a setting cannot run in the form', async ({ page }) => {
        const pid = Number(loadState().redcap.projectId);
        const title = module_.sql(`SELECT app_title FROM redcap_projects WHERE project_id = ${pid}`);
        const domain = module_.getProjectSetting('pseudonym-domain');
        const inTitle = 'E2E</script><script>window.__xss=1</script>';
        const inDomain = 'x</script><script>window.__xss=2</script>';
        const quoted = (value) => `'${value.replace(/'/g, "''")}'`;

        module_.sql(`UPDATE redcap_projects SET app_title = ${quoted(inTitle)} WHERE project_id = ${pid}`);
        module_.setSetting(pid, 'pseudonym-domain', 'string', inDomain);
        try {
            redcap.trackDialogs(page);
            await redcap.openRecordForm(page, `sx${RUN}c`);
            expect(await page.evaluate(() => window.__xss)).toBeUndefined();
            // The widget started, with the values intact.
            const config = await page.evaluate(() => window.GirderUploaderConfig);
            expect(config.pseudonym.domain).toBe(inDomain);
            expect(config.projectTitle).toContain('</script>');
            page.removeAllListeners('dialog');
        } finally {
            module_.sql(`UPDATE redcap_projects SET app_title = ${quoted(title)} WHERE project_id = ${pid}`);
            if (domain === null) {
                module_.removeSetting(pid, 'pseudonym-domain');
            } else {
                module_.setSetting(pid, 'pseudonym-domain', 'string', domain);
            }
        }
    });
});
