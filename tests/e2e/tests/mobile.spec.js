// Phone layout (Android app / phone browser): app bar, drawer, bottom navigation,
// full-screen reader, sheets, swipe actions.
const { test, expect } = require('@playwright/test');
const { row, api } = require('./helpers');

test.use({
  viewport: { width: 393, height: 852 },
  isMobile: true,
  hasTouch: true,
  userAgent: 'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36',
});

async function openApp(page) {
  await page.goto('/');
  await expect(page.locator('.msg').first()).toBeVisible();
  await expect(page.locator('[data-testid=m-title]')).toHaveText('Posteingang');
}

/** Touch gesture on a list row: horizontal swipe by dx, or a long press (dx = 0, hold ms). */
async function gesture(page, locator, dx, hold = 0) {
  const box = await locator.boundingBox();
  await page.evaluate(async ({ x, y, dx, hold }) => {
    const el = document.elementFromPoint(x, y);
    const ev = (type, cx) => new PointerEvent(type, { bubbles: true, pointerId: 7, pointerType: 'touch', isPrimary: true, clientX: cx, clientY: y });
    el.dispatchEvent(ev('pointerdown', x));
    for (let i = 1; i <= 8; i++) el.dispatchEvent(ev('pointermove', x + (dx * i) / 8));
    if (hold) await new Promise((r) => setTimeout(r, hold));
    el.dispatchEvent(ev('pointerup', x + dx));
    el.dispatchEvent(new MouseEvent('click', { bubbles: true, clientX: x + dx, clientY: y }));
  }, { x: box.x + box.width / 2, y: box.y + box.height / 2, dx, hold });
}

test('Handy: App-Leiste und Navigation statt Desktop-Werkzeugleiste', async ({ page }) => {
  await openApp(page);
  await expect(page.locator('#topbar')).toBeHidden();
  await expect(page.locator('#toolbar')).toBeHidden();
  await expect(page.locator('[data-testid=m-tab-mail]')).toHaveClass(/on/);
  // nothing may widen the page (Chrome would zoom the whole layout out)
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(393);

  await page.locator('[data-testid=m-tab-calendar]').tap();
  await expect(page.locator('[data-testid=sprint-day]').first()).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(393);
  await page.locator('[data-testid=m-tab-contacts]').tap();
  await expect(page.locator('[data-testid=contact-row]').first()).toBeVisible();
  await page.locator('[data-testid=contact-row]').first().tap();
  await expect(page.locator('body')).toHaveClass(/m-ct-detail/);
  await page.goBack();
  await expect(page.locator('body')).not.toHaveClass(/m-ct-detail/);
  await page.locator('[data-testid=m-tab-mail]').tap();
  await expect(page.locator('[data-testid=m-title]')).toHaveText('Posteingang');
});

test('Handy: Mail öffnet im Vollbild, Zurück schließt sie', async ({ page }) => {
  await openApp(page);
  await row(page, 'Anforderungen Schnittstelle ERP').first().tap();
  await expect(page.locator('body')).toHaveClass(/m-reading/);
  await expect(page.locator('.rp-subject')).toContainText('Anforderungen Schnittstelle ERP');
  await expect(page.locator('[data-testid=m-read-archive]')).toBeVisible();
  // Android back button = history back
  await page.goBack();
  await expect(page.locator('body')).not.toHaveClass(/m-reading/);
  await row(page, 'Anforderungen Schnittstelle ERP').first().tap();
  await page.locator('[data-testid=m-read-back]').tap();
  await expect(page.locator('body')).not.toHaveClass(/m-reading/);
});

test('Handy: Ordner im Seitenmenü wählen', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=m-menu]').tap();
  await expect(page.locator('body')).toHaveClass(/m-drawer-open/);
  await page.locator('[data-mailbox-root]').first().locator('.tree-node', { hasText: 'Gesendete Elemente' }).tap();
  await expect(page.locator('[data-testid=m-title]')).toHaveText('Gesendete Elemente');
  await expect(page.locator('body')).not.toHaveClass(/m-drawer-open/);
  await page.locator('[data-testid=m-tab-mail]').tap();
  await expect(page.locator('[data-testid=m-title]')).toHaveText('Posteingang');
});

test('Handy: Wischen nach rechts archiviert, nach links öffnet die Wiedervorlage', async ({ page }) => {
  await page.goto('/');
  const mb = (await api(page, 'list_accounts'))[0].mailboxes[0].id;
  const subject = `Wisch-Test ${Date.now()}`;
  await api(page, 'create_task', { mailboxId: mb, subject, bodyHtml: '', due: null });
  await openApp(page);
  await expect(row(page, subject)).toBeVisible();

  await gesture(page, row(page, subject), -150);
  await expect(page.locator('.msheet')).toBeVisible();
  await expect(page.locator('.msheet-title')).toHaveText('Auf Wiedervorlage');
  await page.goBack();
  await expect(page.locator('.msheet')).toHaveCount(0);
  await expect(row(page, subject)).toBeVisible();

  await gesture(page, row(page, subject), 150);
  await expect(row(page, subject)).toHaveCount(0);
  await expect(page.locator('.toast', { hasText: 'archiviert' })).toBeVisible();
});

test('Handy: Langes Drücken startet die Mehrfachauswahl', async ({ page }) => {
  await openApp(page);
  await gesture(page, page.locator('.msg').nth(0), 0, 650);
  await expect(page.locator('body')).toHaveClass(/m-select/);
  await expect(page.locator('[data-testid=m-select-count]')).toHaveText('1 ausgewählt');
  await page.locator('.msg').nth(1).tap();
  await expect(page.locator('[data-testid=m-select-count]')).toHaveText('2 ausgewählt');
  await expect(page.locator('body')).not.toHaveClass(/m-reading/);
  await page.locator('[data-testid=m-select-close]').tap();
  await expect(page.locator('body')).not.toHaveClass(/m-select/);
  await expect(page.locator('[data-testid=m-title]')).toHaveText('Posteingang');
});

test('Handy: Verfassen öffnet als Ebene und schließt wieder', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=m-fab]').tap();
  await page.locator('[data-testid=m-new-mail]').tap();
  const frame = page.frameLocator('[data-testid=m-frame-compose] iframe');
  await expect(frame.locator('[data-testid=m-frame-close]')).toBeVisible();
  await expect(frame.locator('#subject, input[name=subject]').first()).toBeVisible();
  await frame.locator('[data-testid=m-frame-close]').tap();
  await expect(page.locator('[data-testid=m-frame-compose]')).toHaveCount(0);
  // the FAB still works afterwards (no stale layer)
  await page.locator('[data-testid=m-fab]').tap();
  await expect(page.locator('[data-testid=m-new-task]')).toBeVisible();
  await page.goBack();
  await expect(page.locator('.m-fab-wrap')).not.toHaveClass(/open/);
});

test('Handy: Menüs erscheinen als Sheet von unten', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=m-account]').tap();
  await expect(page.locator('.msheet')).toBeVisible();
  await expect(page.locator('.menu')).toHaveCount(0);
  await expect(page.locator('.msheet-item', { hasText: 'Einstellungen' })).toBeVisible();
  await page.goBack();
  await expect(page.locator('.msheet')).toHaveCount(0);
});

test('Handy: Abruf bei geschlossener App einstellen', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=m-account]').tap();
  await page.locator('.msheet-item', { hasText: 'Einstellungen' }).tap();
  const sel = page.locator('[data-testid=set-bg-interval]');
  await expect(sel).toBeVisible();
  await expect(sel).toHaveValue('15');
  await sel.selectOption('60');
  await page.locator('#settings-ok').tap();
  await expect(page.locator('.modal', { hasText: 'Einstellungen' })).toHaveCount(0);
  const cfg = await (await page.request.post('/api/call/get_config', { data: {} })).json();
  expect(cfg.backgroundIntervalMins).toBe(60);
  await api(page, 'save_config', { config: { ...cfg, backgroundIntervalMins: 15 } });
});
