// @ts-check
const { defineConfig, devices } = require('@playwright/test');

module.exports = defineConfig({
    testDir: './specs',
    globalSetup: './helpers/global-setup.js',
    // The scenarios share one REDCap project and one Girder collection.
    fullyParallel: false,
    workers: 1,
    retries: 0,
    timeout: 120_000,
    expect: { timeout: 15_000 },
    reporter: [['list'], ['html', { open: 'never' }]],
    use: {
        ...devices['Desktop Chrome'],
        // Chromium build 1217, from the local Playwright cache (never `playwright install`).
        browserName: 'chromium',
        headless: true,
        baseURL: 'http://localhost:18081',
        trace: 'retain-on-failure',
        screenshot: 'only-on-failure',
    },
});
