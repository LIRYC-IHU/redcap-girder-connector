// REDCap helpers: API (project token from provision.sh) and the data-entry form.
const { expect } = require('@playwright/test');
const { loadState } = require('./state');

function redcap() {
    return loadState().redcap;
}

async function api(params) {
    const { baseUrl, projectToken } = redcap();
    const body = new URLSearchParams({ token: projectToken, format: 'json', returnFormat: 'json', ...params });
    const send = () => fetch(`${baseUrl}/api/`, { method: 'POST', body });
    // Apache recycles its workers, and fetch may just have reused a closed connection:
    // the request never reached REDCap, so sending it again is safe.
    const response = await send().catch((error) => {
        if (error.cause && error.cause.code === 'UND_ERR_SOCKET') {
            return send();
        }
        throw error;
    });
    const text = await response.text();
    if (!response.ok) {
        throw new Error(`REDCap API ${params.content} failed (${response.status}): ${text}`);
    }
    return JSON.parse(text);
}

/** Create the record if needed, optionally overwriting fields. */
async function ensureRecord(recordId, fields = {}) {
    const data = [{ record_id: String(recordId), ...fields }];
    return api({ content: 'record', overwriteBehavior: 'overwrite', data: JSON.stringify(data) });
}

/** Raw value of the @GIRDER_UPLOAD field ('' when empty). */
async function readUploadField(recordId, field = redcap().uploadField) {
    const rows = await api({ content: 'record', type: 'flat', 'records[0]': String(recordId), 'fields[0]': field });
    return rows.length ? String(rows[0][field] || '') : '';
}

/** The upload field parsed as the widget's JSON metadata, or null. */
async function readUploadMetadata(recordId, field) {
    const raw = await readUploadField(recordId, field);
    if (!raw.trim()) {
        return null;
    }
    try {
        return JSON.parse(raw);
    } catch {
        return { unparsable: raw };
    }
}

/**
 * Clean slate for a record: it exists, its upload field is empty, and whatever an earlier
 * run stored in Girder for it is moved out of its folder.
 */
async function resetRecord(recordId) {
    const girder = require('./girder');
    await ensureRecord(recordId, { [redcap().uploadField]: '' });
    await girder.moveRecordFolderToTrash(recordId);
    await warmRecordListCache();
}

/**
 * Make sure the project's record-list cache is built. While it is NOT_STARTED, REDCap
 * redirects the first request into the project to the cache builder; if that request is
 * the module's AJAX POST, the module gets HTML instead of JSON and the upload fails.
 * REDCap's cron also drops the cache of idle projects, so this can recur on a long-lived
 * stack. A test that wants to exercise that case should skip this.
 */
async function warmRecordListCache() {
    const { baseUrl, version, projectId } = redcap();
    const url = `${baseUrl}/redcap_v${version}/index.php?pid=${projectId}&route=DataEntryController:buildRecordListCache`;
    const response = await fetch(url, { redirect: 'follow' });
    await response.arrayBuffer();
}

function formUrl(recordId) {
    const { version, projectId, instrument, eventId } = redcap();
    const query = new URLSearchParams({ pid: projectId, id: String(recordId), page: instrument, event_id: eventId });
    return `/redcap_v${version}/DataEntry/index.php?${query}`;
}

/** Open the data-entry form of a record (created first if needed) and wait for the widget. */
async function openRecordForm(page, recordId, { create = true } = {}) {
    if (create) {
        await ensureRecord(recordId);
    }
    await page.goto(formUrl(recordId));
    await expect(widget(page)).toBeVisible();
    return widget(page);
}

/**
 * Answer the page's window.confirm() dialogs (the delete button's) and REDCap's "leave
 * this page?" prompt. Call before acting. Returns the live array of messages; pass
 * { accept: false } to dismiss instead.
 */
function trackDialogs(page, { accept = true } = {}) {
    const messages = [];
    page.on('dialog', async (dialog) => {
        // REDCap's "leave this page?" prompt when a form is reloaded: not ours.
        if (dialog.type() === 'beforeunload') {
            await dialog.accept();
            return;
        }
        messages.push(dialog.message());
        if (accept) {
            await dialog.accept();
        } else {
            await dialog.dismiss();
        }
    });
    return messages;
}

/** The window asking the uploader to review a transfer. */
function review(page) {
    return page.locator('.girder-transfer-review');
}

/** Answer the open review window; resolves with its text. */
async function answerReview(page, { accept = true } = {}) {
    const window = review(page);
    const text = await window.innerText();
    await window.getByRole('button', { name: accept ? /^Upload/ : 'Cancel' }).click();
    await expect(window).toHaveCount(0);
    return text;
}

/**
 * Wait for what an upload does first: open the review window, or end without one.
 * Resolves with 'review', or with the stored metadata when it was refused outright.
 */
async function waitForReviewOrOutcome(page, recordId, { field, timeout = 90_000 } = {}) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
        if (await review(page).isVisible().catch(() => false)) {
            return 'review';
        }
        const metadata = await readUploadMetadata(recordId, field);
        const status = metadata && metadata.uploadState && metadata.uploadState.status;
        if (status === 'completed' || status === 'failed') {
            return metadata;
        }
        await new Promise((resolve) => setTimeout(resolve, 250));
    }
    throw new Error(`neither a review nor an outcome on record ${recordId} within ${timeout} ms`);
}

/** Go on as `username` in this project, REDCap's "View project as user"; '' stops. */
async function viewProjectAs(page, username) {
    const { projectId } = redcap();
    const answer = await page.evaluate(async ([pid, user]) => window.jQuery.post(
        `${window.app_path_webroot}index.php?pid=${pid}&route=UserRightsController:impersonateUser`,
        { user }
    ), [projectId, username]);
    expect(String(answer), 'impersonation refused').not.toBe('0');
}

/** Call a module action as the page would, from the console. */
async function callModule(page, action, payload) {
    return page.evaluate(async ([name, data]) => window.GirderUploaderModule.ajax(name, data), [action, payload]);
}

function widget(page, field = redcap().uploadField) {
    return page.locator(`textarea[name="${field}"] + .girder-uploader-wrapper`);
}

/** Feed files to the widget's hidden <input type=file> (same path as the file picker). */
async function uploadFiles(page, filePaths, field) {
    await widget(page, field).locator('input[type="file"]').setInputFiles(filePaths);
}

/** Status line and info-zone title currently shown by the widget. */
async function readWidgetStatus(page, field) {
    const w = widget(page, field);
    const status = w.locator('.girder-upload-status');
    const title = w.locator('.girder-file-info h5');
    return {
        status: (await status.count()) && (await status.isVisible()) ? (await status.innerText()).trim() : '',
        title: (await title.count()) ? (await title.first().innerText()).trim() : '',
    };
}

/**
 * Wait until the upload has finished or failed, by polling what the widget saved into the
 * record (independent of the page reload the widget triggers when it is done).
 * Resolves with the stored metadata; uploadState.status is 'completed' or 'failed'.
 */
async function waitForUploadOutcome(recordId, { field, timeout = 90_000, interval = 1_000 } = {}) {
    const deadline = Date.now() + timeout;
    let last = null;
    while (Date.now() < deadline) {
        last = await readUploadMetadata(recordId, field);
        const status = last && last.uploadState && last.uploadState.status;
        if (status === 'completed' || status === 'failed') {
            return last;
        }
        await new Promise((resolve) => setTimeout(resolve, interval));
    }
    throw new Error(`upload on record ${recordId} did not finish within ${timeout} ms; last field value: ${JSON.stringify(last)}`);
}

/**
 * Delete every record whose id starts with `prefix`: a project in development
 * status holds at most 50 records, and scenarios that need fresh records would
 * otherwise exhaust them over a few runs.
 */
async function deleteRecordsWithPrefix(prefix) {
    const rows = await api({ content: 'record', type: 'flat', 'fields[0]': 'record_id' });
    const ids = [...new Set(rows.map((row) => String(row.record_id)).filter((id) => id.startsWith(prefix)))];
    if (!ids.length) {
        return 0;
    }
    const params = { content: 'record', action: 'delete' };
    ids.forEach((id, index) => {
        params[`records[${index}]`] = id;
    });
    await api(params);
    return ids.length;
}

module.exports = {
    api,
    deleteRecordsWithPrefix,
    ensureRecord,
    readUploadField,
    readUploadMetadata,
    resetRecord,
    warmRecordListCache,
    formUrl,
    openRecordForm,
    widget,
    uploadFiles,
    trackDialogs,
    review,
    answerReview,
    waitForReviewOrOutcome,
    viewProjectAs,
    callModule,
    readWidgetStatus,
    waitForUploadOutcome,
};
