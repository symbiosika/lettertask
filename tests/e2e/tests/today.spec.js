// Heute view, Fokuszeit, recurring tasks, hidden "[#…]" categories, confetti.
const { test, expect } = require('@playwright/test');
const { openApp, row, openFolder, select, popupFrom, api } = require('./helpers');

async function primary(page) {
  const acc = (await api(page, 'list_accounts'))[0];
  return { acc, mb: acc.mailboxes[0].id };
}
async function inboxItems(page, mb) {
  const inbox = (await api(page, 'list_folders', { mailboxId: mb })).find((f) => f.wellKnown === 'inbox');
  return (await api(page, 'list_messages', { mailboxId: mb, folderId: inbox.id, limit: 500 })).items;
}

test('Heute: fällige Aufgaben und erste Lane, morgen Fälliges nicht', async ({ page }) => {
  await openApp(page);
  const { acc, mb } = await primary(page);
  const tomorrow = new Date(); tomorrow.setDate(tomorrow.getDate() + 1); tomorrow.setHours(8, 0, 0, 0);
  const later = `Morgen erledigen ${Date.now()}`;
  await api(page, 'create_task', { mailboxId: mb, subject: later, bodyHtml: '', due: tomorrow.toISOString() });
  const soon = new Date(Date.now() + 3 * 60_000);
  const dueToday = soon.getDate() === new Date().getDate();
  const nowTask = `Heute erledigen ${Date.now()}`;
  if (dueToday) await api(page, 'create_task', { mailboxId: mb, subject: nowTask, bodyHtml: '', due: soon.toISOString() });
  await api(page, 'save_lanes', { accountId: acc.id, lanes: ['Fokus', 'Später'] });
  const m = (await inboxItems(page, mb)).find((x) => x.subject.startsWith('Freigabe erforderlich'));
  await api(page, 'set_lane', { mailboxId: mb, ids: [m.id], lane: 'Fokus' });
  await page.reload();
  await page.locator('.msg').first().waitFor();
  const node = page.locator('[data-testid=today-node]');
  await expect(node).toBeVisible();
  await expect(node.locator('[data-testid=today-count]')).toBeVisible();
  await node.click();
  await expect(page.locator('.listpane .folder-name')).toHaveText('Heute');
  await expect(page.locator('.m-sep.today-sec', { hasText: 'Fokus' })).toBeVisible();
  const laneRow = row(page, 'Freigabe erforderlich');
  await expect(laneRow.first()).toBeVisible();
  await expect(row(page, later)).toHaveCount(0);
  if (dueToday) {
    await expect(page.locator('.m-sep.today-sec', { hasText: 'Heute fällig' })).toBeVisible();
    await expect(row(page, nowTask).locator('.m-due')).toBeVisible();
  }
  // normal actions work in Heute: open the mail in the reading pane, archive with E
  await laneRow.first().locator('.m-subj').click();
  await expect(page.locator('.rp-subject')).toContainText('Freigabe erforderlich');
  await page.keyboard.press('e');
  await expect(page.locator('.toast', { hasText: 'archiviert' })).toBeVisible();
  await expect(row(page, m.subject)).toHaveCount(0);
  await api(page, 'save_lanes', { accountId: acc.id, lanes: [] });
});

test('Fokuszeit: 1 Stunde starten und wieder beenden', async ({ page }) => {
  await openApp(page);
  const btn = page.locator('[data-testid=focus-btn]');
  await expect(btn).toHaveText(/Fokuszeit/);
  await btn.click();
  await page.locator('[data-testid=focus-1]').click();
  await expect(btn).toHaveText(/Fokus bis \d\d:\d\d/);
  await expect(btn).toHaveClass(/on/);
  const f = await api(page, 'get_focus');
  expect(f.until).toBeTruthy();
  expect(new Date(f.until) - Date.now()).toBeGreaterThan(55 * 60_000);
  await expect(page.locator('[data-testid=sync-status]')).toHaveText('Fokuszeit – automatischer Abruf pausiert');
  // state survives a reload (get_focus on startup)
  await page.reload();
  await expect(page.locator('[data-testid=focus-btn]')).toHaveText(/Fokus bis/);
  await page.locator('[data-testid=focus-btn]').click();
  await page.locator('[data-testid=focus-end]').click();
  await expect(page.locator('[data-testid=focus-btn]')).toHaveText(/Fokuszeit/);
  expect((await api(page, 'get_focus')).until).toBeNull();
});

test('Wiederkehrende Aufgabe: ↻-Badge, Archivieren plant die nächste ein', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=tb-new-menu]').click();
  const popup = await popupFrom(page, () => page.locator('[data-testid=new-task]').click());
  await expect(popup.locator('body[data-ready="1"]')).toHaveCount(1);
  const subject = `Monatsabschluss ${Date.now()}`;
  await popup.locator('#subject').fill(subject);
  await popup.locator('[data-testid=task-due-opt0]').click();
  await popup.locator('[data-testid=task-repeat]').selectOption('monthly');
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=task-create]').click();
  await closed;
  await openFolder(page, 'Wiedervorlage');
  const r = row(page, subject);
  await expect(r).toBeVisible({ timeout: 15_000 });
  await expect(r.locator('.rec-badge')).toBeVisible();
  await select(page, subject);
  await expect(page.locator('[data-testid=rp-rec]')).toContainText('Monatlich');
  // hidden "[#…]" never shows as a tag
  await expect(page.locator('.rp-tags .tag-pill', { hasText: '[#' })).toHaveCount(0);
  await page.locator('[data-testid=tag-add]').click();
  const pop = page.locator('[data-testid=tag-popover]');
  await pop.locator('.tagpop-filter').fill('[#');
  await expect(pop.locator('.tag-opt')).toHaveCount(0);
  await page.keyboard.press('Escape');
  // back to the inbox now, then archive → next occurrence lands in Wiedervorlage
  await page.locator('[data-testid=tb-snooze]').click();
  await page.locator('[data-testid=snooze-unsnooze]').click();
  await expect(row(page, subject)).toHaveCount(0);
  await openFolder(page, 'Posteingang');
  await select(page, subject);
  await expect(page.locator('[data-testid=rp-rec]')).toContainText('Monatlich');
  await page.locator('#msglist').focus();
  await page.keyboard.press('e');
  await expect(page.locator('.toast', { hasText: 'Nächste Wiederholung' })).toBeVisible({ timeout: 15_000 });
  await openFolder(page, 'Wiedervorlage');
  await expect(row(page, subject)).toBeVisible({ timeout: 15_000 });
  // change / remove via the reading pane badge
  await select(page, subject);
  await page.locator('[data-testid=rp-rec] .rec-btn').click();
  await page.locator('[data-testid=rec-none]').click();
  await expect(page.locator('[data-testid=rp-rec] .rec-badge')).toHaveCount(0);
  await expect(row(page, subject).locator('.rec-badge')).toHaveCount(0);
});

test('Konfetti beim Archivieren – abschaltbar', async ({ page }) => {
  await openApp(page);
  await select(page, 'Wochenbericht Vertrieb KW 41');
  await page.locator('#msglist').focus();
  await page.keyboard.press('e');
  await expect(page.locator('canvas.confetti')).toHaveCount(1);
  await expect(page.locator('canvas.confetti')).toHaveCount(0, { timeout: 5000 });
  // switch off in Einstellungen › Allgemein
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=set-confetti]').uncheck();
  await page.locator('#settings-ok').click();
  await expect(page.locator('.toast', { hasText: 'Einstellungen gespeichert' })).toBeVisible();
  await select(page, 'Ticket #48231');
  await page.locator('#msglist').focus();
  await page.keyboard.press('e');
  await expect(row(page, 'Ticket #48231')).toHaveCount(0);
  await page.waitForTimeout(300);
  await expect(page.locator('canvas.confetti')).toHaveCount(0);
});
