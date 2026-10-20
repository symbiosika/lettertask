// E2E tests: real UI + real Rust service layer (devserver) against the fake Graph demo data.
const { defineConfig } = require('@playwright/test');

module.exports = defineConfig({
  testDir: './tests',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  workers: 1, // all tests share one backend with mutable demo data
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: 'http://127.0.0.1:8080',
    viewport: { width: 1366, height: 820 },
    locale: 'de-DE',
    timezoneId: 'Europe/Berlin',
    trace: 'retain-on-failure',
  },
  webServer: {
    // Build first, then exec the binary itself so Playwright's shutdown signal
    // reaches the server (with `cargo run` it lingered and the next run reused
    // its already modified demo data).
    command: 'cargo build -q -p devserver --bin devserver && exec target/debug/devserver --demo --port 8080',
    cwd: '../..',
    url: 'http://127.0.0.1:8080/',
    timeout: 600_000,
    // Always start fresh: the tests rely on pristine demo data.
    reuseExistingServer: false,
    stdout: 'ignore',
    stderr: 'pipe',
  },
});
