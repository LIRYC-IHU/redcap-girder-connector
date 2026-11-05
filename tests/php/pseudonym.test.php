<?php

/**
 * The patient pseudonym on the REDCap side: evaluating the browser's blinded
 * element, and checking the patient signature of each upload against the
 * record's.
 */

declare(strict_types=1);

use ExternalModules\GirderUploaderModule\GirderUploaderModule;
use ExternalModules\GirderUploaderModule\Voprf;

use function GirderUploaderTests\invoke;

require_once __DIR__ . '/../../src/Voprf.php';

function pseudonymTestModule(array $projectSettings = [], array $systemSettings = []): GirderUploaderModule
{
    $module = new class extends GirderUploaderModule {
        /** @var string|null Exception message Girder answers with. */
        public $girderFailure = null;
        /** @var int Uploads the record holds in Girder. */
        public $recordUploads = 0;
        /** @var array<int, array> Entries of the project's Logging page. */
        public $projectEvents = [];

        protected function getGirderUploadFields($projectId, $instrument)
        {
            return ['scans'];
        }

        protected function assertGirderReachable($settings)
        {
            if ($this->girderFailure !== null) {
                throw new \Exception($this->girderFailure);
            }
        }

        protected function countRecordUploads($settings, $groupId, $record)
        {
            return $this->recordUploads;
        }

        protected function logProjectEvent($description, $changes, $record)
        {
            $this->projectEvents[] = [$description, $changes, $record];
        }
    };
    $module->projectSettings = array_merge([
        // A closed local port: no test may reach the network.
        'girder-backchannel-api-url' => 'http://127.0.0.1:9/api/v1',
        'girder-frontchannel-base-url' => 'http://127.0.0.1:9',
        'api-key' => 'girder-api-key',
        'root-collection-id' => 'collection-1',
    ], $projectSettings);
    $module->systemSettings = $systemSettings;

    return $module;
}

function protocolVectors(): array
{
    return json_decode((string) file_get_contents(__DIR__ . '/../../protocol/test-vectors/pseudonym.json'), true);
}

function evaluate(GirderUploaderModule $module, string $record = 'REC-1', ?string $blinded = null): array
{
    return invoke($module, 'handlePseudonymEvaluate', [
        ['fieldName' => 'scans', 'blindedElement' => $blinded ?? protocolVectors()['blindedElement']],
        12,
        'imaging',
        $record,
    ]);
}

function register(GirderUploaderModule $module, string $fingerprint, string $record = 'REC-1', bool $confirmedMismatch = false): array
{
    return invoke($module, 'handlePseudonymRegister', [
        ['fieldName' => 'scans', 'fingerprint' => $fingerprint, 'confirmedMismatch' => $confirmedMismatch],
        12,
        'imaging',
        $record,
    ]);
}

function check(GirderUploaderModule $module, string $fingerprint, string $record = 'REC-1'): array
{
    return invoke($module, 'handlePseudonymCheck', [
        ['fieldName' => 'scans', 'fingerprint' => $fingerprint],
        12,
        'imaging',
        $record,
    ]);
}

const FINGERPRINT_A = 'a1';
const FINGERPRINT_B = 'b2';

return function ($t): void {
    // --- Evaluation ------------------------------------------------------

    $t->test('the server evaluates, and its proof verifies', function ($t): void {
        $answer = evaluate(pseudonymTestModule());
        $blinded = protocolVectors()['blindedElement'];

        $t->assertTrue($answer['ok'], (string) ($answer['error'] ?? ''));
        $t->assertTrue(Voprf::verifyProof($answer['publicKey'], $blinded, $answer['evaluatedElement'], $answer['proof']));
    });

    $t->test('the REDCap key is created on first use and then kept', function ($t): void {
        $module = pseudonymTestModule();
        $first = evaluate($module);
        $key = $module->getSystemSetting('pseudonym-redcap-key');

        $t->assertTrue(Voprf::isValidKey((string) $key));
        $t->assertSame(Voprf::publicKey(Voprf::deriveProjectKey((string) $key, 'redcap:12')), $first['publicKey']);
        evaluate($module);
        $t->assertSame($key, $module->getSystemSetting('pseudonym-redcap-key'), 'the key must never be regenerated');
    });

    $t->test('the key evaluated with is the project domain\'s, whatever the browser says', function ($t): void {
        $module = pseudonymTestModule(['pseudonym-domain' => 'redcap:12'], ['pseudonym-redcap-key' => Voprf::generateKey()]);
        $other = pseudonymTestModule(['pseudonym-domain' => 'redcap:13'], $module->systemSettings);
        $blinded = protocolVectors()['blindedElement'] ?? null;

        $mine = evaluate($module, 'REC-1', $blinded);
        $theirs = evaluate($other, 'REC-1', $blinded);
        $master = Voprf::publicKey($module->getSystemSetting('pseudonym-redcap-key'));

        $t->assertTrue($mine['publicKey'] !== $theirs['publicKey']);
        $t->assertTrue($mine['evaluatedElement'] !== $theirs['evaluatedElement']);
        $t->assertTrue($mine['publicKey'] !== $master, 'the module key itself is never used');
    });

    $t->test('only an administrator may set the domain', function ($t): void {
        $config = json_decode(file_get_contents(__DIR__ . '/../../src/config.json'), true);
        $setting = array_values(array_filter($config['project-settings'], function ($entry) {
            return $entry['key'] === 'pseudonym-domain';
        }))[0];

        $t->assertTrue($setting['super-users-only']);
    });

    $t->test('the same blinded element always evaluates the same', function ($t): void {
        $module = pseudonymTestModule();

        $t->assertSame(evaluate($module)['evaluatedElement'], evaluate($module)['evaluatedElement']);
    });

    $t->test('an unusable REDCap key stops the evaluation', function ($t): void {
        $t->assertFalse(evaluate(pseudonymTestModule([], ['pseudonym-redcap-key' => 'not a key']))['ok']);
    });

    $t->test('a malformed blinded element is refused', function ($t): void {
        $module = pseudonymTestModule();
        foreach ([str_repeat('00', 32), 'abcd', str_repeat('zz', 32)] as $blinded) {
            $t->assertFalse(evaluate($module, 'REC-1', $blinded)['ok'], $blinded);
        }
    });

    $t->test('the daily limit stops evaluations', function ($t): void {
        $module = pseudonymTestModule([], ['pseudonym-daily-limit' => '2']);

        $t->assertTrue(evaluate($module)['ok']);
        $t->assertTrue(evaluate($module)['ok']);
        $third = evaluate($module);
        $t->assertFalse($third['ok']);
        $t->assertSame('rate-limit', $third['code']);
    });

    $t->test('an unreachable Girder stops the check, so nothing gets stored', function ($t): void {
        $module = pseudonymTestModule();
        $module->girderFailure = 'Connection refused';
        $answer = check($module, FINGERPRINT_A);

        $t->assertFalse($answer['ok']);
        $t->assertTrue(strpos($answer['error'], 'Girder is unavailable') !== false, $answer['error']);
    });

    $t->test('the domain defaults to the project id and is kept', function ($t): void {
        $module = pseudonymTestModule();

        $t->assertSame('redcap:12', invoke($module, 'pseudonymDomain', [12]));
        $t->assertSame('redcap:12', $module->getProjectSetting('pseudonym-domain'));
        $module->setProjectSetting('pseudonym-domain', 'redcap:migrated-7');
        $t->assertSame('redcap:migrated-7', invoke($module, 'pseudonymDomain', [12]));
    });

    // --- Fingerprint check -------------------------------------------------

    $t->test('the check before confirmation stores nothing', function ($t): void {
        $module = pseudonymTestModule();

        $t->assertSame('new', check($module, FINGERPRINT_A)['status']);
        $t->assertSame(null, $module->getProjectSetting('pseudonym-fingerprint:REC-1'));
        $t->assertSame('new', check($module, FINGERPRINT_B)['status'], 'still nothing stored');
    });

    $t->test('the check tells a known patient from another one', function ($t): void {
        $module = pseudonymTestModule();
        register($module, FINGERPRINT_A);

        $t->assertSame('matched', check($module, FINGERPRINT_A)['status']);
        $other = check($module, FINGERPRINT_B);
        $t->assertTrue($other['ok'], 'a mismatch is for the uploader to see, not an error');
        $t->assertSame('mismatch', $other['status']);
        $t->assertSame(FINGERPRINT_A, $module->getProjectSetting('pseudonym-fingerprint:REC-1'));
        $t->assertSame('[PSEUDONYM] fingerprint mismatch', end($module->logs)[0]);
    });

    $t->test('the check says how many uploads the record already holds', function ($t): void {
        $module = pseudonymTestModule();
        $t->assertSame(0, check($module, FINGERPRINT_A)['existingUploads']);

        // Uploads of an earlier version: content in Girder, no fingerprint.
        $module->recordUploads = 2;
        $answer = check($module, FINGERPRINT_A);
        $t->assertSame('new', $answer['status']);
        $t->assertSame(2, $answer['existingUploads']);
    });

    $t->test('with the box unticked the check answers unchecked', function ($t): void {
        $module = pseudonymTestModule(['pseudonym-fingerprint-check' => '0']);

        $t->assertSame('unchecked', check($module, FINGERPRINT_A)['status']);
        $t->assertFalse(check($module, 'not a fingerprint')['ok'], 'a malformed fingerprint is refused all the same');
    });

    $t->test("the record's first upload stores its fingerprint", function ($t): void {
        $module = pseudonymTestModule();
        $answer = register($module, FINGERPRINT_A);

        $t->assertTrue($answer['ok']);
        $t->assertSame('registered', $answer['status']);
        $t->assertSame(FINGERPRINT_A, $module->getProjectSetting('pseudonym-fingerprint:REC-1'));
    });

    $t->test('a later upload of the same patient is accepted', function ($t): void {
        $module = pseudonymTestModule();
        register($module, FINGERPRINT_A);
        $answer = register($module, strtoupper(FINGERPRINT_A));

        $t->assertTrue($answer['ok']);
        $t->assertSame('matched', $answer['status']);
    });

    $t->test('a mismatch the uploader never saw is refused', function ($t): void {
        $module = pseudonymTestModule();
        register($module, FINGERPRINT_A);
        $answer = register($module, FINGERPRINT_B);

        $t->assertFalse($answer['ok']);
        $t->assertSame('fingerprint-mismatch', $answer['code']);
        $t->assertSame(FINGERPRINT_A, $module->getProjectSetting('pseudonym-fingerprint:REC-1'), 'the stored fingerprint is kept');
        $t->assertSame([], $module->projectEvents);
    });

    $t->test('a confirmed mismatch goes through and leaves an alert', function ($t): void {
        $module = pseudonymTestModule();
        register($module, FINGERPRINT_A);
        $answer = register($module, FINGERPRINT_B, 'REC-1', true);

        $t->assertTrue($answer['ok']);
        $t->assertSame('overridden', $answer['status']);
        $t->assertSame(FINGERPRINT_A, $module->getProjectSetting('pseudonym-fingerprint:REC-1'), "the record keeps its fingerprint");
        $t->assertTrue(invoke($module, 'verifyPseudonymTicket', [12, 'REC-1', $answer['ticket']]));

        $t->assertSame(GirderUploaderModule::PSEUDONYM_OVERRIDE_LOG, end($module->logs)[0]);
        $t->assertSame(['record' => 'REC-1', 'field' => 'scans'], end($module->logs)[1]);
        $t->assertSame(1, count($module->projectEvents));
        $t->assertSame('REC-1', $module->projectEvents[0][2]);
    });

    $t->test('confirming when nothing differs raises no alert', function ($t): void {
        $module = pseudonymTestModule();
        register($module, FINGERPRINT_A);

        $t->assertSame('matched', register($module, FINGERPRINT_A, 'REC-1', true)['status']);
        $t->assertSame('registered', register($module, FINGERPRINT_B, 'REC-2', true)['status']);
        $t->assertSame([], $module->projectEvents);
    });

    $t->test('records are checked independently', function ($t): void {
        $module = pseudonymTestModule();
        register($module, FINGERPRINT_A, 'REC-1');

        $t->assertSame('registered', register($module, FINGERPRINT_B, 'REC-2')['status']);
    });

    $t->test('with the box unticked nothing is stored or compared', function ($t): void {
        $module = pseudonymTestModule(['pseudonym-fingerprint-check' => '0']);
        register($module, FINGERPRINT_A);
        $answer = register($module, FINGERPRINT_B);

        $t->assertTrue($answer['ok']);
        $t->assertSame('unchecked', $answer['status']);
        $t->assertSame(null, $module->getProjectSetting('pseudonym-fingerprint:REC-1'));
    });

    $t->test('the check is on by default', function ($t): void {
        $t->assertTrue(invoke(pseudonymTestModule(), 'isFingerprintCheckEnabled'));
    });

    $t->test('a malformed fingerprint is refused', function ($t): void {
        $module = pseudonymTestModule();
        foreach (['', 'abc', 'gg', str_repeat('a', 64)] as $fingerprint) {
            $t->assertFalse(register($module, $fingerprint)['ok'], $fingerprint);
        }
    });

    // --- Defaults -----------------------------------------------------------

    $t->test('every format is deidentified and names are neutral by default', function ($t): void {
        $settings = invoke(pseudonymTestModule(), 'loadSettings');

        $t->assertTrue($settings['deidentifyDicom']);
        $t->assertTrue($settings['deidentifyXmlEcg']);
        $t->assertTrue($settings['deidentifySchillerHolter']);
        $t->assertFalse($settings['preserveUploadFolderArchitecture']);
    });

    $t->test('a project can still opt out explicitly', function ($t): void {
        $settings = invoke(pseudonymTestModule([
            'deidentify-xml-ecg' => false,
            'deidentify-schiller-holter' => '',
            'preserve-upload-folder-architecture' => true,
        ]), 'loadSettings');

        $t->assertFalse($settings['deidentifyXmlEcg']);
        $t->assertFalse($settings['deidentifySchillerHolter']);
        $t->assertTrue($settings['preserveUploadFolderArchitecture']);
    });

    // --- Ticket -------------------------------------------------------------

    $t->test('a ticket opens the batch of its own record only', function ($t): void {
        $module = pseudonymTestModule();
        $ticket = register($module, FINGERPRINT_A)['ticket'];

        $t->assertTrue(invoke($module, 'verifyPseudonymTicket', [12, 'REC-1', $ticket]));
        $t->assertFalse(invoke($module, 'verifyPseudonymTicket', [12, 'REC-2', $ticket]), 'another record');
        $t->assertFalse(invoke($module, 'verifyPseudonymTicket', [13, 'REC-1', $ticket]), 'another project');
        $t->assertFalse(invoke($module, 'verifyPseudonymTicket', [12, 'REC-1', '']), 'no ticket');
        $t->assertFalse(invoke($module, 'verifyPseudonymTicket', [12, 'REC-1', substr($ticket, 0, -1) . 'x']), 'tampered');
    });

    $t->test('a ticket dies with the fingerprint it was issued for', function ($t): void {
        $module = pseudonymTestModule();
        $ticket = register($module, FINGERPRINT_A)['ticket'];
        $module->removeProjectSetting('pseudonym-fingerprint:REC-1');

        $t->assertFalse(invoke($module, 'verifyPseudonymTicket', [12, 'REC-1', $ticket]));
    });

    $t->test('an ordinary ticket cannot pass for a confirmed mismatch', function ($t): void {
        $module = pseudonymTestModule();
        $ticket = register($module, FINGERPRINT_B, 'REC-2')['ticket'];
        $module->setProjectSetting('pseudonym-fingerprint:REC-2', FINGERPRINT_A);
        $t->assertFalse(invoke($module, 'verifyPseudonymTicket', [12, 'REC-2', $ticket]), 'its fingerprint is no longer the record\'s');

        [$expires, $fingerprint, , $mac] = explode('.', $ticket);
        $forged = implode('.', [$expires, $fingerprint, GirderUploaderModule::PSEUDONYM_TICKET_OVERRIDE, $mac]);
        $t->assertFalse(invoke($module, 'verifyPseudonymTicket', [12, 'REC-2', $forged]), 'the mode is signed');
    });

    $t->test('a confirmed mismatch keeps a ticket only while the record has a fingerprint', function ($t): void {
        $module = pseudonymTestModule();
        register($module, FINGERPRINT_A);
        $ticket = register($module, FINGERPRINT_B, 'REC-1', true)['ticket'];
        $module->removeProjectSetting('pseudonym-fingerprint:REC-1');

        $t->assertFalse(invoke($module, 'verifyPseudonymTicket', [12, 'REC-1', $ticket]));
    });

    $t->test('a batch of deidentified files needs a ticket', function ($t): void {
        $module = pseudonymTestModule();
        $batch = function (string $ticket, bool $deidentified) use ($module) {
            return invoke($module, 'handleInitBatch', [[
                'fieldName' => 'scans',
                'pseudonymTicket' => $ticket,
                'files' => [['relativePath' => '0001.dcm', 'fileSize' => 10, 'deidentified' => $deidentified]],
            ], 12, 'imaging', null, 'REC-1', 1]);
        };
        $ticketError = 'The patient fingerprint of this upload was not checked against the record. Start the upload again.';

        $t->assertSame($ticketError, $batch('', true)['error']);
        // Past the check, the batch goes on to Girder, which is not there.
        $ticket = register($module, FINGERPRINT_A)['ticket'];
        $t->assertTrue(($batch($ticket, true)['error'] ?? '') !== $ticketError, 'a valid ticket passes');
        $t->assertTrue(($batch('', false)['error'] ?? '') !== $ticketError, 'files sent as they are need none');
    });

    // --- Upgrading from 1.x ---------------------------------------------------

    $t->test('a page opened before the update cannot start a batch', function ($t): void {
        $module = pseudonymTestModule(['pseudonym-fingerprint-check' => false]);
        // As 1.x sends it: no ticket, no file declared as deidentified.
        $answer = invoke($module, 'handleInitBatch', [[
            'fieldName' => 'scans',
            'files' => [['relativePath' => '0001.dcm', 'fileName' => '0001.dcm', 'fileSize' => 10]],
        ], 12, 'imaging', null, 'REC-1', 1]);

        $t->assertFalse($answer['ok']);
        $t->assertTrue(strpos($answer['error'], 'Reload') !== false, $answer['error']);
    });

    $t->test('a project configured under 1.x gets the check, and keeps its choices', function ($t): void {
        // What 1.x leaves behind: the stored booleans, and nothing about pseudonyms.
        $module = pseudonymTestModule([
            'deidentify-dicom' => true,
            'deidentify-xml-ecg' => false,
            'deidentify-schiller-holter' => false,
            'preserve-upload-folder-architecture' => true,
        ]);
        $settings = invoke($module, 'loadSettings');

        $t->assertTrue(invoke($module, 'isFingerprintCheckEnabled'), 'on, though the project never saw the box');
        $t->assertFalse($settings['deidentifyXmlEcg'], 'an explicit choice stays');
        $t->assertFalse($settings['deidentifySchillerHolter']);
        $t->assertTrue($settings['preserveUploadFolderArchitecture']);
    });

    $t->test('the project default of preserved folders survives as 1.x stored it', function ($t): void {
        // Enabling a project stores the default as written in config.json.
        $module = pseudonymTestModule(['preserve-upload-folder-architecture' => '1']);

        $t->assertTrue(invoke($module, 'loadSettings')['preserveUploadFolderArchitecture']);
    });

    $t->test('a key stored by nobody is created by the first evaluation', function ($t): void {
        $module = pseudonymTestModule();

        $t->assertTrue(evaluate($module)['ok']);
        $t->assertTrue(Voprf::isValidKey($module->getSystemSetting('pseudonym-redcap-key')));
    });

    $t->test('the module declares the PHP the protocol needs', function ($t): void {
        $config = json_decode(file_get_contents(__DIR__ . '/../../src/config.json'), true);

        $t->assertSame('8.1.0', $config['compatibility']['php-version-min']);
        $t->assertTrue(Voprf::isAvailable());
    });

    // --- Settings -----------------------------------------------------------

    $t->test('the settings form refuses an unusable key or limit', function ($t): void {
        $module = pseudonymTestModule();

        $t->assertSame(null, $module->validateSettings(['pseudonym-redcap-key' => '', 'pseudonym-daily-limit' => '']));
        $t->assertSame(null, $module->validateSettings(['pseudonym-redcap-key' => Voprf::generateKey(), 'pseudonym-daily-limit' => '500']));
        $t->assertTrue(is_string($module->validateSettings(['pseudonym-redcap-key' => 'abc'])));
        $t->assertTrue(is_string($module->validateSettings(['pseudonym-redcap-key' => str_repeat('00', 32)])), 'zero is not a key');
        $t->assertTrue(is_string($module->validateSettings(['pseudonym-daily-limit' => '0'])));
    });

    $t->test('the settings form keeps the domain to a plain identifier', function ($t): void {
        $module = pseudonymTestModule();

        $t->assertSame(null, $module->validateSettings(['pseudonym-domain' => 'redcap:12']));
        $t->assertSame(null, $module->validateSettings(['pseudonym-domain' => '']));
        $t->assertTrue(is_string($module->validateSettings(['pseudonym-domain' => 'x</script><script>alert(1)</script>'])));
    });

    $t->test('nothing in the page context can close its script block', function ($t): void {
        $module = pseudonymTestModule();
        $payload = 'x</script><script>alert(1)</script><!-- & \'';
        $encoded = invoke($module, 'encodeForInlineScript', [['projectTitle' => $payload, 'url' => 'https://a/b']]);

        $t->assertTrue(stripos($encoded, '</script') === false, $encoded);
        $t->assertTrue(strpos($encoded, '<!--') === false, $encoded);
        $t->assertSame($payload, json_decode($encoded, true)['projectTitle'], 'the browser reads the same value');
    });

    $t->test('the event folder carries the id and the name of the event', function ($t): void {
        $module = new class extends GirderUploaderModule {
            protected function eventName($eventId)
            {
                return 'Visit: M6 / follow-up';
            }
        };

        $name = invoke($module, 'eventFolderName', [43]);
        $t->assertSame('43 (Visit_ M6 _ follow-up)', $name);
        $t->assertTrue(invoke($module, 'isEventFolderName', [$name]));
    });

    $t->test('a field folder or a numbered record is no event folder', function ($t): void {
        $module = pseudonymTestModule();

        foreach (['girder_files', 'girder_files -2', '123', ''] as $name) {
            $t->assertFalse(invoke($module, 'isEventFolderName', [$name]), $name);
        }
    });

    $t->test('uploads are counted under events and, from 1.x, under the record', function ($t): void {
        $module = new class extends GirderUploaderModule {
            protected function listChildFolders($settings, $folderId)
            {
                return [
                    'record' => [
                        ['_id' => 'legacy', 'name' => 'scans'],
                        ['_id' => 'failed', 'name' => 'follow_up'],
                        ['_id' => 'event', 'name' => '43 (Event 1)'],
                    ],
                    'event' => [['_id' => 'new', 'name' => 'scans'], ['_id' => 'gone', 'name' => 'follow_up']],
                ][$folderId] ?? [];
            }

            protected function folderHoldsSomething($settings, $folderId)
            {
                return $folderId !== 'failed';
            }
        };

        $t->assertSame(3, invoke($module, 'countUploadsUnder', [[], 'record']));
        $t->assertSame(2, invoke($module, 'countUploadsUnder', [[], 'record', 'gone']), 'the folder being deleted does not count');
    });

    // --- Folders named by the browser --------------------------------------

    $placed = function (array $path, string $name = 'scans') {
        $module = new class extends GirderUploaderModule {
            /** @var array{0: array, 1: array}|null */
            public $placement = null;
            public $failure = null;
            public $uploadFields = [];

            protected function getGirderUploadFields($projectId, $instrument)
            {
                return $this->uploadFields;
            }

            protected function folderWithPath($settings, $folderId)
            {
                if ($this->failure !== null) {
                    throw new \Exception($this->failure);
                }
                return $this->placement;
            }
        };
        $folder = function ($id, $folderName) {
            return ['object' => ['_id' => $id, 'name' => $folderName]];
        };
        $module->placement = [['_id' => 'f', 'name' => $name], array_map(function ($entry) use ($folder) {
            return $folder($entry[0], $entry[1]);
        }, $path)];
        return $module;
    };
    $settings = ['rootCollectionId' => 'collection-1'];
    $locate = function ($module, string $record = 'REC-1', string $field = 'scans') use ($settings) {
        return invoke($module, 'locateUpload', [$settings, 'f', $record, $field]);
    };
    $collection = ['collection-1', 'REDCap'];
    $dag = ['dag', 'NO_DAG'];
    $record = ['record', 'REC-1'];
    $event = ['event', '43 (Event 1)'];

    $t->test("a field's folder is found, with or without its event", function ($t) use ($placed, $locate, $collection, $dag, $record, $event): void {
        $t->assertSame('record', $locate($placed([$collection, $dag, $record, $event])));
        $t->assertSame('record', $locate($placed([$collection, $dag, $record])), 'as 1.x stored it');
        $t->assertSame('record', $locate($placed([$collection, $dag, $record, $event], 'scans -2')), 'a repeating instance');
    });

    $t->test('a folder anywhere else is refused', function ($t) use ($placed, $locate, $collection, $dag, $record, $event): void {
        $cases = [
            'another record' => $placed([$collection, $dag, ['other', 'REC-2'], $event]),
            'another collection' => $placed([['collection-2', 'REDCap'], $dag, $record, $event]),
            'another field' => $placed([$collection, $dag, $record, $event], 'consent_scans'),
            'a field with the same start' => $placed([$collection, $dag, $record, $event], 'scans_extra'),
            'the record itself' => $placed([$collection, $dag], 'REC-1'),
            'a folder inside an upload' => $placed([$collection, $dag, $record, $event, ['field', 'consent_scans']]),
            'under something that is no event' => $placed([$collection, $dag, $record, ['field', 'consent_scans']]),
        ];
        foreach ($cases as $label => $module) {
            $t->assertSame(false, $locate($module), $label);
        }
    });

    $t->test('a folder Girder no longer has is gone, not an error', function ($t) use ($placed, $locate, $collection, $dag, $record): void {
        $module = $placed([$collection, $dag, $record]);
        $module->failure = 'Girder request failed (400): {"message": "Invalid folder id (abc).", "type": "rest"}';
        $t->assertSame(null, $locate($module));

        $module->failure = 'Girder request failed (500): boom';
        $failed = false;
        try {
            $locate($module);
        } catch (\Exception $error) {
            $failed = true;
        }
        $t->assertTrue($failed, 'an outage is not a missing folder');
    });

    $t->test('deleting or refreshing a folder out of scope is refused and logged', function ($t) use ($placed, $collection, $dag, $event): void {
        $module = $placed([$collection, $dag, ['other', 'REC-2'], $event]);
        $module->projectSettings = pseudonymTestModule()->projectSettings;
        $call = function (string $method) use ($module) {
            return invoke($module, $method, [['fieldName' => 'scans', 'folderId' => 'f'], 12, 'imaging', 'REC-1']);
        };
        $module->uploadFields = ['scans'];

        foreach (['handleDeleteUploadContents', 'handleRefreshUploadMetadata'] as $method) {
            $answer = $call($method);
            $t->assertFalse($answer['ok'], $method);
            $t->assertTrue(strpos($answer['error'], 'does not belong') !== false, $answer['error']);
        }
        $t->assertSame('[GIRDER] folder outside the field refused', end($module->logs)[0]);
    });

    $t->test('enabling the module creates the REDCap key and the project domain', function ($t): void {
        $module = pseudonymTestModule();
        $module->redcap_module_system_enable('v2.0.0');
        $module->redcap_module_project_enable('v2.0.0', 12);

        $t->assertTrue(Voprf::isValidKey((string) $module->getSystemSetting('pseudonym-redcap-key')));
        $t->assertSame('redcap:12', $module->getProjectSetting('pseudonym-domain'));
    });
};
