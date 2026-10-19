const { test, expect } = require('@playwright/test');
const { openApp, row, openFolder, popupFrom } = require('./helpers');

async function typeBody(popup, text) {
  const body = popup.frameLocator('iframe.editor-frame').locator('body');
  await body.click({ position: { x: 20, y: 10 } });
  await popup.keyboard.type(text);
}

test('Neue E-Mail verfassen und senden', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('#subject')).toBeVisible();
  const subject = `E2E Test ${Date.now()}`;
  const to = popup.locator('.rcpt-input[data-field=to]');
  await to.fill('kunde@example.com');
  await to.press('Tab');
  await expect(popup.locator('.rcpt-box[data-field=to] .rcpt-token')).toHaveText(/kunde@example.com/);
  await popup.locator('#subject').fill(subject);
  await typeBody(popup, 'Hallo, dies ist ein automatischer Test.');
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=compose-send]').click();
  await closed;
  await openFolder(page, 'Gesendete Elemente');
  await expect(row(page, subject)).toBeVisible({ timeout: 20_000 });
  await row(page, subject).click();
  await expect(page.frameLocator('#readpane iframe.mail-frame').locator('body')).toContainText('automatischer Test');
});

test('Antworten', async ({ page }) => {
  await openApp(page);
  await row(page, 'Rückfrage Artikelstammdaten').click();
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-reply]').click());
  await expect(popup.locator('#subject')).toHaveValue(/^(AW|RE): Rückfrage Artikelstammdaten/i);
  await expect(popup.locator('.rcpt-box[data-field=to] .rcpt-token')).toHaveCount(1);
  await typeBody(popup, 'Danke, erledigt.');
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=compose-send]').click();
  await closed;
  await openFolder(page, 'Gesendete Elemente');
  await expect(row(page, 'Rückfrage Artikelstammdaten').first()).toBeVisible({ timeout: 20_000 });
});

test('Empfängerprüfung verhindert Senden ohne Empfänger', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await popup.locator('#subject').fill('ohne Empfänger');
  await popup.locator('[data-testid=compose-send]').click();
  await expect(popup.locator('.toast.error')).toContainText('mindestens einen Empfänger');
  await popup.close({ runBeforeUnload: false });
});
