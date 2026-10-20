const { test, expect } = require('@playwright/test');
const { openApp, select, popupFrom, api } = require('./helpers');

async function openSignatures(page) {
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-signatures]').click();
  await expect(page.locator('.sig-panel')).toBeVisible();
}

async function typeSig(page, text) {
  const body = page.frameLocator('.sig-editor-box iframe.editor-frame').locator('body');
  await body.click();
  await page.keyboard.type(text);
}

function editorBody(popup) {
  return popup.frameLocator('iframe.editor-frame').locator('body');
}

test('Signaturen anlegen, Standard wählen und Einfügeregeln speichern', async ({ page }) => {
  await openApp(page);
  await openSignatures(page);
  const items = page.locator('[data-testid=sig-item]');
  const before = await items.count();

  await page.locator('[data-testid=sig-new]').click();
  await page.locator('input[name=sigName]').fill('Geschäftlich');
  await typeSig(page, 'Beste Grüße, Max Mustermann – Contoso GmbH');
  await page.locator('[data-testid=sig-new]').click();
  await page.locator('input[name=sigName]').fill('Kurz');
  await typeSig(page, 'VG Max');
  await expect(items).toHaveCount(before + 2);

  // "Kurz" becomes the default
  const kurz = items.filter({ hasText: 'Kurz' });
  await kurz.hover();
  await kurz.locator('[data-testid=sig-default]').click();
  await expect(kurz.locator('.sig-star')).toHaveClass(/on/);
  // defaults: new + reply on, forward off
  await expect(page.locator('input[name=useOnNew]')).toBeChecked();
  await expect(page.locator('input[name=useOnReply]')).toBeChecked();
  await expect(page.locator('input[name=useOnForward]')).not.toBeChecked();
  await page.locator('#settings-ok').click();
  await expect(page.locator('.modal-overlay')).toHaveCount(0);

  const accountId = (await api(page, 'list_accounts'))[0].id;
  const saved = await api(page, 'get_signatures', { accountId });
  const def = saved.signatures.find((s) => s.id === saved.defaultId);
  expect(def.name).toBe('Kurz');
  expect(saved.signatures.map((s) => s.name)).toEqual(expect.arrayContaining(['Geschäftlich', 'Kurz']));

  // persists after reload
  await page.reload();
  await openSignatures(page);
  await expect(items.filter({ hasText: 'Geschäftlich' })).toHaveCount(1);
  await expect(items.filter({ hasText: 'Kurz' }).locator('.sig-star')).toHaveClass(/on/);
  await items.filter({ hasText: 'Geschäftlich' }).click();
  await expect(page.frameLocator('.sig-editor-box iframe.editor-frame').locator('body')).toContainText('Contoso GmbH');
  await page.keyboard.press('Escape');
});

test('Standardsignatur bei Neu und Antwort, nicht bei Weiterleitung; Wechsel per Dropdown', async ({ page }) => {
  await openApp(page);
  // new mail → default signature
  let popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(editorBody(popup).locator('.signature')).toContainText('VG Max');
  // switch via dropdown replaces the block
  await popup.locator('[data-testid=compose-signature]').click();
  await popup.locator('.menu-item', { hasText: 'Geschäftlich' }).click();
  await expect(editorBody(popup).locator('.signature')).toHaveCount(1);
  await expect(editorBody(popup).locator('.signature')).toContainText('Contoso GmbH');
  await expect(editorBody(popup).locator('.signature')).not.toContainText('VG Max');
  // "Keine Signatur" removes it
  await popup.locator('[data-testid=compose-signature]').click();
  await popup.locator('[data-testid=sigpick-none]').click();
  await expect(editorBody(popup).locator('.signature')).toHaveCount(0);
  await popup.close({ runBeforeUnload: false });

  // reply → present
  await select(page, 'Rückfrage Artikelstammdaten');
  popup = await popupFrom(page, () => page.locator('[data-testid=tb-reply]').click());
  await expect(popup.locator('#subject')).toHaveValue(/Rückfrage/);
  await expect(editorBody(popup).locator('.signature')).toContainText('VG Max');
  await popup.close({ runBeforeUnload: false });

  // forward → absent (useOnForward is off)
  popup = await popupFrom(page, () => page.locator('[data-testid=tb-forward]').click());
  await expect(popup.locator('#subject')).toHaveValue(/Rückfrage/);
  await expect(editorBody(popup)).toContainText('Rückfrage', { timeout: 10_000 });
  await expect(editorBody(popup).locator('.signature')).toHaveCount(0);
  // but can be added manually: inserted above the quote
  await popup.locator('[data-testid=compose-signature]').click();
  await popup.locator('.menu-item', { hasText: 'Kurz' }).click();
  await expect(editorBody(popup).locator('.signature')).toContainText('VG Max');
  const first = await editorBody(popup).evaluate((b) => { const s = b.querySelector('.signature'); return [...b.children].indexOf(s); });
  expect(first).toBeLessThan(3);
  await popup.close({ runBeforeUnload: false });
});
