<?php

/**
 * Where a file ends up in Girder: path sanitization, neutral file names, and
 * the DICOM hint that decides the item layout.
 */

declare(strict_types=1);

use function GirderUploaderTests\invoke;
use function GirderUploaderTests\moduleWithSettings;

return function ($t): void {
    $t->test('path parts unusable as Girder folder names are sanitized', function ($t): void {
        $module = moduleWithSettings();

        // Kept in step with sanitizePathPart() in js/girder-uploader-core.js:
        // the browser previews the path, PHP creates it.
        $t->assertSame('CT_ chest_abdomen', invoke($module, 'sanitizePathPart', ['CT: chest/abdomen', 'fallback']));
        $t->assertSame('spaced out', invoke($module, 'sanitizePathPart', ['  spaced    out  ', 'fallback']));
        $t->assertSame('fallback', invoke($module, 'sanitizePathPart', ['', 'fallback']));
        $t->assertSame('___', invoke($module, 'sanitizePathPart', ['///', 'fallback']));
    });

    $t->test('a relative path is split into safe folder segments', function ($t): void {
        $module = moduleWithSettings();

        $t->assertSame(
            ['study', 'series 1', '0001.dcm'],
            invoke($module, 'splitRelativePath', ['/study/series  1/0001.dcm'])
        );
        $t->assertSame(
            ['study', '0001.dcm'],
            invoke($module, 'splitRelativePath', ['study\\0001.dcm']),
            'Windows separators must be understood'
        );
        $t->assertSame(['uploaded-file'], invoke($module, 'splitRelativePath', ['']));
        $t->assertSame(['uploaded-file'], invoke($module, 'splitRelativePath', ['///']));
    });

    $t->test('path traversal cannot escape the field folder', function ($t): void {
        $module = moduleWithSettings();

        // `..` is not a separator-bearing segment, so it survives sanitization
        // as a literal folder name: Girder never resolves it as a parent.
        $parts = invoke($module, 'splitRelativePath', ['../../etc/passwd']);
        $t->assertSame(['..', '..', 'etc', 'passwd'], $parts);
    });

    $t->test('neutral names are numbered and keep a safe extension', function ($t): void {
        $module = moduleWithSettings();

        $t->assertSame('0001.dcm', invoke($module, 'buildNeutralFileName', [1, 'PATIENT-DUPONT.dcm', '', true]));
        $t->assertSame('0042.xml', invoke($module, 'buildNeutralFileName', [42, 'marie-dupont.xml', '', false]));
        $t->assertSame('9999.dcm', invoke($module, 'buildNeutralFileName', [99999, 'x.dcm', '', true]));
        $t->assertSame('0001.dcm', invoke($module, 'buildNeutralFileName', [0, 'x.dcm', '', true]));
    });

    $t->test('a missing extension is inferred from the detected format', function ($t): void {
        $module = moduleWithSettings();

        $t->assertSame('0001.dcm', invoke($module, 'buildNeutralFileName', [1, 'IM_0001', '', true]));
        $t->assertSame('0001.xml', invoke($module, 'buildNeutralFileName', [1, 'ecg-export', 'application/xml', false]));
        $t->assertSame('0001', invoke($module, 'buildNeutralFileName', [1, 'holter-data', '', false]));
    });

    $t->test('the file name never leaks identity through the extension', function ($t): void {
        $module = moduleWithSettings();

        // Only short alphanumeric extensions are carried over; anything else is
        // dropped rather than copied into the neutral name.
        $t->assertSame('dcm', invoke($module, 'getSafeFileExtension', ['0001.DCM']));
        $t->assertSame('', invoke($module, 'getSafeFileExtension', ['DUPONT-Marie']));
        $t->assertSame('', invoke($module, 'getSafeFileExtension', ['scan.dupont-marie']));
        $t->assertSame('', invoke($module, 'getSafeFileExtension', ['scan.verylongextension']));
    });

    $t->test('an unsaved record cannot receive uploads', function ($t): void {
        // REDCap hands the module a placeholder while the record does not exist
        // yet. Filing uploads under it would create a Girder folder no record
        // points at, and bake the placeholder into the deidentified files.
        $module = moduleWithSettings();

        $t->assertFalse(invoke($module, 'isUsableRecordId', ['']));
        $t->assertFalse(invoke($module, 'isUsableRecordId', ['   ']));
        $t->assertFalse(invoke($module, 'isUsableRecordId', [null]));
        $t->assertFalse(invoke($module, 'isUsableRecordId', [
            'external-modules-temporary-record-id-1788122427-192289319',
        ]));

        $t->assertTrue(invoke($module, 'isUsableRecordId', ['103']));
        $t->assertTrue(invoke($module, 'isUsableRecordId', ['REC-42']));
    });

    $t->test('DICOM files are recognized by extension or MIME type', function ($t): void {
        $module = moduleWithSettings();

        $t->assertTrue(invoke($module, 'isDicomFile', ['0001.dcm', '']));
        $t->assertTrue(invoke($module, 'isDicomFile', ['0001.DCM', '']));
        $t->assertTrue(invoke($module, 'isDicomFile', ['unnamed', 'application/dicom']));
        $t->assertFalse(invoke($module, 'isDicomFile', ['ecg.xml', 'application/xml']));
        $t->assertFalse(invoke($module, 'isDicomFile', ['', '']));
    });
};
