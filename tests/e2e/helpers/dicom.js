// Minimal DICOM Part 10 reader for assertions: top-level elements of the data set
// (sequences are skipped), explicit or implicit VR little endian. Enough to check what
// the deidentifier wrote; not a general-purpose parser.

const LONG_VRS = new Set(['OB', 'OD', 'OF', 'OL', 'OV', 'OW', 'SQ', 'SV', 'UC', 'UN', 'UR', 'UT', 'UV']);
const UNDEFINED = 0xffffffff;

const TAGS = {
    PatientName: 0x00100010,
    PatientID: 0x00100020,
    PatientBirthDate: 0x00100030,
    PatientSex: 0x00100040,
    PatientAge: 0x00101010,
    StudyDate: 0x00080020,
    SeriesDate: 0x00080021,
    ContentDate: 0x00080023,
    StudyTime: 0x00080030,
    InstitutionName: 0x00080080,
    StudyInstanceUID: 0x0020000d,
    DeidentificationMethod: 0x00120063,
    PatientIdentityRemoved: 0x00120062,
    LongitudinalTemporalInformationModified: 0x00280303,
    TransferSyntaxUID: 0x00020010,
};

function readTag(buf, pos) {
    return ((buf.readUInt16LE(pos) << 16) | buf.readUInt16LE(pos + 2)) >>> 0;
}

/** Skip an undefined-length sequence or item body, returning the position after it. */
function skipUndefined(buf, pos, explicit) {
    while (pos + 8 <= buf.length) {
        const tag = readTag(buf, pos);
        if (tag === 0xfffee0dd || tag === 0xfffee00d) {
            return pos + 8; // sequence / item delimiter
        }
        if (tag === 0xfffee000) {
            const len = buf.readUInt32LE(pos + 4);
            pos += 8;
            pos = len === UNDEFINED ? skipUndefined(buf, pos, explicit) : pos + len;
            continue;
        }
        pos = readElement(buf, pos, explicit).next;
    }
    return pos;
}

function readElement(buf, pos, explicit) {
    const tag = readTag(buf, pos);
    let vr = null;
    let len;
    let valuePos;
    if (explicit && (tag >>> 16) !== 0xfffe) {
        vr = buf.toString('latin1', pos + 4, pos + 6);
        if (LONG_VRS.has(vr)) {
            len = buf.readUInt32LE(pos + 8);
            valuePos = pos + 12;
        } else {
            len = buf.readUInt16LE(pos + 6);
            valuePos = pos + 8;
        }
    } else {
        len = buf.readUInt32LE(pos + 4);
        valuePos = pos + 8;
    }
    if (len === UNDEFINED) {
        return { tag, vr, value: null, next: skipUndefined(buf, valuePos, explicit) };
    }
    return { tag, vr, value: buf.subarray(valuePos, valuePos + len), next: valuePos + len };
}

/**
 * Parse a Part 10 buffer. Returns { elements: Map<tag, {vr, value: Buffer|null}>,
 * get(name) -> trimmed string | null (absent) | '' (present, empty), has(name) }.
 */
function parseDicom(buffer) {
    const buf = Buffer.from(buffer);
    if (buf.toString('latin1', 128, 132) !== 'DICM') {
        throw new Error('not a DICOM Part 10 file (no DICM at offset 128)');
    }
    const elements = new Map();
    let pos = 132;
    // File meta group: always explicit VR little endian.
    while (pos + 8 <= buf.length && (buf.readUInt16LE(pos) === 0x0002)) {
        const el = readElement(buf, pos, true);
        elements.set(el.tag, el);
        pos = el.next;
    }
    const ts = elements.has(TAGS.TransferSyntaxUID)
        ? elements.get(TAGS.TransferSyntaxUID).value.toString('latin1').replace(/[\0 ]+$/, '')
        : '1.2.840.10008.1.2.1';
    if (ts !== '1.2.840.10008.1.2' && ts !== '1.2.840.10008.1.2.1') {
        throw new Error(`unsupported transfer syntax ${ts}`);
    }
    const explicit = ts !== '1.2.840.10008.1.2';
    while (pos + 8 <= buf.length) {
        const el = readElement(buf, pos, explicit);
        elements.set(el.tag, el);
        pos = el.next;
    }

    const lookup = (name) => {
        const tag = typeof name === 'number' ? name : TAGS[name];
        if (tag === undefined) {
            throw new Error(`unknown tag name ${name}`);
        }
        return elements.get(tag);
    };
    return {
        elements,
        has: (name) => lookup(name) !== undefined,
        get: (name) => {
            const el = lookup(name);
            if (!el) {
                return null;
            }
            return el.value ? el.value.toString('latin1').replace(/[\0 ]+$/, '').trim() : '';
        },
    };
}

/** 'YYYYMMDD' -> days since 1970-01-01 (UTC). */
function daNumber(da) {
    const m = /^(\d{4})(\d{2})(\d{2})$/.exec(da || '');
    if (!m) {
        throw new Error(`not a DA value: ${JSON.stringify(da)}`);
    }
    return Date.UTC(+m[1], +m[2] - 1, +m[3]) / 86_400_000;
}

/** original - shifted, in days (positive when the date moved back). */
function daysShiftedBack(originalDa, shiftedDa) {
    return daNumber(originalDa) - daNumber(shiftedDa);
}

module.exports = { parseDicom, daNumber, daysShiftedBack, TAGS };
