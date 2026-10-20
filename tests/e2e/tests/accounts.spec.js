const { test, expect } = require('@playwright/test');
const { openApp } = require('./helpers');

test('Konto hinzufügen: Auswahl und IMAP-Assistent mit Erkennung und Verbindungstest', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-add-account]').click();
  await expect(page.locator('[data-testid=kind-o365]')).toBeVisible();
  await page.locator('[data-testid=kind-imap]').click();

  await page.locator('input[name=imapName]').fill('Erika Test');
  await page.locator('input[name=imapEmail]').fill('erika@unbekannte-domain-e2e.example');
  await page.locator('input[name=imapPassword]').fill('geheim');
  await page.locator('#imap-next').click();

  // unknown domain → guessed settings
  await expect(page.locator('[data-testid=discover-source]')).toContainText('Geschätzt', { timeout: 30_000 });
  await expect(page.locator('input[name=imapHost]')).toHaveValue(/unbekannte-domain-e2e\.example$/);
  await expect(page.locator('input[name=smtpHost]')).toHaveValue(/unbekannte-domain-e2e\.example$/);
  await expect(page.locator('input[name=imapUser]')).toHaveValue('erika@unbekannte-domain-e2e.example');

  // security change updates the default port
  await page.locator('select[name=imapSecurity]').selectOption('starttls');
  await expect(page.locator('input[name=imapPort]')).toHaveValue('143');
  await page.locator('select[name=imapSecurity]').selectOption('ssl');
  await expect(page.locator('input[name=imapPort]')).toHaveValue('993');

  // unreachable server → ✗ with an error message
  await page.locator('input[name=imapHost]').fill('127.0.0.1');
  await page.locator('input[name=imapPort]').fill('1');
  await page.locator('input[name=smtpHost]').fill('127.0.0.1');
  await page.locator('input[name=smtpPort]').fill('1');
  await page.locator('[data-testid=imap-test]').click();
  const out = page.locator('[data-testid=test-out]');
  await expect(out.locator('.test-line.fail').first()).toBeVisible({ timeout: 30_000 });
  await expect(out.locator('.test-line.fail .test-msg').first()).not.toBeEmpty();
  await expect(out.locator('.test-line.ok')).toHaveCount(0);

  // back to step 1 keeps the input
  await page.locator('.modal .btn', { hasText: 'Zurück' }).click();
  await expect(page.locator('input[name=imapEmail]')).toHaveValue('erika@unbekannte-domain-e2e.example');
  await page.locator('.modal .btn', { hasText: 'Abbrechen' }).click();
  await expect(page.locator('.modal-overlay')).toHaveCount(0);
});

test('Einstellungen › Konten zeigt das Microsoft-365-Konto', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=settings-tab-accounts]').click();
  const item = page.locator('[data-testid=acc-item]').first();
  await expect(item).toContainText('max.mustermann@contoso.example');
  await expect(item.locator('.kind-badge')).toHaveText('M365');
  await expect(page.locator('[data-testid=acc-detail]')).toContainText('Microsoft-365-Konto');
  await expect(page.locator('[data-testid=acc-remove]')).toBeVisible();
  // account badge in the mail sidebar
  await page.keyboard.press('Escape');
  await expect(page.locator('.tree-root.account .kind-badge').first()).toHaveText('M365');
});
