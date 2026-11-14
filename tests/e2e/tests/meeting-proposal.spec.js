const { test, expect } = require('@playwright/test');
const { openApp, select } = require('./helpers');

const SUBJECT = 'Neue vorgeschlagene Zeit: Kundentermin Alpha';

test('Zeitvorschlag zeigt neue und bisherige Zeit, Antwortstatus und Kalender drumherum', async ({ page }) => {
  await openApp(page);
  await select(page, SUBJECT);
  const bar = page.getByTestId('meeting-bar');
  await expect(bar).toContainText('Neue Zeit vorgeschlagen');
  await expect(bar.getByTestId('meeting-answer')).toHaveText('Mit Vorbehalt');
  const details = page.getByTestId('invite-details');
  // Demo: the customer asks for one hour later (11–12 → 12–13).
  await expect(details.getByTestId('invite-proposed')).toContainText(/Vorgeschlagen: \w+, \d{1,2}\. \w+ \d{4} · 12:00 – 13:00 \(1 Std\.\)/);
  await expect(details.getByTestId('invite-current')).toContainText(/Bisher: \w+, \d{1,2}\. \w+ \d{4} · 11:00 – 12:00 \(1 Std\.\)/);
  const panel = page.getByTestId('proposal-respond');
  await expect(panel.getByTestId('proposal-slot')).toContainText('12:00 – 13:00');
  await expect(panel.getByTestId('proposal-conflict-summary')).toBeVisible();
  await expect(panel.getByTestId('proposal-accept')).toBeVisible();
  await expect(panel.getByTestId('proposal-decline')).toBeVisible();
});

test('Andere Zeit zeigt Konflikte der gewählten Zeit – nur aus eigenen Kalendern', async ({ page }) => {
  await openApp(page);
  // A meeting at 15:00 on the day of the proposal (set once the day is known).
  let conflict = null;
  await page.route('**/api/call/list_events', async (route) => {
    const res = await route.fetch();
    const list = await res.json();
    const a = route.request().postDataJSON();
    if (conflict && new Date(a.start) < conflict.end && new Date(a.end) > conflict.start) {
      list.push({ id: 'x-conflict', accountId: a.accountId, calendarId: a.calendarIds[0], subject: 'Austausch Symbiosika', isAllDay: false, isCancelled: false, showAs: 'busy',
        start: conflict.start.toISOString(), end: conflict.end.toISOString(), attendees: [], categories: [] });
    }
    await route.fulfill({ response: res, json: list });
  });
  await select(page, SUBJECT);
  const panel = page.getByTestId('proposal-respond');
  await expect(panel.getByTestId('proposal-conflict-summary')).toHaveText(/Keine Konflikte/);
  await panel.getByTestId('proposal-other-toggle').click();
  await expect(panel.getByTestId('proposal-accept')).toBeHidden();
  const start = await panel.getByTestId('proposal-other-start').inputValue();
  // November: Berlin is UTC+1.
  conflict = { start: new Date(start.slice(0, 11) + '15:00:00+01:00'), end: new Date(start.slice(0, 11) + '16:00:00+01:00') };
  await panel.getByTestId('proposal-other-start').fill(start.slice(0, 11) + '14:00');
  await panel.getByTestId('proposal-other-start').dispatchEvent('change');
  // The length is kept.
  await expect(panel.getByTestId('proposal-other-end')).toHaveValue(start.slice(0, 11) + '15:00');
  // Adjacent is no conflict; Laura's workshop (13–16, her calendar) is not the user's.
  await expect(panel.getByTestId('proposal-conflict-summary')).toHaveText(/Keine Konflikte/);
  await expect(panel.getByTestId('proposal-around')).toContainText(['Austausch Symbiosika']);
  await panel.getByTestId('proposal-other-start').fill(start.slice(0, 11) + '14:30');
  await panel.getByTestId('proposal-other-start').dispatchEvent('change');
  await expect(panel.getByTestId('proposal-conflict-summary')).toHaveText(/1 Konflikt/);
  await expect(panel.getByTestId('proposal-conflict')).toContainText('Austausch Symbiosika');
  await expect(panel).not.toContainText('Kundenworkshop Fabrikam');
});

test('Ablehnen öffnet eine Antwort an den Absender', async ({ page }) => {
  await openApp(page);
  await select(page, SUBJECT);
  const [popup] = await Promise.all([
    page.waitForEvent('popup', { timeout: 5000 }).catch(() => null),
    page.getByTestId('proposal-decline').click(),
  ]);
  const target = popup || page;
  await expect(target.locator('body')).toContainText('Thomas Müller');
});

test('Vorschlag annehmen verschiebt den Termin auf die vorgeschlagene Zeit', async ({ page }) => {
  const moved = [];
  await page.route('**/api/call/move_event', async (route) => {
    moved.push(route.request().postDataJSON());
    await route.fulfill({ json: {} });
  });
  await openApp(page);
  await select(page, SUBJECT);
  await page.getByTestId('proposal-accept').click();
  await expect(page.locator('.toast').last()).toContainText('Termin verschoben');
  expect(moved).toHaveLength(1);
  expect(moved[0].sendUpdates).toBe(true);
  expect(new Date(moved[0].end) - new Date(moved[0].start)).toBe(3600e3);
  expect(new Date(moved[0].start).getMinutes()).toBe(0);
});
