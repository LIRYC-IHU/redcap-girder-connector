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

    /** How many set-aside files the review window names. */
    var REVIEW_SET_ASIDE_LIMIT = 10;

    /**
     * A file the batch could not take, with the reason.
     *
     * One bad file must not cancel the rest — a DICOM archive routinely carries
     * a DICOMDIR, a README or a viewer's XML alongside the images — but a
     * rejection must never be silent either: the user has to be able to see
     * which file did not make it and why, after the fact.
     */
    function rejection(file, stage, reason) {
        return {
            name: getFileDisplayName(file) || String((file && file.name) || 'unknown file'),
            stage: String(stage || 'deidentification'),
            reason: String((reason && reason.message) || reason || 'Unknown error')
        };
    }

    /**
     * Rejections as stored in the REDCap field. Their name is the path on the
     * uploader's disk, which can carry the patient's identity, so it stays out
     * unless the project keeps those names in Girder anyway.
     */
    function storedRejections(rejected, keepNames) {
        return (rejected || []).map(function (entry, index) {
            if (keepNames) {
                return entry;
            }
            var extension = /\.[A-Za-z0-9]{1,8}$/.exec(String(entry.name || ''));
            return Object.assign({}, entry, {
                name: 'File ' + (index + 1) + (extension ? ' (' + extension[0] + ')' : '')
            });
        });
    }

    function copyRejection(entry) {
        if (!entry || typeof entry !== 'object') {
            return null;
        }

        return {
            name: entry.name || 'unknown file',
            stage: entry.stage || 'deidentification',
            reason: entry.reason || 'Unknown error'
        };
    }

    /** One line summarizing what the batch left behind. */
    function summarizeOutcome(uploadedCount, skippedCount, rejected) {
        var parts = [uploadedCount + ' file' + (uploadedCount === 1 ? '' : 's') + ' uploaded'];
        if (skippedCount > 0) {
            parts.push(skippedCount + ' skipped (unsupported format)');
        }
        var rejectedCount = Array.isArray(rejected) ? rejected.length : Number(rejected || 0);
        if (rejectedCount > 0) {
            parts.push(rejectedCount + ' rejected');
        }

        return parts.join(', ') + '.';
    }

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
     * Is this file DICOM, for the purpose of picking the Girder item layout?
     *
     * The deidentifier's verdict wins when there is one: it reads the bytes,
     * and DICOM arrives under `.dcm`, `.vim` or no extension at all, so the
     * name proves nothing. The MIME type and the extension are the fallback
     * for files that have not been through the worker.
     */
    function isLikelyDicomFile(file) {
        if (!file) {
            return false;
        }

        if (String(file.deidentifiedFormat || '').toLowerCase() === 'dicom') {
            return true;
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

    function deidentificationFlags(config) {
        var settings = (config && config.settings) || {};
        return {
            enableDicom: !!settings.deidentifyDicom,
            enableXml: !!settings.deidentifyXmlEcg,
            enableSchiller: !!settings.deidentifySchillerHolter
        };
    }

    /** Ask the worker what a file is and who it belongs to, before deidentifying. */
    function buildInspectRequest(file, config, bytes) {
        return Object.assign({
            type: 'inspect',
            bytes: bytes,
            fileName: String((file && file.name) || getFileDisplayName(file) || '')
        }, deidentificationFlags(config));
    }

    var MISSING_NIP_REASON = 'The file carries no patient identifier (NIP): its dates cannot be shifted '
        + 'and it cannot be checked against the record.';

    /**
     * Sort inspected files: the ones to deidentify are grouped by patient, and
     * those that would need a pseudonym but carry no NIP are set apart. Files
     * sent as they are (their format's deidentification is off) need neither.
     *
     * `entries` are `{ file, inspection }`, `inspection` being the worker's
     * answer to an inspect request.
     */
    function groupByPatient(entries) {
        var byNip = new Map();
        var missingNip = [];
        var passthrough = [];

        (entries || []).forEach(function (entry) {
            var inspection = entry.inspection || {};
            if (!inspection.deidentified) {
                passthrough.push(entry);
                return;
            }
            var nip = String(inspection.nip || '');
            if (!nip) {
                missingNip.push(entry);
                return;
            }
            if (!byNip.has(nip)) {
                byNip.set(nip, {
                    nip: nip,
                    rawNip: '',
                    familyName: '',
                    givenName: '',
                    birthDate: '',
                    entries: []
                });
            }
            var patient = byNip.get(nip);
            patient.entries.push(entry);
            // Some files of a series carry the name or birth date and others not.
            patient.rawNip = patient.rawNip || String(inspection.rawNip || '');
            patient.familyName = patient.familyName || String(inspection.familyName || '');
            patient.givenName = patient.givenName || String(inspection.givenName || '');
            patient.birthDate = patient.birthDate || String(inspection.birthDate || '');
        });

        return {
            patients: Array.from(byNip.values()),
            missingNip: missingNip,
            passthrough: passthrough
        };
    }

    function plural(count, word) {
        return count + ' ' + word + (count === 1 ? '' : 's');
    }

    /**
     * What a batch holds, for the uploader: DICOM series (modality,
     * description, file count), ECGs and Holters.
     */
    function summarizeTransfer(entries) {
        var series = new Map();
        var summary = { dicomFiles: 0, series: [], ecgCount: 0, holterCount: 0, otherCount: 0 };

        (entries || []).forEach(function (entry) {
            var inspection = entry.inspection || {};
            var format = String(inspection.format || '');
            if (format === 'dicom') {
                summary.dicomFiles += 1;
                var uid = String(inspection.seriesUid || '');
                var key = uid || 'no-series';
                if (!series.has(key)) {
                    series.set(key, {
                        modality: String(inspection.modality || ''),
                        description: String(inspection.seriesDescription || ''),
                        fileCount: 0
                    });
                }
                series.get(key).fileCount += 1;
            } else if (format === 'xml') {
                summary.ecgCount += 1;
            } else if (format === 'schiller') {
                summary.holterCount += 1;
            } else {
                summary.otherCount += 1;
            }
        });

        summary.series = Array.from(series.values());
        return summary;
    }

    /**
     * What the fingerprint check found, as the uploader reads it. Earlier
     * uploads with no fingerprint on the record predate fingerprints: nothing
     * can be said about them, and saying "first upload" would be false.
     */
    function describePatientCheck(status, existingUploads) {
        var earlier = plural(Number(existingUploads) || 0, 'earlier upload');
        switch (status) {
            case 'matched':
                return {
                    level: 'ok',
                    title: 'Same patient signature',
                    text: 'Consistent with the ' + earlier + ' of this record.'
                };
            case 'new':
                return Number(existingUploads) > 0
                    ? {
                        level: 'warning',
                        title: 'Earlier uploads cannot be compared',
                        text: 'This record holds ' + earlier + ' from a version of the module without patient signature. '
                            + 'Check that they belong to this patient. Their dates are on another time scale, so intervals '
                            + 'with this upload are meaningless. The signature of this upload will be recorded.'
                    }
                    : {
                        level: 'info',
                        title: 'First upload for this record',
                        text: "The patient's signature will be recorded."
                    };
            case 'mismatch':
                return {
                    level: 'danger',
                    title: 'Patient signature mismatch',
                    text: 'These files do not match the earlier uploads of this record: they probably belong to another patient. '
                        + 'If you upload anyway, the alert is kept in the project log and on this field.'
                };
            case 'unchecked':
                return {
                    level: 'info',
                    title: 'Patient check disabled',
                    text: 'This project does not compare patient signatures.'
                };
            default:
                return null;
        }
    }

    /**
     * What the uploader reviews before anything is sent: the patient as read
     * from the files, what the batch holds, and what the fingerprint check
     * found. Shown in the browser only.
     */
    function buildTransferReview(options) {
        var patient = options.patient;
        var summary = options.summary;
        var check = describePatientCheck(options.checkStatus, options.existingUploads);
        var content = summary.series.map(function (entry) {
            return {
                type: 'DICOM',
                detail: [entry.modality || 'Unknown modality', entry.description].filter(Boolean).join(' - '),
                files: entry.fileCount
            };
        });
        if (summary.ecgCount) {
            content.push({ type: 'ECG', detail: '', files: summary.ecgCount });
        }
        if (summary.holterCount) {
            content.push({ type: 'Holter', detail: '', files: summary.holterCount });
        }
        if (summary.otherCount) {
            content.push({ type: 'Other', detail: '', files: summary.otherCount });
        }

        var notes = [];
        if (options.passthroughCount) {
            notes.push(plural(options.passthroughCount, 'file')
                + ' sent without deidentification (disabled for their format).');
        }
        if (options.setAsideCount) {
            notes.push(plural(options.setAsideCount, 'file') + ' set aside.');
        }
        var setAside = options.setAside || [];

        return {
            recordId: String(options.recordId || ''),
            patient: patient ? [
                { label: 'Last name', value: patient.familyName || '(not given)' },
                { label: 'First name', value: patient.givenName || '(not given)' },
                { label: 'Date of birth', value: patient.birthDate || '(not given)' },
                { label: 'NIP', value: patient.rawNip || patient.nip }
            ] : null,
            content: content,
            notes: notes,
            // Their real names, in this window only: see storedRejections.
            setAside: setAside.slice(0, REVIEW_SET_ASIDE_LIMIT).map(function (entry) {
                return entry.name + ': ' + entry.reason;
            }),
            setAsideOmitted: Math.max(0, setAside.length - REVIEW_SET_ASIDE_LIMIT),
            check: check,
            mismatch: options.checkStatus === 'mismatch',
            confirmLabel: options.checkStatus === 'mismatch' ? 'Upload anyway' : 'Upload'
        };
    }

    /**
     * One upload, one patient: a mixed batch is refused as a whole. This message
     * is stored in the REDCap field, so it must name nobody.
     */
    function buildMultiplePatientsError(patients) {
        return 'These files belong to ' + patients.length
            + ' different patients. Upload one patient at a time. Nothing was uploaded.';
    }

    /**
     * The worker's URL on the page's own origin. REDCap builds module URLs from
     * its configured base URL, and a browser refuses a worker from any other
     * origin, so a REDCap reached under another name, an alias or an IP address,
     * could not start one. The same server answers on both.
     */
    function sameOriginUrl(url, pageUrl) {
        var target = new URL(url, pageUrl);
        return new URL(pageUrl).origin + target.pathname + target.search;
    }

    /** Message sent to the deidentification worker for a single file. */
    function buildDeidentifyRequest(file, config, bytes, keys) {
        var settings = (config && config.settings) || {};
        var recordId = String((config && config.recordId) || '').trim();
        var projectTitle = String((config && config.projectTitle) || '').trim();
        var relativePath = getFileDisplayName(file) || String((file && file.name) || '');

        return {
            type: 'deidentify',
            bytes: bytes,
            fileName: String((file && file.name) || relativePath || ''),
            relativePath: relativePath,
            recordId: recordId,
            projectTitle: projectTitle,
            patientName: buildPatientName(projectTitle, recordId),
            // Null makes the worker refuse rather than keep real dates and UIDs.
            dateOffsetDays: keys && Number.isInteger(keys.dateOffsetDays) ? keys.dateOffsetDays : null,
            uidKey: keys && typeof keys.uidKey === 'string' ? keys.uidKey : null,
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

    /**
     * While a project's record-list cache is not built (a new project, or one
     * whose cache REDCap's cron dropped after inactivity), REDCap redirects the
     * first request into the project to its cache builder. When that request is
     * a module AJAX POST, the browser follows the redirect and the EM framework
     * fails to parse the HTML page it lands on: JSON.parse throws a SyntaxError
     * ("Unexpected token '<'" in Chrome, "JSON.parse: unexpected character" in
     * Firefox, "JSON Parse error" in Safari). Errors reported by the module or
     * the framework itself are plain strings or Errors, not SyntaxErrors.
     */
    function isNonJsonResponseError(error) {
        if (!error || typeof error !== 'object') {
            return false;
        }
        if (error.name === 'SyntaxError') {
            return true;
        }
        return /is not valid JSON|JSON\.parse|JSON Parse error/i.test(String(error.message || ''));
    }

    /**
     * Run `request`; if its response was not JSON, run `rebuildCache` and send
     * the request once more. A redirected request never reached the module, so
     * sending it again is safe. A failing rebuild is ignored: the second attempt
     * reports whatever is still wrong.
     */
    function retryAfterCacheRebuild(request, rebuildCache) {
        return Promise.resolve().then(request).catch(function (error) {
            if (!isNonJsonResponseError(error)) {
                throw error;
            }
            return Promise.resolve()
                .then(rebuildCache)
                .catch(function () {})
                .then(request);
        });
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

    /** The rejected list carried in the stored metadata, bounded like the files. */
    function buildRejectedSample(payload) {
        var rejected = payload && Array.isArray(payload.rejectedFiles) ? payload.rejectedFiles : [];
        var count = Number(payload && payload.rejectedCount);
        if (!isFinite(count) || count < rejected.length) {
            count = rejected.length;
        }

        return {
            rejectedFiles: rejected.slice(0, STORED_FILE_SAMPLE_LIMIT).map(copyRejection).filter(Boolean),
            rejectedCount: count
        };
    }

    /**
     * Carry the rejections of `previous` into a refreshed snapshot.
     *
     * Rejections record what happened while uploading; Girder has no idea they
     * exist, so a snapshot rebuilt from the folder listing would silently drop
     * them and the user would lose the trace of the files left behind.
     */
    function carryRejectionsForward(previous, refreshed) {
        if (!refreshed || typeof refreshed !== 'object') {
            return refreshed;
        }

        var carried = buildRejectedSample(previous || {});
        if (!carried.rejectedCount) {
            return refreshed;
        }

        refreshed.rejectedFiles = carried.rejectedFiles;
        refreshed.rejectedCount = carried.rejectedCount;
        return refreshed;
    }

    /**
     * Carry a confirmed fingerprint mismatch into a refreshed snapshot: like
     * rejections, Girder has no idea it happened.
     */
    function carryPatientAlertForward(previous, refreshed) {
        if (refreshed && typeof refreshed === 'object' && previous && previous.patientAlert) {
            refreshed.patientAlert = previous.patientAlert;
        }
        return refreshed;
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

        if (!Array.isArray(normalized.rejectedFiles)) {
            normalized.rejectedFiles = [];
        }
        if (!isFinite(Number(normalized.rejectedCount)) || Number(normalized.rejectedCount) < normalized.rejectedFiles.length) {
            normalized.rejectedCount = normalized.rejectedFiles.length;
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
        var rejected = buildRejectedSample(payload);
        var girder = payload.girder && typeof payload.girder === 'object' ? payload.girder : {};
        return {
            version: payload.version || 1,
            encoding: 'summary-v1',
            uploadedAt: payload.uploadedAt || null,
            uploadedFiles: summary.sampleFiles,
            uploadSummary: summary,
            totalSizeBytes: summary.totalSizeBytes,
            rejectedFiles: rejected.rejectedFiles,
            rejectedCount: rejected.rejectedCount,
            patientAlert: payload.patientAlert || null,
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
        rejection: rejection,
        storedRejections: storedRejections,
        summarizeOutcome: summarizeOutcome,
        buildRejectedSample: buildRejectedSample,
        carryRejectionsForward: carryRejectionsForward,
        carryPatientAlertForward: carryPatientAlertForward,
        buildPatientName: buildPatientName,
        buildDeidentifyRequest: buildDeidentifyRequest,
        sameOriginUrl: sameOriginUrl,
        buildInspectRequest: buildInspectRequest,
        groupByPatient: groupByPatient,
        summarizeTransfer: summarizeTransfer,
        describePatientCheck: describePatientCheck,
        buildTransferReview: buildTransferReview,
        buildMultiplePatientsError: buildMultiplePatientsError,
        MISSING_NIP_REASON: MISSING_NIP_REASON,
        isSkipError: isSkipError,
        isNonJsonResponseError: isNonJsonResponseError,
        retryAfterCacheRebuild: retryAfterCacheRebuild,
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
