const { test, expect } = require('@playwright/test');
const { openApp, popupFrom } = require('./helpers');

async function openContacts(page) {
  await openApp(page);
  await page.locator('[data-testid=nav-contacts]').click();
  await expect(page.locator('[data-testid=contact-row]').first()).toBeVisible();
}

const contactRow = (page, name) => page.locator('[data-testid=contact-row]', { hasText: name });

test('Kontakte: synchronisierte Liste, Suche und Detailansicht', async ({ page }) => {
  await openContacts(page);
  await expect(page.locator('.sb-title')).toHaveText('Kontakte');
  await expect(contactRow(page, 'Anna Fischer')).toBeVisible();
  expect(await page.locator('[data-testid=contact-row]').count()).toBeGreaterThanOrEqual(10);
  // alphabet separators
  await expect(page.locator('.ct-letter').first()).toBeVisible();

  await page.locator('[data-testid=contacts-search]').fill('Erika');
  await expect(page.locator('[data-testid=contact-row]')).toHaveCount(1);
  await contactRow(page, 'Erika Beispiel').click();
  await expect(page.locator('[data-testid=contact-name]')).toHaveText('Erika Beispiel');
  await expect(page.locator('[data-testid=contact-email]').first()).toContainText('@');
  await page.locator('[data-testid=contacts-search]').fill('');
  await expect(contactRow(page, 'Anna Fischer')).toBeVisible();

  // account filter in the sidebar
  await page.locator('[data-testid=ct-src-account]').click();
  await expect(page.locator('#ct-title')).toContainText('max.mustermann@contoso.example');
  await page.locator('[data-testid=ct-src-all]').click();
  await expect(page.locator('#ct-title')).toHaveText('Alle Kontakte');
});

test('Kontakt anlegen, bearbeiten, E-Mail schreiben und löschen', async ({ page }) => {
  await openContacts(page);
  const stamp = Date.now().toString().slice(-6);
  const last = `Testkontakt${stamp}`;
  await page.locator('[data-testid=tb-new-contact]').click();
  await page.locator('input[name=givenName]').fill('Paula');
  await page.locator('input[name=surname]').fill(last);
  await expect(page.locator('input[name=displayName]')).toHaveValue(`Paula ${last}`);
  await page.locator('input[name=email0]').fill(`paula.${stamp}@example.org`);
  await page.locator('[data-testid=ct-add-email]').click();
  await page.locator('input[name=email1]').fill(`p${stamp}@privat.example`);
  await page.locator('input[name=company]').fill('Beispiel AG');
  await page.locator('#contact-save').click();
  await expect(page.locator('[data-testid=contact-name]')).toHaveText(`Paula ${last}`);
  await expect(page.locator('[data-testid=contact-email]')).toHaveCount(2);
  await expect(contactRow(page, last)).toHaveClass(/selected/);

  // edit
  await page.locator('[data-testid=contact-edit]').click();
  await page.locator('input[name=jobTitle]').fill('Einkauf');
  await page.locator('#contact-save').click();
  await expect(page.locator('.ct-role')).toHaveText('Einkauf · Beispiel AG');

  // write e-mail → compose with recipient
  const popup = await popupFrom(page, () => page.locator('[data-testid=contact-write]').click());
  await expect(popup.locator('.rcpt-box[data-field=to] .rcpt-token')).toContainText(`Paula ${last}`);
  await popup.close({ runBeforeUnload: false });

  // delete
  await page.locator('[data-testid=contact-delete]').click();
  await page.locator('.modal .btn.primary', { hasText: 'Löschen' }).click();
  await expect(contactRow(page, last)).toHaveCount(0);
});

test('Adressvorschläge beim Verfassen kommen aus den Kontakten', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  const to = popup.locator('.rcpt-input[data-field=to]');
  await to.click();
  await to.pressSequentially('Erika', { delay: 30 });
  const sugg = popup.locator('[data-testid=rcpt-sugg]', { hasText: 'Erika Beispiel' });
  await expect(sugg).toBeVisible();
  await to.press('Enter');
  await expect(popup.locator('.rcpt-box[data-field=to] .rcpt-token')).toHaveText(/Erika Beispiel/);
  // pasting a list creates several tokens
  const cc = popup.locator('[data-testid=add-ccbcc]');
  await cc.click();
  await popup.locator('.rcpt-input[data-field=cc]').evaluate((el) => {
    const dt = new DataTransfer();
    dt.setData('text/plain', 'Max Muster <max@example.org>; zweite@example.org');
    el.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
  });
  await expect(popup.locator('.rcpt-box[data-field=cc] .rcpt-token')).toHaveCount(2);
  await popup.close({ runBeforeUnload: false });
});
