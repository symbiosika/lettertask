const { test, expect } = require('@playwright/test');
const { openApp, row, folder, openFolder, select, popupFrom } = require('./helpers');

test('Ordnerbaum, Aktionsleiste und Nachrichtenliste', async ({ page }) => {
  await openApp(page);
  await expect(page.locator('.tree-root', { hasText: 'Favoriten' })).toBeVisible();
  for (const name of ['Posteingang', 'Entwürfe', 'Gesendete Elemente', 'Gelöschte Elemente', 'Archiv', 'Junk-E-Mail']) {
    await expect(folder(page, name)).toBeVisible();
  }
  // shared mailboxes are gone
  await expect(page.locator('[data-mailbox-root]')).toHaveCount(1);
  await expect(page.locator('.listpane .folder-name')).toHaveText('Posteingang');
  for (const id of ['tb-new', 'tb-reply', 'tb-replyall', 'tb-forward', 'tb-archive', 'tb-snooze', 'tb-delete']) {
    await expect(page.locator(`[data-testid=${id}]`)).toBeVisible();
  }
  await expect(page.locator('[data-testid=tb-reply]')).toBeDisabled();
  await expect(page.locator('.m-sep', { hasText: 'Heute' })).toBeVisible();
  // one mail = one row (no conversation grouping)
  expect(await page.locator('.msg').count()).toBeGreaterThan(10);
  await expect(folder(page, 'Posteingang').locator('.count')).toHaveText(/^\d+$/);
});

test('Nachricht im Lesebereich öffnen', async ({ page }) => {
  await openApp(page);
  await select(page, 'Re: Einziger Fokus');
  await expect(page.locator('[data-testid=tb-reply]')).toBeEnabled();
  await expect(page.locator('.rp-fromname')).toHaveText('Laura Schmidt');
  const body = page.frameLocator('#readpane iframe.mail-frame').locator('body');
  await expect(body).toContainText('Hallo Max');
});

test('Adress-Karte beim Überfahren: Name und Adresse kopieren', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await openApp(page);
  await select(page, 'Re: Einziger Fokus');
  await page.locator('.rp-fromname').hover();
  const card = page.locator('[data-testid=contact-card]');
  await expect(card).toBeVisible();
  await expect(card).toContainText('Laura Schmidt');
  await card.locator('[data-testid=cc-copy-addr]').click();
  const addr = await page.evaluate(() => navigator.clipboard.readText());
  expect(addr).toContain('@');
  await expect(card).toContainText(addr);
  await card.locator('[data-testid=cc-copy-name]').click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Laura Schmidt');
  // Leaving card and address closes it.
  await page.mouse.move(5, 5);
  await expect(card).toHaveCount(0);
});

test('Links im Nachrichtentext: auch reine Text-Adressen sind klickbar', async ({ page }) => {
  // Unit-level: the srcdoc builder turns plain URLs and www. links into https links.
  await openApp(page);
  const html = await page.evaluate(async () => {
    const { buildSrcdoc } = await import('/js/mailview.js');
    return buildSrcdoc('<p>Siehe https://example.com/a?b=1. und www.example.org sowie <a href="www.x.de">x</a></p>', false);
  });
  expect(html).toContain('<a href="https://example.com/a?b=1">https://example.com/a?b=1</a>.');
  expect(html).toContain('<a href="https://www.example.org">www.example.org</a>');
  expect(html).toContain('href="https://www.x.de"');
});

test('Als gelesen/ungelesen markieren', async ({ page }) => {
  await openApp(page);
  const r = () => row(page, 'Wartungsfenster Mailserver');
  await expect(r()).not.toHaveClass(/unread/);
  await select(page, 'Wartungsfenster Mailserver');
  await page.locator('#msglist').focus();
  await page.keyboard.press('Control+U');
  await expect(r()).toHaveClass(/unread/);
  await page.reload();
  await expect(r()).toHaveClass(/unread/);
  await select(page, 'Wartungsfenster Mailserver');
  await page.locator('[data-testid=tb-read]').click();
  await expect(r()).not.toHaveClass(/unread/);
  await page.reload();
  await expect(r()).not.toHaveClass(/unread/);
});

test('Kennzeichnen per Klick in der Liste und über die Aktionsleiste', async ({ page }) => {
  await openApp(page);
  const r = () => row(page, 'Testzugang Staging');
  await expect(r().locator('.flagcell svg.ghost')).toHaveCount(1);
  await r().hover();
  await r().locator('.flagcell').click();
  await expect(r().locator('.flagcell svg.flagged')).toHaveCount(1);
  await page.reload();
  await expect(r().locator('.flagcell svg.flagged')).toHaveCount(1);
  await select(page, 'Testzugang Staging');
  await expect(page.locator('.infobar.flag')).toContainText('Nachverfolgung');
  await page.locator('[data-testid=tb-flag]').click();
  await expect(r().locator('.flagcell svg.ghost')).toHaveCount(1);
});

test('Verschieben über das Kontextmenü', async ({ page }) => {
  await openApp(page);
  await row(page, 'Bestellung 4711').first().click({ button: 'right' });
  await page.locator('[data-testid=ctx-move]').hover();
  await page.locator('[data-testid="folder-pick-Projekte"]').click();
  await expect(row(page, 'Bestellung 4711')).toHaveCount(0);
  await openFolder(page, 'Projekte');
  await expect(row(page, 'Bestellung 4711')).toBeVisible();
});

test('Löschen verschiebt nach Gelöschte Elemente', async ({ page }) => {
  await openApp(page);
  await select(page, 'Messeplanung Hannover');
  await page.locator('#msglist').focus();
  await page.keyboard.press('Delete');
  await expect(row(page, 'Messeplanung Hannover')).toHaveCount(0);
  await openFolder(page, 'Gelöschte Elemente');
  await expect(row(page, 'Messeplanung Hannover')).toBeVisible();
});

test('Archivieren verschiebt ins Archiv', async ({ page }) => {
  await openApp(page);
  await select(page, 'Social-Media-Plan November');
  await page.locator('[data-testid=tb-archive]').click();
  await expect(row(page, 'Social-Media-Plan November')).toHaveCount(0);
  await expect(page.locator('.toast')).toContainText('archiviert');
  // quick action on hover
  const r = row(page, 'Lizenzverlängerung 2027');
  await r.hover();
  await r.locator('[data-qa=archive]').click();
  await expect(row(page, 'Lizenzverlängerung 2027')).toHaveCount(0);
  await openFolder(page, 'Archiv');
  await expect(row(page, 'Social-Media-Plan November')).toBeVisible();
  await expect(row(page, 'Lizenzverlängerung 2027')).toBeVisible();
});

test('Auf Wiedervorlage und wieder aufheben', async ({ page }) => {
  await openApp(page);
  await select(page, 'Reisekostenabrechnung September');
  await page.locator('[data-testid=tb-snooze]').click();
  const first = page.locator('[data-testid=snooze-opt-0]');
  await expect(first).toContainText('Morgen 8 Uhr');
  await expect(first).toContainText('08:00');
  await expect(page.locator('[data-testid=snooze-opt-7]')).toContainText('In 30 Tagen');
  await first.click();
  await expect(row(page, 'Reisekostenabrechnung September')).toHaveCount(0);
  await expect(page.locator('.toast')).toContainText('Wiedervorlage bis');
  // new folder "Wiedervorlage" right after Posteingang
  await expect(folder(page, 'Wiedervorlage')).toBeVisible();
  await openFolder(page, 'Wiedervorlage');
  const r = row(page, 'Reisekostenabrechnung September');
  await expect(r).toBeVisible();
  await expect(r.locator('.m-due')).toContainText('bis');
  await select(page, 'Reisekostenabrechnung September');
  await expect(page.locator('.infobar.snooze')).toContainText('Wiedervorlage bis');
  await page.locator('[data-testid=tb-snooze]').click();
  await page.locator('[data-testid=snooze-unsnooze]').click();
  await expect(row(page, 'Reisekostenabrechnung September')).toHaveCount(0);
  await openFolder(page, 'Posteingang');
  await expect(row(page, 'Reisekostenabrechnung September')).toBeVisible();
});

test('Suche auf dem Server und lokaler Filter', async ({ page }) => {
  await openApp(page);
  const box = page.locator('#search-input');
  await box.click();
  await expect(page.locator('[data-testid=search-panel]')).toBeVisible();
  await box.fill('Jahresabschluss');
  await box.press('Enter');
  await expect(page.locator('.listpane .folder-name')).toHaveText('Suchergebnisse');
  await expect(row(page, 'Unterlagen Jahresabschluss')).toBeVisible();
  expect(await page.locator('.msg').count()).toBeLessThan(5);
  await page.locator('[data-testid=search-close]').click();
  await expect(page.locator('.listpane .folder-name')).toHaveText('Posteingang');
  // local filter while typing
  await box.fill('Wochenbericht');
  await expect(page.locator('.msg').first()).toContainText('Wochenbericht');
  await expect(page.locator('.msg', { hasNotText: 'Wochenbericht' })).toHaveCount(0);
  await box.press('Escape');
  await expect.poll(() => page.locator('.msg').count()).toBeGreaterThan(10);
});

test('Suchoptionen: nur Absender, alle Ordner', async ({ page }) => {
  await openApp(page);
  const box = page.locator('#search-input');
  await box.click();
  await page.locator('[data-testid=scope-all]').click();
  await page.locator('[data-testid=field-from]').click();
  await expect(page.locator('[data-testid=field-from]')).toHaveClass(/on/);
  await box.fill('Peter Wagner');
  await box.press('Enter');
  await expect(page.locator('.search-bar .sb-meta')).toContainText('alle Ordner');
  await expect(page.locator('.search-bar .sb-meta')).toContainText('nur Von');
  await expect(row(page, 'Unterlagen Jahresabschluss')).toBeVisible();
  // "Honorarrechnung Q3" lives in the sub folder Rechnungen → only found with "Alle Ordner"
  await expect(row(page, 'Honorarrechnung Q3')).toBeVisible();
  const n = await page.locator('.msg').count();
  for (let i = 0; i < n; i++) await expect(page.locator('.msg').nth(i).locator('.m-from')).toHaveText('Peter Wagner');
  // reset options for other tests
  await box.click();
  await page.locator('[data-testid=scope-folder]').click();
  await page.locator('[data-testid=field-all]').click();
});

test('Filter Ungelesen', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=filter-unread]').click();
  await expect(page.locator('.msg:not(.unread)')).toHaveCount(0);
  expect(await page.locator('.msg.unread').count()).toBeGreaterThan(0);
  await page.locator('[data-testid=filter-all]').click();
});

test('Neu-Termin-Button öffnet Termin-Fenster', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=new-event]').click());
  await expect(popup.locator('input[name=subject]')).toBeVisible();
  expect(popup.url()).toContain('event.html');
  await popup.close({ runBeforeUnload: false });
});

test('Kanban-Knopf öffnet den Posteingang als Kanban', async ({ page }) => {
  await openApp(page);
  await openFolder(page, 'Gesendete Elemente');
  await page.locator('[data-testid=tb-kanban]').click();
  await expect(page.locator('.listpane .folder-name')).toHaveText('Posteingang');
  await expect(page.locator('[data-testid=kanban-board]')).toBeVisible();
  await page.screenshot({ path: 'test-results/kanban-btn.png', clip: { x: 0, y: 40, width: 700, height: 60 } });
  // back to the list for the following tests
  await page.evaluate(() => localStorage.setItem('mailView', JSON.stringify('list')));
});
