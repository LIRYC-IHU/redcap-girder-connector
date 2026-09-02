(function () {
    'use strict';

    var core = (typeof window !== 'undefined' && window.GirderUploaderCore) || null;
    if (!core) {
        throw new Error('girder-uploader-core.js must be loaded before girder-uploader.js.');
    }

    var deidentifyBridge = null;
    var sanitizePathPart = core.sanitizePathPart;
    var getFileDisplayName = core.getFileDisplayName;
    var isZipFile = core.isZipFile;
    var isLikelyDicomFile = core.isLikelyDicomFile;
    var normalizeUploadFiles = core.normalizeUploadFiles;
    var formatSize = core.formatSize;
    var buildFolderUrl = core.buildFolderUrl;
    var buildUploadSummary = core.buildUploadSummary;
    var normalizeMetadataPayload = core.normalizeMetadataPayload;
    var buildStoredMetadataPayload = core.buildStoredMetadataPayload;
    var isCompletedPayload = core.isCompletedPayload;
    var isFailedPayload = core.isFailedPayload;

    function debug(message, details) {
        if (typeof console === 'undefined' || typeof console.debug !== 'function') {
            return;
        }

        if (typeof details === 'undefined') {
            console.debug('[GirderUploader] ' + message);
            return;
        }

        console.debug('[GirderUploader] ' + message, details);
    }

    function onReady(callback) {
        if (document.readyState === 'loading') {
            document.addEventListener('DOMContentLoaded', callback);
            return;
        }
        callback();
    }

    function escapeSelector(value) {
        if (window.CSS && typeof window.CSS.escape === 'function') {
            return window.CSS.escape(value);
        }
        return value.replace(/([ #;?%&,.+*~\':"!^$\[\]()=>|/@])/g, '\\$1');
    }

    function delay(ms) {
        return new Promise(function (resolve) {
            setTimeout(resolve, ms);
        });
    }

    function getModuleObject() {
        var moduleObject = window.GirderUploaderModule;
        if (!moduleObject || typeof moduleObject.ajax !== 'function') {
            throw new Error('REDCap module AJAX bridge is unavailable.');
        }
        return moduleObject;
    }

    async function moduleAjax(action, payload) {
        var moduleObject = getModuleObject();
        debug('AJAX request -> ' + action, payload);
        var response = await moduleObject.ajax(action, payload);

        if (!response || response.ok !== true) {
            var message = response && response.error ? response.error : 'Unknown server error';
            debug('AJAX failure <- ' + action, response || {});
            throw new Error(message);
        }

        debug('AJAX success <- ' + action, response);
        return response;
    }

    function blobToBase64(blob) {
        return new Promise(function (resolve, reject) {
            var reader = new FileReader();
            reader.onload = function () {
                var result = String(reader.result || '');
                var commaIndex = result.indexOf(',');
                resolve(commaIndex >= 0 ? result.slice(commaIndex + 1) : '');
            };
            reader.onerror = function () {
                reject(new Error('Unable to read chunk data.'));
            };
            reader.readAsDataURL(blob);
        });
    }

    function escapeHtml(value) {
        return String(value == null ? '' : value)
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#39;');
    }

    function toVirtualFileFromZipBlob(blob, baseName) {
        try {
            return new File([blob], baseName, { type: blob.type || 'application/octet-stream' });
        } catch (error) {
            blob.name = baseName;
            return blob;
        }
    }

    async function expandSingleZipFile(zipFile) {
        if (!window.JSZip || typeof window.JSZip.loadAsync !== 'function') {
            throw new Error('ZIP support is unavailable (JSZip not loaded).');
        }

        var zip = await window.JSZip.loadAsync(zipFile);
        var outputFiles = [];
        var entryPaths = Object.keys(zip.files || {});

        for (var i = 0; i < entryPaths.length; i += 1) {
            var entryPath = String(entryPaths[i] || '');
            var entry = zip.files[entryPath];
            if (!entry || entry.dir) {
                continue;
            }

            var normalizedPath = entryPath.replace(/^\/+/, '').replace(/\\/g, '/').trim();
            if (!normalizedPath) {
                continue;
            }

            var pathParts = normalizedPath.split('/').filter(Boolean);
            if (!pathParts.length) {
                continue;
            }

            var baseName = pathParts[pathParts.length - 1];
            if (!baseName) {
                continue;
            }

            // ZIP files located in subfolders are discarded.
            if (baseName.toLowerCase().endsWith('.zip') && pathParts.length > 1) {
                continue;
            }

            var blob = await entry.async('blob');
            var virtualFile = toVirtualFileFromZipBlob(blob, baseName);
            virtualFile.girderRelativePath = normalizedPath;
            outputFiles.push(virtualFile);
        }

        return normalizeUploadFiles(outputFiles);
    }

    async function resolveSelectedFilesForUpload(fileList) {
        var files = Array.isArray(fileList) ? fileList.slice() : Array.from(fileList || []);
        var normalized = normalizeUploadFiles(files);

        if (normalized.length === 1 && isZipFile(normalized[0])) {
            return expandSingleZipFile(normalized[0]);
        }

        return normalized.filter(function (file) {
            return !isZipFile(file);
        });
    }

    function createDeidentifyBridge(workerUrl) {
        var worker = new Worker(workerUrl, { type: 'module' });
        var pending = new Map();
        var nextId = 1;

        worker.onmessage = function (event) {
            var data = event && event.data ? event.data : {};
            var id = Number(data.id || 0);
            if (!pending.has(id)) {
                return;
            }

            var handlers = pending.get(id);
            pending.delete(id);
            if (data.ok) {
                handlers.resolve(data);
            } else {
                handlers.reject(new Error(String(data.error || 'Deidentification failed.')));
            }
        };

        worker.onerror = function (errorEvent) {
            var message = errorEvent && errorEvent.message ? errorEvent.message : 'Worker error';
            pending.forEach(function (handlers) {
                handlers.reject(new Error(message));
            });
            pending.clear();
        };

        return {
            run: function (payload) {
                return new Promise(function (resolve, reject) {
                    var id = nextId;
                    nextId += 1;
                    pending.set(id, { resolve: resolve, reject: reject });

                    try {
                        var message = Object.assign({ id: id }, payload);
                        worker.postMessage(message, [payload.bytes]);
                    } catch (error) {
                        pending.delete(id);
                        reject(error);
                    }
                });
            }
        };
    }

    function getDeidentifyBridge(config) {
        if (deidentifyBridge) {
            return deidentifyBridge;
        }

        var workerUrl = String(config.settings.deidentifyWorkerUrl || '').trim();
        if (!workerUrl) {
            throw new Error('Deidentification worker URL is missing.');
        }

        deidentifyBridge = createDeidentifyBridge(workerUrl);
        return deidentifyBridge;
    }

    /**
     * Deidentify a batch, one file at a time.
     *
     * A file the worker refuses is set aside, not fatal: a DICOM archive
     * routinely carries a DICOMDIR, a README or a viewer's XML next to the
     * images, and one of those must not cancel a thousand-file upload. The
     * rejections are returned so the caller can show and store them — dropping
     * a file quietly is exactly what we are trying to avoid.
     */
    async function deidentifyFilesIfNeeded(files, config, statusLine, deidentifyProgressBar, rejected) {
        var bridge = getDeidentifyBridge(config);
        if (!bridge) {
            throw new Error('Deidentification worker is unavailable.');
        }

        var resultFiles = [];
        var skippedCount = 0;
        if (deidentifyProgressBar) {
            deidentifyProgressBar.style.width = '0%';
        }

        for (var i = 0; i < files.length; i += 1) {
            var file = files[i];
            var relativePath = getFileDisplayName(file) || file.name;
            if (deidentifyProgressBar) {
                deidentifyProgressBar.style.width = Math.round((i / files.length) * 100) + '%';
            }
            statusLine.textContent = 'Deidentifying ' + (i + 1) + '/' + files.length + ': ' + relativePath;
            var inputArrayBuffer = await file.arrayBuffer();
            var workerResult;
            try {
                workerResult = await bridge.run(core.buildDeidentifyRequest(file, config, inputArrayBuffer));
            } catch (error) {
                if (core.isSkipError(error)) {
                    skippedCount += 1;
                } else {
                    rejected.push(core.rejection(file, 'deidentification', error));
                    debug('File rejected during deidentification', { file: relativePath, error: String(error) });
                }
                continue;
            }

            var outputBytes = workerResult && workerResult.bytes ? workerResult.bytes : null;
            if (!outputBytes) {
                rejected.push(core.rejection(file, 'deidentification', 'Deidentification produced no output.'));
                continue;
            }

            var outputFile;
            var outputMimeType = workerResult && workerResult.mimeType
                ? String(workerResult.mimeType)
                : 'application/octet-stream';
            try {
                outputFile = new File([outputBytes], file.name, {
                    type: outputMimeType
                });
            } catch (error) {
                outputFile = new Blob([outputBytes], { type: outputMimeType });
                outputFile.name = file.name;
            }
            outputFile.girderRelativePath = relativePath;
            resultFiles.push(outputFile);
            if (deidentifyProgressBar) {
                deidentifyProgressBar.style.width = Math.round(((i + 1) / files.length) * 100) + '%';
            }
        }

        if (skippedCount > 0 || rejected.length) {
            statusLine.textContent = core.summarizeOutcome(resultFiles.length, skippedCount, rejected)
                + ' Preparing upload...';
        }

        return resultFiles;
    }

    async function readFileEntry(entry) {
        return new Promise(function (resolve, reject) {
            try {
                entry.file(function (file) {
                    if (file && entry.fullPath) {
                        file.girderRelativePath = String(entry.fullPath).replace(/^\/+/, '');
                    }
                    resolve([file]);
                }, function (error) {
                    reject(error || new Error('Unable to read dropped file.'));
                });
            } catch (error) {
                reject(error);
            }
        });
    }

    async function readDirectoryEntry(entry) {
        return new Promise(function (resolve, reject) {
            var reader = entry.createReader();
            var entries = [];

            function readBatch() {
                reader.readEntries(function (results) {
                    if (!results || !results.length) {
                        resolve(entries);
                        return;
                    }
                    entries = entries.concat(Array.from(results));
                    readBatch();
                }, function (error) {
                    reject(error || new Error('Unable to read dropped directory.'));
                });
            }

            readBatch();
        });
    }

    async function flattenEntryFiles(entry) {
        if (!entry) {
            return [];
        }

        if (entry.isFile) {
            return readFileEntry(entry);
        }

        if (entry.isDirectory) {
            var children = await readDirectoryEntry(entry);
            var files = [];
            for (var i = 0; i < children.length; i += 1) {
                var childFiles = await flattenEntryFiles(children[i]);
                files = files.concat(childFiles);
            }
            return files;
        }

        return [];
    }

    async function getDroppedFilesFromEvent(event) {
        var dataTransfer = event && event.dataTransfer ? event.dataTransfer : null;
        if (!dataTransfer) {
            return [];
        }

        if (dataTransfer.items && dataTransfer.items.length) {
            var files = [];
            for (var i = 0; i < dataTransfer.items.length; i += 1) {
                var item = dataTransfer.items[i];
                var entry = item && typeof item.webkitGetAsEntry === 'function' ? item.webkitGetAsEntry() : null;
                if (entry) {
                    var entryFiles = await flattenEntryFiles(entry);
                    files = files.concat(entryFiles);
                    continue;
                }

                var fallbackFile = item && typeof item.getAsFile === 'function' ? item.getAsFile() : null;
                if (fallbackFile) {
                    files.push(fallbackFile);
                }
            }
            return normalizeUploadFiles(files);
        }

        return normalizeUploadFiles(dataTransfer.files);
    }

    function getBackendChunkSize(settings) {
        var girderMinimumChunkSize = 5 * 1024 * 1024;
        var backendMaximumChunkSize = girderMinimumChunkSize;
        var configuredChunkSize = Number(settings && settings.chunkSize ? settings.chunkSize : 10485760);
        if (!isFinite(configuredChunkSize) || configuredChunkSize <= 0) {
            configuredChunkSize = 10485760;
        }

        return Math.max(girderMinimumChunkSize, Math.min(configuredChunkSize, backendMaximumChunkSize));
    }

    async function uploadFileViaBackend(settings, fieldName, uploadData, file, onProgress) {
        var attempt = 0;
        var maxRetries = Number(settings && settings.maxRetries ? settings.maxRetries : 3);
        var retryDelay = Number(settings && settings.retryDelay ? settings.retryDelay : 1000);
        var chunkSize = getBackendChunkSize(settings);
        var offset = 0;
        var lastFileEntity = null;

        if (!file || !file.size) {
            throw new Error('Cannot upload an empty file.');
        }

        var totalChunks = Math.ceil(file.size / chunkSize);
        onProgress(5, {
            chunkIndex: 1,
            totalChunks: totalChunks,
            uploadedBytes: 0,
            totalBytes: file.size,
            percent: 5
        });
        while (offset < file.size) {
            var end = Math.min(offset + chunkSize, file.size);
            var chunk = file.slice(offset, end);
            var chunkIndex = Math.floor(offset / chunkSize) + 1;
            var startingPercent = Math.max(5, Math.round((offset / file.size) * 100));

            onProgress(startingPercent, {
                chunkIndex: chunkIndex,
                totalChunks: totalChunks,
                uploadedBytes: offset,
                totalBytes: file.size,
                percent: startingPercent
            });

            while (true) {
                try {
                    var fileBase64 = await blobToBase64(chunk);
                    var response = await moduleAjax('upload-file', {
                        fieldName: fieldName,
                        uploadId: uploadData.uploadId,
                        offset: offset,
                        fileBase64: fileBase64
                    });
                    lastFileEntity = response.file || lastFileEntity;
                    offset = end;
                    attempt = 0;
                    var completedPercent = Math.max(5, Math.round((offset / file.size) * 100));
                    onProgress(completedPercent, {
                        chunkIndex: chunkIndex,
                        totalChunks: totalChunks,
                        uploadedBytes: offset,
                        totalBytes: file.size,
                        percent: completedPercent
                    });
                    break;
                } catch (error) {
                    attempt += 1;
                    if (attempt > maxRetries) {
                        throw error;
                    }
                    await delay(retryDelay);
                }
            }
        }

        onProgress(100, {
            chunkIndex: totalChunks,
            totalChunks: totalChunks,
            uploadedBytes: file.size,
            totalBytes: file.size,
            percent: 100
        });
        return lastFileEntity;
    }

    function setFieldValue(textarea, value) {
        textarea.value = value;
        textarea.dispatchEvent(new Event('input', { bubbles: true }));
        textarea.dispatchEvent(new Event('change', { bubbles: true }));
    }

    function getDataEntryFormElement() {
        return document.getElementById('form')
            || document.querySelector('form[name="form"]')
            || document.querySelector('form');
    }

    function forceSaveAndStay(formData) {
        var saveTriggerFields = [
            'submit-btn-saverecord',
            'submit-btn-savecontinue',
            'submit-btn-saveexitrecord',
            'submit-btn-savenextrecord',
            'submit-btn-deleteform',
            'submit-action'
        ];

        saveTriggerFields.forEach(function (field) {
            if (formData.has(field)) {
                formData.delete(field);
            }
        });

        formData.append('submit-btn-savecontinue', '1');
        formData.append('submit-action', 'submit-btn-savecontinue');
    }

    async function saveFormInBackground() {
        var form = getDataEntryFormElement();
        if (!form) {
            throw new Error('Data entry form not found on page.');
        }

        var action = form.getAttribute('action') || window.location.href;
        var formData = new FormData(form);
        forceSaveAndStay(formData);

        var response = await fetch(action, {
            method: 'POST',
            body: formData,
            credentials: 'same-origin',
            headers: {
                'X-Requested-With': 'XMLHttpRequest'
            }
        });

        if (!response.ok) {
            throw new Error('Background form save failed (' + response.status + ').');
        }
    }

    function saveFormInForeground() {
        if (typeof window.dataEntrySubmit === 'function') {
            window.dataEntrySubmit('submit-btn-savecontinue');
            return;
        }

        var button = document.getElementById('submit-btn-savecontinue');
        if (button && typeof button.click === 'function') {
            button.click();
            return;
        }

        throw new Error('Save & Stay submission is unavailable on this page.');
    }

    function detectReadOnlyPageState() {
        var saveButton = document.getElementById('submit-btn-savecontinue');
        if (!saveButton || saveButton.disabled) {
            return true;
        }

        var lockCheckbox = document.getElementById('__LOCKRECORD__');
        if (lockCheckbox && lockCheckbox.checked) {
            return true;
        }

        return false;
    }

    function renderFileState(target, payload) {
        if (!payload) {
            target.innerHTML = '';
            return;
        }

        var uploadStatus = payload.uploadState && payload.uploadState.status
            ? String(payload.uploadState.status)
            : '';
        var isFailed = uploadStatus === 'failed';
        var uploadedAt = payload.uploadedAt || 'Unknown';
        var uploadedFiles = Array.isArray(payload.uploadedFiles) ? payload.uploadedFiles : [];
        var summary = payload.uploadSummary && typeof payload.uploadSummary === 'object' ? payload.uploadSummary : {};
        var fileCount = Number(summary.fileCount);
        if (!isFinite(fileCount) || fileCount < uploadedFiles.length) {
            fileCount = uploadedFiles.length;
        }
        var firstUploadedFile = uploadedFiles.length ? uploadedFiles[0] : {};
        var displayName = fileCount === 1
            ? (firstUploadedFile.name || firstUploadedFile.originalName || 'Unknown file')
            : (fileCount + ' files');
        var totalSizeBytes = Number(payload.totalSizeBytes);
        if (!isFinite(totalSizeBytes) || totalSizeBytes < 0) {
            totalSizeBytes = Number(summary.totalSizeBytes);
        }
        if (!isFinite(totalSizeBytes) || totalSizeBytes < 0) {
            totalSizeBytes = uploadedFiles.reduce(function (sum, fileInfo) {
                var value = Number(fileInfo && fileInfo.size ? fileInfo.size : 0);
                return sum + (isNaN(value) ? 0 : value);
            }, 0);
        }
        var size = formatSize(totalSizeBytes);
        var itemId = payload.girder && payload.girder.itemId ? payload.girder.itemId : 'n/a';
        var folderId = payload.girder && payload.girder.parentFolderId ? payload.girder.parentFolderId : 'n/a';
        var folderUrl = payload.girder && payload.girder.parentFolderUrl ? payload.girder.parentFolderUrl : '';
        var folderHtml = folderUrl
            ? '<a href="' + escapeHtml(folderUrl) + '" target="_blank" rel="noopener noreferrer">' + escapeHtml(folderId) + '</a>'
            : escapeHtml(folderId);
        var errorMessage = payload.uploadState && payload.uploadState.error
            ? String(payload.uploadState.error)
            : 'Unknown upload error.';
        var cardClass = isFailed ? 'girder-file-card girder-file-card-error' : 'girder-file-card';
        var statusTitle = isFailed ? 'Upload failed' : 'Uploaded file';
        var errorLine = isFailed
            ? '<div class="girder-file-error"><strong>Error:</strong> ' + escapeHtml(errorMessage) + '</div>'
            : '';
        var nameLine = fileCount === 1
            ? '<div><strong>Name:</strong> ' + escapeHtml(displayName) + '</div>'
            : '';
        var lastItemLine = isFailed && itemId !== 'n/a'
            ? '<div><strong>Last item ID:</strong> ' + escapeHtml(itemId) + '</div>'
            : '';
        var lastSyncedLine = payload.uploadState && payload.uploadState.syncedAt
            ? '<div><strong>Last synced:</strong> ' + escapeHtml(String(payload.uploadState.syncedAt)) + '</div>'
            : '';
        // Files the batch left behind. They are shown rather than merely
        // counted: a rejection the user cannot see is a silent data loss.
        var rejectedFiles = Array.isArray(payload.rejectedFiles) ? payload.rejectedFiles : [];
        var rejectedCount = Number(payload.rejectedCount);
        if (!isFinite(rejectedCount) || rejectedCount < rejectedFiles.length) {
            rejectedCount = rejectedFiles.length;
        }
        var rejectedLine = '';
        if (rejectedCount > 0) {
            var rejectedItems = rejectedFiles.map(function (entry) {
                return '<li>' + escapeHtml(String(entry.name || 'unknown file'))
                    + ' — ' + escapeHtml(String(entry.reason || 'Unknown error')) + '</li>';
            }).join('');
            var omitted = rejectedCount - rejectedFiles.length;
            rejectedLine = '<div class="girder-file-rejected">'
                + '<strong>Not uploaded (' + rejectedCount + '):</strong>'
                + '<ul>' + rejectedItems + '</ul>'
                + (omitted > 0 ? '<div>… and ' + omitted + ' more.</div>' : '')
                + '</div>';
        }

        target.innerHTML = ''
            + '<div class="' + cardClass + '">'
            + '<h5>' + escapeHtml(statusTitle) + '</h5>'
            + rejectedLine
            + nameLine
            + '<div><strong>Files:</strong> ' + String(fileCount || 0) + '</div>'
            + '<div><strong>Size:</strong> ' + size + '</div>'
            + '<div><strong>Uploaded at:</strong> ' + escapeHtml(uploadedAt) + '</div>'
            + lastSyncedLine
            + '<div><strong>Girder folder:</strong> ' + folderHtml + '</div>'
            + lastItemLine
            + errorLine
            + '</div>';
    }

    async function clearMetadataAndSave(textarea) {
        setFieldValue(textarea, '');
        await saveFormInBackground();
    }

    async function refreshMetadataFromGirder(fieldName, payload) {
        if (!payload || !payload.girder || !payload.girder.parentFolderId) {
            return { missing: true, metadata: null };
        }

        var response = await moduleAjax('refresh-upload-metadata', {
            fieldName: fieldName,
            folderId: payload.girder.parentFolderId,
            uploadedAt: payload.uploadedAt || ''
        });

        return {
            missing: !!response.missing,
            metadata: response.metadata ? normalizeMetadataPayload(response.metadata) : null
        };
    }

    async function deleteUploadContents(fieldName, payload) {
        if (!payload || !payload.girder || !payload.girder.parentFolderId) {
            throw new Error('Missing Girder folder id for deletion.');
        }

        await moduleAjax('delete-upload-contents', {
            fieldName: fieldName,
            folderId: payload.girder.parentFolderId
        });
    }

    async function persistMetadata(textarea, payload, shouldBackgroundSave) {
        setFieldValue(textarea, JSON.stringify(buildStoredMetadataPayload(payload)));
        if (!shouldBackgroundSave) {
            return;
        }

        try {
            await saveFormInBackground();
            debug('Background save completed.');
        } catch (error) {
            debug('Background save failed', { error: error.message });
        }
    }

    function buildWidget(textarea, fieldName, config) {
        var wrapper = document.createElement('div');
        var dropzone = document.createElement('div');
        var infoZone = document.createElement('div');
        var actionBar = document.createElement('div');
        var deleteButton = document.createElement('button');
        var progressContainer = document.createElement('div');
        var deidentifyProgressBar = document.createElement('div');
        var progressBar = document.createElement('div');
        var statusLine = document.createElement('div');
        var input = document.createElement('input');

        wrapper.className = 'girder-uploader-wrapper';
        dropzone.className = 'girder-dropzone';
        dropzone.tabIndex = 0;
        dropzone.innerHTML = '<strong>Upload to Girder</strong><br>Drop files or a folder here, or click to choose files.';

        statusLine.className = 'girder-upload-status';
        statusLine.textContent = 'Waiting for file selection.';

        progressContainer.className = 'upload-progress-container';
        deidentifyProgressBar.className = 'upload-progress-bar upload-progress-bar-deidentifying';
        progressBar.className = 'upload-progress-bar upload-progress-bar-uploading';
        progressContainer.appendChild(deidentifyProgressBar);
        progressContainer.appendChild(progressBar);

        infoZone.className = 'girder-file-info';
        actionBar.className = 'girder-uploader-actions';

        deleteButton.type = 'button';
        deleteButton.className = 'girder-delete-button';
        deleteButton.textContent = 'Delete uploaded contents';
        deleteButton.style.display = 'none';
        actionBar.appendChild(deleteButton);

        input.type = 'file';
        input.multiple = true;
        input.style.display = 'none';

        wrapper.appendChild(dropzone);
        wrapper.appendChild(statusLine);
        wrapper.appendChild(progressContainer);
        wrapper.appendChild(infoZone);
        wrapper.appendChild(actionBar);
        wrapper.appendChild(input);

        textarea.style.display = 'none';
        textarea.setAttribute('data-girder-upload-enhanced', '1');
        textarea.insertAdjacentElement('afterend', wrapper);

        var existingValue = String(textarea.value || '').trim();
        var isLocked = false;
        var isBusy = false;
        var currentMetadata = null;
        var permissions = (config && config.permissions) || {};
        var canModify = !!permissions.canModify;
        // A record that has not been saved has no id yet, and the record id is
        // what the deidentifier writes into the files and what names the Girder
        // folder. Uploading now would bake a placeholder into both.
        var recordSaved = !!permissions.recordSaved;
        var actionsAllowed = canModify && recordSaved && !detectReadOnlyPageState();

        function lockWidgetWithMessage(message) {
            isLocked = true;
            input.disabled = true;
            dropzone.classList.add('girder-dropzone-locked');
            dropzone.setAttribute('aria-disabled', 'true');
            statusLine.textContent = message;
        }

        function switchToInfoOnlyMode() {
            isLocked = true;
            input.disabled = true;
            dropzone.style.display = 'none';
            statusLine.style.display = 'none';
            progressContainer.style.display = 'none';
        }

        function restoreUploadMode() {
            isLocked = false;
            input.disabled = false;
            dropzone.classList.remove('girder-dropzone-locked');
            dropzone.removeAttribute('aria-disabled');
            dropzone.style.display = '';
            statusLine.style.display = '';
            progressContainer.style.display = '';
        }

        function setDeleteAvailability(enabled) {
            var visible = actionsAllowed && enabled;
            deleteButton.style.display = visible ? '' : 'none';
            deleteButton.disabled = !visible || isBusy;
        }

        function updateInfoState(payload) {
            currentMetadata = payload ? normalizeMetadataPayload(payload) : null;
            renderFileState(infoZone, currentMetadata);
            setDeleteAvailability(isCompletedPayload(currentMetadata));
        }

        function resetWidgetToEmpty(message) {
            currentMetadata = null;
            deidentifyProgressBar.style.width = '0%';
            progressBar.style.width = '0%';
            infoZone.innerHTML = '';
            if (actionsAllowed) {
                restoreUploadMode();
            } else {
                switchToInfoOnlyMode();
            }
            setDeleteAvailability(false);
            setFieldValue(textarea, '');
            statusLine.textContent = message || 'Waiting for file selection.';
        }

        async function refreshExistingMetadata(existingPayload) {
            updateInfoState(existingPayload);
            if (!isCompletedPayload(existingPayload)) {
                return;
            }

            setUploadingState(true);
            switchToInfoOnlyMode();
            statusLine.style.display = '';
            statusLine.textContent = 'Refreshing Girder metadata...';

            try {
                var refreshed = await refreshMetadataFromGirder(fieldName, existingPayload);
                if (refreshed.missing || !refreshed.metadata || !Array.isArray(refreshed.metadata.uploadedFiles) || !refreshed.metadata.uploadedFiles.length) {
                    resetWidgetToEmpty('Uploaded contents were not found in Girder. You can upload again.');
                    return;
                }

                refreshed.metadata.uploadState = refreshed.metadata.uploadState || {};
                refreshed.metadata.uploadState.syncedAt = new Date().toISOString();
                // Girder knows nothing of the files we refused; without this the
                // refresh would erase the record of what was left behind.
                core.carryRejectionsForward(existingPayload, refreshed.metadata);
                updateInfoState(refreshed.metadata);
                setFieldValue(textarea, JSON.stringify(buildStoredMetadataPayload(currentMetadata)));
                progressBar.style.width = '100%';
                switchToInfoOnlyMode();
                if (actionsAllowed) {
                    statusLine.style.display = '';
                    statusLine.textContent = 'Girder metadata refreshed.';
                }
            } catch (error) {
                progressBar.style.width = '100%';
                switchToInfoOnlyMode();
                if (actionsAllowed || !currentMetadata) {
                    statusLine.style.display = '';
                    statusLine.textContent = 'Unable to refresh Girder metadata. Showing saved metadata.';
                }
                debug('Metadata refresh failed', {
                    fieldName: fieldName,
                    error: error.message
                });
            } finally {
                setUploadingState(false);
            }
        }

        if (existingValue) {
            try {
                var existingPayload = normalizeMetadataPayload(JSON.parse(existingValue));
                updateInfoState(existingPayload);

                if (isFailedPayload(existingPayload)) {
                    progressBar.style.width = '0%';
                    if (actionsAllowed) {
                        restoreUploadMode();
                        statusLine.textContent = 'Previous upload failed. You can re-upload.';
                    } else {
                        switchToInfoOnlyMode();
                        if (!currentMetadata) {
                            statusLine.style.display = '';
                            statusLine.textContent = 'Upload failed. Editing is disabled on this page.';
                        }
                    }
                } else {
                    progressBar.style.width = '100%';
                    refreshExistingMetadata(existingPayload);
                }
            } catch (error) {
                lockWidgetWithMessage('Field already contains data. Upload is disabled.');
            }
        }

        function setUploadingState(uploading) {
            isBusy = uploading;
            dropzone.style.pointerEvents = uploading ? 'none' : 'auto';
            dropzone.style.opacity = uploading ? '0.6' : '1';
            deleteButton.disabled = uploading || !isCompletedPayload(currentMetadata);
            debug('Uploading state updated', {
                fieldName: fieldName,
                uploading: uploading
            });
        }

        async function handleFiles(selectedFiles) {
            if (!actionsAllowed) {
                statusLine.textContent = 'Upload actions are disabled on this page.';
                return;
            }

            var files = await resolveSelectedFilesForUpload(selectedFiles);
            if (!files.length || isLocked) {
                statusLine.textContent = 'No eligible files found for upload.';
                return;
            }
            if (files.length > 9999) {
                statusLine.textContent = 'A maximum of 9999 files can be uploaded at once.';
                return;
            }

            debug('Files selected', {
                fieldName: fieldName,
                count: files.length,
                names: files.map(getFileDisplayName)
            });

            setUploadingState(true);
            deidentifyProgressBar.style.width = '0%';
            progressBar.style.width = '0%';
            statusLine.style.display = '';
            progressContainer.style.display = '';
            statusLine.textContent = 'Preparing upload...';
            var uploadData = null;
            var finalMetadata = null;
            var pendingPayload = null;
            var rejected = [];

            try {
                files = await deidentifyFilesIfNeeded(files, config, statusLine, deidentifyProgressBar, rejected);
                if (!files.length) {
                    throw new Error(rejected.length
                        ? ('No file could be deidentified. First reason: ' + rejected[0].reason)
                        : 'No eligible files after deidentification.');
                }
                progressBar.style.width = '0%';

                pendingPayload = {
                    version: 1,
                    uploadedAt: null,
                    uploadedFiles: [],
                    uploadState: {
                        status: 'uploading',
                        stage: 'initializing',
                        updatedAt: new Date().toISOString(),
                        error: null
                    },
                    girder: {}
                };
                if (files.length > 1) {
                    statusLine.textContent = 'Saving upload summary...';
                    await persistMetadata(textarea, pendingPayload, true);
                }

                var uploadedFiles = [];
                var lastSuccessfulGirder = null;
                var totalFiles = files.length;
                statusLine.textContent = 'Préparation des dossiers de réception...';
                var batchResponse = await moduleAjax('init-batch', {
                    fieldName: fieldName,
                    files: files.map(function (file) {
                        var relativePath = getFileDisplayName(file) || file.name;
                        return {
                            relativePath: relativePath,
                            fileName: relativePath,
                            fileSize: file.size,
                            mimeType: file.type || 'application/octet-stream',
                            isDicom: (file.type || '').toLowerCase() === 'application/dicom'
                                || isLikelyDicomFile(file)
                        };
                    })
                });
                var batchData = batchResponse.batch || {};
                var batchUploads = Array.isArray(batchData.uploads) ? batchData.uploads : [];
                if (batchUploads.length !== totalFiles) {
                    throw new Error('Batch initialization mismatch between selected files and upload plan.');
                }

                for (var fileIndex = 0; fileIndex < totalFiles; fileIndex += 1) {
                    var file = files[fileIndex];
                    try {
                        var currentFileLabel = getFileDisplayName(file) || file.name;
                        var filePrefix = (fileIndex + 1) + '/' + totalFiles + ' ';
                        var uploadPlan = batchUploads[fileIndex];
                        if (!uploadPlan || !uploadPlan.folderId || !uploadPlan.storedFileName) {
                            throw new Error('Missing upload plan for file #' + (fileIndex + 1));
                        }
                        var storedFileName = uploadPlan.storedFileName || currentFileLabel;

                        statusLine.textContent = filePrefix + 'Préparation du fichier...';
                        var uploadResponse = await moduleAjax('init-file-upload', {
                            fieldName: fieldName,
                            uploadPlan: uploadPlan
                        });
                        uploadData = uploadResponse.upload || {};
                        if (!uploadData.uploadId) {
                            throw new Error('Missing upload id for file #' + (fileIndex + 1));
                        }
                        storedFileName = uploadData.storedFileName || storedFileName;
                        statusLine.textContent = filePrefix + 'Uploading file...';
                        debug('Upload initialized', uploadData);

                        pendingPayload.girder = {
                            baseApiUrl: uploadData.girderUrl,
                            baseUrl: uploadData.girderFrontchannelBaseUrl || null,
                            rootCollectionId: uploadData.rootCollectionId,
                            dagName: uploadData.dagName,
                            recordId: uploadData.recordId,
                            instanceId: uploadData.instanceId,
                            parentFolderId: uploadData.parentFolderId || uploadData.folderId,
                            parentFolderUrl: (uploadData.girderFrontchannelBaseUrl && (uploadData.parentFolderId || uploadData.folderId))
                                ? (uploadData.girderFrontchannelBaseUrl + '/#folder/' + (uploadData.parentFolderId || uploadData.folderId))
                                : null,
                            folderId: uploadData.folderId,
                            itemId: uploadData.itemId,
                            uploadId: uploadData.uploadId,
                            fileId: null
                        };
                        pendingPayload.uploadState.stage = 'uploading';
                        pendingPayload.uploadState.updatedAt = new Date().toISOString();

                        var fileEntity = await uploadFileViaBackend(config.settings, fieldName, uploadData, file, function (percent, chunkInfo) {
                            var normalizedPercent = Math.round(((fileIndex + (percent / 100)) / totalFiles) * 100);
                            progressBar.style.width = normalizedPercent + '%';
                            if (chunkInfo && chunkInfo.totalChunks > 1) {
                                statusLine.textContent = filePrefix
                                    + 'Uploading chunk '
                                    + chunkInfo.chunkIndex
                                    + '/'
                                    + chunkInfo.totalChunks
                                    + ' - '
                                    + formatSize(chunkInfo.uploadedBytes)
                                    + '/'
                                    + formatSize(chunkInfo.totalBytes);
                            } else {
                                statusLine.textContent = filePrefix + 'Uploading...';
                            }
                        });

                        uploadedFiles.push({
                            name: storedFileName,
                            size: file.size,
                            mimeType: file.type || 'application/octet-stream',
                            folderId: uploadData.folderId,
                            itemId: uploadData.itemId,
                            uploadId: uploadData.uploadId,
                            fileId: fileEntity && fileEntity._id ? fileEntity._id : null
                        });

                        pendingPayload.uploadedFiles = uploadedFiles.slice();
                        pendingPayload.girder.itemId = uploadData.itemId;
                        pendingPayload.girder.uploadId = uploadData.uploadId;
                        pendingPayload.girder.fileId = fileEntity && fileEntity._id ? fileEntity._id : null;
                        pendingPayload.uploadState.updatedAt = new Date().toISOString();
                        // Now that a file has landed, remember where: a later
                        // failure must not leave the metadata pointing at it.
                        lastSuccessfulGirder = Object.assign({}, pendingPayload.girder);
                    } catch (fileError) {
                        // One file failing to reach Girder does not cancel the
                        // others; it is recorded and the batch carries on.
                        rejected.push(core.rejection(file, 'upload', fileError));
                        debug('File rejected during upload', {
                            file: getFileDisplayName(file) || file.name,
                            error: String(fileError)
                        });
                    }
                }

                if (!uploadedFiles.length) {
                    throw new Error(rejected.length
                        ? ('No file could be uploaded. First reason: ' + rejected[0].reason)
                        : 'No file was uploaded.');
                }

                finalMetadata = {
                    version: 1,
                    uploadedAt: new Date().toISOString(),
                    uploadedFiles: pendingPayload.uploadedFiles.slice(),
                    totalSizeBytes: pendingPayload.uploadedFiles.reduce(function (sum, fileInfo) {
                        return sum + Number(fileInfo && fileInfo.size ? fileInfo.size : 0);
                    }, 0),
                    rejectedFiles: rejected.slice(),
                    rejectedCount: rejected.length,
                    uploadState: {
                        status: 'completed',
                        stage: 'done',
                        updatedAt: new Date().toISOString(),
                        error: null
                    },
                    girder: lastSuccessfulGirder || pendingPayload.girder
                };
                finalMetadata.uploadSummary = buildUploadSummary(finalMetadata);

                await persistMetadata(textarea, finalMetadata, true);
                updateInfoState(finalMetadata);
                progressBar.style.width = '100%';
                statusLine.textContent = core.summarizeOutcome(uploadedFiles.length, 0, rejected)
                    + ' Saving form...';
                lockWidgetWithMessage('Upload complete. Saving form...');

                debug('Upload completed', {
                    fieldName: fieldName,
                    itemId: finalMetadata.girder.itemId,
                    fileCount: finalMetadata.uploadSummary ? finalMetadata.uploadSummary.fileCount : finalMetadata.uploadedFiles.length
                });
            } catch (error) {
                statusLine.textContent = 'Upload failed: ' + error.message;
                deidentifyProgressBar.style.width = '0%';
                progressBar.style.width = '0%';
                finalMetadata = {
                    version: 1,
                    uploadedAt: null,
                    uploadedFiles: pendingPayload && Array.isArray(pendingPayload.uploadedFiles)
                        ? pendingPayload.uploadedFiles
                        : [],
                    uploadState: {
                        status: 'failed',
                        stage: 'error',
                        updatedAt: new Date().toISOString(),
                        error: error.message
                    },
                    girder: {
                        baseApiUrl: uploadData && uploadData.girderUrl ? uploadData.girderUrl : null,
                        baseUrl: uploadData && uploadData.girderFrontchannelBaseUrl ? uploadData.girderFrontchannelBaseUrl : null,
                        rootCollectionId: uploadData && uploadData.rootCollectionId ? uploadData.rootCollectionId : null,
                        dagName: uploadData && uploadData.dagName ? uploadData.dagName : null,
                        recordId: uploadData && uploadData.recordId ? uploadData.recordId : null,
                        instanceId: uploadData && uploadData.instanceId ? uploadData.instanceId : null,
                        parentFolderId: uploadData && (uploadData.parentFolderId || uploadData.folderId) ? (uploadData.parentFolderId || uploadData.folderId) : null,
                        parentFolderUrl: uploadData && uploadData.girderFrontchannelBaseUrl && (uploadData.parentFolderId || uploadData.folderId)
                            ? (uploadData.girderFrontchannelBaseUrl + '/#folder/' + (uploadData.parentFolderId || uploadData.folderId))
                            : null,
                        folderId: uploadData && uploadData.folderId ? uploadData.folderId : null,
                        itemId: uploadData && uploadData.itemId ? uploadData.itemId : null,
                        uploadId: uploadData && uploadData.uploadId ? uploadData.uploadId : null,
                        fileId: null
                    }
                };
                finalMetadata.uploadSummary = buildUploadSummary(finalMetadata);
                await persistMetadata(textarea, finalMetadata, true);
                updateInfoState(finalMetadata);
                restoreUploadMode();
                debug('Upload failed', {
                    fieldName: fieldName,
                    error: error.message
                });
            } finally {
                setUploadingState(false);
                try {
                    saveFormInForeground();
                } catch (saveError) {
                    debug('Foreground save failed', { error: saveError.message });
                }
            }
        }

        if (!actionsAllowed) {
            if (canModify && !recordSaved && !currentMetadata) {
                lockWidgetWithMessage('Save this record before uploading: files are stored under its record id.');
                statusLine.style.display = '';
            } else {
                switchToInfoOnlyMode();
                if (currentMetadata) {
                    statusLine.style.display = 'none';
                } else {
                    statusLine.style.display = '';
                    statusLine.textContent = 'Uploads are disabled because this instrument is read-only or locked.';
                }
            }
            setDeleteAvailability(false);
        }

        dropzone.addEventListener('click', function () {
            if (isLocked) {
                return;
            }
            input.click();
        });

        dropzone.addEventListener('keydown', function (event) {
            if (isLocked) {
                return;
            }
            if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                input.click();
            }
        });

        input.addEventListener('change', function () {
            if (isLocked) {
                input.value = '';
                return;
            }
            if (input.files && input.files.length) {
                handleFiles(Array.from(input.files));
            }
            input.value = '';
        });

        deleteButton.addEventListener('click', async function () {
            if (isBusy || !isCompletedPayload(currentMetadata)) {
                return;
            }

            if (!window.confirm('Delete the uploaded contents from Girder and clear this field?')) {
                return;
            }

            setUploadingState(true);
            statusLine.style.display = '';
            progressContainer.style.display = '';
            deidentifyProgressBar.style.width = '0%';
            progressBar.style.width = '0%';
            statusLine.textContent = 'Deleting uploaded contents...';

            try {
                await deleteUploadContents(fieldName, currentMetadata);
                await clearMetadataAndSave(textarea);
                resetWidgetToEmpty('Uploaded contents deleted. You can upload again.');
                debug('Upload contents deleted', {
                    fieldName: fieldName
                });
            } catch (error) {
                switchToInfoOnlyMode();
                statusLine.style.display = '';
                statusLine.textContent = 'Delete failed: ' + error.message;
                updateInfoState(currentMetadata);
                debug('Upload delete failed', {
                    fieldName: fieldName,
                    error: error.message
                });
            } finally {
                setUploadingState(false);
            }
        });

        dropzone.addEventListener('dragover', function (event) {
            if (isLocked) {
                return;
            }
            event.preventDefault();
            dropzone.classList.add('dragover');
        });

        dropzone.addEventListener('dragleave', function () {
            dropzone.classList.remove('dragover');
        });

        dropzone.addEventListener('drop', function (event) {
            if (isLocked) {
                return;
            }
            event.preventDefault();
            dropzone.classList.remove('dragover');
            getDroppedFilesFromEvent(event)
                .then(function (droppedFiles) {
                    if (droppedFiles.length) {
                        handleFiles(droppedFiles);
                    }
                })
                .catch(function (error) {
                    statusLine.textContent = 'Upload failed: ' + error.message;
                    debug('Drop processing failed', { error: error.message });
                });
        });
    }

    onReady(function () {
        var config = window.GirderUploaderConfig;
        if (!config || !config.settings || !Array.isArray(config.fields)) {
            debug('Configuration missing or invalid, uploader not initialized.');
            return;
        }

        if (!window.GirderUploaderModule || typeof window.GirderUploaderModule.ajax !== 'function') {
            debug('Module AJAX object unavailable, uploader not initialized.');
            return;
        }

        config.settings.chunkSize = Number(config.settings.chunkSize || 10485760);
        config.settings.maxRetries = Number(config.settings.maxRetries || 3);
        config.settings.retryDelay = Number(config.settings.retryDelay || 1000);

        debug('Uploader configuration ready', {
            fields: config.fields,
            chunkSize: config.settings.chunkSize,
            maxRetries: config.settings.maxRetries,
            retryDelay: config.settings.retryDelay
        });

        config.fields.forEach(function (fieldName) {
            var selector = 'textarea[name="' + escapeSelector(fieldName) + '"]';
            var textarea = document.querySelector(selector);
            if (!textarea || textarea.getAttribute('data-girder-upload-enhanced') === '1') {
                debug('Skipping field initialization', {
                    fieldName: fieldName,
                    reason: textarea ? 'already-enhanced' : 'field-not-found'
                });
                return;
            }

            debug('Initializing field widget', { fieldName: fieldName });
            buildWidget(textarea, fieldName, config);
        });
    });
})();
