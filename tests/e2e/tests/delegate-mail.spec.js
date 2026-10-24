const { test, expect } = require('@playwright/test');
const { openApp, row, select, popupFrom, openFolder } = require('./helpers');

test('Bestehende Mail delegieren: Weiterleitung an eine Person, Kopie unter „Delegiert“', async ({ page }) => {
  await openApp(page);
  const subject = 'Neues Logo für den Website-Relaunch';
  // Another test archives this mail ("Senden & archivieren").
  if (!(await row(page, subject).count())) await openFolder(page, 'Archiv');
  await select(page, subject);
  await expect(page.getByTestId('tb-delegate')).toBeVisible();
  const popup = await popupFrom(page, () => page.getByTestId('rp-delegate').click());
  await expect(popup.locator('.cw-title')).toHaveText(/Delegieren|Fwd|WG/);
  await expect(popup.getByTestId('delegate-row')).toBeVisible();
  await expect(popup.getByTestId('remind-row')).toHaveCount(0);
  await expect(popup.getByTestId('compose-send')).toHaveText(/^Delegieren/);

  // Two people in "An" are refused.
  const to = popup.locator('.rcpt-input[data-field=to]');
  await to.fill('anna@fabrikam.example');
  await to.press('Enter');
  await to.fill('bob@fabrikam.example');
  await to.press('Enter');
  await popup.getByTestId('compose-send').click();
  await expect(popup.locator('.toast', { hasText: 'genau eine Person' })).toBeVisible();
  await popup.locator('.rcpt-box[data-field=to] .rcpt-token', { hasText: 'bob' }).locator('.rcpt-x').click();
  await expect(popup.locator('.rcpt-box[data-field=to] .rcpt-token')).toHaveCount(1);

  const closed = popup.waitForEvent('close');
  await popup.getByTestId('compose-send').click();
  await closed;

  await page.locator('[data-testid=delegated-node]').click();
  await expect(page.locator('.listpane .folder-name')).toHaveText('Delegiert');
  await expect(row(page, `ToDo: `).filter({ hasText: subject }).first()).toBeVisible({ timeout: 20_000 });
  await openFolder(page, 'Gesendete Elemente');
  await expect(row(page, subject).first()).toBeVisible({ timeout: 20_000 });
});
