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

// --- Pseudonym ---------------------------------------------------------------

function inspected(name, inspection) {
    return { file: file(name), inspection: Object.assign({ deidentified: true }, inspection) };
}

test('the deidentification request carries the patient keys', () => {
    const request = core.buildDeidentifyRequest(file('scan.dcm'), CONFIG, new ArrayBuffer(0), { dateOffsetDays: -20000, uidKey: 'ab'.repeat(32) });

    assert.equal(request.type, 'deidentify');
    assert.equal(request.dateOffsetDays, -20000);
    assert.equal(request.uidKey, 'ab'.repeat(32));
});

test('missing or malformed keys reach the worker as null', () => {
    // The worker then refuses: a zero offset would keep the real dates.
    for (const keys of [undefined, null, {}, { dateOffsetDays: '12', uidKey: 7 }, { dateOffsetDays: 12.5 }]) {
        const request = core.buildDeidentifyRequest(file('scan.dcm'), CONFIG, new ArrayBuffer(0), keys);
        assert.equal(request.dateOffsetDays, null, JSON.stringify(keys));
        assert.equal(request.uidKey, null, JSON.stringify(keys));
    }
});

test('the inspection request mirrors the deidentification settings', () => {
    const request = core.buildInspectRequest(file('scan.dcm'), CONFIG, new ArrayBuffer(4));

    assert.equal(request.type, 'inspect');
    assert.equal(request.fileName, 'scan.dcm');
    assert.equal(request.enableDicom, true);
    assert.equal(request.enableXml, false);
});

test('files are grouped by patient, and files without NIP are set apart', () => {
    const groups = core.groupByPatient([
        inspected('a.dcm', { nip: '1000001', rawNip: '0001000001', familyName: 'DOE', givenName: 'JOHN', birthDate: '12/02/1954' }),
        inspected('b.dcm', { nip: '1000001' }),
        inspected('c.dcm', { nip: '' }),
        { file: file('d.dcm'), inspection: { deidentified: false, nip: '' } }
    ]);

    assert.equal(groups.patients.length, 1);
    assert.equal(groups.patients[0].entries.length, 2);
    assert.equal(groups.patients[0].familyName, 'DOE');
    assert.equal(groups.patients[0].rawNip, '0001000001');
    assert.deepEqual(groups.missingNip.map((entry) => entry.file.name), ['c.dcm']);
    assert.deepEqual(groups.passthrough.map((entry) => entry.file.name), ['d.dcm'], 'files sent as they are need no NIP');
});

test('the name shown comes from whichever file of the patient carries it', () => {
    const groups = core.groupByPatient([
        inspected('a.dcm', { nip: '1000001' }),
        inspected('b.dcm', { nip: '1000001', familyName: 'DOE', givenName: 'JOHN' })
    ]);

    assert.equal(groups.patients[0].familyName, 'DOE');
    assert.equal(groups.patients[0].givenName, 'JOHN');
});

test('the transfer summary counts DICOM series, ECGs and Holters', () => {
    const summary = core.summarizeTransfer([
        inspected('1.dcm', { format: 'dicom', seriesUid: '1.2.3', modality: 'XA', seriesDescription: 'Coronarography' }),
        inspected('2.dcm', { format: 'dicom', seriesUid: '1.2.3', modality: 'XA', seriesDescription: 'Coronarography' }),
        inspected('3.dcm', { format: 'dicom', seriesUid: '1.2.4', modality: 'CT' }),
        inspected('ecg.xml', { format: 'xml' }),
        inspected('holter.bin', { format: 'schiller' }),
        inspected('holter2.bin', { format: 'schiller' })
    ]);

    assert.equal(summary.dicomFiles, 3);
    assert.deepEqual(summary.series, [
        { modality: 'XA', description: 'Coronarography', fileCount: 2 },
        { modality: 'CT', description: '', fileCount: 1 }
    ]);
    assert.equal(summary.ecgCount, 1);
    assert.equal(summary.holterCount, 2);
});

function reviewFor(checkStatus, existingUploads) {
    const entries = [
        inspected('1.dcm', { format: 'dicom', nip: '1000001', rawNip: '0001000001', familyName: 'DOE', givenName: 'JOHN', birthDate: '12/02/1954', seriesUid: '1.2.3', modality: 'XA', seriesDescription: 'Coronarography' }),
        inspected('2.dcm', { format: 'dicom', nip: '1000001', seriesUid: '1.2.3', modality: 'XA', seriesDescription: 'Coronarography' }),
        inspected('3.dcm', { format: 'dicom', nip: '1000001', seriesUid: '1.2.4', modality: 'CT' }),
        inspected('ecg.xml', { format: 'xml', nip: '1000001' }),
        inspected('holter.bin', { format: 'schiller', nip: '1000001' })
    ];
    const groups = core.groupByPatient(entries);
    return core.buildTransferReview({
        recordId: 'REC-42',
        patient: groups.patients[0],
        summary: core.summarizeTransfer(entries),
        passthroughCount: 0,
        setAsideCount: 1,
        checkStatus,
        existingUploads
    });
}

test('the review shows the patient as read from the files', () => {
    assert.deepEqual(reviewFor('new', 0).patient, [
        { label: 'Last name', value: 'DOE' },
        { label: 'First name', value: 'JOHN' },
        { label: 'Date of birth', value: '12/02/1954' },
        { label: 'NIP', value: '0001000001' }
    ]);
});

test('the review lists each DICOM series with its modality, then ECGs and Holters', () => {
    const review = reviewFor('new', 0);

    assert.deepEqual(review.content, [
        { type: 'DICOM', detail: 'XA - Coronarography', files: 2 },
        { type: 'DICOM', detail: 'CT', files: 1 },
        { type: 'ECG', detail: '', files: 1 },
        { type: 'Holter', detail: '', files: 1 }
    ]);
    assert.deepEqual(review.notes, ['1 file set aside.']);
});

test('the review says what the signature check found', () => {
    assert.equal(reviewFor('new', 0).check.title, 'First upload for this record');
    assert.match(reviewFor('matched', 3).check.text, /3 earlier uploads of this record/);
    assert.equal(reviewFor('matched', 3).check.level, 'ok');
    assert.equal(reviewFor('unchecked', 3).check.title, 'Patient check disabled');
});

test('uploads without a signature are not called a first upload', () => {
    const { check } = reviewFor('new', 2);

    assert.equal(check.level, 'warning');
    assert.equal(check.title, 'Earlier uploads cannot be compared');
    assert.match(check.text, /2 earlier uploads/);
});

test('a mismatch can be confirmed, and says so', () => {
    const review = reviewFor('mismatch', 1);

    assert.equal(review.mismatch, true);
    assert.equal(review.check.level, 'danger');
    assert.equal(review.confirmLabel, 'Upload anyway');
    assert.equal(reviewFor('matched', 1).confirmLabel, 'Upload');
    assert.equal(reviewFor('matched', 1).mismatch, false);
});

test('files sent as they are have no patient and no check to show', () => {
    const entries = [inspected('ecg.xml', { format: 'xml', deidentified: false })];
    const review = core.buildTransferReview({
        recordId: 'REC-42',
        patient: null,
        summary: core.summarizeTransfer(entries),
        passthroughCount: 1,
        setAsideCount: 0,
        checkStatus: ''
    });

    assert.equal(review.patient, null);
    assert.equal(review.check, null);
    assert.deepEqual(review.notes, ['1 file sent without deidentification (disabled for their format).']);
});

test('the review names the files set aside, up to a limit', () => {
    const setAside = Array.from({ length: 12 }, (_, index) => ({ name: `DOE_${index}.dcm`, reason: 'no NIP' }));
    const review = core.buildTransferReview({
        recordId: 'REC-42',
        patient: null,
        summary: core.summarizeTransfer([]),
        passthroughCount: 0,
        setAsideCount: 12,
        setAside,
        checkStatus: ''
    });

    assert.equal(review.setAside.length, 10);
    assert.equal(review.setAside[0], 'DOE_0.dcm: no NIP');
    assert.equal(review.setAsideOmitted, 2);
});

test('the worker is asked of the origin the page was reached on', () => {
    const worker = 'http://localhost:18081/modules/girder_uploader_v2.0.1/js/deidentify-worker.js?1793830408';

    assert.equal(
        core.sameOriginUrl(worker, 'http://127.0.0.1:18081/redcap_v15.9.3/DataEntry/index.php?pid=16'),
        'http://127.0.0.1:18081/modules/girder_uploader_v2.0.1/js/deidentify-worker.js?1793830408'
    );
    assert.equal(core.sameOriginUrl(worker, 'http://localhost:18081/redcap_v15.9.3/DataEntry/index.php'), worker);
    assert.equal(
        core.sameOriginUrl('https://redcap.example.org/redcap/modules/m_v1/js/w.js', 'https://redcap.intranet/redcap/DataEntry/index.php'),
        'https://redcap.intranet/redcap/modules/m_v1/js/w.js',
        'a REDCap under a path keeps it'
    );
});

test('a batch of several patients is refused as a whole', () => {
    const groups = core.groupByPatient([
        inspected('a.dcm', { nip: '1000001', familyName: 'DOE', givenName: 'JOHN' }),
        inspected('b.dcm', { nip: '1000002', familyName: 'ROE', givenName: 'RITA' })
    ]);
    const message = core.buildMultiplePatientsError(groups.patients);

    assert.equal(groups.patients.length, 2);
    assert.match(message, /2 different patients/);
    assert.doesNotMatch(message, /DOE|JOHN|ROE|RITA|1000001|1000002/, 'it is stored in the REDCap field');
    assert.match(message, /Nothing was uploaded/);
});
