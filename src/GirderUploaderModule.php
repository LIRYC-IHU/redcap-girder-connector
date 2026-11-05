<?php

namespace ExternalModules\GirderUploaderModule;

use ExternalModules\AbstractExternalModule;

require_once __DIR__ . '/Voprf.php';

class GirderUploaderModule extends AbstractExternalModule
{
    /** System setting: the OPRF key behind every pseudonym (hex scalar). */
    const PSEUDONYM_KEY_SETTING = 'pseudonym-redcap-key';
    /** System setting: evaluations accepted per project and per 24 hours. */
    const PSEUDONYM_LIMIT_SETTING = 'pseudonym-daily-limit';
    const PSEUDONYM_DEFAULT_DAILY_LIMIT = 500;
    /** Project setting: separates one project's pseudonyms from another's. */
    const PSEUDONYM_DOMAIN_SETTING = 'pseudonym-domain';
    /** Project checkbox: compare each upload's fingerprint with the record's. */
    const PSEUDONYM_CHECK_SETTING = 'pseudonym-fingerprint-check';
    /** Internal project settings, one per record: the stored fingerprint. */
    const PSEUDONYM_FINGERPRINT_PREFIX = 'pseudonym-fingerprint:';
    /** Log message counted by the rate limit. */
    const PSEUDONYM_EVALUATION_LOG = 'pseudonym-evaluate';
    /** Log message of an upload confirmed despite a fingerprint mismatch. */
    const PSEUDONYM_OVERRIDE_LOG = '[PSEUDONYM] fingerprint mismatch confirmed by the uploader';
    const PSEUDONYM_TICKET_TTL_SECONDS = 3600;
    const PSEUDONYM_TICKET_MATCH = 'match';
    const PSEUDONYM_TICKET_OVERRIDE = 'override';

    public function redcap_module_system_enable($version)
    {
        $this->ensurePseudonymKey();
    }

    public function redcap_module_project_enable($version, $project_id)
    {
        $this->pseudonymDomain($project_id);
    }

    public function redcap_module_save_configuration($project_id)
    {
        if ($project_id) {
            $this->pseudonymDomain($project_id);
        } else {
            $this->ensurePseudonymKey();
        }
    }

    /**
     * Called by REDCap before a settings form is saved; a returned string blocks
     * the save and is shown to the user.
     */
    public function validateSettings($settings)
    {
        $key = isset($settings[self::PSEUDONYM_KEY_SETTING]) && is_scalar($settings[self::PSEUDONYM_KEY_SETTING])
            ? trim((string) $settings[self::PSEUDONYM_KEY_SETTING])
            : '';
        if ($key !== '' && !Voprf::isValidKey($key)) {
            return 'The REDCap pseudonym key must be 64 hexadecimal characters encoding a valid scalar, or left empty to have one generated.';
        }

        $domain = isset($settings[self::PSEUDONYM_DOMAIN_SETTING]) && is_scalar($settings[self::PSEUDONYM_DOMAIN_SETTING])
            ? trim((string) $settings[self::PSEUDONYM_DOMAIN_SETTING])
            : '';
        if ($domain !== '' && !preg_match('/^[A-Za-z0-9:._-]{1,128}$/', $domain)) {
            return 'The pseudonym domain may only hold letters, digits and the characters : . _ - (128 at most).';
        }

        $limit = isset($settings[self::PSEUDONYM_LIMIT_SETTING]) && is_scalar($settings[self::PSEUDONYM_LIMIT_SETTING])
            ? trim((string) $settings[self::PSEUDONYM_LIMIT_SETTING])
            : '';
        if ($limit !== '' && (!ctype_digit($limit) || (int) $limit < 1)) {
            return 'The pseudonym daily limit must be a positive whole number.';
        }

        return null;
    }

    public function redcap_module_link_check_display($project_id, $link)
    {
        $settings = $this->loadSettings();

        if (is_array($link) && ($link['name'] ?? '') === 'Girder Collection') {
            if (!empty($settings['girderFrontchannelBaseUrl']) && !empty($settings['rootCollectionId'])) {
                $base = rtrim((string) $settings['girderFrontchannelBaseUrl'], '/');
                $id = rawurlencode((string) $settings['rootCollectionId']);

                // trailing ? makes appended params become hash-query suffix
                // e.g. #collection/<id>?&pid=16
                $link['url'] = "{$base}/#collection/{$id}?";
            } else {
                $link['url'] = '#';
            }
        }

        return $link;
    }

    public function redcap_data_entry_form($project_id, $record, $instrument, $event_id, $group_id = null, $repeat_instance = 1)
    {
        $this->consolePrint('Hello from data entry form top!');
        $fields = $this->getGirderUploadFields($project_id, $instrument);
        if (empty($fields)) {
            return;
        }

        $settings = $this->loadSettings();
        if (empty($settings['girderUrl']) || empty($settings['girderFrontchannelBaseUrl']) || empty($settings['apiKey']) || empty($settings['rootCollectionId'])) {
            return;
        }

        $this->consolePrint('Getting context');
        $context = [
            'projectId' => $project_id,
            'projectTitle' => $this->getProjectTitle($project_id),
            'instrument' => $instrument,
            'recordId' => $record,
            'pseudonym' => [
                'domain' => $this->pseudonymDomain($project_id),
                'fingerprintCheck' => $this->isFingerprintCheckEnabled(),
            ],
            'eventId' => $event_id,
            'instanceId' => $repeat_instance,
            'dagName' => $this->getDagName($group_id),
            'fields' => array_values($fields),
            'recordListCacheUrl' => $this->getRecordListCacheUrl($project_id),
            'permissions' => [
                'canModify' => $this->userCanModifyInstrument($instrument),
                'recordSaved' => $this->isUsableRecordId($record),
            ],
            'settings' => [
                'chunkSize' => (int) $settings['chunkSize'],
                'maxRetries' => (int) $settings['maxRetries'],
                'retryDelay' => (int) $settings['retryDelay'],
                'deidentifyDicom' => (bool) $settings['deidentifyDicom'],
                'deidentifyXmlEcg' => (bool) $settings['deidentifyXmlEcg'],
                'deidentifySchillerHolter' => (bool) $settings['deidentifySchillerHolter'],
                'preserveUploadFolderArchitecture' => (bool) $settings['preserveUploadFolderArchitecture'],
                'deidentifyWorkerUrl' => $this->getUrl('js/deidentify-worker.js', true),
            ],
        ];

        $this->consolePrint('Initializing Girder Uploader...');

        print($this->initializeJavascriptModuleObject());

        $this->consolePrint('Javascript module object initialized: ' . $this->getJavascriptModuleObjectName());
        print('<script>'. 'window.GirderUploaderConfig=' . $this->encodeForInlineScript($context) . ';' . 'window.GirderUploaderModule=' . $this->getJavascriptModuleObjectName() . ';' . '</script>');
        if (file_exists(__DIR__ . '/js/vendor/jszip.min.js')) {
            $this->includeJs('./js/vendor/jszip.min.js');
        } else {
            // Fallback if vendor file is not deployed.
            echo '<script src="https://cdn.jsdelivr.net/npm/jszip@3.10.1/dist/jszip.min.js"></script>';
        }
        $this->includeJs('./js/girder-uploader-core.js');
        $this->includeJs('./js/girder-uploader.js');
        $this->includeCSS('./css/style.css');
        $this->consolePrint('Girder Uploader initialized');
    }

    public function redcap_module_ajax($action, $payload, $project_id, $record, $instrument, $event_id, $repeat_instance, $survey_hash, $response_id, $survey_queue_hash, $page, $page_full, $user_id, $group_id)
    {
        $this->debugAjax('Incoming AJAX action', [
            'action' => (string) $action,
            'project_id' => (int) $project_id,
            'record' => (string) $record,
            'instrument' => (string) $instrument,
            'event_id' => (int) $event_id,
            'repeat_instance' => (int) $repeat_instance,
            'user_id' => (string) $user_id,
            'group_id' => (int) $group_id,
        ]);

        if (in_array($action, ['init-batch', 'init-file-upload', 'upload-file', 'pseudonym-evaluate', 'pseudonym-check', 'pseudonym-register'], true)
            && !$this->isUsableRecordId($record)) {
            $this->debugAjax('rejected: the record does not exist yet', [
                'action' => (string) $action,
            ]);
            return [
                'ok' => false,
                'error' => 'Save this record before uploading: files are stored under its record id.',
            ];
        }

        if ($action === 'init-batch') {
            if (!$this->userCanModifyInstrument((string) $instrument)) {
                return [
                    'ok' => false,
                    'error' => 'Uploads are disabled because this instrument is read-only for the current user.',
                ];
            }
            return $this->handleInitBatch($payload, (int) $project_id, (string) $instrument, $group_id, (string) $record, (int) $repeat_instance, (int) $event_id);
        }

        if (in_array($action, ['pseudonym-evaluate', 'pseudonym-check', 'pseudonym-register'], true)) {
            if (!$this->userCanModifyInstrument((string) $instrument)) {
                return [
                    'ok' => false,
                    'error' => 'Uploads are disabled because this instrument is read-only for the current user.',
                ];
            }
            if ($action === 'pseudonym-evaluate') {
                return $this->handlePseudonymEvaluate($payload, (int) $project_id, (string) $instrument, (string) $record);
            }
            if ($action === 'pseudonym-check') {
                return $this->handlePseudonymCheck($payload, (int) $project_id, (string) $instrument, (string) $record, $group_id);
            }
            return $this->handlePseudonymRegister($payload, (int) $project_id, (string) $instrument, (string) $record);
        }

        if ($action === 'upload-file') {
            if (!$this->userCanModifyInstrument((string) $instrument)) {
                return [
                    'ok' => false,
                    'error' => 'Uploads are disabled because this instrument is read-only for the current user.',
                ];
            }
            return $this->handleUploadFile($payload, (int) $project_id, (string) $instrument);
        }

        if ($action === 'init-file-upload') {
            if (!$this->userCanModifyInstrument((string) $instrument)) {
                return [
                    'ok' => false,
                    'error' => 'Uploads are disabled because this instrument is read-only for the current user.',
                ];
            }
            return $this->handleInitFileUpload($payload, (int) $project_id, (string) $instrument);
        }

        if ($action === 'refresh-upload-metadata') {
            return $this->handleRefreshUploadMetadata($payload, (int) $project_id, (string) $instrument, (string) $record);
        }

        if ($action === 'delete-upload-contents') {
            if (!$this->userCanModifyInstrument((string) $instrument)) {
                return [
                    'ok' => false,
                    'error' => 'Deletion is disabled because this instrument is read-only for the current user.',
                ];
            }
            return $this->handleDeleteUploadContents($payload, (int) $project_id, (string) $instrument, (string) $record);
        }

        $this->debugAjax('Unknown AJAX action', [
            'action' => (string) $action,
        ]);

        return [
            'ok' => false,
            'error' => 'Unknown action.',
        ];
    }

    /**
     * JSON for an inline <script>. The project title and the settings are free
     * text: without the hex flags, a `</script>` in one of them would end the
     * block and run what follows on every user opening the form.
     */
    private function encodeForInlineScript($value)
    {
        return json_encode($value, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_HEX_TAG | JSON_HEX_AMP);
    }

    private function handleInitBatch($payload, $projectId, $instrument, $groupId, $record, $repeatInstance, $eventId = 0)
    {
        $data = is_array($payload) ? $payload : [];
        $fieldName = isset($data['fieldName']) ? (string) $data['fieldName'] : '';
        $files = isset($data['files']) && is_array($data['files']) ? $data['files'] : [];

        $this->debugAjax('init-batch payload parsed', [
            'fieldName' => $fieldName,
            'fileCount' => count($files),
            'projectId' => (int) $projectId,
            'instrument' => (string) $instrument,
            'repeatInstance' => (int) $repeatInstance,
        ]);

        if ($fieldName === '' || empty($files)) {
            $this->debugAjax('init-batch rejected: invalid payload');
            return [
                'ok' => false,
                'error' => 'Invalid upload batch initialization payload.',
            ];
        }
        if (count($files) > 9999) {
            $this->debugAjax('init-batch rejected: too many files', [
                'fileCount' => count($files),
            ]);
            return [
                'ok' => false,
                'error' => 'A maximum of 9999 files can be uploaded at once.',
            ];
        }

        $allowedFields = $this->getGirderUploadFields($projectId, $instrument);
        if (!in_array($fieldName, $allowedFields, true)) {
            $this->debugAjax('init-batch rejected: unauthorized field', [
                'fieldName' => $fieldName,
            ]);
            return [
                'ok' => false,
                'error' => 'Field is not allowed for Girder upload.',
            ];
        }

        $settings = $this->loadSettings();
        if (empty($settings['girderUrl']) || empty($settings['girderFrontchannelBaseUrl']) || empty($settings['apiKey']) || empty($settings['rootCollectionId'])) {
            $this->debugAjax('init-batch rejected: incomplete settings');
            return [
                'ok' => false,
                'error' => 'Girder Uploader module settings are incomplete.',
            ];
        }

        // A page opened before the module was updated runs the old deidentification
        // and declares no file as deidentified: it would slip past the check below.
        if (!array_key_exists('pseudonymTicket', $data)) {
            $this->debugAjax('init-batch rejected: page older than the module');
            return [
                'ok' => false,
                'error' => 'This page was opened before the module was updated. Reload it and start the upload again. Nothing was uploaded.',
            ];
        }

        $carriesDeidentifiedFiles = false;
        foreach ($files as $fileInfo) {
            if (is_array($fileInfo) && !empty($fileInfo['deidentified'])) {
                $carriesDeidentifiedFiles = true;
                break;
            }
        }
        $ticket = isset($data['pseudonymTicket']) ? (string) $data['pseudonymTicket'] : '';
        if ($carriesDeidentifiedFiles && $this->isFingerprintCheckEnabled()
            && !$this->verifyPseudonymTicket($projectId, $record, $ticket)) {
            $this->debugAjax('init-batch rejected: no valid pseudonym ticket');
            return [
                'ok' => false,
                'error' => 'The patient fingerprint of this upload was not checked against the record. Start the upload again.',
            ];
        }

        try {
            $dagName = $this->sanitizePathPart($this->getDagName($groupId), 'NO_DAG');
            $recordId = $this->sanitizePathPart($record, 'UNASSIGNED_RECORD');
            $fieldFolderName = $this->sanitizePathPart($fieldName, 'upload');
            if ($repeatInstance > 1) {
                $fieldFolderName .= ' -' . $this->sanitizePathPart((string) $repeatInstance, '1');
            }

            $dagFolder = $this->ensureFolder($settings, 'collection', (string) $settings['rootCollectionId'], $dagName);
            $recordFolder = $this->ensureFolder($settings, 'folder', (string) $dagFolder['_id'], $recordId);
            $eventFolder = $this->ensureFolder($settings, 'folder', (string) $recordFolder['_id'], $this->eventFolderName($eventId));
            $fieldFolder = $this->ensureFolder($settings, 'folder', (string) $eventFolder['_id'], $fieldFolderName);
            $dicomFlatItemNameBySourceFolder = [];
            $preserveUploadFolderArchitecture = !empty($settings['preserveUploadFolderArchitecture']);

            $uploads = [];
            foreach ($files as $index => $fileInfo) {
                if (!is_array($fileInfo)) {
                    throw new \Exception('Invalid file entry at index ' . $index . '.');
                }

                $relativePath = isset($fileInfo['relativePath']) ? (string) $fileInfo['relativePath'] : '';
                $fileName = isset($fileInfo['fileName']) ? (string) $fileInfo['fileName'] : '';
                $fileSize = isset($fileInfo['fileSize']) ? (int) $fileInfo['fileSize'] : 0;
                $mimeType = isset($fileInfo['mimeType']) ? (string) $fileInfo['mimeType'] : 'application/octet-stream';

                if ($relativePath === '' || $fileName === '' || $fileSize <= 0) {
                    throw new \Exception('Invalid file payload for batch item ' . $index . '.');
                }

                $isDicom = null;
                if (array_key_exists('isDicom', $fileInfo)) {
                    $rawIsDicom = $fileInfo['isDicom'];
                    if (is_bool($rawIsDicom)) {
                        $isDicom = $rawIsDicom;
                    } else {
                        $normalized = strtolower(trim((string) $rawIsDicom));
                        $isDicom = in_array($normalized, ['1', 'true', 'yes', 'on'], true);
                    }
                }

                $uploads[] = $this->buildUploadPlanForRelativePath(
                    $settings,
                    $fieldFolder,
                    $relativePath,
                    $fileName,
                    $fileSize,
                    $mimeType,
                    $dagName,
                    $recordId,
                    $repeatInstance,
                    $isDicom,
                    $preserveUploadFolderArchitecture,
                    $dicomFlatItemNameBySourceFolder,
                    $index + 1
                );
            }

            $this->debugAjax('init-batch completed', [
                'fieldName' => $fieldName,
                'uploadCount' => count($uploads),
                'parentFolderId' => (string) $fieldFolder['_id'],
            ]);

            return [
                'ok' => true,
                'batch' => [
                    'parentFolderId' => (string) $fieldFolder['_id'],
                    'dagName' => $dagName,
                    'recordId' => $recordId,
                    'instanceId' => $repeatInstance > 0 ? (string) $repeatInstance : '1',
                    'girderUrl' => (string) $settings['girderUrl'],
                    'girderFrontchannelBaseUrl' => (string) $settings['girderFrontchannelBaseUrl'],
                    'rootCollectionId' => (string) $settings['rootCollectionId'],
                    'uploads' => $uploads,
                ],
            ];
        } catch (\Throwable $exception) {
            $this->debugAjax('init-batch failed', [
                'fieldName' => $fieldName,
                'error' => $exception->getMessage(),
            ]);
            return [
                'ok' => false,
                'error' => $exception->getMessage(),
            ];
        }
    }

    private function handleUploadFile($payload, $projectId, $instrument)
    {
        $data = is_array($payload) ? $payload : [];
        $fieldName = isset($data['fieldName']) ? (string) $data['fieldName'] : '';
        $uploadId = isset($data['uploadId']) ? (string) $data['uploadId'] : '';
        $offset = isset($data['offset']) ? (int) $data['offset'] : 0;
        $fileBase64 = isset($data['fileBase64']) ? (string) $data['fileBase64'] : '';

        $this->debugAjax('upload-file payload parsed', [
            'fieldName' => $fieldName,
            'uploadId' => $uploadId,
            'offset' => $offset,
            'fileBase64Length' => strlen($fileBase64),
            'projectId' => (int) $projectId,
            'instrument' => (string) $instrument,
        ]);

        if ($fieldName === '' || $uploadId === '' || $offset < 0 || $fileBase64 === '') {
            $this->debugAjax('upload-file rejected: invalid payload');
            return [
                'ok' => false,
                'error' => 'Invalid file chunk upload payload.',
            ];
        }

        $allowedFields = $this->getGirderUploadFields($projectId, $instrument);
        if (!in_array($fieldName, $allowedFields, true)) {
            $this->debugAjax('upload-file rejected: unauthorized field', [
                'fieldName' => $fieldName,
            ]);
            return [
                'ok' => false,
                'error' => 'Field is not allowed for Girder upload.',
            ];
        }

        $binaryFile = base64_decode($fileBase64, true);
        if ($binaryFile === false) {
            $this->debugAjax('upload-file rejected: base64 decode failed', [
                'uploadId' => $uploadId,
            ]);
            return [
                'ok' => false,
                'error' => 'File decoding failed.',
            ];
        }

        $this->debugAjax('upload-file decoded', [
            'uploadId' => $uploadId,
            'offset' => $offset,
            'fileBytes' => strlen($binaryFile),
        ]);

        $settings = $this->loadSettings();
        if (empty($settings['girderUrl']) || empty($settings['girderFrontchannelBaseUrl']) || empty($settings['apiKey']) || empty($settings['rootCollectionId'])) {
            $this->debugAjax('upload-file rejected: incomplete settings');
            return [
                'ok' => false,
                'error' => 'Girder Uploader module settings are incomplete.',
            ];
        }

        try {
            $fileEntity = $this->uploadChunk($settings, $uploadId, $offset, $binaryFile);
            $this->debugAjax('upload-file completed', [
                'uploadId' => $uploadId,
                'offset' => $offset,
                'fileId' => isset($fileEntity['_id']) ? (string) $fileEntity['_id'] : null,
            ]);
            return [
                'ok' => true,
                'file' => $fileEntity,
            ];
        } catch (\Throwable $exception) {
            $this->debugAjax('upload-file failed', [
                'uploadId' => $uploadId,
                'error' => $exception->getMessage(),
            ]);
            return [
                'ok' => false,
                'error' => $exception->getMessage(),
            ];
        }
    }

    private function handleInitFileUpload($payload, $projectId, $instrument)
    {
        $data = is_array($payload) ? $payload : [];
        $fieldName = isset($data['fieldName']) ? (string) $data['fieldName'] : '';
        $plan = isset($data['uploadPlan']) && is_array($data['uploadPlan']) ? $data['uploadPlan'] : [];

        $this->debugAjax('init-file-upload payload parsed', [
            'fieldName' => $fieldName,
            'targetFolderId' => isset($plan['folderId']) ? (string) $plan['folderId'] : null,
            'storedFileName' => isset($plan['storedFileName']) ? (string) $plan['storedFileName'] : null,
        ]);

        if ($fieldName === '' || empty($plan)) {
            return [
                'ok' => false,
                'error' => 'Invalid file upload initialization payload.',
            ];
        }

        $allowedFields = $this->getGirderUploadFields($projectId, $instrument);
        if (!in_array($fieldName, $allowedFields, true)) {
            return [
                'ok' => false,
                'error' => 'Field is not allowed for Girder upload.',
            ];
        }

        $settings = $this->loadSettings();
        if (empty($settings['girderUrl']) || empty($settings['girderFrontchannelBaseUrl']) || empty($settings['apiKey']) || empty($settings['rootCollectionId'])) {
            return [
                'ok' => false,
                'error' => 'Girder Uploader module settings are incomplete.',
            ];
        }

        try {
            $uploadPlan = $this->initializeUploadFromPlan($settings, $plan);
            $this->debugAjax('init-file-upload completed', [
                'fieldName' => $fieldName,
                'uploadId' => isset($uploadPlan['uploadId']) ? (string) $uploadPlan['uploadId'] : null,
                'itemId' => isset($uploadPlan['itemId']) ? (string) $uploadPlan['itemId'] : null,
            ]);

            return [
                'ok' => true,
                'upload' => $uploadPlan,
            ];
        } catch (\Throwable $exception) {
            $this->debugAjax('init-file-upload failed', [
                'fieldName' => $fieldName,
                'error' => $exception->getMessage(),
            ]);
            return [
                'ok' => false,
                'error' => $exception->getMessage(),
            ];
        }
    }

    private function handleRefreshUploadMetadata($payload, $projectId, $instrument, $record = '')
    {
        $data = is_array($payload) ? $payload : [];
        $fieldName = isset($data['fieldName']) ? (string) $data['fieldName'] : '';
        $folderId = isset($data['folderId']) ? trim((string) $data['folderId']) : '';
        $uploadedAt = isset($data['uploadedAt']) ? trim((string) $data['uploadedAt']) : '';

        if ($fieldName === '' || $folderId === '') {
            return [
                'ok' => false,
                'error' => 'Invalid metadata refresh payload.',
            ];
        }

        $allowedFields = $this->getGirderUploadFields($projectId, $instrument);
        if (!in_array($fieldName, $allowedFields, true)) {
            return [
                'ok' => false,
                'error' => 'Field is not allowed for Girder upload.',
            ];
        }

        $settings = $this->loadSettings();
        if (empty($settings['girderUrl']) || empty($settings['girderFrontchannelBaseUrl']) || empty($settings['apiKey']) || empty($settings['rootCollectionId'])) {
            return [
                'ok' => false,
                'error' => 'Girder Uploader module settings are incomplete.',
            ];
        }

        try {
            $location = $this->locateUpload($settings, $folderId, $record, $fieldName);
            if ($location === null) {
                return [
                    'ok' => true,
                    'missing' => true,
                ];
            }
            if ($location === false) {
                return $this->uploadOutOfScope($record, $fieldName);
            }

            $metadata = $this->buildMetadataSnapshotFromFolder($settings, $folderId, $uploadedAt);
            if ($metadata === null) {
                return [
                    'ok' => true,
                    'missing' => true,
                ];
            }

            return [
                'ok' => true,
                'missing' => false,
                'metadata' => $metadata,
            ];
        } catch (\Throwable $exception) {
            if ($this->isGirderNotFoundException($exception)) {
                return [
                    'ok' => true,
                    'missing' => true,
                ];
            }
            return [
                'ok' => false,
                'error' => $exception->getMessage(),
            ];
        }
    }

    private function handleDeleteUploadContents($payload, $projectId, $instrument, $record = '')
    {
        $data = is_array($payload) ? $payload : [];
        $fieldName = isset($data['fieldName']) ? (string) $data['fieldName'] : '';
        $folderId = isset($data['folderId']) ? trim((string) $data['folderId']) : '';

        if ($fieldName === '' || $folderId === '') {
            return [
                'ok' => false,
                'error' => 'Invalid delete payload.',
            ];
        }

        $allowedFields = $this->getGirderUploadFields($projectId, $instrument);
        if (!in_array($fieldName, $allowedFields, true)) {
            return [
                'ok' => false,
                'error' => 'Field is not allowed for Girder upload.',
            ];
        }

        $settings = $this->loadSettings();
        if (empty($settings['girderUrl']) || empty($settings['girderFrontchannelBaseUrl']) || empty($settings['apiKey']) || empty($settings['rootCollectionId'])) {
            return [
                'ok' => false,
                'error' => 'Girder Uploader module settings are incomplete.',
            ];
        }

        try {
            $recordFolderId = $this->locateUpload($settings, $folderId, $record, $fieldName);
            if ($recordFolderId === false) {
                return $this->uploadOutOfScope($record, $fieldName);
            }
            if ($recordFolderId === null) {
                // Already gone: deletion stays idempotent, the fingerprint stays.
                return ['ok' => true, 'deleted' => false, 'fingerprintCleared' => false];
            }
            $deleted = $this->deleteFolderTree($settings, $folderId);
            $fingerprintCleared = $this->clearFingerprintIfRecordIsEmpty($settings, (string) $record, $recordFolderId, $folderId);
            return [
                'ok' => true,
                'deleted' => $deleted,
                'fingerprintCleared' => $fingerprintCleared,
            ];
        } catch (\Throwable $exception) {
            return [
                'ok' => false,
                'error' => $exception->getMessage(),
            ];
        }
    }

    /**
     * Evaluate the browser's blinded element with the module's key, with a proof
     * that this key was used. The NIP is never seen here: only a uniformly
     * random group element. See docs/design/pseudonymisation.md.
     */
    private function handlePseudonymEvaluate($payload, $projectId, $instrument, $record)
    {
        $data = is_array($payload) ? $payload : [];
        $fieldName = isset($data['fieldName']) ? (string) $data['fieldName'] : '';
        $blinded = isset($data['blindedElement']) ? (string) $data['blindedElement'] : '';

        if (!in_array($fieldName, $this->getGirderUploadFields($projectId, $instrument), true)) {
            return ['ok' => false, 'error' => 'Field is not allowed for Girder upload.'];
        }
        if (!Voprf::isAvailable()) {
            return [
                'ok' => false,
                'error' => "This server's PHP cannot compute pseudonyms: it needs the sodium extension with ristretto255 (PHP 8.1 or later, libsodium 1.0.18 or later).",
            ];
        }
        $key = $this->ensurePseudonymKey();
        if ($key === '') {
            return [
                'ok' => false,
                'error' => 'The REDCap pseudonym key is not set: an administrator must save the module settings in the Control Center.',
            ];
        }

        $limit = $this->pseudonymDailyLimit();
        if ($this->countRecentPseudonymEvaluations() >= $limit) {
            $this->log('[PSEUDONYM] daily limit reached', ['limit' => $limit]);
            return [
                'ok' => false,
                'code' => 'rate-limit',
                'error' => "The project's pseudonym limit ({$limit} per 24 hours) is reached. Try again later or ask an administrator.",
            ];
        }

        try {
            $projectKey = Voprf::deriveProjectKey($key, $this->pseudonymDomain($projectId));
            $evaluation = Voprf::blindEvaluate($projectKey, $blinded);
        } catch (\InvalidArgumentException $error) {
            return ['ok' => false, 'error' => 'Invalid pseudonym request: ' . $error->getMessage()];
        }

        $this->log(self::PSEUDONYM_EVALUATION_LOG, ['record' => (string) $record]);

        return array_merge(['ok' => true], $evaluation);
    }

    /**
     * Before the uploader confirms: compare the upload's fingerprint with the
     * one stored for the record, without storing anything, and say how many
     * uploads the record already holds. `new` means no fingerprint yet, which
     * with earlier uploads means they predate the fingerprints. `mismatch` is
     * shown to the uploader, who decides.
     *
     * Girder is checked here too: past this point the fingerprint gets stored,
     * and an upload that then could not start would leave it behind with no
     * content to delete it through.
     */
    private function handlePseudonymCheck($payload, $projectId, $instrument, $record, $groupId = null)
    {
        $fingerprint = $this->validatedFingerprint($payload, $projectId, $instrument);
        if (is_array($fingerprint)) {
            return $fingerprint;
        }
        try {
            $settings = $this->loadSettings();
            $this->assertGirderReachable($settings);
            $existingUploads = $this->countRecordUploads($settings, $groupId, $record);
        } catch (\Throwable $error) {
            return ['ok' => false, 'error' => 'Girder is unavailable: ' . $error->getMessage()];
        }

        $status = 'unchecked';
        if ($this->isFingerprintCheckEnabled()) {
            $stored = strtolower(trim((string) $this->getProjectSetting($this->fingerprintSettingKey($record))));
            if ($stored === '') {
                $status = 'new';
            } elseif (hash_equals($stored, $fingerprint)) {
                $status = 'matched';
            } else {
                $status = 'mismatch';
                $this->log('[PSEUDONYM] fingerprint mismatch', ['record' => (string) $record]);
            }
        }
        return ['ok' => true, 'status' => $status, 'existingUploads' => $existingUploads];
    }

    /**
     * Once the uploader has confirmed: store the fingerprint on the record's
     * first upload — or check it again, another upload may have stored one in
     * the meantime — and hand back the ticket `init-batch` requires.
     *
     * A mismatch goes through only with `confirmedMismatch`, the uploader having
     * seen it. The record keeps its fingerprint and the override leaves a trace.
     * One that appears only now was never shown, so it is refused.
     */
    private function handlePseudonymRegister($payload, $projectId, $instrument, $record)
    {
        $fingerprint = $this->validatedFingerprint($payload, $projectId, $instrument);
        if (is_array($fingerprint)) {
            return $fingerprint;
        }
        if (!$this->isFingerprintCheckEnabled()) {
            return ['ok' => true, 'status' => 'unchecked'];
        }
        if ($this->ensurePseudonymKey() === '') {
            return ['ok' => false, 'error' => 'The REDCap pseudonym key is not set.'];
        }
        $confirmedMismatch = isset($payload['confirmedMismatch']) && $payload['confirmedMismatch'] === true;
        $fieldName = (string) $payload['fieldName'];

        return $this->withRecordLock($projectId, $record, function () use ($projectId, $record, $fingerprint, $confirmedMismatch, $fieldName) {
            $settingKey = $this->fingerprintSettingKey($record);
            $stored = strtolower(trim((string) $this->getProjectSetting($settingKey)));
            $overridden = false;
            if ($stored === '') {
                $this->setProjectSetting($settingKey, $fingerprint);
                $status = 'registered';
            } elseif (hash_equals($stored, $fingerprint)) {
                $status = 'matched';
            } elseif ($confirmedMismatch) {
                $overridden = true;
                $status = 'overridden';
                $this->recordMismatchOverride($record, $fieldName);
            } else {
                $this->log('[PSEUDONYM] fingerprint mismatch', ['record' => (string) $record]);
                return $this->fingerprintMismatch();
            }

            return [
                'ok' => true,
                'status' => $status,
                'ticket' => $this->issuePseudonymTicket($projectId, $record, $fingerprint, $overridden),
            ];
        });
    }

    /** The alert of an override: the module's log, and the project's for its users. */
    private function recordMismatchOverride($record, $fieldName)
    {
        $this->log(self::PSEUDONYM_OVERRIDE_LOG, ['record' => (string) $record, 'field' => $fieldName]);
        $this->logProjectEvent(
            'Girder Uploader: patient signature mismatch confirmed',
            "Upload to field {$fieldName} confirmed although its patient signature differs from the record's earlier uploads.",
            $record
        );
    }

    /** An entry of the project's Logging page. */
    protected function logProjectEvent($description, $changes, $record)
    {
        \REDCap::logEvent($description, $changes, null, (string) $record);
    }

    /** Throws unless Girder answers for the project's collection. */
    protected function assertGirderReachable($settings)
    {
        $this->girderRequestJson($settings, 'GET', '/collection/' . rawurlencode((string) $settings['rootCollectionId']), null);
    }

    /** The record's earlier uploads. */
    protected function countRecordUploads($settings, $groupId, $record)
    {
        $dagName = $this->sanitizePathPart($this->getDagName($groupId), 'NO_DAG');
        $dagFolder = $this->findChildFolder($settings, 'collection', (string) $settings['rootCollectionId'], $dagName);
        if ($dagFolder === null) {
            return 0;
        }
        $recordName = $this->sanitizePathPart($record, 'UNASSIGNED_RECORD');
        $recordFolder = $this->findChildFolder($settings, 'folder', (string) $dagFolder['_id'], $recordName);
        return $recordFolder === null ? 0 : $this->countUploadsUnder($settings, (string) $recordFolder['_id']);
    }

    /**
     * Field folders holding something under a record folder, leaving one out:
     * under an event, or directly under the record for uploads made before 2.0.0.
     */
    private function countUploadsUnder($settings, $recordFolderId, $ignoredFolderId = '')
    {
        $fieldFolders = [];
        foreach ($this->listChildFolders($settings, $recordFolderId) as $folder) {
            if ($this->isEventFolderName($folder['name'] ?? '')) {
                $fieldFolders = array_merge($fieldFolders, $this->listChildFolders($settings, (string) $folder['_id']));
            } else {
                $fieldFolders[] = $folder;
            }
        }

        $count = 0;
        foreach ($fieldFolders as $folder) {
            if ((string) $folder['_id'] === (string) $ignoredFolderId) {
                continue;
            }
            if ($this->folderHoldsSomething($settings, (string) $folder['_id'])) {
                $count += 1;
            }
        }
        return $count;
    }

    protected function folderHoldsSomething($settings, $folderId)
    {
        $details = $this->girderRequestJson($settings, 'GET', '/folder/' . rawurlencode((string) $folderId) . '/details', null);
        return (int) ($details['nItems'] ?? 0) + (int) ($details['nFolders'] ?? 0) > 0;
    }

    /** `{event_id} ({event_name})`, the level between a record and its field folders. */
    private function eventFolderName($eventId)
    {
        return $this->sanitizePathPart((int) $eventId . ' (' . $this->eventName($eventId) . ')', (string) (int) $eventId);
    }

    /** Field names start with a letter, so a field folder cannot look like this. */
    private function isEventFolderName($name)
    {
        return preg_match('/^\d+ \(.*\)$/', (string) $name) === 1;
    }

    /** Read from the table: REDCap's own getter answers only in longitudinal projects. */
    protected function eventName($eventId)
    {
        $row = $this->query('SELECT descrip FROM redcap_events_metadata WHERE event_id = ?', [(int) $eventId])->fetch_assoc();
        return is_array($row) && isset($row['descrip']) ? (string) $row['descrip'] : '';
    }

    /**
     * Check a folder id sent by the browser before acting on it with the
     * project's Girder key: it must be this field's folder, on this record, in
     * this project's collection.
     *
     * Returns the record folder's id, null when the folder no longer exists, and
     * false when it is anything else. The path is read by position (collection,
     * DAG, record, then the event): a record may well be called `123`.
     */
    private function locateUpload($settings, $folderId, $record, $fieldName)
    {
        try {
            [$folder, $path] = $this->folderWithPath($settings, $folderId);
        } catch (\Throwable $error) {
            if ($this->isGirderNotFoundException($error)
                || stripos($error->getMessage(), 'invalid folder id') !== false) {
                return null;
            }
            throw $error;
        }

        $depth = count($path);
        $underEvent = $depth === 4 && $this->isEventFolderName($path[3]['object']['name'] ?? '');
        $inPlace = ($depth === 3 || $underEvent)
            && (string) ($path[0]['object']['_id'] ?? '') === (string) $settings['rootCollectionId']
            && (string) ($path[2]['object']['name'] ?? '') === $this->sanitizePathPart($record, 'UNASSIGNED_RECORD')
            && $this->isFieldFolderName($folder['name'] ?? '', $fieldName);

        return $inPlace ? (string) $path[2]['object']['_id'] : false;
    }

    /** A folder and its ancestors, from the collection down. Throws when Girder has no such folder. */
    protected function folderWithPath($settings, $folderId)
    {
        $id = rawurlencode((string) $folderId);
        return [
            $this->girderRequestJson($settings, 'GET', '/folder/' . $id, null),
            $this->girderRequestJson($settings, 'GET', '/folder/' . $id . '/rootpath', null),
        ];
    }

    /** `{field}`, or `{field} -{instance}` for a repeating instance. */
    private function isFieldFolderName($name, $fieldName)
    {
        $base = $this->sanitizePathPart($fieldName, 'upload');
        return $name === $base || strpos((string) $name, $base . ' -') === 0;
    }

    private function uploadOutOfScope($record, $fieldName)
    {
        $this->log('[GIRDER] folder outside the field refused', ['record' => (string) $record, 'field' => (string) $fieldName]);
        return ['ok' => false, 'error' => 'This Girder folder does not belong to this field of this record.'];
    }

    /** The payload's fingerprint, or the error to answer with. */
    private function validatedFingerprint($payload, $projectId, $instrument)
    {
        $data = is_array($payload) ? $payload : [];
        $fieldName = isset($data['fieldName']) ? (string) $data['fieldName'] : '';
        $fingerprint = isset($data['fingerprint']) ? strtolower(trim((string) $data['fingerprint'])) : '';

        if (!in_array($fieldName, $this->getGirderUploadFields($projectId, $instrument), true)) {
            return ['ok' => false, 'error' => 'Field is not allowed for Girder upload.'];
        }
        if (!preg_match('/^[0-9a-f]{2}$/', $fingerprint)) {
            return ['ok' => false, 'error' => 'Invalid patient fingerprint.'];
        }
        return $fingerprint;
    }

    private function fingerprintMismatch()
    {
        return [
            'ok' => false,
            'code' => 'fingerprint-mismatch',
            'error' => 'Another upload has just stored a different patient for this record. Nothing was uploaded: start again to review the difference.',
        ];
    }

    /** `redcap:<pid>` unless the project says otherwise; persisted once known. */
    private function pseudonymDomain($projectId)
    {
        $domain = trim((string) $this->getProjectSetting(self::PSEUDONYM_DOMAIN_SETTING, $projectId));
        if ($domain !== '') {
            return $domain;
        }
        $domain = 'redcap:' . (int) $projectId;
        try {
            $this->setProjectSetting(self::PSEUDONYM_DOMAIN_SETTING, $domain, $projectId);
        } catch (\Throwable $error) {
            // Derived from the project id, so it is the same whether stored or not.
        }
        return $domain;
    }

    private function isFingerprintCheckEnabled()
    {
        return $this->readBoolSettingTreatEmptyAsFalse(self::PSEUDONYM_CHECK_SETTING, true);
    }

    /**
     * The OPRF key, created on first use. An administrator creates it
     * by enabling the module or saving its system settings; the lazy creation
     * covers installations enabled earlier, and fails closed.
     */
    private function ensurePseudonymKey()
    {
        $key = strtolower(trim((string) $this->getSystemSetting(self::PSEUDONYM_KEY_SETTING)));
        if ($key !== '') {
            return Voprf::isValidKey($key) ? $key : '';
        }
        try {
            $this->setSystemSetting(self::PSEUDONYM_KEY_SETTING, Voprf::generateKey());
        } catch (\Throwable $error) {
            $this->log('[PSEUDONYM] could not create the REDCap key: ' . $error->getMessage());
            return '';
        }
        $key = strtolower(trim((string) $this->getSystemSetting(self::PSEUDONYM_KEY_SETTING)));
        return Voprf::isValidKey($key) ? $key : '';
    }

    private function pseudonymDailyLimit()
    {
        $value = trim((string) $this->getSystemSetting(self::PSEUDONYM_LIMIT_SETTING));
        return ctype_digit($value) && (int) $value > 0 ? (int) $value : self::PSEUDONYM_DEFAULT_DAILY_LIMIT;
    }

    /** Evaluations logged for this project over the last 24 hours. */
    protected function countRecentPseudonymEvaluations()
    {
        $result = $this->queryLogs(
            'select count(1) as evaluations where message = ? and timestamp > ?',
            [self::PSEUDONYM_EVALUATION_LOG, date('Y-m-d H:i:s', time() - 86400)]
        );
        $row = $result ? $result->fetch_assoc() : null;
        return $row ? (int) $row['evaluations'] : 0;
    }

    private function fingerprintSettingKey($record)
    {
        // Setting keys are bounded in length; record ids are not.
        $record = (string) $record;
        return self::PSEUDONYM_FINGERPRINT_PREFIX . (strlen($record) <= 64 ? $record : sha1($record));
    }

    /**
     * Run `$body` holding a database lock on the record, so that two first
     * uploads cannot both store a fingerprint.
     */
    private function withRecordLock($projectId, $record, callable $body)
    {
        $name = 'girder_uploader_fp_' . sha1((int) $projectId . '|' . (string) $record);
        if (!$this->acquireDatabaseLock($name)) {
            return [
                'ok' => false,
                'error' => 'Another upload is being checked for this record. Try again in a moment.',
            ];
        }
        try {
            return $body();
        } finally {
            $this->releaseDatabaseLock($name);
        }
    }

    protected function acquireDatabaseLock($name)
    {
        $result = $this->query('SELECT GET_LOCK(?, 10) AS acquired', [(string) $name]);
        $row = $result ? $result->fetch_assoc() : null;
        return $row && (int) $row['acquired'] === 1;
    }

    protected function releaseDatabaseLock($name)
    {
        $this->query('SELECT RELEASE_LOCK(?) AS released', [(string) $name]);
    }

    private function pseudonymTicketKey()
    {
        return hash_hmac('sha256', 'girder-uploader-pseudonym-ticket', $this->ensurePseudonymKey(), true);
    }

    private function pseudonymTicketMac($projectId, $record, $fingerprint, $expires, $mode)
    {
        return hash_hmac('sha256', implode('|', [(int) $projectId, (string) $record, $fingerprint, (int) $expires, $mode]), $this->pseudonymTicketKey());
    }

    private function issuePseudonymTicket($projectId, $record, $fingerprint, $overridden = false)
    {
        $expires = time() + self::PSEUDONYM_TICKET_TTL_SECONDS;
        $mode = $overridden ? self::PSEUDONYM_TICKET_OVERRIDE : self::PSEUDONYM_TICKET_MATCH;
        return implode('.', [$expires, $fingerprint, $mode, $this->pseudonymTicketMac($projectId, $record, $fingerprint, $expires, $mode)]);
    }

    /**
     * A ticket is valid for its record, within its hour, and only while the
     * record has a fingerprint: its own, unless the uploader overrode a mismatch.
     */
    private function verifyPseudonymTicket($projectId, $record, $ticket)
    {
        $parts = explode('.', (string) $ticket);
        if (count($parts) !== 4 || !ctype_digit($parts[0]) || (int) $parts[0] < time()) {
            return false;
        }
        [$expires, $fingerprint, $mode, $mac] = $parts;
        if ($this->ensurePseudonymKey() === '') {
            return false;
        }
        if (!hash_equals($this->pseudonymTicketMac($projectId, $record, $fingerprint, $expires, $mode), $mac)) {
            return false;
        }
        $stored = strtolower(trim((string) $this->getProjectSetting($this->fingerprintSettingKey($record))));
        if ($stored === '') {
            return false;
        }
        return $mode === self::PSEUDONYM_TICKET_OVERRIDE || hash_equals($stored, $fingerprint);
    }

    /**
     * A wrongly registered fingerprint must be correctable: once nothing is left
     * under the record in Girder, its fingerprint goes too.
     */
    private function clearFingerprintIfRecordIsEmpty($settings, $record, $recordFolderId, $deletedFolderId)
    {
        if ($record === '' || $recordFolderId === '') {
            return false;
        }
        // Girder may still be deleting the folder: it does not count, whatever it holds.
        if ($this->countUploadsUnder($settings, $recordFolderId, $deletedFolderId) > 0
            || !empty($this->listItemsInFolder($settings, $recordFolderId))) {
            return false;
        }
        $this->removeProjectSetting($this->fingerprintSettingKey($record));
        $this->log('[PSEUDONYM] fingerprint cleared with the record contents', ['record' => (string) $record]);
        return true;
    }

    private function loadSettings()
    {
        $backchannelApiUrl = $this->normalizeApiUrl((string) $this->getProjectSetting('girder-backchannel-api-url'));
        $frontchannelBaseUrl = $this->normalizeBaseUrl((string) $this->getProjectSetting('girder-frontchannel-base-url'));

        // Backward compatibility fallback for older setting keys.
        if ($backchannelApiUrl === '') {
            $backchannelApiUrl = $this->normalizeApiUrl((string) $this->getProjectSetting('girder-url'));
        }
        if ($frontchannelBaseUrl === '') {
            $frontchannelBaseUrl = $this->normalizeBaseUrl((string) $this->getProjectSetting('girder-base-url'));
        }
        if ($frontchannelBaseUrl === '' && $backchannelApiUrl !== '') {
            $frontchannelBaseUrl = $this->deriveBaseUrlFromLegacyApiUrl($backchannelApiUrl);
        }

        $settings = [
            'girderUrl' => $backchannelApiUrl,
            'girderFrontchannelBaseUrl' => $frontchannelBaseUrl,
            'apiKey' => (string) $this->getProjectSetting('api-key'),
            'rootCollectionId' => (string) $this->getProjectSetting('root-collection-id'),
            'chunkSize' => $this->readIntSetting('chunk-size', 10485760),
            'maxRetries' => $this->readIntSetting('max-retries', 3),
            'retryDelay' => $this->readIntSetting('retry-delay', 1000),
            'deidentifyDicom' => $this->readBoolSettingTreatEmptyAsFalse('deidentify-dicom', true),
            'deidentifyXmlEcg' => $this->readBoolSettingTreatEmptyAsFalse('deidentify-xml-ecg', true),
            'deidentifySchillerHolter' => $this->readBoolSettingTreatEmptyAsFalse('deidentify-schiller-holter', true),
            'preserveUploadFolderArchitecture' => $this->readBoolSettingTreatEmptyAsFalse('preserve-upload-folder-architecture', false),
        ];
        return $settings;
    }

    protected function getGirderUploadFields($projectId, $instrument)
    {
        if (!class_exists('REDCap')) {
            return [];
        }

        $metadata = \REDCap::getDataDictionary($projectId, 'array');
        if (!is_array($metadata)) {
            return [];
        }

        $fields = [];
        foreach ($metadata as $fieldName => $field) {
            if (!is_array($field)) {
                continue;
            }

            $formName = isset($field['form_name']) ? (string) $field['form_name'] : '';
            if ($formName !== $instrument) {
                continue;
            }

            $annotation = isset($field['field_annotation']) ? strtoupper((string) $field['field_annotation']) : '';
            if ($annotation === '' || strpos($annotation, '@GIRDER_UPLOAD') === false) {
                continue;
            }

            $validation = isset($field['text_validation_type_or_show_slider_number'])
                ? strtolower((string) $field['text_validation_type_or_show_slider_number'])
                : '';

            if ($validation !== '' && $validation !== 'none') {
                continue;
            }

            $fields[] = (string) $fieldName;
        }

        return $fields;
    }

    /**
     * REDCap's record-list cache builder for the project. Until the cache is built, REDCap
     * redirects the module's AJAX POSTs to this route and the widget gets HTML instead of
     * JSON; the widget calls it, then retries. PLUGIN makes it exit once the cache is built
     * instead of redirecting back to the referring page.
     */
    private function getRecordListCacheUrl($projectId)
    {
        return APP_PATH_WEBROOT . 'index.php?' . http_build_query([
            'pid' => (int) $projectId,
            'route' => 'DataEntryController:buildRecordListCache',
            'PLUGIN' => 1,
        ]);
    }

    private function getDagName($group_id)
		{
		if (empty($group_id)) {
			return null;
		}

		$group_name = \REDCap::getGroupNames(FALSE, $group_id);
			return $group_name ? $group_name : null;
		}

    /**
     * May the current user upload to, or delete from, this instrument?
     *
     * Anything we cannot establish is a refusal: no instrument, no user, a user
     * who is not on the project, an instrument missing from their rights.
     */
    private function userCanModifyInstrument($instrument)
    {
        if (!is_string($instrument) || trim($instrument) === '') {
            return false;
        }

        return self::formRightAllowsEditing($this->currentUserFormRight($instrument));
    }

    /**
     * The current user's encoded data viewing right on an instrument, or null
     * when they have none.
     *
     * REDCap's own `$user_rights` comes first: it is what REDCap enforces on this
     * request, with roles, super users, impersonation and the read-only
     * downgrade of a project whose data is locked already applied. Without it,
     * the right is read from the project's user rights through the framework.
     */
    private function currentUserFormRight($instrument)
    {
        $globalRights = isset($GLOBALS['user_rights']) ? $GLOBALS['user_rights'] : null;
        if (is_array($globalRights) && isset($globalRights['forms']) && is_array($globalRights['forms'])) {
            return isset($globalRights['forms'][$instrument]) ? $globalRights['forms'][$instrument] : null;
        }

        $username = defined('USERID') ? (string) USERID : '';
        if ($username === '' && isset($GLOBALS['userid'])) {
            $username = (string) $GLOBALS['userid'];
        }
        if ($username === '') {
            return null;
        }

        $project = $this->getProject();
        if (!$project) {
            return null;
        }

        $userRights = $project->getRights($username);
        if (!is_array($userRights)) {
            // Not on the project. REDCap still lets a super user in, with the
            // view & edit + delete right it grants super users on every form.
            return $this->getUser($username)->isSuperUser() ? '146' : null;
        }

        if (isset($userRights['forms']) && is_array($userRights['forms'])) {
            return isset($userRights['forms'][$instrument]) ? $userRights['forms'][$instrument] : null;
        }

        // The raw `data_entry` column, e.g. "[demographics,1][uploads,130]".
        $data = isset($userRights['data_entry']) ? (string) $userRights['data_entry'] : '';
        if (!preg_match_all('/\[([^,\]]+)(?:,([^\]]*))?\]/', $data, $matches, PREG_SET_ORDER)) {
            return null;
        }
        foreach ($matches as $match) {
            if ($match[1] === $instrument) {
                return isset($match[2]) ? $match[2] : '0';
            }
        }

        return null;
    }

    /**
     * Does an encoded data viewing right allow editing the instrument?
     *
     * Mirrors REDCap's UserRights::hasDataViewingRights($value, 'view-edit'),
     * which only exists in recent versions, and keeps both encodings working:
     *
     * - legacy (below 128): 0 no access, 1 view & edit, 2 read-only,
     *   3 view & edit + edit survey responses;
     * - bitmask (REDCap 15): 128 marks it, then 1 read-only, 2 view & edit,
     *   8 edit survey responses, 16 delete records. 130, 138, 146 and 154 all
     *   carry view & edit.
     */
    private static function formRightAllowsEditing($value)
    {
        if (is_int($value)) {
            $value = (string) $value;
        }
        if (!is_string($value) || !ctype_digit(trim($value))) {
            return false;
        }

        $value = (int) trim($value);
        if ($value < 128) {
            return $value === 1 || $value === 3;
        }

        return ($value & 2) === 2;
    }

    /**
     * Is this a record id we can file uploads under?
     *
     * While a record has not been saved, REDCap hands the module a placeholder
     * instead of an id. Uploading then would create a Girder folder that no
     * record ever points at, and — worse — bake that placeholder into the
     * deidentified files, where it cannot be corrected afterwards.
     */
    private function isUsableRecordId($record)
    {
        $record = trim((string) $record);
        if ($record === '') {
            return false;
        }

        return strpos($record, 'external-modules-temporary-record-id') === false;
    }

    private function sanitizePathPart($value, $fallback)
    {
        $text = trim((string) $value);
        if ($text === '') {
            return (string) $fallback;
        }

        $text = preg_replace('/[\\\\\/:*?"<>|]/', '_', $text);
        $text = preg_replace('/\s+/', ' ', (string) $text);
        $text = trim((string) $text);

        return $text !== '' ? $text : (string) $fallback;
    }

    private function splitRelativePath($relativePath)
    {
        $relativePath = str_replace('\\', '/', (string) $relativePath);
        $relativePath = trim($relativePath);
        $relativePath = trim($relativePath, '/');
        if ($relativePath === '') {
            return ['uploaded-file'];
        }

        $rawParts = explode('/', $relativePath);
        $parts = [];
        foreach ($rawParts as $part) {
            $part = $this->sanitizePathPart($part, '');
            if ($part !== '') {
                $parts[] = $part;
            }
        }

        if (empty($parts)) {
            return ['uploaded-file'];
        }

        return $parts;
    }

    private function ensureFolder($settings, $parentType, $parentId, $name)
    {
        $query = http_build_query([
            'parentType' => $parentType,
            'parentId' => $parentId,
            'name' => $name,
            'reuseExisting' => 'true',
        ]);

        return $this->girderRequestJson($settings, 'POST', '/folder?' . $query, null, [
            'Content-Type: text/plain',
        ]);
    }

    private function getFolder($settings, $folderId)
    {
        return $this->girderRequestJsonOrNull($settings, 'GET', '/folder/' . rawurlencode((string) $folderId), null, [], [404]);
    }

    /** The folder of that name under its parent, or null: unlike ensureFolder, creates nothing. */
    private function findChildFolder($settings, $parentType, $parentId, $name)
    {
        $query = http_build_query([
            'parentType' => $parentType,
            'parentId' => $parentId,
            'name' => $name,
            'limit' => 1,
        ]);

        $folders = $this->girderRequestJson($settings, 'GET', '/folder?' . $query, null);
        return is_array($folders) && !empty($folders) ? $folders[0] : null;
    }

    protected function listChildFolders($settings, $folderId)
    {
        $query = http_build_query([
            'parentType' => 'folder',
            'parentId' => (string) $folderId,
            'limit' => 0,
        ]);

        $folders = $this->girderRequestJson($settings, 'GET', '/folder?' . $query, null);
        return is_array($folders) ? array_values($folders) : [];
    }

    private function ensureItem($settings, $folderId, $name)
    {
        $query = http_build_query([
            'folderId' => $folderId,
            'name' => $name,
            'reuseExisting' => 'true',
        ]);

        return $this->girderRequestJson($settings, 'POST', '/item?' . $query, null, [
            'Content-Type: text/plain',
        ]);
    }

    private function listItemsInFolder($settings, $folderId)
    {
        $query = http_build_query([
            'folderId' => (string) $folderId,
            'limit' => 0,
        ]);

        $items = $this->girderRequestJson($settings, 'GET', '/item?' . $query, null);
        return is_array($items) ? array_values($items) : [];
    }

    private function listFilesInItem($settings, $itemId)
    {
        $query = http_build_query([
            'limit' => 0,
        ]);

        $files = $this->girderRequestJson($settings, 'GET', '/item/' . rawurlencode((string) $itemId) . '/files?' . $query, null);
        return is_array($files) ? array_values($files) : [];
    }

    private function initUpload($settings, $itemId, $fileName, $fileSize, $mimeType)
    {
        $query = http_build_query([
            'parentType' => 'item',
            'parentId' => $itemId,
            'name' => $fileName,
            'size' => (int) $fileSize,
            'mimeType' => $mimeType !== '' ? $mimeType : 'application/octet-stream',
        ]);

        return $this->girderRequestJson($settings, 'POST', '/file?' . $query, null, [
            'Content-Type: text/plain',
        ]);
    }

    private function uploadChunk($settings, $uploadId, $offset, $binaryChunk)
    {
        $query = http_build_query([
            'uploadId' => $uploadId,
            'offset' => (int) $offset,
        ]);

        return $this->girderRequestJson($settings, 'POST', '/file/chunk?' . $query, $binaryChunk, [
            'Content-Type: application/octet-stream',
        ]);
    }

    private function uploadBinaryFileInChunks($settings, $uploadId, $binaryFile)
    {
        $chunkSize = isset($settings['chunkSize']) ? (int) $settings['chunkSize'] : 10485760;
        if ($chunkSize <= 0) {
            $chunkSize = 10485760;
        }

        $offset = 0;
        $fileLength = strlen($binaryFile);
        $lastFileEntity = null;

        while ($offset < $fileLength) {
            $chunk = substr($binaryFile, $offset, $chunkSize);
            if ($chunk === '' || $chunk === false) {
                break;
            }

            $lastFileEntity = $this->uploadChunk($settings, $uploadId, $offset, $chunk);
            $offset += strlen($chunk);
        }

        if (!is_array($lastFileEntity)) {
            throw new \Exception('No file response was returned by Girder after chunked upload.');
        }

        return $lastFileEntity;
    }

    private function buildUploadPlanForRelativePath(
        $settings,
        $fieldFolder,
        $relativePath,
        $fileName,
        $fileSize,
        $mimeType,
        $dagName,
        $recordId,
        $repeatInstance,
        $isDicomHint = null,
        $preserveUploadFolderArchitecture = true,
        &$dicomFlatItemNameBySourceFolder = [],
        $uploadIndex = 1
    )
    {
        $pathParts = $this->splitRelativePath($relativePath);
        $leafFileName = $this->sanitizePathPart((string) end($pathParts), $fileName);
        $subfolders = array_slice($pathParts, 0, max(0, count($pathParts) - 1));
        $sourceFolderKey = empty($subfolders) ? '.' : implode('/', $subfolders);
        $isDicom = is_bool($isDicomHint) ? $isDicomHint : $this->isDicomFile($leafFileName, $mimeType);
        $storedFileName = $preserveUploadFolderArchitecture
            ? $leafFileName
            : $this->buildNeutralFileName($uploadIndex, $leafFileName, $mimeType, $isDicom);

        $targetFolder = $fieldFolder;
        if ($preserveUploadFolderArchitecture) {
            foreach ($subfolders as $segment) {
                $targetFolder = $this->ensureFolder($settings, 'folder', (string) $targetFolder['_id'], $segment);
            }
        }

        $targetFolderId = (string) $targetFolder['_id'];
        $itemName = $storedFileName;

        if ($isDicom) {
            if (!is_array($dicomFlatItemNameBySourceFolder)) {
                $dicomFlatItemNameBySourceFolder = [];
            }

            if ($preserveUploadFolderArchitecture) {
                $itemName = 'DICOM_DATA';
            } else {
                $flatTargetFolderId = (string) $fieldFolder['_id'];
                if (!isset($dicomFlatItemNameBySourceFolder[$sourceFolderKey]) || $dicomFlatItemNameBySourceFolder[$sourceFolderKey] === '') {
                    $nextIndex = count($dicomFlatItemNameBySourceFolder) + 1;
                    $dicomFlatItemNameBySourceFolder[$sourceFolderKey] = 'DICOM_DATA_' . $nextIndex;
                }
                $targetFolderId = $flatTargetFolderId;
                $targetFolder = $fieldFolder;
                $itemName = $dicomFlatItemNameBySourceFolder[$sourceFolderKey];
            }
        }

        $uploadPlan = [
            'folderId' => (string) $targetFolder['_id'],
            'parentFolderId' => (string) $fieldFolder['_id'],
            'dagName' => $dagName,
            'recordId' => $recordId,
            'instanceId' => $repeatInstance > 0 ? (string) $repeatInstance : '1',
            'girderUrl' => (string) $settings['girderUrl'],
            'girderFrontchannelBaseUrl' => (string) $settings['girderFrontchannelBaseUrl'],
            'rootCollectionId' => (string) $settings['rootCollectionId'],
            'storedFileName' => $storedFileName,
            'itemName' => $itemName,
            'fileSize' => (int) $fileSize,
            'mimeType' => $mimeType !== '' ? $mimeType : 'application/octet-stream',
            'isDicom' => $isDicom,
        ];

        if ($preserveUploadFolderArchitecture) {
            $uploadPlan['relativePath'] = (string) $relativePath;
        }

        $uploadPlan['planSignature'] = $this->signUploadPlan($settings, $uploadPlan);
        return $uploadPlan;
    }

    private function initializeUploadFromPlan($settings, $plan)
    {
        if (!is_array($plan)) {
            throw new \Exception('Invalid upload plan.');
        }

        $folderId = isset($plan['folderId']) ? trim((string) $plan['folderId']) : '';
        $parentFolderId = isset($plan['parentFolderId']) ? trim((string) $plan['parentFolderId']) : '';
        $storedFileName = isset($plan['storedFileName']) ? $this->sanitizePathPart((string) $plan['storedFileName'], '') : '';
        $itemName = isset($plan['itemName']) ? $this->sanitizePathPart((string) $plan['itemName'], '') : '';
        $fileSize = isset($plan['fileSize']) ? (int) $plan['fileSize'] : 0;
        $mimeType = isset($plan['mimeType']) ? (string) $plan['mimeType'] : 'application/octet-stream';
        $planSignature = isset($plan['planSignature']) ? (string) $plan['planSignature'] : '';

        if ($folderId === '' || $parentFolderId === '' || $storedFileName === '' || $itemName === '' || $fileSize <= 0) {
            throw new \Exception('Incomplete upload plan.');
        }
        if (!$this->verifyUploadPlanSignature($settings, $plan, $planSignature)) {
            throw new \Exception('Invalid upload plan signature.');
        }

        $item = $this->ensureItem($settings, $folderId, $itemName);
        if (!is_array($item) || empty($item['_id'])) {
            throw new \Exception('Unable to initialize Girder item for upload.');
        }

        $upload = $this->initUpload($settings, (string) $item['_id'], $storedFileName, $fileSize, $mimeType);
        if (!is_array($upload) || empty($upload['_id'])) {
            throw new \Exception('Unable to initialize Girder file upload.');
        }

        $uploadPlan = $plan;
        $uploadPlan['uploadId'] = (string) $upload['_id'];
        $uploadPlan['itemId'] = (string) $item['_id'];
        $uploadPlan['folderId'] = $folderId;
        $uploadPlan['parentFolderId'] = $parentFolderId;
        $uploadPlan['storedFileName'] = $storedFileName;
        $uploadPlan['itemName'] = $itemName;
        $uploadPlan['fileSize'] = $fileSize;
        $uploadPlan['mimeType'] = $mimeType !== '' ? $mimeType : 'application/octet-stream';

        return $uploadPlan;
    }

    private function signUploadPlan($settings, $plan)
    {
        $secret = isset($settings['apiKey']) ? (string) $settings['apiKey'] : '';
        return hash_hmac('sha256', $this->canonicalizeUploadPlanForSignature($plan), $secret);
    }

    private function verifyUploadPlanSignature($settings, $plan, $signature)
    {
        if ($signature === '') {
            return false;
        }

        return hash_equals($this->signUploadPlan($settings, $plan), $signature);
    }

    private function canonicalizeUploadPlanForSignature($plan)
    {
        $payload = [
            'folderId' => isset($plan['folderId']) ? (string) $plan['folderId'] : '',
            'parentFolderId' => isset($plan['parentFolderId']) ? (string) $plan['parentFolderId'] : '',
            'dagName' => isset($plan['dagName']) ? (string) $plan['dagName'] : '',
            'recordId' => isset($plan['recordId']) ? (string) $plan['recordId'] : '',
            'instanceId' => isset($plan['instanceId']) ? (string) $plan['instanceId'] : '',
            'girderUrl' => isset($plan['girderUrl']) ? (string) $plan['girderUrl'] : '',
            'girderFrontchannelBaseUrl' => isset($plan['girderFrontchannelBaseUrl']) ? (string) $plan['girderFrontchannelBaseUrl'] : '',
            'rootCollectionId' => isset($plan['rootCollectionId']) ? (string) $plan['rootCollectionId'] : '',
            'storedFileName' => isset($plan['storedFileName']) ? (string) $plan['storedFileName'] : '',
            'itemName' => isset($plan['itemName']) ? (string) $plan['itemName'] : '',
            'fileSize' => isset($plan['fileSize']) ? (int) $plan['fileSize'] : 0,
            'mimeType' => isset($plan['mimeType']) ? (string) $plan['mimeType'] : '',
            'isDicom' => !empty($plan['isDicom']),
            'relativePath' => isset($plan['relativePath']) ? (string) $plan['relativePath'] : '',
        ];

        return json_encode($payload, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE);
    }

    private function buildNeutralFileName($uploadIndex, $leafFileName, $mimeType, $isDicom)
    {
        $index = max(1, min(9999, (int) $uploadIndex));
        $extension = $this->getSafeFileExtension($leafFileName);

        if ($extension === '' && $isDicom) {
            $extension = 'dcm';
        } elseif ($extension === '' && strtolower(trim((string) $mimeType)) === 'application/xml') {
            $extension = 'xml';
        }

        $name = str_pad((string) $index, 4, '0', STR_PAD_LEFT);
        return $extension !== '' ? ($name . '.' . $extension) : $name;
    }

    private function getSafeFileExtension($fileName)
    {
        $fileName = strtolower(trim((string) $fileName));
        if ($fileName === '' || strpos($fileName, '.') === false) {
            return '';
        }

        $extension = (string) pathinfo($fileName, PATHINFO_EXTENSION);
        return preg_match('/^[a-z0-9]{1,10}$/', $extension) ? $extension : '';
    }

    private function isDicomFile($fileName, $mimeType)
    {
        $fileName = strtolower(trim((string) $fileName));
        $mimeType = strtolower(trim((string) $mimeType));

        if ($mimeType !== '' && (strpos($mimeType, 'dicom') !== false || $mimeType === 'application/dicom')) {
            return true;
        }

        if ($fileName === '') {
            return false;
        }

        if (substr($fileName, -4) === '.dcm') {
            return true;
        }

        return false;
    }

    private function buildMetadataSnapshotFromFolder($settings, $folderId, $uploadedAt = '')
    {
        try {
            $rootFolder = $this->getFolder($settings, $folderId);
            if (!is_array($rootFolder) || empty($rootFolder['_id'])) {
                return null;
            }

            $listingSummary = $this->collectFolderListingSummary($settings, $rootFolder, '', 10);
        } catch (\Throwable $exception) {
            if ($this->isGirderNotFoundException($exception)) {
                return null;
            }
            throw $exception;
        }
        $sampleFiles = isset($listingSummary['sampleFiles']) && is_array($listingSummary['sampleFiles'])
            ? $listingSummary['sampleFiles']
            : [];
        usort($sampleFiles, function ($left, $right) {
            $leftName = isset($left['name']) ? strtolower((string) $left['name']) : '';
            $rightName = isset($right['name']) ? strtolower((string) $right['name']) : '';
            if ($leftName === $rightName) {
                return 0;
            }
            return $leftName < $rightName ? -1 : 1;
        });

        $rootFolderId = (string) $rootFolder['_id'];
        $frontBaseUrl = rtrim((string) $settings['girderFrontchannelBaseUrl'], '/');

        return [
            'version' => 1,
            'encoding' => 'summary-v1',
            'uploadedAt' => $uploadedAt !== '' ? $uploadedAt : (isset($rootFolder['updated']) ? (string) $rootFolder['updated'] : (isset($rootFolder['created']) ? (string) $rootFolder['created'] : null)),
            'uploadedFiles' => $sampleFiles,
            'uploadSummary' => [
                'mode' => 'summary',
                'fileCount' => isset($listingSummary['fileCount']) ? (int) $listingSummary['fileCount'] : 0,
                'totalSizeBytes' => isset($listingSummary['totalSizeBytes']) ? (int) $listingSummary['totalSizeBytes'] : 0,
                'sampleLimit' => 10,
                'sampleFiles' => $sampleFiles,
                'omittedFileCount' => max(0, (isset($listingSummary['fileCount']) ? (int) $listingSummary['fileCount'] : 0) - count($sampleFiles)),
            ],
            'totalSizeBytes' => isset($listingSummary['totalSizeBytes']) ? (int) $listingSummary['totalSizeBytes'] : 0,
            'uploadState' => [
                'status' => 'completed',
                'stage' => 'done',
                'updatedAt' => gmdate('c'),
                'error' => null,
            ],
            'girder' => [
                'baseUrl' => $frontBaseUrl !== '' ? $frontBaseUrl : null,
                'parentFolderId' => $rootFolderId,
                'parentFolderUrl' => $frontBaseUrl !== '' ? ($frontBaseUrl . '/#folder/' . $rootFolderId) : null,
            ],
        ];
    }

    private function collectFolderListingSummary($settings, $folder, $prefix, $sampleLimit = 10)
    {
        $summary = [
            'fileCount' => 0,
            'totalSizeBytes' => 0,
            'sampleFiles' => [],
        ];
        $folderId = isset($folder['_id']) ? (string) $folder['_id'] : '';
        if ($folderId === '') {
            return $summary;
        }

        $items = $this->listItemsInFolder($settings, $folderId);
        foreach ($items as $item) {
            if (!is_array($item) || empty($item['_id'])) {
                continue;
            }

            $itemId = (string) $item['_id'];
            $files = $this->listFilesInItem($settings, $itemId);
            foreach ($files as $file) {
                if (!is_array($file) || empty($file['_id'])) {
                    continue;
                }

                $name = isset($file['name']) ? (string) $file['name'] : 'uploaded-file';
                $relativeName = $prefix !== '' ? ($prefix . '/' . $name) : $name;
                $size = isset($file['size']) ? (int) $file['size'] : 0;
                $summary['fileCount'] += 1;
                $summary['totalSizeBytes'] += $size;

                if (count($summary['sampleFiles']) < $sampleLimit) {
                    $summary['sampleFiles'][] = [
                        'name' => $relativeName,
                        'originalName' => $name,
                        'size' => $size,
                        'mimeType' => isset($file['mimeType']) ? (string) $file['mimeType'] : 'application/octet-stream',
                    ];
                }
            }
        }

        $childFolders = $this->listChildFolders($settings, $folderId);
        foreach ($childFolders as $childFolder) {
            if (!is_array($childFolder) || empty($childFolder['_id'])) {
                continue;
            }

            $childFolderEntity = $this->getFolder($settings, (string) $childFolder['_id']);
            if (!is_array($childFolderEntity) || empty($childFolderEntity['_id'])) {
                continue;
            }

            $childName = isset($childFolderEntity['name']) ? $this->sanitizePathPart((string) $childFolderEntity['name'], 'folder') : 'folder';
            $childPrefix = $prefix !== '' ? ($prefix . '/' . $childName) : $childName;
            $childSummary = $this->collectFolderListingSummary($settings, $childFolderEntity, $childPrefix, $sampleLimit);
            $summary['fileCount'] += isset($childSummary['fileCount']) ? (int) $childSummary['fileCount'] : 0;
            $summary['totalSizeBytes'] += isset($childSummary['totalSizeBytes']) ? (int) $childSummary['totalSizeBytes'] : 0;

            $childSamples = isset($childSummary['sampleFiles']) && is_array($childSummary['sampleFiles'])
                ? $childSummary['sampleFiles']
                : [];
            foreach ($childSamples as $sampleFile) {
                if (count($summary['sampleFiles']) >= $sampleLimit) {
                    break;
                }
                $summary['sampleFiles'][] = $sampleFile;
            }
        }

        return $summary;
    }

    private function girderRequestJson($settings, $method, $pathWithQuery, $body, $extraHeaders = [])
    {
        $token = $this->getGirderAuthToken($settings, false);
        $response = $this->girderRequestWithToken($settings, $method, $pathWithQuery, $body, $extraHeaders, $token);

        if ($this->girderResponseNeedsFreshToken($response)) {
            $this->clearCachedGirderAuthToken($settings);
            $token = $this->getGirderAuthToken($settings, true);
            $response = $this->girderRequestWithToken($settings, $method, $pathWithQuery, $body, $extraHeaders, $token);
        }

        $responseBody = (string) $response['body'];
        $statusCode = (int) $response['statusCode'];

        if ($statusCode < 200 || $statusCode >= 300) {
            throw new \Exception('Girder request failed (' . $statusCode . '): ' . $responseBody);
        }

        $decoded = json_decode($responseBody, true);
        if (!is_array($decoded)) {
            throw new \Exception('Girder returned an invalid JSON response.');
        }

        return $decoded;
    }

    private function girderRequestJsonOrNull($settings, $method, $pathWithQuery, $body, $extraHeaders = [], $nullStatusCodes = [])
    {
        $token = $this->getGirderAuthToken($settings, false);
        $response = $this->girderRequestWithToken($settings, $method, $pathWithQuery, $body, $extraHeaders, $token);

        if ($this->girderResponseNeedsFreshToken($response)) {
            $this->clearCachedGirderAuthToken($settings);
            $token = $this->getGirderAuthToken($settings, true);
            $response = $this->girderRequestWithToken($settings, $method, $pathWithQuery, $body, $extraHeaders, $token);
        }

        $statusCode = (int) $response['statusCode'];
        if (in_array($statusCode, $nullStatusCodes, true)) {
            return null;
        }

        $responseBody = (string) $response['body'];
        if ($statusCode < 200 || $statusCode >= 300) {
            throw new \Exception('Girder request failed (' . $statusCode . '): ' . $responseBody);
        }

        if ($responseBody === '') {
            return [];
        }

        $decoded = json_decode($responseBody, true);
        if (!is_array($decoded)) {
            throw new \Exception('Girder returned an invalid JSON response.');
        }

        return $decoded;
    }

    private function girderRequestNoContent($settings, $method, $pathWithQuery, $body = null, $extraHeaders = [], $allowedStatusCodes = [200, 202, 204])
    {
        $token = $this->getGirderAuthToken($settings, false);
        $response = $this->girderRequestWithToken($settings, $method, $pathWithQuery, $body, $extraHeaders, $token);

        if ($this->girderResponseNeedsFreshToken($response)) {
            $this->clearCachedGirderAuthToken($settings);
            $token = $this->getGirderAuthToken($settings, true);
            $response = $this->girderRequestWithToken($settings, $method, $pathWithQuery, $body, $extraHeaders, $token);
        }

        $statusCode = (int) $response['statusCode'];
        if (in_array($statusCode, $allowedStatusCodes, true)) {
            return true;
        }

        throw new \Exception('Girder request failed (' . $statusCode . '): ' . (string) $response['body']);
    }

    private function deleteFolderTree($settings, $folderId)
    {
        $folderId = trim((string) $folderId);
        if ($folderId === '') {
            throw new \Exception('Folder id is required for deletion.');
        }

        return $this->girderRequestNoContent($settings, 'DELETE', '/folder/' . rawurlencode($folderId), null, [], [200, 202, 204, 404]);
    }

    private function girderRequestWithToken($settings, $method, $pathWithQuery, $body, $extraHeaders, $authToken)
    {
        $headers = array_merge([
            'Girder-Token: ' . (string) $authToken,
            'Authorization: Bearer ' . (string) $authToken,
            'Accept: application/json',
        ], is_array($extraHeaders) ? $extraHeaders : []);

        return $this->executeCurlRequest(
            rtrim((string) $settings['girderUrl'], '/') . (string) $pathWithQuery,
            $method,
            $headers,
            $body
        );
    }

    private function girderResponseNeedsFreshToken($response)
    {
        $statusCode = isset($response['statusCode']) ? (int) $response['statusCode'] : 0;
        if ($statusCode === 401) {
            return true;
        }

        if ($statusCode !== 403) {
            return false;
        }

        $decoded = json_decode(isset($response['body']) ? (string) $response['body'] : '', true);
        if (!is_array($decoded)) {
            return false;
        }

        $type = isset($decoded['type']) ? strtolower((string) $decoded['type']) : '';
        $message = isset($decoded['message']) ? strtolower((string) $decoded['message']) : '';

        return $type === 'access'
            && strpos($message, 'user none') !== false
            && strpos($message, 'access denied') !== false;
    }

    private function getGirderAuthToken($settings, $forceRefresh = false)
    {
        if (!$forceRefresh) {
            $cachedToken = $this->getCachedGirderAuthToken($settings);
            if ($cachedToken !== null && $cachedToken !== '') {
                return $cachedToken;
            }
        }

        $apiKey = (string) $settings['apiKey'];
        if ($apiKey === '') {
            throw new \Exception('Girder API key is missing.');
        }

        $tokenResponse = $this->requestGirderTokenByApiKey((string) $settings['girderUrl'], $apiKey);
        $authToken = $this->extractAuthTokenValue($tokenResponse);

        if ($authToken === '') {
            throw new \Exception('Girder token response does not contain authToken.');
        }

        $expiresAt = $this->extractTokenExpiryTimestamp($tokenResponse);
        $this->cacheGirderAuthToken($settings, $authToken, $expiresAt);
        return $authToken;
    }

    private function requestGirderTokenByApiKey($girderUrl, $apiKey)
    {
        $baseUrl = rtrim((string) $girderUrl, '/');
        $headers = [
            'Accept: application/json',
            'Content-Type: application/x-www-form-urlencoded',
        ];

        $response = $this->executeCurlRequest(
            $baseUrl . '/api_key/token',
            'POST',
            $headers,
            http_build_query(['key' => $apiKey])
        );

        if ((int) $response['statusCode'] < 200 || (int) $response['statusCode'] >= 300) {
            $fallback = $this->executeCurlRequest(
                $baseUrl . '/api_key/token',
                'POST',
                $headers,
                http_build_query(['apiKey' => $apiKey])
            );
            $response = $fallback;
        }

        if ((int) $response['statusCode'] < 200 || (int) $response['statusCode'] >= 300) {
            throw new \Exception('Girder token request failed (' . (int) $response['statusCode'] . '): ' . (string) $response['body']);
        }

        $decoded = json_decode((string) $response['body'], true);
        if (!is_array($decoded)) {
            throw new \Exception('Girder token endpoint returned invalid JSON.');
        }

        return $decoded;
    }

    private function executeCurlRequest($url, $method, $headers, $body = null)
    {
        $methodUpper = strtoupper((string) $method);
        $ch = curl_init((string) $url);
        if ($ch === false) {
            throw new \Exception('Failed to initialize cURL.');
        }

        curl_setopt($ch, CURLOPT_CUSTOMREQUEST, (string) $method);
        curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
        curl_setopt($ch, CURLOPT_CONNECTTIMEOUT, 10);
        curl_setopt($ch, CURLOPT_TIMEOUT, 120);
        curl_setopt($ch, CURLOPT_HTTPHEADER, is_array($headers) ? $headers : []);

        if ($body !== null) {
            curl_setopt($ch, CURLOPT_POSTFIELDS, $body);
        } elseif (in_array($methodUpper, ['POST', 'PUT', 'PATCH'], true)) {
            curl_setopt($ch, CURLOPT_POSTFIELDS, '');
        }

        $responseBody = curl_exec($ch);
        $statusCode = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
        $curlError = curl_error($ch);
        curl_close($ch);

        if ($responseBody === false) {
            throw new \Exception('Girder request failed: ' . $curlError);
        }

        return [
            'statusCode' => $statusCode,
            'body' => (string) $responseBody,
        ];
    }

    private function tokenCacheKey($settings)
    {
        $raw = rtrim((string) $settings['girderUrl'], '/') . '|' . (string) $settings['apiKey'];
        return hash('sha256', $raw);
    }

    private function getCachedGirderAuthToken($settings)
    {
        if (!isset($_SESSION) || !is_array($_SESSION)) {
            return null;
        }

        if (!isset($_SESSION['girderUploaderAuthTokens']) || !is_array($_SESSION['girderUploaderAuthTokens'])) {
            return null;
        }

        $key = $this->tokenCacheKey($settings);
        $entry = isset($_SESSION['girderUploaderAuthTokens'][$key]) ? $_SESSION['girderUploaderAuthTokens'][$key] : null;
        if (!is_array($entry)) {
            return null;
        }

        $token = isset($entry['token']) ? trim((string) $entry['token']) : '';
        if ($token === '') {
            return null;
        }

        $expiresAt = isset($entry['expiresAt']) ? (int) $entry['expiresAt'] : 0;
        if ($expiresAt > 0 && time() >= max(0, $expiresAt - 30)) {
            unset($_SESSION['girderUploaderAuthTokens'][$key]);
            return null;
        }

        return $token;
    }

    private function cacheGirderAuthToken($settings, $token, $expiresAt = 0)
    {
        if (!isset($_SESSION) || !is_array($_SESSION)) {
            return;
        }

        if (!isset($_SESSION['girderUploaderAuthTokens']) || !is_array($_SESSION['girderUploaderAuthTokens'])) {
            $_SESSION['girderUploaderAuthTokens'] = [];
        }

        $_SESSION['girderUploaderAuthTokens'][$this->tokenCacheKey($settings)] = [
            'token' => (string) $token,
            'expiresAt' => (int) $expiresAt,
        ];
    }

    private function clearCachedGirderAuthToken($settings)
    {
        if (!isset($_SESSION) || !is_array($_SESSION)) {
            return;
        }
        if (!isset($_SESSION['girderUploaderAuthTokens']) || !is_array($_SESSION['girderUploaderAuthTokens'])) {
            return;
        }

        $key = $this->tokenCacheKey($settings);
        unset($_SESSION['girderUploaderAuthTokens'][$key]);
    }

    private function extractTokenExpiryTimestamp($tokenResponse)
    {
        if (!is_array($tokenResponse)) {
            return time() + 3600;
        }

        if (isset($tokenResponse['authToken']) && is_array($tokenResponse['authToken'])) {
            $nestedAuthToken = $tokenResponse['authToken'];
            if (isset($nestedAuthToken['expires']) && is_string($nestedAuthToken['expires'])) {
                $parsed = strtotime($nestedAuthToken['expires']);
                if ($parsed !== false && $parsed > 0) {
                    return (int) $parsed;
                }
            }
        }

        $knownIntegerKeys = ['expiresAt', 'expires_at', 'expiry', 'expiration', 'expires'];
        foreach ($knownIntegerKeys as $key) {
            if (isset($tokenResponse[$key]) && is_numeric($tokenResponse[$key])) {
                $value = (int) $tokenResponse[$key];
                if ($value > 0) {
                    return $value > 2000000000 ? (int) floor($value / 1000) : $value;
                }
            }
        }

        $knownDurationKeys = ['duration', 'ttl', 'expiresIn', 'expires_in'];
        foreach ($knownDurationKeys as $key) {
            if (isset($tokenResponse[$key]) && is_numeric($tokenResponse[$key])) {
                $duration = (int) $tokenResponse[$key];
                if ($duration > 0) {
                    return time() + $duration;
                }
            }
        }

        return time() + 3600;
    }

    private function extractAuthTokenValue($tokenResponse)
    {
        if (!is_array($tokenResponse)) {
            return '';
        }

        if (isset($tokenResponse['authToken']) && is_string($tokenResponse['authToken'])) {
            return trim($tokenResponse['authToken']);
        }

        if (isset($tokenResponse['authToken']) && is_array($tokenResponse['authToken'])) {
            $authTokenObject = $tokenResponse['authToken'];
            if (isset($authTokenObject['token']) && is_string($authTokenObject['token'])) {
                return trim($authTokenObject['token']);
            }
        }

        if (isset($tokenResponse['token']) && is_string($tokenResponse['token'])) {
            return trim($tokenResponse['token']);
        }

        return '';
    }

    private function normalizeApiUrl($url)
    {
        $url = trim($url);
        if ($url === '') {
            return '';
        }

        return rtrim($url, '/');
    }

    private function normalizeBaseUrl($url)
    {
        $url = trim((string) $url);
        if ($url === '') {
            return '';
        }

        return rtrim($url, '/');
    }

    private function deriveBaseUrlFromLegacyApiUrl($legacyApiUrl)
    {
        $legacyApiUrl = $this->normalizeApiUrl($legacyApiUrl);
        if ($legacyApiUrl === '') {
            return '';
        }

        $suffix = '/api/v1';
        if (substr($legacyApiUrl, -strlen($suffix)) === $suffix) {
            return substr($legacyApiUrl, 0, -strlen($suffix));
        }

        return $legacyApiUrl;
    }

    private function readIntSetting($key, $default)
    {
        $value = $this->getProjectSetting($key);
        if ($value === null || $value === '') {
            return (int) $default;
        }

        if (!is_numeric($value)) {
            return (int) $default;
        }

        $intValue = (int) $value;
        return $intValue > 0 ? $intValue : (int) $default;
    }

    private function readBoolSetting($key, $default = false)
    {
        $value = $this->getProjectSetting($key);
        if (is_bool($value)) {
            return $value;
        }
        if ($value === null || $value === '') {
            return (bool) $default;
        }

        $normalized = strtolower(trim((string) $value));
        if (in_array($normalized, ['1', 'true', 'yes', 'on'], true)) {
            return true;
        }
        if (in_array($normalized, ['0', 'false', 'no', 'off'], true)) {
            return false;
        }

        return (bool) $default;
    }

    private function isGirderNotFoundException($exception)
    {
        if (!$exception instanceof \Throwable) {
            return false;
        }

        $message = strtolower(trim((string) $exception->getMessage()));
        if ($message === '') {
            return false;
        }

        return strpos($message, 'girder request failed (404)') !== false
            || strpos($message, 'not found') !== false;
    }

    private function readBoolSettingTreatEmptyAsFalse($key, $default = false)
    {
        $value = $this->getProjectSetting($key);
        if (is_bool($value)) {
            return $value;
        }
        if ($value === null) {
            return (bool) $default;
        }
        if ($value === '') {
            return false;
        }

        $normalized = strtolower(trim((string) $value));
        if (in_array($normalized, ['1', 'true', 'yes', 'on'], true)) {
            return true;
        }
        if (in_array($normalized, ['0', 'false', 'no', 'off'], true)) {
            return false;
        }
        if ($normalized === '') {
            return false;
        }

        return (bool) $default;
    }

    private function getProjectTitle($projectId)
    {
        $projectId = (int) $projectId;
        if ($projectId <= 0) {
            return '';
        }

        $sql = "SELECT app_title FROM redcap_projects WHERE project_id = {$projectId} LIMIT 1";
        $result = db_query($sql);
        if ($result && ($row = db_fetch_assoc($result))) {
            return isset($row['app_title']) ? trim((string) $row['app_title']) : '';
        }

        return '';
    }

    private function consolePrint($message)
    // Print to javascript console if in web context, or to stdout if in CLI context
    {
        if (php_sapi_name() === 'cli') {
            echo '[GirderUploaderModule] ' . $message . PHP_EOL;
        } else {
            echo '<script>console.log("[GirderUploaderModule] ' . addslashes($message) . '");</script>';
        }
    }

    private function debugAjax($message, $context = [])
    {
        $parameters = [];
        if (is_array($context)) {
            foreach ($context as $key => $value) {
                $parameterKey = preg_replace('/[^A-Za-z0-9 _\-$]/', '_', (string) $key);
                if ($parameterKey === '') {
                    continue;
                }

                if (is_scalar($value) || $value === null) {
                    $parameters[$parameterKey] = $value;
                    continue;
                }

                $parameters[$parameterKey] = json_encode($value, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE);
            }
        }

        $this->log('[AJAX] ' . (string) $message, $parameters);
    }

    private function includeJs($path)
    {
        echo '<script src="' . $this->getUrl($path, true) . '"></script>';
    }

    private function includeCSS($path)
    {
        echo '<link rel="stylesheet" type="text/css" href="' . $this->getUrl($path, true) . '">';
    }
}
