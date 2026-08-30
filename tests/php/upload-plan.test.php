<?php

/**
 * Upload plans are built server-side, handed to the browser, and sent back with
 * every chunk. The signature is what stops a browser from rewriting the plan to
 * push files into another project's collection.
 */

declare(strict_types=1);

use function GirderUploaderTests\invoke;
use function GirderUploaderTests\moduleWithSettings;

function girderTestPlan(array $overrides = []): array
{
    return array_merge([
        'folderId' => 'folder-1',
        'parentFolderId' => 'folder-0',
        'dagName' => 'BORDEAUX',
        'recordId' => 'REC-42',
        'instanceId' => '1',
        'girderUrl' => 'https://girder.example.org/api/v1',
        'girderFrontchannelBaseUrl' => 'https://girder.example.org',
        'rootCollectionId' => 'collection-1',
        'storedFileName' => '0001.dcm',
        'itemName' => 'DICOM_DATA',
        'fileSize' => 2048,
        'mimeType' => 'application/dicom',
        'isDicom' => true,
        'relativePath' => 'study/0001.dcm',
    ], $overrides);
}

return function ($t): void {
    $settings = ['apiKey' => 'girder-api-key'];

    $t->test('a freshly signed plan verifies', function ($t) use ($settings): void {
        $module = moduleWithSettings();
        $plan = girderTestPlan();
        $signature = invoke($module, 'signUploadPlan', [$settings, $plan]);

        $t->assertTrue(invoke($module, 'verifyUploadPlanSignature', [$settings, $plan, $signature]));
    });

    $t->test('an unsigned plan is rejected', function ($t) use ($settings): void {
        $module = moduleWithSettings();

        $t->assertFalse(invoke($module, 'verifyUploadPlanSignature', [$settings, girderTestPlan(), '']));
        $t->assertFalse(invoke($module, 'verifyUploadPlanSignature', [$settings, girderTestPlan(), 'deadbeef']));
    });

    $t->test('every routing field is covered by the signature', function ($t) use ($settings): void {
        $module = moduleWithSettings();
        $plan = girderTestPlan();
        $signature = invoke($module, 'signUploadPlan', [$settings, $plan]);

        $tampered = [
            'folderId' => 'someone-elses-folder',
            'parentFolderId' => 'someone-elses-folder',
            'rootCollectionId' => 'someone-elses-collection',
            'recordId' => 'REC-99',
            'dagName' => 'OTHER-SITE',
            'instanceId' => '2',
            'storedFileName' => 'DUPONT-Marie.dcm',
            'itemName' => 'DUPONT-Marie',
            'fileSize' => 4096,
            'mimeType' => 'text/plain',
            'isDicom' => false,
            'girderUrl' => 'https://attacker.example.org/api/v1',
            'girderFrontchannelBaseUrl' => 'https://attacker.example.org',
            'relativePath' => '../elsewhere/0001.dcm',
        ];

        foreach ($tampered as $field => $value) {
            $t->assertFalse(
                invoke($module, 'verifyUploadPlanSignature', [$settings, girderTestPlan([$field => $value]), $signature]),
                "tampering with {$field} was not detected"
            );
        }
    });

    $t->test('a signature does not carry over to another project', function ($t) use ($settings): void {
        $module = moduleWithSettings();
        $plan = girderTestPlan();
        $signature = invoke($module, 'signUploadPlan', [$settings, $plan]);

        $otherProject = ['apiKey' => 'another-girder-api-key'];
        $t->assertFalse(invoke($module, 'verifyUploadPlanSignature', [$otherProject, $plan, $signature]));
    });

    $t->test('signing ignores fields the server recomputes', function ($t) use ($settings): void {
        $module = moduleWithSettings();
        $plan = girderTestPlan();
        $signature = invoke($module, 'signUploadPlan', [$settings, $plan]);

        // The upload and item ids are assigned after signing, so they must not
        // invalidate the plan the browser sends back.
        $withIds = girderTestPlan(['uploadId' => 'upload-1', 'itemId' => 'item-1', 'planSignature' => $signature]);
        $t->assertTrue(invoke($module, 'verifyUploadPlanSignature', [$settings, $withIds, $signature]));
    });

    $t->test('the canonical form is stable regardless of key order', function ($t) use ($settings): void {
        $module = moduleWithSettings();
        $plan = girderTestPlan();
        $shuffled = array_reverse($plan, true);

        $t->assertSame(
            invoke($module, 'canonicalizeUploadPlanForSignature', [$plan]),
            invoke($module, 'canonicalizeUploadPlanForSignature', [$shuffled])
        );
    });
};
