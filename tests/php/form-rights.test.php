<?php

/**
 * Who may upload to, or delete from, an instrument. REDCap has two encodings
 * for a user's right on a form, and they disagree on what `1` and `2` mean, so
 * every value REDCap can store is pinned down here, in both directions.
 */

declare(strict_types=1);

use function GirderUploaderTests\invoke;
use function GirderUploaderTests\moduleWithSettings;

/** Run $body as $username, with REDCap's $user_rights global as given (null: unset). */
function asUser(string $username, ?array $globalUserRights, callable $body)
{
    $saved = [
        'userid' => $GLOBALS['userid'] ?? null,
        'user_rights' => $GLOBALS['user_rights'] ?? null,
    ];
    $GLOBALS['userid'] = $username;
    $GLOBALS['user_rights'] = $globalUserRights;

    try {
        return $body();
    } finally {
        foreach ($saved as $key => $value) {
            if ($value === null) {
                unset($GLOBALS[$key]);
            } else {
                $GLOBALS[$key] = $value;
            }
        }
    }
}

return function ($t): void {
    $t->test('legacy form rights: only view & edit and edit survey responses may modify', function ($t): void {
        $module = moduleWithSettings();

        $t->assertFalse(invoke($module, 'formRightAllowsEditing', ['0']), '0 is no access');
        $t->assertTrue(invoke($module, 'formRightAllowsEditing', ['1']), '1 is view & edit');
        $t->assertFalse(invoke($module, 'formRightAllowsEditing', ['2']), '2 is read-only');
        $t->assertTrue(invoke($module, 'formRightAllowsEditing', ['3']), '3 is view & edit + edit survey responses');
        $t->assertTrue(invoke($module, 'formRightAllowsEditing', [1]), 'an integer works too');
        $t->assertFalse(invoke($module, 'formRightAllowsEditing', [2]), 'an integer works too');
    });

    $t->test('bitmask form rights: any value carrying view & edit may modify', function ($t): void {
        $module = moduleWithSettings();

        // Every value REDCap 15 accepts (UserRights::isValidDataViewingRightsValue).
        $t->assertFalse(invoke($module, 'formRightAllowsEditing', ['128']), '128 is no access');
        $t->assertFalse(invoke($module, 'formRightAllowsEditing', ['129']), '129 is read-only');
        $t->assertTrue(invoke($module, 'formRightAllowsEditing', ['130']), '130 is view & edit');
        $t->assertTrue(invoke($module, 'formRightAllowsEditing', ['138']), '138 is view & edit + edit survey responses');
        $t->assertTrue(invoke($module, 'formRightAllowsEditing', ['146']), '146 is view & edit + delete');
        $t->assertTrue(invoke($module, 'formRightAllowsEditing', ['154']), '154 is all of them');
    });

    $t->test('unreadable form rights refuse', function ($t): void {
        $module = moduleWithSettings();

        foreach (['', ' ', 'view-edit', '1.5', '-1', null, false, true, []] as $value) {
            $t->assertFalse(invoke($module, 'formRightAllowsEditing', [$value]), var_export($value, true));
        }
    });

    $t->test("REDCap's own user rights for the request decide when present", function ($t): void {
        $module = moduleWithSettings();
        // Present in the global: the raw rows must not be consulted.
        $module->projectRights = ['alice' => ['data_entry' => '[uploads,2]']];

        $rights = ['forms' => ['uploads' => '130', 'consent' => '129', 'baseline' => '1', 'legacy_ro' => '2']];
        asUser('alice', $rights, function () use ($t, $module): void {
            $t->assertTrue(invoke($module, 'userCanModifyInstrument', ['uploads']));
            $t->assertTrue(invoke($module, 'userCanModifyInstrument', ['baseline']));
            $t->assertFalse(invoke($module, 'userCanModifyInstrument', ['consent']));
            $t->assertFalse(invoke($module, 'userCanModifyInstrument', ['legacy_ro']));
            $t->assertFalse(invoke($module, 'userCanModifyInstrument', ['absent']), 'an instrument without a right');
            $t->assertFalse(invoke($module, 'userCanModifyInstrument', ['']), 'no instrument');
        });
    });

    $t->test('without the global, the raw data_entry column of the project rights is read', function ($t): void {
        $module = moduleWithSettings();
        $module->projectRights = [
            'editor' => ['data_entry' => '[demographics,129][uploads,130]'],
            'legacy_editor' => ['data_entry' => '[demographics,2][uploads,1]'],
            'survey_editor' => ['data_entry' => '[uploads,3]'],
            'reader' => ['data_entry' => '[demographics,130][uploads,129]'],
            'legacy_reader' => ['data_entry' => '[uploads,2]'],
            'no_access' => ['data_entry' => '[uploads,128]'],
            'bare' => ['data_entry' => '[uploads]'],
            'empty' => ['data_entry' => ''],
        ];

        $expected = [
            'editor' => true,
            'legacy_editor' => true,
            'survey_editor' => true,
            'reader' => false,
            'legacy_reader' => false,
            'no_access' => false,
            'bare' => false,
            'empty' => false,
        ];
        foreach ($expected as $username => $allowed) {
            $actual = asUser($username, null, function () use ($module) {
                return invoke($module, 'userCanModifyInstrument', ['uploads']);
            });
            $t->assertSame($allowed, $actual, $username);
        }
    });

    $t->test('form rights already decoded by the framework are used as such', function ($t): void {
        $module = moduleWithSettings();
        $module->projectRights = ['alice' => ['forms' => ['uploads' => '130', 'consent' => '2']]];

        asUser('alice', null, function () use ($t, $module): void {
            $t->assertTrue(invoke($module, 'userCanModifyInstrument', ['uploads']));
            $t->assertFalse(invoke($module, 'userCanModifyInstrument', ['consent']));
        });
    });

    $t->test('a user who is not on the project may modify only as a super user', function ($t): void {
        $module = moduleWithSettings();
        $module->projectRights = ['alice' => ['data_entry' => '[uploads,130]']];
        $module->superUsers = ['admin'];

        asUser('mallory', null, function () use ($t, $module): void {
            $t->assertFalse(invoke($module, 'userCanModifyInstrument', ['uploads']));
        });
        asUser('admin', null, function () use ($t, $module): void {
            $t->assertTrue(invoke($module, 'userCanModifyInstrument', ['uploads']));
        });
    });

    $t->test('no user or no project context refuses', function ($t): void {
        $module = moduleWithSettings();
        $module->projectRights = ['alice' => ['data_entry' => '[uploads,130]']];

        asUser('', null, function () use ($t, $module): void {
            $t->assertFalse(invoke($module, 'userCanModifyInstrument', ['uploads']), 'no user');
        });

        $module->projectRights = null;
        asUser('alice', null, function () use ($t, $module): void {
            $t->assertFalse(invoke($module, 'userCanModifyInstrument', ['uploads']), 'no project');
        });
    });
};
