const { test, expect } = require('@playwright/test');
const { openApp, row, openFolder, select, popupFrom } = require('./helpers');

async function typeBody(popup, text) {
  const body = popup.frameLocator('iframe.editor-frame').locator('body');
  await body.click({ position: { x: 30, y: 12 } });
  await popup.keyboard.type(text);
}

test('Neue E-Mail verfassen und senden', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('#subject')).toBeVisible();
  await expect(popup.locator('#from-chip')).toContainText('max.mustermann@contoso.example');
  const subject = `E2E Test ${Date.now()}`;
  const to = popup.locator('.rcpt-input[data-field=to]');
  await to.fill('kunde@example.com');
  await to.press('Tab');
  await expect(popup.locator('.rcpt-box[data-field=to] .rcpt-token')).toHaveText(/kunde@example.com/);
  await popup.locator('#subject').fill(subject);
  await expect(popup.locator('.cw-title')).toHaveText(subject);
  await typeBody(popup, 'Hallo, dies ist ein automatischer Test.');
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=compose-send]').click();
  await closed;
  await openFolder(page, 'Gesendete Elemente');
  await expect(row(page, subject)).toBeVisible({ timeout: 20_000 });
  await select(page, subject);
  await expect(page.frameLocator('#readpane iframe.mail-frame').locator('body')).toContainText('automatischer Test');
});

test('Cc & Bcc hinzufügen blendet die Felder ein', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('.rcpt-box[data-field=cc]')).toBeHidden();
  await popup.locator('[data-testid=add-ccbcc]').click();
  await expect(popup.locator('.rcpt-box[data-field=cc]')).toBeVisible();
  await expect(popup.locator('.rcpt-box[data-field=bcc]')).toBeVisible();
  await expect(popup.locator('[data-testid=add-ccbcc]')).toBeHidden();
  // importance toggle in the formatting bar
  await popup.locator('[data-testid=compose-importance]').click();
  await expect(popup.locator('.cw-flags')).toContainText('Wichtig');
  await popup.close({ runBeforeUnload: false });
});

test('Antworten', async ({ page }) => {
  await openApp(page);
  await select(page, 'Rückfrage Artikelstammdaten');
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

test('Langsames Laden zieht den Cursor nicht aus dem Betreff', async ({ page }) => {
  await openApp(page);
  // Hold the draft back so the window finishes loading while the user types.
  let release;
  const gate = new Promise((r) => { release = r; });
  await page.context().route('**/api/call/create_draft', async (route) => { await gate; await route.continue(); });
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await popup.locator('#subject').click();
  await popup.keyboard.type('Schnell getippt');
  release();
  await popup.waitForResponse('**/api/call/create_draft');
  await popup.keyboard.type(' und weiter');
  await expect(popup.locator('#subject')).toHaveValue('Schnell getippt und weiter');
  await page.context().unroute('**/api/call/create_draft');
  await popup.close({ runBeforeUnload: false });
});

test('Dateien per Drag & Drop anfügen – auf das Fenster und in den Text', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('#subject')).toBeVisible();
  const editorBody = popup.frameLocator('iframe.editor-frame').locator('body');
  await expect(editorBody).toBeVisible();
  // Drops a file on the element: on the window (header) and on the text (editor frame).
  const frame = await (await popup.locator('iframe.editor-frame').elementHandle()).contentFrame();
  const drop = async (locator, name, text, ctx = popup) => {
    const dt = await ctx.evaluateHandle(([n, t]) => {
      const d = new DataTransfer();
      d.items.add(new File([t], n, { type: '' }));
      return d;
    }, [name, text]);
    await locator.dispatchEvent('dragover', { dataTransfer: dt });
    await locator.dispatchEvent('drop', { dataTransfer: dt });
  };
  await drop(popup.locator('#subject'), 'Angebot.pdf', '%PDF-1.4');
  await drop(frame.locator('body'), 'notizen.txt', 'Hallo', frame);
  await expect(popup.locator('#atts .attachment')).toHaveCount(2);
  await expect(popup.locator('#atts')).toContainText('Angebot.pdf');
  await expect(popup.locator('#atts')).toContainText('notizen.txt');
  // Nothing of the file ends up in the text itself.
  await expect(editorBody).not.toContainText('Hallo');
});
