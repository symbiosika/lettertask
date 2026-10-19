const { test, expect } = require('@playwright/test');
const { openApp, row, folder, openFolder } = require('./helpers');

test('Ordnerbaum und Nachrichtenliste werden angezeigt', async ({ page }) => {
  await openApp(page);
  await expect(page.locator('.tree-root', { hasText: 'Favoriten' })).toBeVisible();
  for (const name of ['Posteingang', 'Entwürfe', 'Gesendete Elemente', 'Gelöschte Elemente', 'Archiv', 'Junk-E-Mail']) {
    await expect(folder(page, name)).toBeVisible();
  }
  await expect(page.locator('.tree-root', { hasText: 'datenschutz@contoso.example' })).toBeVisible();
  await expect(page.locator('.listpane .folder-name')).toHaveText('Posteingang');
  await expect(page.locator('.msgtable thead th', { hasText: 'Betreff' })).toBeVisible();
  await expect(page.locator('tr.group', { hasText: 'Heute' })).toBeVisible();
  expect(await page.locator('tr.msg').count()).toBeGreaterThan(10);
  await expect(page.locator('[data-testid=status-count]')).toContainText('Elemente:');
  // unread count of the inbox is shown
  await expect(folder(page, 'Posteingang').locator('.count')).toHaveText(/\(\d+\)/);
});

test('Nachricht im Lesebereich öffnen', async ({ page }) => {
  await openApp(page);
  await row(page, 'Re: Einziger Fokus').click();
  await expect(page.locator('.rp-subject')).toHaveText(/Einziger Fokus/);
  await expect(page.locator('.rp-fromname')).toHaveText('Laura Schmidt');
  const body = page.frameLocator('#readpane iframe.mail-frame').locator('body');
  await expect(body).toContainText('Hallo Max');
});

test('Als gelesen/ungelesen markieren', async ({ page }) => {
  await openApp(page);
  const r = row(page, 'Wartungsfenster Mailserver');
  await expect(r).not.toHaveClass(/unread/);
  await r.click();
  await page.locator('#msglist').focus();
  await page.keyboard.press('Control+U');
  await expect(r).toHaveClass(/unread/);
  await page.reload();
  await expect(row(page, 'Wartungsfenster Mailserver')).toHaveClass(/unread/);
  await row(page, 'Wartungsfenster Mailserver').click({ position: { x: 300, y: 8 } });
  await page.locator('[data-testid=tb-read]').click();
  await expect(row(page, 'Wartungsfenster Mailserver')).not.toHaveClass(/unread/);
  await page.reload();
  await expect(row(page, 'Wartungsfenster Mailserver')).not.toHaveClass(/unread/);
});

test('Kennzeichnen per Klick auf die Flaggen-Spalte', async ({ page }) => {
  await openApp(page);
  const r = row(page, 'Testzugang Staging');
  await expect(r.locator('.flagcell svg.flagOutline')).toHaveCount(1);
  await r.locator('.flagcell').click();
  await expect(r.locator('.flagcell svg.flagOutline')).toHaveCount(0);
  await page.reload();
  await expect(row(page, 'Testzugang Staging').locator('.flagcell svg.flagOutline')).toHaveCount(0);
  // set to completed via toolbar menu
  await row(page, 'Testzugang Staging').locator('.c-subject').click();
  await page.locator('[data-testid=tb-flag]').click();
  await page.locator('.menu-item', { hasText: 'Als erledigt markieren' }).click();
  await expect(page.locator('.infobar.flag')).toContainText('Erledigt');
});

test('Kategorisieren über die Symbolleiste', async ({ page }) => {
  await openApp(page);
  const r = row(page, 'Neue Laptops sind da');
  await r.locator('.c-subject').click();
  await page.locator('[data-testid=tb-categorize]').click();
  await page.locator('[data-testid="cat-Wichtig"]').click();
  await expect(r.locator('.cat-chip', { hasText: 'Wichtig' })).toBeVisible();
  await expect(page.locator('.rp-catbar .cat-chip', { hasText: 'Wichtig' })).toBeVisible();
  await page.reload();
  await expect(row(page, 'Neue Laptops sind da').locator('.cat-chip', { hasText: 'Wichtig' })).toBeVisible();
});

test('Verschieben über das Kontextmenü', async ({ page }) => {
  await openApp(page);
  await row(page, 'Bestellung 4711').click({ button: 'right' });
  await page.locator('[data-testid=ctx-move]').hover();
  await page.locator('[data-testid="folder-pick-Projekte"]').click();
  await expect(row(page, 'Bestellung 4711')).toHaveCount(0);
  await openFolder(page, 'Projekte');
  await expect(row(page, 'Bestellung 4711')).toBeVisible();
});

test('Löschen verschiebt nach Gelöschte Elemente', async ({ page }) => {
  await openApp(page);
  await row(page, 'Messeplanung Hannover').click();
  await page.locator('#msglist').focus();
  await page.keyboard.press('Delete');
  await expect(row(page, 'Messeplanung Hannover')).toHaveCount(0);
  await openFolder(page, 'Gelöschte Elemente');
  await expect(row(page, 'Messeplanung Hannover')).toBeVisible();
});

test('Suche auf dem Server und lokaler Filter', async ({ page }) => {
  await openApp(page);
  const box = page.locator('#search-input');
  await box.fill('Jahresabschluss');
  await box.press('Enter');
  await expect(page.locator('.listpane .folder-name')).toHaveText('Suchergebnisse');
  await expect(row(page, 'Unterlagen Jahresabschluss')).toBeVisible();
  expect(await page.locator('tr.msg').count()).toBeLessThan(5);
  await page.locator('.filterbar .link', { hasText: 'Suche schließen' }).click();
  await expect(page.locator('.listpane .folder-name')).toHaveText('Posteingang');
  // local filter while typing
  await box.fill('Wochenbericht');
  await expect(page.locator('tr.msg').first()).toContainText('Wochenbericht');
  await expect(page.locator('tr.msg', { hasNotText: 'Wochenbericht' })).toHaveCount(0);
});

test('Filter Ungelesen', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=filter-unread]').click();
  await expect(page.locator('tr.msg:not(.unread)')).toHaveCount(0);
  expect(await page.locator('tr.msg.unread').count()).toBeGreaterThan(0);
});
