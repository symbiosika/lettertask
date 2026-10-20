// E2E tests: real UI + real Rust service layer (devserver) against the fake Graph demo data.
const { defineConfig } = require('@playwright/test');

// E2E_PORT / CARGO_TARGET_DIR allow parallel runs from several worktrees.
const PORT = Number(process.env.E2E_PORT || 8080);
const TARGET = process.env.CARGO_TARGET_DIR || 'target';

module.exports = defineConfig({
  testDir: './tests',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  workers: 1, // all tests share one backend with mutable demo data
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1366, height: 820 },
    locale: 'de-DE',
    timezoneId: 'Europe/Berlin',
    trace: 'retain-on-failure',
  },
  webServer: {
    // Build first, then exec the binary itself so Playwright's shutdown signal
    // reaches the server (with `cargo run` it lingered and the next run reused
    // its already modified demo data).
    command: `cargo build -q -p devserver --bin devserver && exec ${TARGET}/debug/devserver --demo --port ${PORT} --ui-dir ui`,
    cwd: '../..',
    url: `http://127.0.0.1:${PORT}/`,
    timeout: 600_000,
    // Always start fresh: the tests rely on pristine demo data.
    reuseExistingServer: false,
    stdout: 'ignore',
    stderr: 'pipe',
  },
});
