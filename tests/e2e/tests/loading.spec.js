const { test, expect } = require('@playwright/test');
const { openApp, folder, popupFrom, select } = require('./helpers');

/** Holds back one backend command until release() is called; counts the calls. */
async function holdCommand(context, cmd) {
  let release;
  const gate = new Promise((r) => { release = r; });
  const hold = { calls: 0, release: () => release() };
  await context.route('**/api/call/' + cmd, async (route) => {
    hold.calls++;
    await gate;
    await route.continue();
  });
  return hold;
}

test('Senden zeigt einen Spinner und lässt sich nicht doppelt auslösen', async ({ page, context }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('#subject')).toBeVisible();
  const to = popup.locator('.rcpt-input[data-field=to]');
  await to.fill('kunde@example.com');
  await to.press('Tab');
  await popup.locator('#subject').fill(`Doppelklick ${Date.now()}`);
  const hold = await holdCommand(context, 'send_draft');
  const send = popup.locator('[data-testid=compose-send]');
  await send.click();
  await expect(send).toHaveClass(/pending/);
  await expect(send).toContainText('Wird gesendet…');
  await expect(send).toBeDisabled();
  await send.click({ force: true });
  await popup.keyboard.press('Control+Enter');
  expect(hold.calls).toBe(1);
  const closed = popup.waitForEvent('close');
  hold.release();
  await closed;
  expect(hold.calls).toBe(1);
});

test('Ordner öffnen zeigt einen Spinner, bis die Liste da ist', async ({ page, context }) => {
  await openApp(page);
  const hold = await holdCommand(context, 'list_messages');
  await folder(page, 'Archiv').click();
  await expect(page.locator('[data-testid=list-loading]')).toBeVisible();
  await expect(page.locator('.msglist .empty')).toHaveCount(0);
  await expect(page.locator('.listpane .lh-busy')).toHaveClass(/on/);
  hold.release();
  await expect(page.locator('[data-testid=list-loading]')).toHaveCount(0);
  await expect(page.locator('.listpane .lh-busy')).not.toHaveClass(/on/);
});

test('Aktionsbutton zeigt einen Spinner, solange die Aktion läuft', async ({ page, context }) => {
  await openApp(page);
  await page.locator('.msg').first().locator('.m-subj').click();
  const hold = await holdCommand(context, 'archive_messages');
  const archive = page.locator('[data-testid=tb-archive]');
  await archive.click();
  await expect(archive).toHaveClass(/pending/);
  await archive.click();
  expect(hold.calls).toBe(1);
  hold.release();
  await expect(archive).not.toHaveClass(/pending/);
});

test('Kalender zeigt beim Laden der Termine einen Hinweis', async ({ page, context }) => {
  await openApp(page);
  const hold = await holdCommand(context, 'list_events');
  await page.locator('[data-testid=nav-calendar]').click();
  await expect(page.locator('[data-testid=cal-loading]')).toBeVisible();
  hold.release();
  await expect(page.locator('[data-testid=cal-loading]')).toBeHidden();
  await expect(page.locator('.timegrid .ev').first()).toBeVisible();
});

test('Besprechungsanfrage: Antwort lässt sich nicht doppelt senden', async ({ page, context }) => {
  await openApp(page);
  await select(page, 'Einladung: Präsentation Angebot Northwind');
  const bar = page.locator('#readpane .infobar.meeting');
  await expect(bar).toBeVisible();
  const hold = await holdCommand(context, 'respond_event');
  const accept = bar.locator('.btn', { hasText: 'Annehmen' });
  await accept.click();
  await expect(accept).toHaveClass(/pending/);
  await accept.click();
  await bar.locator('.btn', { hasText: 'Ablehnen' }).click();
  expect(hold.calls).toBe(1);
  hold.release();
  // Answered once is answered: no buttons any more, the invitation is archived.
  await expect(page.locator('.msg', { hasText: 'Einladung: Präsentation Angebot Northwind' })).toHaveCount(0);
  await expect(page.locator('.toast', { hasText: 'Einladung archiviert' })).toBeVisible();
  expect(hold.calls).toBe(1);
  await folder(page, 'Archiv').click();
  await expect(page.locator('.msg', { hasText: 'Einladung: Präsentation Angebot Northwind' })).toHaveCount(1);
});
