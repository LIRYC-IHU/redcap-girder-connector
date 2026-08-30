<?php

/**
 * Project settings: URL normalization and the way checkbox settings are read.
 * Getting a deidentification checkbox wrong means uploading identified data,
 * so the empty/absent distinction is pinned down here.
 */

declare(strict_types=1);

use function GirderUploaderTests\invoke;
use function GirderUploaderTests\moduleWithSettings;

return function ($t): void {
    $t->test('urls are normalized without a trailing slash', function ($t): void {
        $module = moduleWithSettings();

        $t->assertSame('https://girder.example.org/api/v1', invoke($module, 'normalizeApiUrl', ['  https://girder.example.org/api/v1/  ']));
        $t->assertSame('https://girder.example.org', invoke($module, 'normalizeBaseUrl', ['https://girder.example.org/']));
        $t->assertSame('', invoke($module, 'normalizeApiUrl', ['   ']));
    });

    $t->test('a legacy api url yields the front-channel base url', function ($t): void {
        $module = moduleWithSettings();

        $t->assertSame(
            'https://girder.example.org',
            invoke($module, 'deriveBaseUrlFromLegacyApiUrl', ['https://girder.example.org/api/v1'])
        );
        $t->assertSame(
            'https://girder.example.org',
            invoke($module, 'deriveBaseUrlFromLegacyApiUrl', ['https://girder.example.org'])
        );
        $t->assertSame('', invoke($module, 'deriveBaseUrlFromLegacyApiUrl', ['']));
    });

    $t->test('numeric settings fall back to their default when unusable', function ($t): void {
        $module = moduleWithSettings([
            'chunk-size' => '5242880',
            'max-retries' => 'not a number',
            'retry-delay' => '0',
        ]);

        $t->assertSame(5242880, invoke($module, 'readIntSetting', ['chunk-size', 10485760]));
        $t->assertSame(3, invoke($module, 'readIntSetting', ['max-retries', 3]));
        $t->assertSame(1000, invoke($module, 'readIntSetting', ['retry-delay', 1000]), 'zero is not a usable delay');
        $t->assertSame(10485760, invoke($module, 'readIntSetting', ['absent', 10485760]));
    });

    $t->test('checkbox settings accept the usual truthy spellings', function ($t): void {
        $module = moduleWithSettings([
            'on' => 'on',
            'yes' => 'YES',
            'one' => '1',
            'off' => 'off',
            'zero' => '0',
            'bool' => true,
        ]);

        foreach (['on', 'yes', 'one', 'bool'] as $key) {
            $t->assertTrue(invoke($module, 'readBoolSetting', [$key, false]), "{$key} should read as enabled");
        }
        foreach (['off', 'zero'] as $key) {
            $t->assertFalse(invoke($module, 'readBoolSetting', [$key, true]), "{$key} should read as disabled");
        }
    });

    $t->test('an unticked checkbox disables deidentification even when it defaults to on', function ($t): void {
        // REDCap stores an unticked checkbox as an empty string, which is not
        // the same as "never configured": a project that ticked the box off
        // must not silently get the enabled-by-default behaviour back.
        $unticked = moduleWithSettings(['deidentify-dicom' => '']);
        $neverSet = moduleWithSettings([]);

        $t->assertFalse(invoke($unticked, 'readBoolSettingTreatEmptyAsFalse', ['deidentify-dicom', true]));
        $t->assertTrue(invoke($neverSet, 'readBoolSettingTreatEmptyAsFalse', ['deidentify-dicom', true]));
    });
};
