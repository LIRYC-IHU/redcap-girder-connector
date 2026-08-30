'use strict';

/**
 * Browser-side half of the deidentification pipeline: which files reach the
 * WASM worker, what the worker is told, and how its answers are interpreted.
 */

const test = require('node:test');
const assert = require('node:assert/strict');

const core = require('../../src/js/girder-uploader-core.js');

/** Minimal stand-in for a browser `File`. */
function file(name, options) {
    return Object.assign({ name, type: '' }, options || {});
}

const CONFIG = {
    recordId: 'REC-42',
    projectTitle: 'ABLATION REGISTRY',
    settings: {
        deidentifyDicom: true,
        deidentifyXmlEcg: false,
        deidentifySchillerHolter: true
    }
};

test('the DICOM patient name carries the project and the record', () => {
    assert.equal(core.buildPatientName('ABLATION REGISTRY', 'REC-42'), 'ABLATION REGISTRY^REC-42');
});

test('the patient name falls back to the record when the project has no title', () => {
    assert.equal(core.buildPatientName('', 'REC-42'), 'REC-42');
    assert.equal(core.buildPatientName('  ', ' REC-42 '), 'REC-42');
});

test('the worker request mirrors the project deidentification settings', () => {
    const bytes = new ArrayBuffer(8);
    const request = core.buildDeidentifyRequest(file('scan.dcm'), CONFIG, bytes);

    assert.equal(request.bytes, bytes);
    assert.equal(request.fileName, 'scan.dcm');
    assert.equal(request.recordId, 'REC-42');
    assert.equal(request.patientName, 'ABLATION REGISTRY^REC-42');
    assert.equal(request.enableDicom, true);
    assert.equal(request.enableXml, false);
    assert.equal(request.enableSchiller, true);
});

test('the worker request keeps the relative path of files picked in a folder', () => {
    const request = core.buildDeidentifyRequest(
        file('0001.dcm', { webkitRelativePath: 'study/series-1/0001.dcm' }),
        CONFIG,
        new ArrayBuffer(0)
    );

    // The file name drives format detection (`.xml`), the relative path drives
    // the folder layout in Girder: both must reach the worker.
    assert.equal(request.fileName, '0001.dcm');
    assert.equal(request.relativePath, 'study/series-1/0001.dcm');
});

test('missing settings mean no deidentification is requested', () => {
    const request = core.buildDeidentifyRequest(file('scan.dcm'), {}, new ArrayBuffer(0));

    assert.equal(request.enableDicom, false);
    assert.equal(request.enableXml, false);
    assert.equal(request.enableSchiller, false);
    assert.equal(request.recordId, '');
});

test('SKIP errors are recognized so unsupported files are dropped, not fatal', () => {
    assert.ok(core.isSkipError(new Error('SKIP: file is unsupported')));
    assert.ok(core.isSkipError(new Error('SKIP: input is not a valid DICOM file: bad header')));
    assert.ok(core.isSkipError('SKIP: file is unsupported'));
});

test('genuine failures are not mistaken for skips', () => {
    assert.equal(core.isSkipError(new Error('WASM deidentify function is unavailable.')), false);
    assert.equal(core.isSkipError(new Error('Worker error')), false);
    // A failure that merely mentions skipping must still fail the batch.
    assert.equal(core.isSkipError(new Error('could not SKIP: internal error')), false);
    assert.equal(core.isSkipError(null), false);
});
