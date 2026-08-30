'use strict';

/**
 * File selection: what makes it into an upload batch, in which order, and how
 * paths are sanitized before they become Girder folder names.
 */

const test = require('node:test');
const assert = require('node:assert/strict');

const core = require('../../src/js/girder-uploader-core.js');

function file(name, options) {
    return Object.assign({ name, type: '' }, options || {});
}

test('OS noise and editor backups never reach the uploader', () => {
    assert.ok(core.shouldSkipUploadFile(file('.DS_Store')));
    assert.ok(core.shouldSkipUploadFile(file('.hidden.dcm')));
    assert.ok(core.shouldSkipUploadFile(file('~$report.docx')));
    assert.ok(core.shouldSkipUploadFile(file('study/.DS_Store', { girderRelativePath: 'study/.DS_Store' })));
    assert.ok(core.shouldSkipUploadFile(file('')));
    assert.ok(core.shouldSkipUploadFile(null));
});

test('regular files are kept', () => {
    assert.equal(core.shouldSkipUploadFile(file('0001.dcm')), false);
    assert.equal(core.shouldSkipUploadFile(file('ecg.xml')), false);
    assert.equal(
        core.shouldSkipUploadFile(file('0001.dcm', { webkitRelativePath: 'study/series/0001.dcm' })),
        false
    );
});

test('a nested ZIP is dropped while a top-level one is kept', () => {
    // Only a single top-level archive is expanded client-side; archives found
    // inside a folder upload are discarded rather than uploaded opaque.
    assert.ok(core.shouldSkipUploadFile(file('inner.zip', { girderRelativePath: 'study/inner.zip' })));
    assert.equal(core.shouldSkipUploadFile(file('archive.zip')), false);
});

test('ZIP archives are detected by name or MIME type', () => {
    assert.ok(core.isZipFile(file('archive.zip')));
    assert.ok(core.isZipFile(file('archive.bin', { type: 'application/x-zip-compressed' })));
    assert.equal(core.isZipFile(file('scan.dcm')), false);
});

test('DICOM files are guessed from the extension or the MIME type', () => {
    assert.ok(core.isLikelyDicomFile(file('0001.dcm')));
    assert.ok(core.isLikelyDicomFile(file('0001.DCM')));
    assert.ok(core.isLikelyDicomFile(file('unnamed', { type: 'application/dicom' })));
    assert.ok(core.isLikelyDicomFile(file('0001.dcm', { girderRelativePath: 'study/0001.dcm' })));
    assert.equal(core.isLikelyDicomFile(file('ecg.xml')), false);
    assert.equal(core.isLikelyDicomFile(file('dicom-notes.txt')), false);
});

test('a batch is filtered and sorted by path so series stay in order', () => {
    const batch = core.normalizeUploadFiles([
        file('0002.dcm', { girderRelativePath: 'study/0002.dcm' }),
        file('.DS_Store', { girderRelativePath: 'study/.DS_Store' }),
        file('0001.dcm', { girderRelativePath: 'study/0001.dcm' }),
        null
    ]);

    assert.deepEqual(batch.map(core.getFileDisplayName), ['study/0001.dcm', 'study/0002.dcm']);
});

test('the display name prefers the explicit relative path', () => {
    assert.equal(
        core.getFileDisplayName(file('0001.dcm', { girderRelativePath: 'from/zip/0001.dcm' })),
        'from/zip/0001.dcm'
    );
    assert.equal(
        core.getFileDisplayName(file('0001.dcm', { webkitRelativePath: 'from/folder/0001.dcm' })),
        'from/folder/0001.dcm'
    );
    assert.equal(core.getFileDisplayName(file('0001.dcm')), '0001.dcm');
    assert.equal(core.getFileDisplayName(null), '');
});

test('path parts unusable as Girder folder names are sanitized', () => {
    assert.equal(core.sanitizePathPart('CT: chest/abdomen', 'fallback'), 'CT_ chest_abdomen');
    assert.equal(core.sanitizePathPart('  spaced    out  ', 'fallback'), 'spaced out');
    assert.equal(core.sanitizePathPart('', 'fallback'), 'fallback');
    // Separators become underscores rather than vanishing, so a folder named
    // only of separators still yields a usable (if odd) name.
    assert.equal(core.sanitizePathPart('///', 'fallback'), '___');
});
