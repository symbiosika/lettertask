const { test, expect } = require('@playwright/test');

// Errors must be visible in the app – not only in a terminal the user never sees.

test('Ordner lassen sich nicht laden: Statuszeile zeigt den Fehler, Klick zeigt Details', async ({ page, context }) => {
  await context.route('**/api/call/list_folders', (route) => route.fulfill({ status: 500, json: { error: 'Interner Fehler bei „list_folders“: Testfehler' } }));
  await context.route('**/api/call/sync_folders', (route) => route.fulfill({ status: 500, json: { error: 'Interner Fehler bei „sync_folders“: Testfehler' } }));
  await page.goto('/');
  const status = page.locator('[data-testid=sync-status]');
  await expect(status).toContainText('Testfehler');
  await expect(page.locator('.toast.error').first()).toContainText('Ordner konnten nicht geladen werden');
  await expect(page.locator('#statusbar')).not.toContainText('null');
  await status.click();
  await expect(page.locator('[data-testid=sync-error-detail]')).toContainText('Interner Fehler bei „sync_folders“: Testfehler');
  await page.locator('.modal .btn', { hasText: 'Schließen' }).click();
});

test('Unerwarteter Fehler im Frontend erscheint als Meldung', async ({ page }) => {
  await page.goto('/');
  await page.locator('.msg').first().waitFor();
  await expect(page.locator('#statusbar')).not.toContainText('null');
  await page.evaluate(() => { Promise.reject(new Error('Unerwarteter Testfehler')); });
  await expect(page.locator('.toast.error', { hasText: 'Unerwarteter Testfehler' })).toBeVisible();
});
