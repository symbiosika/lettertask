// Inbox Zero: tasks, "Erinnerung an mich", hidden lane categories, Kanban, triage shortcuts.
const { test, expect } = require('@playwright/test');
const { openApp, row, openFolder, select, popupFrom, api } = require('./helpers');

async function account(page) { return (await api(page, 'list_accounts'))[0]; }

async function newTask(page) {
  await page.locator('[data-testid=tb-new-menu]').click();
  const popup = await popupFrom(page, () => page.locator('[data-testid=new-task]').click());
  await expect(popup.locator('body[data-ready="1"]')).toHaveCount(1);
  return popup;
}

test('Aufgabe „Sofort“ landet ungelesen im Posteingang', async ({ page }) => {
  await openApp(page);
  const popup = await newTask(page);
  await expect(popup.locator('.cw-title')).toHaveText('Neue Aufgabe');
  // no recipients, only subject + notes + due
  await expect(popup.locator('.rcpt-box')).toHaveCount(0);
  await expect(popup.locator('[data-testid=task-due-now]')).toHaveClass(/on/);
  const subject = `Angebot prüfen ${Date.now()}`;
  await popup.locator('#subject').fill(subject);
  await popup.frameLocator('iframe.editor-frame').locator('body').click({ position: { x: 30, y: 12 } });
  await popup.keyboard.type('Preise mit Vorjahr vergleichen');
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=task-create]').click();
  await closed;
  await expect(page.locator('.toast', { hasText: 'Aufgabe angelegt' })).toBeVisible();
  const r = row(page, subject);
  await expect(r).toBeVisible({ timeout: 15_000 });
  await expect(r).toHaveClass(/unread/);
  await expect(r.locator('.m-from')).toHaveText('Max Mustermann');
  await select(page, subject);
  await expect(page.frameLocator('#readpane iframe.mail-frame').locator('body')).toContainText('Preise mit Vorjahr');
});

test('Aufgabe mit Fälligkeit „Morgen 8 Uhr“ liegt auf Wiedervorlage', async ({ page }) => {
  await openApp(page);
  const popup = await newTask(page);
  const subject = `Rückruf Steuerberater ${Date.now()}`;
  await popup.locator('#subject').fill(subject);
  await popup.locator('[data-testid=task-due-opt0]').click();
  await expect(popup.locator('[data-testid=task-due-preview]')).toContainText('08:00');
  const closed = popup.waitForEvent('close');
  await popup.keyboard.press('Control+Enter');
  await closed;
  await expect(page.locator('.toast', { hasText: /fällig .*08:00/ })).toBeVisible();
  await expect(row(page, subject)).toHaveCount(0);
  await openFolder(page, 'Wiedervorlage');
  const r = row(page, subject);
  await expect(r).toBeVisible({ timeout: 15_000 });
  await expect(r.locator('.m-due')).toContainText('bis');
});

test('Aufgabe ohne Betreff wird nicht angelegt', async ({ page }) => {
  await openApp(page);
  const popup = await newTask(page);
  await popup.locator('[data-testid=task-create]').click();
  await expect(popup.locator('.toast', { hasText: 'Betreff' })).toBeVisible();
  // custom time in the past is rejected
  await popup.locator('#subject').fill('Etwas erledigen');
  await popup.locator('[data-testid=task-due-custom]').click();
  const at = popup.locator('[data-testid=task-due-at]');
  await expect(at).toBeVisible();
  await at.fill('2020-01-01T08:00');
  await popup.locator('[data-testid=task-create]').click();
  await expect(popup.locator('.toast', { hasText: 'Zukunft' })).toBeVisible();
  await expect(popup.locator('#subject')).toBeVisible();
});

test('Erinnerung an mich: nach dem Senden liegt „Nachfassen“ auf Wiedervorlage', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('#subject')).toBeVisible();
  await expect(popup.locator('[data-testid=remind-off]')).toHaveClass(/on/);
  await expect(popup.locator('[data-testid=remind-when]')).toBeHidden();
  const to = popup.locator('.rcpt-input[data-field=to]');
  await to.fill('lieferant@example.com');
  await to.press('Tab');
  const subject = `Angebot anfordern ${Date.now()}`;
  await popup.locator('#subject').fill(subject);
  await popup.locator('[data-testid=remind-on]').click();
  await expect(popup.locator('[data-testid=remind-when]')).toBeVisible();
  await popup.locator('[data-testid=remind-when]').selectOption('opt1');
  await expect(popup.locator('[data-testid=remind-when-preview]')).toContainText('08:00');
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=compose-send]').click();
  await closed;
  await openFolder(page, 'Wiedervorlage');
  const r = row(page, `Nachfassen: ${subject}`);
  await expect(r).toBeVisible({ timeout: 20_000 });
  await expect(r.locator('.m-due')).toBeVisible();
});

test('Lane-Kategorien sind verborgen und bleiben beim Taggen erhalten', async ({ page }) => {
  await openApp(page);
  const acc = await account(page);
  await api(page, 'save_lanes', { accountId: acc.id, lanes: ['ToDo', 'Warten'] });
  const inbox = (await api(page, 'list_folders', { mailboxId: acc.mailboxes[0].id })).find((f) => f.wellKnown === 'inbox');
  const page1 = await api(page, 'list_messages', { mailboxId: acc.mailboxes[0].id, folderId: inbox.id, limit: 200 });
  const m = page1.items.find((x) => x.subject.startsWith('Kurze Frage zum Rahmenvertrag'));
  await api(page, 'set_categories', { mailboxId: acc.mailboxes[0].id, ids: [m.id], categories: [] });
  await api(page, 'set_lane', { mailboxId: acc.mailboxes[0].id, ids: [m.id], lane: 'ToDo' });
  await page.reload();
  await select(page, 'Kurze Frage zum Rahmenvertrag');
  // nothing lane-like visible
  await expect(page.locator('.rp-tags .tag-pill', { hasText: '[K_' })).toHaveCount(0);
  await expect(row(page, 'Kurze Frage zum Rahmenvertrag').locator('.cat-chip', { hasText: '[K_' })).toHaveCount(0);
  await page.locator('[data-testid=tag-add]').click();
  const pop = page.locator('[data-testid=tag-popover]');
  await expect(pop).toBeVisible();
  await expect(pop.locator('.tag-opt', { hasText: '[K_' })).toHaveCount(0);
  await pop.locator('.tagpop-filter').fill('K_');
  await expect(pop.locator('.tag-opt')).toHaveCount(0);
  await pop.locator('.tagpop-filter').fill('Wichtig');
  await pop.locator('.tagpop-filter').press('Enter');
  await expect(page.locator('.rp-tags .tag-pill', { hasText: 'Wichtig' })).toBeVisible();
  await page.keyboard.press('Escape');
  // lane survived the tag change on the server
  const after = await api(page, 'list_messages', { mailboxId: acc.mailboxes[0].id, folderId: inbox.id, limit: 200 });
  const m2 = after.items.find((x) => x.id === m.id);
  expect(m2.categories).toContain('[K_ToDo]');
  expect(m2.categories).toContain('Wichtig');
  // category filter menu does not offer lanes either
  await page.locator('[data-testid=filter-category]').click();
  await expect(page.locator('.menu .mi', { hasText: '[K_' })).toHaveCount(0);
  await page.keyboard.press('Escape');
});

test('Einstellungen › Kanban: Lanes anlegen, prüfen, sortieren, speichern', async ({ page }) => {
  await openApp(page);
  const acc = await account(page);
  await api(page, 'save_lanes', { accountId: acc.id, lanes: [] });
  await page.reload();
  await page.locator('.msg').first().waitFor();
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=settings-tab-kanban]').click();
  const ed = page.locator('[data-testid=lane-editor]');
  await expect(ed.locator('.lane-none')).toBeVisible();
  const input = ed.locator('[data-testid=lane-input]');
  await input.fill('ToDo');
  await input.press('Enter');
  await input.fill('Warten');
  await ed.locator('[data-testid=lane-add]').click();
  await input.fill('Ungültig!');
  await input.press('Enter');
  await expect(page.locator('.toast', { hasText: 'Ungültiger Name' })).toBeVisible();
  await input.fill('todo');
  await input.press('Enter');
  await expect(page.locator('.toast', { hasText: 'gibt es bereits' })).toBeVisible();
  await expect(ed.locator('[data-testid=lane-item]')).toHaveCount(2);
  // reorder: Warten nach oben
  await ed.locator('[data-testid=lane-item][data-name=Warten] [data-testid=lane-up]').click();
  await expect(ed.locator('[data-testid=lane-item] .lane-name')).toHaveText(['Warten', 'ToDo']);
  await page.locator('#settings-ok').click();
  await expect(page.locator('.toast', { hasText: 'Einstellungen gespeichert' })).toBeVisible();
  expect(await api(page, 'get_lanes', { accountId: acc.id })).toEqual(['Warten', 'ToDo']);
  // persisted after reload
  await page.reload();
  await page.locator('.msg').first().waitFor();
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await page.locator('[data-testid=settings-tab-kanban]').click();
  await expect(page.locator('[data-testid=lane-item] .lane-name')).toHaveText(['Warten', 'ToDo']);
});

test('Kanban-Ansicht: Spalten, Drag & Drop, Taste 2, Lesebereich unten/rechts', async ({ page }) => {
  await openApp(page);
  const acc = await account(page);
  const mb = acc.mailboxes[0].id;
  await api(page, 'save_lanes', { accountId: acc.id, lanes: ['ToDo', 'Warten'] });
  const inbox = (await api(page, 'list_folders', { mailboxId: mb })).find((f) => f.wellKnown === 'inbox');
  const all = await api(page, 'list_messages', { mailboxId: mb, folderId: inbox.id, limit: 500 });
  const laned = all.items.filter((m) => (m.categories || []).some((c) => c.startsWith('[K_'))).map((m) => m.id);
  if (laned.length) await api(page, 'set_lane', { mailboxId: mb, ids: laned, lane: null });
  await page.reload();
  await page.locator('.msg').first().waitFor();
  await page.locator('[data-testid=view-kanban]').first().click();
  const board = page.locator('[data-testid=kanban-board]');
  await expect(board).toBeVisible();
  await expect(page.locator('#listpane')).toBeHidden();
  await expect(board.locator('.kb-col .kb-colname')).toHaveText(['Ohne Lane', 'ToDo', 'Warten']);
  const card = board.locator('[data-testid=kb-col-none] .kb-card', { hasText: 'Wochenbericht Vertrieb KW 42' });
  await expect(card).toBeVisible();
  // reading pane below the board by default
  await card.click();
  await expect(page.locator('.rp-subject')).toContainText('Wochenbericht Vertrieb KW 42');
  const bb = await board.boundingBox();
  const rb = await page.locator('#readpane').boundingBox();
  expect(rb.y).toBeGreaterThan(bb.y + bb.height - 5);
  // drag to ToDo
  await card.dragTo(board.locator('[data-testid=kb-col-ToDo]'));
  await expect(board.locator('[data-testid=kb-col-ToDo] .kb-card', { hasText: 'Wochenbericht Vertrieb KW 42' })).toBeVisible();
  // survives a reload (server-side category)
  await page.reload();
  await expect(board.locator('[data-testid=kb-col-ToDo] .kb-card', { hasText: 'Wochenbericht Vertrieb KW 42' })).toBeVisible();
  // keyboard: select and press 2 → Warten
  await board.locator('.kb-card', { hasText: 'Wochenbericht Vertrieb KW 42' }).click();
  await page.keyboard.press('2');
  await expect(board.locator('[data-testid=kb-col-Warten] .kb-card', { hasText: 'Wochenbericht Vertrieb KW 42' })).toBeVisible();
  // undo via toast
  await page.locator('.toast', { hasText: 'Warten' }).locator('[data-testid=toast-action]').click();
  await expect(board.locator('[data-testid=kb-col-ToDo] .kb-card', { hasText: 'Wochenbericht Vertrieb KW 42' })).toBeVisible();
  // Shift+→ moves to the neighbouring lane
  await board.locator('.kb-card', { hasText: 'Wochenbericht Vertrieb KW 42' }).click();
  await page.keyboard.press('Shift+ArrowRight');
  await expect(board.locator('[data-testid=kb-col-Warten] .kb-card', { hasText: 'Wochenbericht Vertrieb KW 42' })).toBeVisible();
  // reading pane to the right
  await board.locator('[data-testid=kb-toggle-pane]').click();
  // The layout switches after the setting is saved: wait for it, don't measure once.
  await expect.poll(async () => {
    const bb2 = await board.boundingBox();
    const rb2 = await page.locator('#readpane').boundingBox();
    return rb2.x - (bb2.x + bb2.width);
  }).toBeGreaterThan(-5);
  await board.locator('[data-testid=kb-toggle-pane]').click();
  // back to the list
  await board.locator('[data-testid=view-list]').click();
  await expect(page.locator('#listpane')).toBeVisible();
  await expect(board).toBeHidden();
});

test('Kanban ohne Lanes zeigt Hinweis mit Einstellungs-Link', async ({ page }) => {
  await openApp(page);
  const acc = await account(page);
  const before = await api(page, 'get_lanes', { accountId: acc.id });
  await api(page, 'save_lanes', { accountId: acc.id, lanes: [] });
  await page.reload();
  await page.locator('.msg').first().waitFor();
  await page.locator('[data-testid=view-kanban]').first().click();
  await expect(page.locator('[data-testid=kb-empty-lanes]')).toBeVisible();
  await page.locator('[data-testid=kb-setup]').click();
  await expect(page.locator('[data-testid=lane-editor]')).toBeVisible();
  await page.keyboard.press('Escape');
  await api(page, 'save_lanes', { accountId: acc.id, lanes: before });
});

test('Taste E archiviert die ausgewählte Mail', async ({ page }) => {
  await openApp(page);
  await select(page, 'Protokoll Lenkungskreis');
  await page.locator('#msglist').focus();
  await page.keyboard.press('e');
  await expect(page.locator('.toast', { hasText: 'archiviert' })).toBeVisible();
  await expect(row(page, 'Protokoll Lenkungskreis')).toHaveCount(0);
  await openFolder(page, 'Archiv');
  await expect(row(page, 'Protokoll Lenkungskreis')).toBeVisible({ timeout: 15_000 });
});

test('Mail an mich mit #morgen wird automatisch auf Wiedervorlage gelegt', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('#subject')).toBeVisible();
  const to = popup.locator('.rcpt-input[data-field=to]');
  await to.fill('max.mustermann@contoso.example');
  await to.press('Tab');
  const subject = `Unterlagen sortieren ${Date.now()} #morgen`;
  await popup.locator('#subject').fill(subject);
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=compose-send]').click();
  await closed;
  await page.keyboard.press('F9');
  await expect(page.locator('.toast', { hasText: 'automatisch auf Wiedervorlage' })).toBeVisible({ timeout: 20_000 });
  await expect(row(page, subject)).toHaveCount(0);
  await openFolder(page, 'Wiedervorlage');
  await expect(row(page, subject)).toBeVisible({ timeout: 15_000 });
});
