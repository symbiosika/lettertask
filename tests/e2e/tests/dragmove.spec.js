const { test, expect } = require('@playwright/test');
const { openApp, row, folder } = require('./helpers');

/** Drags the mail row onto a folder; modifiers apply to dragover and drop. */
async function dragToFolder(page, subject, folderName, mods = {}) {
  const src = row(page, subject).first();
  const dst = folder(page, folderName);
  const dt = await page.evaluateHandle(() => new DataTransfer());
  await src.dispatchEvent('dragstart', { dataTransfer: dt });
  await dst.dispatchEvent('dragover', { dataTransfer: dt, ...mods });
  const badge = await dst.evaluate((n) => n.classList.contains('drop-copy'));
  await dst.dispatchEvent('drop', { dataTransfer: dt, ...mods });
  return badge;
}

function record(page) {
  const calls = [];
  for (const cmd of ['move_messages', 'copy_messages']) {
    page.route(`**/api/call/${cmd}`, (route) => { calls.push(cmd); return route.fulfill({ json: [] }); });
  }
  return calls;
}

test('Ziehen auf einen Ordner verschiebt – ohne Strg nie eine Kopie', async ({ page }) => {
  const calls = record(page);
  await openApp(page);
  const copyHint = await dragToFolder(page, 'Rechnung 2026-1043', 'Archiv');
  expect(copyHint).toBe(false);
  await expect.poll(() => calls).toEqual(['move_messages']);
  await expect(page.locator('.toast', { hasText: 'verschoben' })).toBeVisible();
});

test('Mit gedrückter Strg-Taste wird kopiert (Hinweis „+ Kopie“ am Ziel)', async ({ page }) => {
  const calls = record(page);
  await openApp(page);
  const copyHint = await dragToFolder(page, 'Rechnung 2026-1043', 'Archiv', { ctrlKey: true });
  expect(copyHint).toBe(true);
  await expect.poll(() => calls).toEqual(['copy_messages']);
  await expect(row(page, 'Rechnung 2026-1043')).toHaveCount(1);
});
