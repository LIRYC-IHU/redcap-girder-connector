/**
 * Pure browser-side logic for the Girder uploader: file selection, the
 * deidentification request contract, and the metadata payload stored in the
 * REDCap field.
 *
 * Nothing here touches the DOM, the network or REDCap globals, so it can be
 * exercised directly by the Node test suite (`tests/js`). The widget itself
 * lives in `girder-uploader.js` and consumes this module through
 * `window.GirderUploaderCore`.
 */
(function (root, factory) {
    'use strict';

    var api = factory();

    if (typeof module === 'object' && module && module.exports) {
        module.exports = api;
    }

    if (root) {
        root.GirderUploaderCore = api;
    }
})(typeof self !== 'undefined' ? self : this, function () {
    'use strict';

    /** How many files are described individually in the stored metadata. */
    var STORED_FILE_SAMPLE_LIMIT = 10;

    function sanitizePathPart(value, fallback) {
        var text = String(value || '').trim();
        if (!text) {
            return fallback;
        }

        return text.replace(/[\\/:*?"<>|]/g, '_').replace(/\s+/g, ' ').trim() || fallback;
    }

    /** Path shown to the user and used as the Girder-relative path. */
    function getFileDisplayName(file) {
        if (!file) {
            return '';
        }
        var customRelative = String(file.girderRelativePath || '').trim();
        if (customRelative) {
            return customRelative;
        }
        var relative = String(file.webkitRelativePath || '').trim();
        return relative || String(file.name || '').trim();
    }

    /**
     * Drop files that are noise rather than data: OS metadata, editor backups,
     * and ZIP archives nested inside a folder upload (only a single top-level
     * ZIP is expanded).
     */
    function shouldSkipUploadFile(file) {
        var displayName = getFileDisplayName(file);
        if (!displayName) {
            return true;
        }

        var parts = displayName.split(/[\\/]/);
        var baseName = parts.length ? parts[parts.length - 1] : displayName;
        if (!baseName) {
            return true;
        }

        if (baseName === '.DS_Store') {
            return true;
        }
        if (baseName.charAt(0) === '~') {
            return true;
        }
        if (baseName.charAt(0) === '.') {
            return true;
        }
        if (baseName.toLowerCase().endsWith('.zip') && parts.length > 1) {
            return true;
        }

        return false;
    }

    function isZipFile(file) {
        if (!file) {
            return false;
        }

        var name = String(file.name || '').toLowerCase();
        var type = String(file.type || '').toLowerCase();
        return name.endsWith('.zip') || type.indexOf('zip') >= 0;
    }

    /**
     * Best-effort DICOM guess used to pick the Girder item layout. The real
     * decision is made by the WASM deidentifier, which reads the file header.
     */
    function isLikelyDicomFile(file) {
        if (!file) {
            return false;
        }

        var mimeType = String(file.type || '').toLowerCase();
        if (mimeType && mimeType.indexOf('dicom') >= 0) {
            return true;
        }

        var displayName = getFileDisplayName(file);
        var baseName = String(displayName || file.name || '').split(/[\\/]/).pop() || '';
        return baseName.toLowerCase().endsWith('.dcm');
    }

    function normalizeUploadFiles(fileList) {
        var files = Array.isArray(fileList) ? fileList.slice() : Array.prototype.slice.call(fileList || []);
        files = files.filter(function (file) {
            return file && !shouldSkipUploadFile(file);
        });

        files.sort(function (a, b) {
            var aName = getFileDisplayName(a).toLowerCase();
            var bName = getFileDisplayName(b).toLowerCase();
            if (aName < bName) {
                return -1;
            }
            if (aName > bName) {
                return 1;
            }
            return 0;
        });

        return files;
    }

    /** `PatientName` written into deidentified DICOM files. */
    function buildPatientName(projectTitle, recordId) {
        var title = String(projectTitle || '').trim();
        var record = String(recordId || '').trim();
        return title ? (title + '^' + record) : record;
    }

    /** Message sent to the deidentification worker for a single file. */
    function buildDeidentifyRequest(file, config, bytes) {
        var settings = (config && config.settings) || {};
        var recordId = String((config && config.recordId) || '').trim();
        var projectTitle = String((config && config.projectTitle) || '').trim();
        var relativePath = getFileDisplayName(file) || String((file && file.name) || '');

        return {
            bytes: bytes,
            fileName: String((file && file.name) || relativePath || ''),
            relativePath: relativePath,
            recordId: recordId,
            projectTitle: projectTitle,
            patientName: buildPatientName(projectTitle, recordId),
            enableDicom: !!settings.deidentifyDicom,
            enableXml: !!settings.deidentifyXmlEcg,
            enableSchiller: !!settings.deidentifySchillerHolter
        };
    }

    /**
     * A `SKIP:` error means the file is not something we know how to
     * deidentify: it is dropped from the batch instead of failing the upload.
     */
    function isSkipError(error) {
        var message = error && error.message ? String(error.message) : String(error || '');
        return message.trim().indexOf('SKIP:') === 0;
    }

    function formatSize(bytes) {
        var size = Number(bytes || 0);
        if (!size) {
            return '0 B';
        }
        var units = ['B', 'KB', 'MB', 'GB', 'TB'];
        var index = 0;
        while (size >= 1024 && index < units.length - 1) {
            size /= 1024;
            index += 1;
        }
        return size.toFixed(index === 0 ? 0 : 2) + ' ' + units[index];
    }

    function buildFolderUrl(girderPayload) {
        if (!girderPayload || !girderPayload.parentFolderId || !girderPayload.baseUrl) {
            return null;
        }
        return String(girderPayload.baseUrl).replace(/\/$/, '') + '/#folder/' + String(girderPayload.parentFolderId);
    }

    function copyFileSummary(fileInfo) {
        if (!fileInfo || typeof fileInfo !== 'object') {
            return null;
        }

        return {
            name: fileInfo.name || fileInfo.originalName || 'uploaded-file',
            originalName: fileInfo.originalName || fileInfo.name || 'uploaded-file',
            size: Number(fileInfo.size || 0),
            mimeType: fileInfo.mimeType || 'application/octet-stream'
        };
    }

    function buildUploadSummary(payload) {
        var files = payload && Array.isArray(payload.uploadedFiles) ? payload.uploadedFiles : [];
        var existingSummary = payload && payload.uploadSummary && typeof payload.uploadSummary === 'object'
            ? payload.uploadSummary
            : {};
        var sampleFiles = Array.isArray(existingSummary.sampleFiles)
            ? existingSummary.sampleFiles.slice(0, STORED_FILE_SAMPLE_LIMIT).map(copyFileSummary).filter(Boolean)
            : files.slice(0, STORED_FILE_SAMPLE_LIMIT).map(copyFileSummary).filter(Boolean);
        var fileCount = Number(existingSummary.fileCount);
        if (!isFinite(fileCount) || fileCount < files.length) {
            fileCount = files.length;
        }
        var totalSizeBytes = Number(payload && payload.totalSizeBytes);
        if (!isFinite(totalSizeBytes) || totalSizeBytes < 0) {
            totalSizeBytes = Number(existingSummary.totalSizeBytes);
        }
        if (!isFinite(totalSizeBytes) || totalSizeBytes < 0) {
            totalSizeBytes = files.reduce(function (sum, fileInfo) {
                var value = Number(fileInfo && fileInfo.size ? fileInfo.size : 0);
                return sum + (isNaN(value) ? 0 : value);
            }, 0);
        }

        return {
            mode: 'summary',
            fileCount: fileCount,
            totalSizeBytes: totalSizeBytes,
            sampleLimit: STORED_FILE_SAMPLE_LIMIT,
            sampleFiles: sampleFiles,
            omittedFileCount: Math.max(0, fileCount - sampleFiles.length)
        };
    }

    function normalizeMetadataPayload(payload) {
        if (!payload || typeof payload !== 'object') {
            return null;
        }

        var normalized = JSON.parse(JSON.stringify(payload));
        if (!normalized.girder || typeof normalized.girder !== 'object') {
            normalized.girder = {};
        }
        if (!normalized.uploadSummary || typeof normalized.uploadSummary !== 'object') {
            normalized.uploadSummary = buildUploadSummary(normalized);
        }
        if (!Array.isArray(normalized.uploadedFiles)) {
            normalized.uploadedFiles = [];
        }
        if (!isFinite(Number(normalized.totalSizeBytes)) || Number(normalized.totalSizeBytes) < 0) {
            normalized.totalSizeBytes = Number(normalized.uploadSummary.totalSizeBytes || 0);
        }

        if (!normalized.girder.parentFolderUrl) {
            normalized.girder.parentFolderUrl = buildFolderUrl(normalized.girder);
        }

        return normalized;
    }

    /**
     * REDCap fields hold a bounded amount of text, so only a sample of the file
     * list is stored; the full listing is re-read from Girder on demand.
     */
    function buildStoredMetadataPayload(payload) {
        if (!payload || typeof payload !== 'object') {
            return payload;
        }

        var summary = buildUploadSummary(payload);
        var girder = payload.girder && typeof payload.girder === 'object' ? payload.girder : {};
        return {
            version: payload.version || 1,
            encoding: 'summary-v1',
            uploadedAt: payload.uploadedAt || null,
            uploadedFiles: summary.sampleFiles,
            uploadSummary: summary,
            totalSizeBytes: summary.totalSizeBytes,
            uploadState: payload.uploadState || null,
            girder: {
                baseUrl: girder.baseUrl || null,
                parentFolderId: girder.parentFolderId || null,
                parentFolderUrl: girder.parentFolderUrl || buildFolderUrl(girder)
            }
        };
    }

    function isCompletedPayload(payload) {
        return !!(payload
            && payload.uploadState
            && String(payload.uploadState.status || '') === 'completed'
            && payload.girder
            && payload.girder.parentFolderId);
    }

    function isFailedPayload(payload) {
        return !!(payload
            && payload.uploadState
            && String(payload.uploadState.status || '') === 'failed');
    }

    return {
        STORED_FILE_SAMPLE_LIMIT: STORED_FILE_SAMPLE_LIMIT,
        sanitizePathPart: sanitizePathPart,
        getFileDisplayName: getFileDisplayName,
        shouldSkipUploadFile: shouldSkipUploadFile,
        isZipFile: isZipFile,
        isLikelyDicomFile: isLikelyDicomFile,
        normalizeUploadFiles: normalizeUploadFiles,
        buildPatientName: buildPatientName,
        buildDeidentifyRequest: buildDeidentifyRequest,
        isSkipError: isSkipError,
        formatSize: formatSize,
        buildFolderUrl: buildFolderUrl,
        copyFileSummary: copyFileSummary,
        buildUploadSummary: buildUploadSummary,
        normalizeMetadataPayload: normalizeMetadataPayload,
        buildStoredMetadataPayload: buildStoredMetadataPayload,
        isCompletedPayload: isCompletedPayload,
        isFailedPayload: isFailedPayload
    };
});
