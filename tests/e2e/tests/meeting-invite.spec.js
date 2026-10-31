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

test('Verschobener Termin: neue Zeit fett, alte durchgestrichen – auch wenn der Termin nicht lesbar ist', async ({ page }) => {
  await page.route('**/api/call/get_message', async (route) => {
    const res = await route.fetch();
    const msg = await res.json();
    if (msg.subject === SUBJECT) {
      expect(msg.invite && msg.invite.start).toBeTruthy(); // comes from the mail itself
      msg.invite.start = '2026-11-02T09:00:00Z';
      msg.invite.end = '2026-11-02T10:00:00Z';
      msg.invite.previousStart = '2026-10-27T12:30:00Z';
      msg.invite.previousEnd = '2026-10-27T13:30:00Z';
    }
    await route.fulfill({ response: res, json: msg });
  });
  // The event cannot be read (other mailbox, deleted, …): the details stay.
  await page.route('**/api/call/get_event', (route) => route.fulfill({ status: 404, json: { error: 'nicht gefunden' } }));
  await openInvite(page);
  const when = page.getByTestId('invite-details').getByTestId('invite-when');
  await expect(when).toContainText('Montag, 2. November 2026 · 10:00 – 11:00 (1 Std.)');
  await expect(when.getByTestId('invite-previous')).toHaveText('Dienstag, 27. Oktober 2026 · 13:30 – 14:30 (1 Std.)');
  await expect(page.getByTestId('meeting-bar')).toContainText('Termin verschoben');
});

test('Antwort mit Nachricht, ohne Benachrichtigung und mit Zeitvorschlag', async ({ page }) => {
  const sent = [];
  await page.route('**/api/call/respond_event', async (route) => {
    sent.push(route.request().postDataJSON());
    await route.fulfill({ json: null });
  });
  await openInvite(page);
  const panel = page.getByTestId('meeting-respond');
  await expect(panel).toContainText('Auf dieses Ereignis antworten');
  await panel.getByTestId('meeting-note').fill('Passt mir leider nicht.');
  await panel.getByTestId('propose-toggle').click();
  // Prefilled with the meeting's own time.
  await expect(panel.getByTestId('propose-start')).not.toHaveValue('');
  await panel.getByTestId('propose-start').fill('2026-11-02T10:00');
  await panel.getByTestId('propose-end').fill('2026-11-02T11:00');
  await panel.getByTestId('propose-send').click();
  await expect(page.getByTestId('meeting-answered')).toContainText('andere Zeit vorgeschlagen');
  expect(sent).toHaveLength(1);
  expect(sent[0].response).toBe('tentativelyAccept');
  expect(sent[0].comment).toBe('Passt mir leider nicht.');
  expect(sent[0].sendResponse).toBe(true);
  expect(sent[0].proposedNewTime).toEqual({ start: '2026-11-02T09:00:00.000Z', end: '2026-11-02T10:00:00.000Z' });
});

test('Zusage ohne Mail an den Organisator', async ({ page }) => {
  const sent = [];
  await page.route('**/api/call/respond_event', async (route) => {
    sent.push(route.request().postDataJSON());
    await route.fulfill({ json: null });
  });
  await openInvite(page);
  await page.getByTestId('meeting-notify').uncheck();
  await page.locator('#readpane .infobar.meeting .btn', { hasText: 'Annehmen' }).click();
  await expect(page.getByTestId('meeting-answered')).toHaveText('Zugesagt');
  expect(sent[0]).toMatchObject({ response: 'accept', sendResponse: false, comment: '' });
});
