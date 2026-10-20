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

test('Konto hinzufügen: Google fragt nach dem OAuth-Client, Gmail-Adresse im IMAP-Assistenten schlägt Google vor', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-add-account]').click();
  await page.locator('[data-testid=kind-google]').click();

  // demo build has no Google client: setup dialog, invalid ids are rejected
  await expect(page.locator('.modal-title')).toContainText('Google-Anmeldung einrichten');
  await page.locator('.modal input[name=googleClientId]').fill('keine-client-id');
  await page.locator('#google-client-ok').click();
  await expect(page.locator('.modal input[name=googleClientId]')).toHaveClass(/invalid/);
  await page.locator('.modal .btn', { hasText: 'Abbrechen' }).click();
  await expect(page.locator('.modal-overlay')).toHaveCount(0);

  // a gmail.com address in the IMAP wizard offers "Mit Google anmelden"
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-add-account]').click();
  await page.locator('[data-testid=kind-imap]').click();
  await page.locator('input[name=imapEmail]').fill('erika.e2e@gmail.com');
  await page.locator('input[name=imapPassword]').fill('app-passwort');
  await page.locator('#imap-next').click();
  await expect(page.locator('.modal').last()).toContainText('liegt bei Google', { timeout: 30_000 });
  await page.locator('.modal .btn', { hasText: 'Mit Passwort fortfahren' }).click();
  await expect(page.locator('[data-testid=srv-imap]')).toBeVisible();
  await page.locator('.modal .btn', { hasText: 'Abbrechen' }).click();
  await expect(page.locator('.modal-overlay')).toHaveCount(0);
});

test('Einstellungen › Konten: Google-OAuth-Client speichern', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=settings-tab-accounts]').click();
  await page.locator('[data-testid=google-adv] summary').click();
  await page.locator('input[name=googleClientId]').fill('123-e2e.apps.googleusercontent.com');
  await page.locator('input[name=googleClientSecret]').fill('e2e-secret');
  await page.locator('#settings-ok').click();
  await expect(page.locator('.modal-overlay')).toHaveCount(0);
  const cfg = await page.evaluate(async () => (await fetch('/api/call/get_config', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })).json());
  expect(cfg.googleClientId).toBe('123-e2e.apps.googleusercontent.com');
  // reset for the other tests
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=settings-tab-accounts]').click();
  await page.locator('[data-testid=google-adv] summary').click();
  await page.locator('input[name=googleClientId]').fill('');
  await page.locator('input[name=googleClientSecret]').fill('');
  await page.locator('#settings-ok').click();
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

test('Eigenes Kontobild: wählen, rund anzeigen, entfernen', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=settings-tab-accounts]').click();
  const detail = page.locator('[data-testid=acc-detail]');
  // 40×30 red PNG – cropped to a square
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAACgAAAAeCAIAAADRv8uKAAAAK0lEQVR4nO3NMQ0AAAgDsEmb/wtJyICjSf9m2hMRi8VisVgsFovFYvHfeAHAuydbA4lSNgAAAABJRU5ErkJggg==', 'base64');
  await detail.locator('[data-testid=acc-avatar-file]').setInputFiles({ name: 'ich.png', mimeType: 'image/png', buffer: png });
  const img = detail.locator('.acc-head .avatar.img img');
  await expect(img).toBeVisible();
  await expect(img).toHaveAttribute('src', /^data:image\/png;base64,/);
  const size = await img.evaluate((el) => [el.naturalWidth, el.naturalHeight, getComputedStyle(el).borderRadius]);
  expect(size.slice(0, 2)).toEqual([128, 128]);
  expect(size[2]).toBe('50%');
  await expect(page.locator('[data-testid=acc-item] .avatar.img')).toHaveCount(1);
  await page.keyboard.press('Escape');
  // folder pane and account button show the picture
  await expect(page.locator('.tree-root.account .avatar.img img').first()).toBeVisible();
  await expect(page.locator('#account-chip .avatar.img img')).toBeVisible();
  // a non-image is rejected
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=settings-tab-accounts]').click();
  await detail.locator('[data-testid=acc-avatar-file]').setInputFiles({ name: 'x.txt', mimeType: 'text/plain', buffer: Buffer.from('hallo') });
  await expect(page.locator('.toast.error')).toContainText('PNG');
  // remove → initials again (also after a reload)
  await detail.locator('[data-testid=acc-avatar-remove]').click();
  await expect(detail.locator('.acc-head .avatar.img')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await page.reload();
  await expect(page.locator('.tree-root.account .avatar').first()).toBeVisible();
  await expect(page.locator('.tree-root.account .avatar.img')).toHaveCount(0);
});

test('Reihenfolge der Konten in den Einstellungen ändern', async ({ page, context }) => {
  // The demo has one account: a second (IMAP) one is simulated in list_accounts.
  let order = null;
  await context.route('**/api/call/list_accounts', async (route) => {
    const res = await route.fetch();
    const accounts = await res.json();
    const imap = { id: 'imap:e2e', email: 'erika@imap.example', displayName: 'Erika IMAP', kind: 'imap', avatar: null, mailboxes: [] };
    let list = [...accounts, imap];
    if (order) list = order.map((id) => list.find((a) => a.id === id)).filter(Boolean);
    await route.fulfill({ response: res, json: list });
  });
  await context.route('**/api/call/set_account_order', async (route) => {
    order = route.request().postDataJSON().accountIds;
    await route.fulfill({ json: null });
  });
  await openApp(page);
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=settings-tab-accounts]').click();
  const items = page.locator('[data-testid=acc-item]');
  await expect(items).toHaveCount(2);
  await expect(items.nth(0)).toContainText('max.mustermann@contoso.example');
  await expect(items.nth(0).locator('[data-testid=acc-up]')).toBeDisabled();
  await expect(items.nth(1).locator('[data-testid=acc-down]')).toBeDisabled();
  await items.nth(0).locator('[data-testid=acc-down]').click();
  await expect(items.nth(0)).toContainText('erika@imap.example');
  await expect(items.nth(1)).toContainText('max.mustermann@contoso.example');
  await expect.poll(() => order).toEqual(['imap:e2e', expect.stringContaining('')]);
  expect(order[1]).not.toBe('imap:e2e');
  // the selected account stays selected (clicking the arrow does not select)
  await expect(page.locator('[data-testid=acc-detail]')).toContainText('max.mustermann@contoso.example');
  // order survives reopening the settings
  await page.keyboard.press('Escape');
  await page.reload();
  await page.locator('.msg').first().waitFor();
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=settings-tab-accounts]').click();
  await expect(items.nth(0)).toContainText('erika@imap.example');
});
