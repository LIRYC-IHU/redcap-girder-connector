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

test('files the batch could not take are recorded, not lost', () => {
    // A DICOM archive routinely carries a DICOMDIR or a viewer's XML. One of
    // those must not cancel the upload — but it must not vanish either.
    const stored = core.buildStoredMetadataPayload({
        uploadedFiles: uploadedFiles(3),
        rejectedFiles: [
            { name: 'study/viewer.xml', stage: 'deidentification', reason: 'is a GE MUSE ECG, not an HL7 Annotated ECG' },
            { name: 'study/0007.dcm', stage: 'upload', reason: 'Girder request failed (502)' }
        ],
        girder: { baseUrl: 'https://girder.example.org', parentFolderId: 'abc123' }
    });

    assert.equal(stored.uploadSummary.fileCount, 3);
    assert.equal(stored.rejectedCount, 2);
    assert.deepEqual(stored.rejectedFiles.map(r => r.name), ['study/viewer.xml', 'study/0007.dcm']);
    assert.equal(stored.rejectedFiles[0].stage, 'deidentification');
    assert.equal(stored.rejectedFiles[1].reason, 'Girder request failed (502)');
});

test('the rejected list is bounded like the file list', () => {
    const many = Array.from({ length: 40 }, (_, i) => ({
        name: `junk-${i}.xml`, stage: 'deidentification', reason: 'unsupported'
    }));
    const stored = core.buildStoredMetadataPayload({ uploadedFiles: uploadedFiles(1), rejectedFiles: many });

    assert.equal(stored.rejectedFiles.length, core.STORED_FILE_SAMPLE_LIMIT);
    assert.equal(stored.rejectedCount, 40, 'the true count must survive the sampling');
});

test('a rejection carries the file path and the reason', () => {
    const entry = core.rejection(
        { name: '0007.dcm', girderRelativePath: 'study/series/0007.dcm' },
        'upload',
        new Error('Girder request failed (502)')
    );

    assert.deepEqual(entry, {
        name: 'study/series/0007.dcm',
        stage: 'upload',
        reason: 'Girder request failed (502)'
    });
});

test('the outcome line accounts for every file', () => {
    assert.equal(core.summarizeOutcome(12, 0, []), '12 files uploaded.');
    assert.equal(core.summarizeOutcome(1, 0, []), '1 file uploaded.');
    assert.equal(
        core.summarizeOutcome(10, 2, [{ name: 'a' }, { name: 'b' }]),
        '10 files uploaded, 2 skipped (unsupported format), 2 rejected.'
    );
});

test('a legacy payload without rejections normalizes cleanly', () => {
    const normalized = core.normalizeMetadataPayload({
        uploadedFiles: uploadedFiles(2),
        girder: { baseUrl: 'https://girder.example.org', parentFolderId: 'abc123' }
    });

    assert.deepEqual(normalized.rejectedFiles, []);
    assert.equal(normalized.rejectedCount, 0);
});

test('a refresh from Girder does not erase the rejections', () => {
    // Girder has no idea which files we refused; a snapshot rebuilt from the
    // folder listing would drop them and lose the trace.
    const stored = core.buildStoredMetadataPayload({
        uploadedFiles: uploadedFiles(2),
        rejectedFiles: [{ name: 'viewer.xml', stage: 'deidentification', reason: 'is a GE MUSE ECG' }],
        girder: { baseUrl: 'https://girder.example.org', parentFolderId: 'abc123' }
    });
    const fromGirder = core.normalizeMetadataPayload({
        uploadedFiles: uploadedFiles(2),
        girder: { baseUrl: 'https://girder.example.org', parentFolderId: 'abc123' }
    });

    assert.equal(fromGirder.rejectedCount, 0, 'the server snapshot starts without them');
    core.carryRejectionsForward(stored, fromGirder);

    assert.equal(fromGirder.rejectedCount, 1);
    assert.equal(fromGirder.rejectedFiles[0].name, 'viewer.xml');
});

test('carrying forward leaves a clean payload alone', () => {
    const fromGirder = { uploadedFiles: [], rejectedFiles: [], rejectedCount: 0 };
    core.carryRejectionsForward({ rejectedFiles: [] }, fromGirder);

    assert.equal(fromGirder.rejectedCount, 0);
    assert.equal(core.carryRejectionsForward({}, null), null);
});
