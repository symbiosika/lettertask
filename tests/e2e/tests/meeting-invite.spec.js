const { test, expect } = require('@playwright/test');
const { openApp, row, select, openFolder } = require('./helpers');

const SUBJECT = 'Einladung: Präsentation Angebot Northwind';

async function openInvite(page) {
  await openApp(page);
  // An answered invitation is archived (loading.spec.js answers it).
  if (!(await row(page, SUBJECT).count())) await openFolder(page, 'Archiv');
  await select(page, SUBJECT);
}

test('Besprechungsanfrage zeigt Datum, Uhrzeit und Organisator', async ({ page }) => {
  await openInvite(page);
  const card = page.getByTestId('invite-details');
  await expect(card.getByTestId('invite-when')).toHaveText(/^\w+, \d{1,2}\. \w+ \d{4} · \d\d:\d\d – \d\d:\d\d \(.+\)$/);
  await expect(card).toContainText('organisiert');
  await expect(card.getByTestId('invite-series')).toHaveCount(0);
});

test('Besprechungsanfrage einer Serie zeigt die Serienregel', async ({ page }) => {
  // The demo invitation is a single meeting: answer as if it were a series.
  await page.route('**/api/call/get_event', async (route) => {
    const res = await route.fetch();
    const ev = await res.json();
    ev.type = 'seriesMaster';
    ev.recurrence = { pattern: 'weekly', interval: 1, daysOfWeek: ['monday'], dayOfMonth: 0, month: 0, startDate: '2026-10-26', endDate: null, occurrences: null };
    await route.fulfill({ response: res, json: ev });
  });
  await openInvite(page);
  await expect(page.getByTestId('invite-series')).toHaveText('Serie: Wöchentlich am Montag · ab 26.10.2026');
});
