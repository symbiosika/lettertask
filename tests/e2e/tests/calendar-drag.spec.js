const { test, expect } = require('@playwright/test');
const { openApp, popupFrom, api } = require('./helpers');

const pad = (n) => String(n).padStart(2, '0');
const ymd = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const SLOT = 24; // px per 30 minutes (calendar.js)

async function createToday(page, subject, from, to, extra = {}) {
  const accounts = await api(page, 'list_accounts');
  const accountId = accounts.find((a) => a.kind !== 'imap').id;
  const cals = await api(page, 'list_calendars', { accountId });
  const day = ymd(new Date());
  return api(page, 'create_event', {
    accountId, calendarId: cals.find((c) => c.isDefault).id,
    input: { subject, start: `${day}T${from}:00`, end: `${day}T${to}:00`, timeZone: 'Europe/Berlin', showAs: 'busy', ...extra },
  });
}

async function openDay(page) {
  await page.locator('[data-testid=nav-calendar]').click();
  await page.locator('[data-testid=tb-view-day]').click();
  await expect(page.locator('.tg-col')).toHaveCount(1);
}

const block = (page, subject) => page.locator('.timegrid .ev', { hasText: subject });

/** Drags with the real mouse from the center of `loc` (or a point in it) by dy pixels. */
async function drag(page, loc, dy, { at = 'center', dx = 0 } = {}) {
  const b = await loc.boundingBox();
  const x = b.x + b.width / 2;
  const y = at === 'center' ? b.y + Math.min(b.height / 2, 10) : at === 'bottom' ? b.y + b.height - 2 : b.y + 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  for (let i = 1; i <= 5; i++) await page.mouse.move(x + (dx * i) / 5, y + (dy * i) / 5);
  await page.mouse.up();
}

test('Bereich im Kalender ziehen legt einen längeren Termin an', async ({ page }) => {
  await openApp(page);
  await openDay(page);
  const col = page.locator('.tg-col').first();
  const slot10 = col.locator('.tg-slot[data-slot="20"]');
  await slot10.scrollIntoViewIfNeeded();
  const popup = await popupFrom(page, () => drag(page, slot10, 3 * SLOT + 4)); // 10:00 … 11:59 → 10:00–12:00
  await expect(popup.locator('input[name=startTime]')).toHaveValue('10:00');
  await expect(popup.locator('input[name=endTime]')).toHaveValue('12:00');
  await popup.close();
});

test('Termin per Drag & Drop verschieben und am Rand verlängern', async ({ page }) => {
  const subject = `Fokuszeit ${Date.now()}`;
  await openApp(page);
  await createToday(page, subject, '09:00', '10:00');
  await openDay(page);
  const ev = block(page, subject);
  await ev.scrollIntoViewIfNeeded();
  await expect(ev).toHaveAttribute('aria-label', /09:00 – 10:00/);
  // one hour down
  await drag(page, ev, 2 * SLOT);
  await expect(ev).toHaveAttribute('aria-label', /10:00 – 11:00/);
  await expect(page.locator('.toast', { hasText: 'Termin verschoben' })).toBeVisible();
  await expect(page.getByTestId('reschedule-dialog')).toHaveCount(0); // no attendees: no question
  // bottom edge: 30 minutes longer
  await drag(page, ev, SLOT, { at: 'bottom' });
  await expect(ev).toHaveAttribute('aria-label', /10:00 – 11:30/);
  // top edge: starts 15 minutes earlier
  await drag(page, ev, -SLOT / 2, { at: 'top' });
  await expect(ev).toHaveAttribute('aria-label', /09:45 – 11:30/);
  // saved on the server
  await page.reload();
  await openDay(page);
  await expect(block(page, subject)).toHaveAttribute('aria-label', /09:45 – 11:30/);
});

test('Besprechung verschieben fragt nach der Aktualisierung für die Teilnehmer', async ({ page }) => {
  const subject = `Abstimmung ${Date.now()}`;
  await openApp(page);
  await createToday(page, subject, '13:00', '13:30', { attendees: [{ name: 'Erika', address: 'erika@contoso.example', type: 'required' }] });
  await openDay(page);
  const ev = block(page, subject);
  await ev.scrollIntoViewIfNeeded();
  const calls = [];
  page.on('request', (r) => { if (r.url().endsWith('/move_event')) calls.push(r.postDataJSON()); });
  await drag(page, ev, 2 * SLOT);
  const dlg = page.getByTestId('reschedule-dialog');
  await expect(dlg).toContainText('Aktualisierung');
  await expect(dlg).toContainText('1 Teilnehmer');
  // cancel: stays where it was
  await page.locator('.modal-footer .btn', { hasText: 'Abbrechen' }).click();
  await expect(ev).toHaveAttribute('aria-label', /13:00 – 13:30/);
  expect(calls).toHaveLength(0);
  await drag(page, ev, 2 * SLOT);
  await page.locator('.modal-footer .btn', { hasText: 'Aktualisierung senden' }).click();
  await expect(ev).toHaveAttribute('aria-label', /14:00 – 14:30/);
  await expect(page.locator('.toast', { hasText: 'Aktualisierung gesendet' })).toBeVisible();
  expect(calls[0]).toMatchObject({ sendUpdates: true });
});

test('Termin in der Woche auf einen anderen Tag ziehen', async ({ page }) => {
  const subject = `Tageswechsel ${Date.now()}`;
  await openApp(page);
  await createToday(page, subject, '08:00', '08:30');
  await page.locator('[data-testid=nav-calendar]').click();
  await page.locator('[data-testid=tb-view-week]').click();
  const cols = page.locator('.tg-col');
  const idx = await cols.evaluateAll((all) => all.findIndex((c) => c.classList.contains('today')));
  const target = idx > 0 ? idx - 1 : idx + 1;
  const ev = block(page, subject);
  await ev.scrollIntoViewIfNeeded();
  const a = await cols.nth(idx).boundingBox(), b = await cols.nth(target).boundingBox();
  await drag(page, ev, 0, { dx: (b.x + b.width / 2) - (a.x + a.width / 2) });
  const targetDate = await cols.nth(target).getAttribute('data-date');
  await expect(cols.nth(target).locator('.ev', { hasText: subject })).toHaveAttribute('aria-label', /08:00 – 08:30/);
  const events = await api(page, 'list_events', {
    accountId: (await api(page, 'list_accounts')).find((x) => x.kind !== 'imap').id,
    calendarIds: (await api(page, 'list_calendars', { accountId: (await api(page, 'list_accounts')).find((x) => x.kind !== 'imap').id })).map((c) => c.id),
    start: `${targetDate}T00:00:00Z`, end: `${targetDate}T23:59:59Z`,
  });
  expect(events.some((e) => e.subject === subject)).toBe(true);
});

test('Einladungen anderer lassen sich nicht ziehen', async ({ page }) => {
  const subject = `Fremde Einladung ${Date.now()}`;
  await openApp(page);
  await createToday(page, subject, '16:00', '17:00');
  // As if somebody else organised it.
  await page.route('**/api/call/list_events', async (route) => {
    const res = await route.fetch();
    const list = (await res.json()).map((e) => (e.subject === subject ? { ...e, isOrganizer: false } : e));
    await route.fulfill({ response: res, json: list });
  });
  const calls = [];
  page.on('request', (r) => { if (r.url().endsWith('/move_event')) calls.push(r); });
  await openDay(page);
  const ev = block(page, subject);
  await ev.scrollIntoViewIfNeeded();
  await expect(ev.locator('.ev-rs')).toHaveCount(0);
  await drag(page, ev, 2 * SLOT);
  await expect(ev).toHaveAttribute('aria-label', /16:00 – 17:00/);
  expect(calls).toHaveLength(0);
});
