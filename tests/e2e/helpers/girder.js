// Girder REST helpers, as the admin user created by provision.sh.
const { loadState } = require('./state');

let adminToken = null;

function api() {
    return loadState().girder.apiUrl;
}

async function token() {
    if (adminToken) {
        return adminToken;
    }
    const { adminLogin, adminPassword } = loadState().girder;
    const auth = Buffer.from(`${adminLogin}:${adminPassword}`).toString('base64');
    const response = await fetch(`${api()}/user/authentication`, { headers: { Authorization: `Basic ${auth}` } });
    if (!response.ok) {
        throw new Error(`Girder admin login failed (${response.status})`);
    }
    adminToken = (await response.json()).authToken.token;
    return adminToken;
}

async function request(method, pathAndQuery, { params, raw = false, allow404 = false } = {}) {
    const url = new URL(api() + pathAndQuery);
    for (const [key, value] of Object.entries(params || {})) {
        url.searchParams.set(key, typeof value === 'string' ? value : JSON.stringify(value));
    }
    const send = async () => fetch(url, { method, headers: { 'Girder-Token': await token() } });
    // Girder closes idle connections, and fetch may just have reused one.
    const response = await send().catch((error) => {
        if (method !== 'GET') {
            throw error;
        }
        return send();
    });
    if (allow404 && response.status === 404) {
        return null;
    }
    if (!response.ok) {
        throw new Error(`Girder ${method} ${pathAndQuery} failed (${response.status}): ${await response.text()}`);
    }
    return raw ? Buffer.from(await response.arrayBuffer()) : response.json();
}

const get = (p, params) => request('GET', p, { params });

async function childFolder(parentType, parentId, name) {
    const folders = await get('/folder', { parentType, parentId, name, limit: '1' });
    return folders[0] || null;
}

/** The record's folder: {collection}/{dag}/{record}, or null if nothing was uploaded. */
async function recordFolder(recordId, { dag = 'NO_DAG' } = {}) {
    const dagFolder = await childFolder('collection', loadState().girder.collectionId, dag);
    return dagFolder ? childFolder('folder', dagFolder._id, String(recordId)) : null;
}

/** Every file under a folder, recursively, as {path, item, file}. */
async function listFilesUnder(folderId, prefix = '') {
    const out = [];
    for (const item of await get('/item', { folderId, limit: '0' })) {
        for (const file of await get(`/item/${item._id}/files`, { limit: '0' })) {
            out.push({ path: `${prefix}${item.name}`, item, file });
        }
    }
    for (const folder of await get('/folder', { parentType: 'folder', parentId: folderId, limit: '0' })) {
        out.push(...await listFilesUnder(folder._id, `${prefix}${folder.name}/`));
    }
    return out;
}

/**
 * Files stored for a record, paths relative to the record folder
 * (e.g. "girder_files/<item>"). Empty when nothing landed.
 */
async function listRecordFiles(recordId, options) {
    const folder = await recordFolder(recordId, options);
    return folder ? listFilesUnder(folder._id) : [];
}

async function downloadFile(fileId) {
    return request('GET', `/file/${fileId}/download`, { raw: true });
}

/**
 * Move a record's folder out of the way, into the admin-only "E2E Trash" collection.
 * A move is synchronous; DELETE /folder is not (Girder 5 hands it to Celery, which the
 * default stack does not run).
 */
async function moveRecordFolderToTrash(recordId, options) {
    const folder = await recordFolder(recordId, options);
    if (!folder) {
        return false;
    }
    let trash = (await get('/collection', { text: 'E2E Trash', limit: '50' })).find((c) => c.name === 'E2E Trash');
    if (!trash) {
        trash = await request('POST', '/collection', { params: { name: 'E2E Trash', public: 'false' } });
    }
    await request('PUT', `/folder/${folder._id}`, {
        params: { parentType: 'collection', parentId: trash._id, name: `${recordId}-${Date.now()}` },
    });
    return true;
}

module.exports = {
    request,
    recordFolder,
    listFilesUnder,
    listRecordFiles,
    downloadFile,
    moveRecordFolderToTrash,
};
