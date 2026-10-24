const { test, expect } = require('@playwright/test');
const { openApp, row, select, openFolder } = require('./helpers');

test('Bild aus einer Mail kopieren legt ein PNG in die Zwischenablage (kein base64-Text)', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await openApp(page);
  // Another test archives this mail ("Senden & archivieren").
  if (!(await row(page, 'Neues Logo für den Website-Relaunch').count())) await openFolder(page, 'Archiv');
  await select(page, 'Neues Logo für den Website-Relaunch');
  const img = page.frameLocator('.mail-frame').locator('img[alt="Banner"]');
  await expect(img).toBeVisible();
  await img.click({ button: 'right' });
  await page.getByTestId('img-copy').click();
  await expect(page.locator('.toast', { hasText: 'Bild kopiert' })).toBeVisible();
  const types = await page.evaluate(async () => (await navigator.clipboard.read()).flatMap((i) => i.types));
  expect(types).toContain('image/png');
  expect(types).not.toContain('text/plain');
});
