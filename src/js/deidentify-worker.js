let wasmReady = null;

async function ensureWasm() {
    if (wasmReady) {
        return wasmReady;
    }

    wasmReady = import('../wasm/dicom_deid/pkg/dicom_deid.js')
        .then(async (module) => {
            const init = module.default;
            await init();
            return module;
        });

    return wasmReady;
}

/**
 * Pseudonym computations in progress, by id. Each holds the blind of one NIP
 * between the request to REDCap and its answer; it never leaves the worker.
 */
const pseudonymSessions = new Map();
let nextSessionId = 1;

function settingsFlags(data) {
    return [!!data.enableDicom, !!data.enableXml, !!data.enableSchiller];
}

const handlers = {
    /** Recognize a file and read who it belongs to, before anything else. */
    inspect(module, data) {
        const result = module.inspect(
            new Uint8Array(data.bytes || []),
            String(data.fileName || data.relativePath || '').trim(),
            ...settingsFlags(data)
        );
        return {
            format: result.formatName(),
            deidentified: result.deidentified(),
            nip: result.nip(),
            rawNip: result.rawNip(),
            familyName: result.familyName(),
            givenName: result.givenName(),
            birthDate: result.birthDate(),
            seriesUid: result.seriesUid(),
            modality: result.modality(),
            seriesDescription: result.seriesDescription()
        };
    },

    'pseudonym-start'(module, data) {
        const session = new module.PseudonymSession(String(data.domain || ''), String(data.nip || ''));
        const sessionId = nextSessionId;
        nextSessionId += 1;
        pseudonymSessions.set(sessionId, session);
        return { sessionId, blindedElement: session.blindedElement() };
    },

    'pseudonym-finish'(module, data) {
        const session = pseudonymSessions.get(Number(data.sessionId));
        if (!session) {
            throw new Error('Unknown pseudonym session.');
        }
        pseudonymSessions.delete(Number(data.sessionId));
        try {
            const result = session.finish(
                String(data.evaluatedElement || ''),
                String(data.proof || ''),
                String(data.publicKey || ''),
                String(data.recordId || '').trim()
            );
            return {
                fingerprint: result.fingerprint(),
                dateOffsetDays: result.dateOffsetDays(),
                uidKey: result.uidKey()
            };
        } finally {
            session.free();
        }
    },

    deidentify(module, data) {
        const recordId = String(data.recordId || '').trim();
        const projectTitle = String(data.projectTitle || '').trim();
        // The main thread builds the patient name (see girder-uploader-core.js);
        // the fallback keeps the worker usable on its own.
        const patientName = String(data.patientName || '').trim()
            || (projectTitle ? (projectTitle + '^' + recordId) : recordId);
        // Dates move by the patient's offset and UIDs are rewritten under the
        // patient's key. Without them the real values would stay: refuse instead.
        const dateOffsetDays = data.dateOffsetDays;
        const uidKey = typeof data.uidKey === 'string' ? data.uidKey : '';
        if ((data.enableDicom || data.enableXml) && !Number.isInteger(dateOffsetDays)) {
            throw new Error('The date offset of this patient is missing; deidentification cannot proceed.');
        }
        if (data.enableDicom && !uidKey) {
            throw new Error('The UID key of this patient is missing; deidentification cannot proceed.');
        }
        const output = module.deidentify(
            new Uint8Array(data.bytes || []),
            String(data.fileName || data.relativePath || '').trim(),
            recordId,
            patientName,
            Number.isInteger(dateOffsetDays) ? dateOffsetDays : 0,
            uidKey,
            ...settingsFlags(data)
        );
        const outputBytes = output.bytes();
        return {
            result: {
                bytes: outputBytes.buffer,
                format: output.formatName(),
                mimeType: output.mimeType()
            },
            transfer: [outputBytes.buffer]
        };
    }
};

self.onmessage = async (event) => {
    const data = event && event.data ? event.data : {};
    const id = Number(data.id || 0);

    try {
        const module = await ensureWasm();
        const handler = handlers[data.type || 'deidentify'];
        if (!handler) {
            throw new Error('Unknown worker request: ' + String(data.type));
        }
        const answer = handler(module, data);
        const result = answer && answer.result ? answer.result : answer;
        self.postMessage(Object.assign({ id, ok: true }, result), (answer && answer.transfer) || []);
    } catch (error) {
        self.postMessage({
            id,
            ok: false,
            // wasm-bindgen errors arrive as plain strings.
            error: error && error.message ? error.message : String(error)
        });
    }
};
