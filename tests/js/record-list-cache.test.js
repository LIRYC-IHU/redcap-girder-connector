'use strict';

/**
 * REDCap's record-list cache redirect: while a project's cache is not built,
 * the module's first AJAX POST is redirected to the cache builder and the EM
 * framework ends up parsing an HTML page as JSON. The widget detects that,
 * builds the cache and sends the request again, once.
 */

const test = require('node:test');
const assert = require('node:assert/strict');

const core = require('../../src/js/girder-uploader-core.js');

/** The error the EM framework rethrows when the response is an HTML page. */
function htmlParseError() {
    try {
        JSON.parse('<!DOCTYPE html><html><head><title>REDCap</title></head></html>');
    } catch (error) {
        return error;
    }
    throw new Error('JSON.parse accepted HTML');
}

test('a JSON parse failure on an HTML page is detected', () => {
    assert.ok(core.isNonJsonResponseError(htmlParseError()));
});

test('the parse failure is recognized from any browser\'s wording', () => {
    // Chrome, Firefox and Safari, should the error lose its SyntaxError type.
    assert.ok(core.isNonJsonResponseError(new Error('Unexpected token \'<\', "<!DOCTYPE "... is not valid JSON')));
    assert.ok(core.isNonJsonResponseError(new Error('JSON.parse: unexpected character at line 1 column 1 of the JSON data')));
    assert.ok(core.isNonJsonResponseError(new Error('JSON Parse error: Unrecognized token \'<\'')));
});

test('errors from the module or the framework are not mistaken for it', () => {
    // The module's own failures, the framework's "logged out" message (a
    // string) and network errors must surface as they are.
    assert.equal(core.isNonJsonResponseError(new Error('Girder request failed (500).')), false);
    assert.equal(core.isNonJsonResponseError('Your session has expired. Please log in again.'), false);
    assert.equal(core.isNonJsonResponseError(new TypeError('Failed to fetch')), false);
    assert.equal(core.isNonJsonResponseError(null), false);
    assert.equal(core.isNonJsonResponseError(undefined), false);
});

test('a request that got HTML back is sent again after the cache is built', async () => {
    const calls = [];
    const result = await core.retryAfterCacheRebuild(
        async () => {
            calls.push('request');
            if (calls.length === 1) {
                throw htmlParseError();
            }
            return { ok: true };
        },
        async () => {
            calls.push('rebuild');
        }
    );
    assert.deepEqual(result, { ok: true });
    assert.deepEqual(calls, ['request', 'rebuild', 'request']);
});

test('a successful request is sent once and the cache is left alone', async () => {
    let rebuilds = 0;
    const result = await core.retryAfterCacheRebuild(async () => ({ ok: true }), async () => {
        rebuilds += 1;
    });
    assert.deepEqual(result, { ok: true });
    assert.equal(rebuilds, 0);
});

test('other failures are not retried', async () => {
    let requests = 0;
    let rebuilds = 0;
    await assert.rejects(
        core.retryAfterCacheRebuild(
            async () => {
                requests += 1;
                throw new Error('Record is locked.');
            },
            async () => {
                rebuilds += 1;
            }
        ),
        /Record is locked/
    );
    assert.equal(requests, 1);
    assert.equal(rebuilds, 0);
});

test('the request is retried only once', async () => {
    let requests = 0;
    await assert.rejects(
        core.retryAfterCacheRebuild(
            async () => {
                requests += 1;
                throw htmlParseError();
            },
            async () => {}
        ),
        SyntaxError
    );
    assert.equal(requests, 2);
});

test('a failing cache build still lets the retry report the outcome', async () => {
    let requests = 0;
    const result = await core.retryAfterCacheRebuild(
        async () => {
            requests += 1;
            if (requests === 1) {
                throw htmlParseError();
            }
            return { ok: true };
        },
        async () => {
            throw new TypeError('Failed to fetch');
        }
    );
    assert.deepEqual(result, { ok: true });
    assert.equal(requests, 2);
});
