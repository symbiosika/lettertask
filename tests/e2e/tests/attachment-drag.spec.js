const { test, expect } = require('@playwright/test');
const { openApp, select, popupFrom } = require('./helpers');

async function newMail(page) {
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('body[data-ready="1"]')).toHaveCount(1);
  const frame = await (await popup.locator('iframe.editor-frame').elementHandle()).contentFrame();
  return { popup, frame, body: popup.frameLocator('iframe.editor-frame').locator('body') };
}

test('Anhang aus der Mail herausziehen und in eine neue Mail fallen lassen', async ({ page }) => {
  await openApp(page);
  await select(page, 'Rechnung 2026-1043');
  const att = page.getByTestId('rp-attachment').filter({ hasText: 'Rechnung_2026-1043.pdf' });
  await expect(att).toHaveAttribute('draggable', 'true');
  // What the drag carries (in the browser: a reference for lettertask windows).
  const dt = await page.evaluateHandle(() => new DataTransfer());
  await att.dispatchEvent('dragstart', { dataTransfer: dt });
  const payload = JSON.parse(await dt.evaluate((d) => d.getData('application/x-lt-attachment')));
  expect(payload).toMatchObject({ name: 'Rechnung_2026-1043.pdf' });

  const { popup, frame, body } = await newMail(page);
  const drop = async (locator, ctx) => {
    const d = await ctx.evaluateHandle((p) => { const x = new DataTransfer(); x.setData('application/x-lt-attachment', p); return x; }, JSON.stringify(payload));
    await locator.dispatchEvent('dragover', { dataTransfer: d });
    await locator.dispatchEvent('drop', { dataTransfer: d });
  };
  await drop(popup.locator('#subject'), popup);
  await expect(popup.locator('#atts .attachment')).toHaveCount(1);
  await expect(popup.locator('#atts')).toContainText('Rechnung_2026-1043.pdf');
  // also onto the text
  await drop(frame.locator('body'), frame);
  await expect(popup.locator('#atts .attachment')).toHaveCount(2);
  await expect(body).not.toContainText('Rechnung');
});

test('Strg+V einer kopierten Datei fügt keine Datei-Adresse in den Text ein', async ({ page }) => {
  await openApp(page);
  const { popup, frame, body } = await newMail(page);
  await body.click();
  // Paste event with file addresses (Windows/macOS/Chromium) …
  await frame.locator('body').evaluate((b) => {
    const dt = new DataTransfer();
    dt.setData('text/uri-list', 'file:///home/max/Angebot%20Q4.pdf');
    dt.setData('text/plain', 'file:///home/max/Angebot%20Q4.pdf');
    b.ownerDocument.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
  });
  // … in the browser the files cannot be read: a hint instead of the address.
  await expect(popup.locator('.toast', { hasText: 'Desktop-App' })).toBeVisible();
  await expect(body).not.toContainText('file://');
  // WebKitGTK inserts the text without a paste event: it is taken out again.
  await frame.locator('body').evaluate((b) => b.ownerDocument.execCommand('insertText', false, 'file:///home/max/Bericht.docx'));
  await expect(body).not.toContainText('file://');
  // Typing a normal text stays.
  await popup.keyboard.type('Hallo');
  await expect(body).toContainText('Hallo');
});

test('Strg+V einer kopierten Datei (als Datei) wird Anhang', async ({ page }) => {
  await openApp(page);
  const { popup, frame, body } = await newMail(page);
  await body.click();
  await frame.locator('body').evaluate((b) => {
    const dt = new DataTransfer();
    dt.items.add(new File(['%PDF-1.4'], 'Vertrag.pdf', { type: 'application/pdf' }));
    b.ownerDocument.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
  });
  await expect(popup.locator('#atts')).toContainText('Vertrag.pdf');
  await expect(body).not.toContainText('Vertrag');
});
