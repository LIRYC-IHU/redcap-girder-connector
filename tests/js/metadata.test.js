'use strict';

/**
 * The JSON payload written back into the tagged REDCap field.
 */

const test = require('node:test');
const assert = require('node:assert/strict');

const core = require('../../src/js/girder-uploader-core.js');

function uploadedFiles(count) {
    return Array.from({ length: count }, (_, index) => ({
        name: String(index + 1).padStart(4, '0') + '.dcm',
        size: 1024,
        mimeType: 'application/dicom'
    }));
}

test('only a sample of the file list is stored in the field', () => {
    const stored = core.buildStoredMetadataPayload({
        uploadedFiles: uploadedFiles(250),
        girder: { baseUrl: 'https://girder.example.org', parentFolderId: 'abc123' }
    });

    // REDCap fields hold a bounded amount of text; the full listing is fetched
    // back from Girder on demand.
    assert.equal(stored.uploadedFiles.length, core.STORED_FILE_SAMPLE_LIMIT);
    assert.equal(stored.uploadSummary.fileCount, 250);
    assert.equal(stored.uploadSummary.omittedFileCount, 240);
    assert.equal(stored.totalSizeBytes, 250 * 1024);
    assert.equal(stored.encoding, 'summary-v1');
});

test('a stored payload survives a round trip without inflating counts', () => {
    const first = core.buildStoredMetadataPayload({
        uploadedFiles: uploadedFiles(250),
        girder: { baseUrl: 'https://girder.example.org', parentFolderId: 'abc123' }
    });
    const second = core.buildStoredMetadataPayload(JSON.parse(JSON.stringify(first)));

    assert.equal(second.uploadSummary.fileCount, 250);
    assert.equal(second.uploadedFiles.length, core.STORED_FILE_SAMPLE_LIMIT);
    assert.equal(second.totalSizeBytes, first.totalSizeBytes);
});

test('the Girder folder link is derived from the stored ids', () => {
    assert.equal(
        core.buildFolderUrl({ baseUrl: 'https://girder.example.org/', parentFolderId: 'abc123' }),
        'https://girder.example.org/#folder/abc123'
    );
    assert.equal(core.buildFolderUrl({ baseUrl: 'https://girder.example.org' }), null);
    assert.equal(core.buildFolderUrl(null), null);
});

test('a legacy payload is normalized rather than rejected', () => {
    const normalized = core.normalizeMetadataPayload({
        uploadedFiles: uploadedFiles(3),
        girder: { baseUrl: 'https://girder.example.org', parentFolderId: 'abc123' }
    });

    assert.equal(normalized.uploadSummary.fileCount, 3);
    assert.equal(normalized.totalSizeBytes, 3 * 1024);
    assert.equal(normalized.girder.parentFolderUrl, 'https://girder.example.org/#folder/abc123');
    assert.equal(core.normalizeMetadataPayload('not an object'), null);
});

test('upload state drives what the widget offers', () => {
    const completed = {
        uploadState: { status: 'completed' },
        girder: { parentFolderId: 'abc123' }
    };

    assert.ok(core.isCompletedPayload(completed));
    assert.equal(core.isFailedPayload(completed), false);
    // A completed upload without a folder is not something we can link to.
    assert.equal(core.isCompletedPayload({ uploadState: { status: 'completed' }, girder: {} }), false);
    assert.ok(core.isFailedPayload({ uploadState: { status: 'failed' } }));
});

test('sizes are rendered in human units', () => {
    assert.equal(core.formatSize(0), '0 B');
    assert.equal(core.formatSize(512), '512 B');
    assert.equal(core.formatSize(1024), '1.00 KB');
    assert.equal(core.formatSize(10 * 1024 * 1024), '10.00 MB');
});
