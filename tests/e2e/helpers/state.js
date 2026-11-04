// Stack state written by provision.sh (.state/state.json) and the fixture manifest.
const fs = require('node:fs');
const path = require('node:path');

const E2E_DIR = path.resolve(__dirname, '..');
const STATE_FILE = path.join(E2E_DIR, '.state', 'state.json');
const FIXTURES_DIR = path.join(E2E_DIR, 'fixtures', 'out');

let cached = null;

function loadState() {
    if (!cached) {
        if (!fs.existsSync(STATE_FILE)) {
            throw new Error(`${STATE_FILE} is missing: run ./run.sh provision`);
        }
        cached = JSON.parse(fs.readFileSync(STATE_FILE, 'utf8'));
    }
    return cached;
}

function fixturePath(name) {
    const file = path.join(FIXTURES_DIR, name);
    if (!fs.existsSync(file)) {
        throw new Error(`fixture ${name} not found in ${FIXTURES_DIR}: run ./run.sh fixtures`);
    }
    return file;
}

function fixtureManifest() {
    return JSON.parse(fs.readFileSync(fixturePath('manifest.json'), 'utf8'));
}

module.exports = { loadState, fixturePath, fixtureManifest, E2E_DIR };
