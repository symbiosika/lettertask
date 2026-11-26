// Import of .ics files: from a mail attachment and from a file on disk. The
// event opens filled in and unsaved, can be adjusted and its calendar chosen.
const { test, expect } = require('@playwright/test');
const { openApp, select, popupFrom, api } = require('./helpers');

async function eventsOn(page, day) {
  const accountId = (await api(page, 'list_accounts')).find((x) => x.kind !== 'imap').id;
  const calendarIds = (await api(page, 'list_calendars', { accountId })).map((c) => c.id);
  return api(page, 'list_events', { accountId, calendarIds, start: `${day}T00:00:00Z`, end: `${day}T23:59:59Z` });
}

test('ICS-Anhang einer Mail: anpassen, Kalender wählen, importieren', async ({ page }) => {
  await openApp(page);
  await select(page, 'Termin Sommerfest zum Eintragen');
  const att = page.locator('.rp-attachments .attachment', { hasText: 'Sommerfest.ics' });
  await expect(att).toHaveAttribute('data-ics', '1');
  const popup = await popupFrom(page, () => att.click());
  await expect(popup.locator('[data-testid=ev-import-banner]')).toBeVisible();
  await expect(popup.locator('input[name=subject]')).toHaveValue('Sommerfest Contoso');
  await expect(popup.locator('input[name=location]')).toHaveValue('Biergarten am See');
  await expect(popup.locator('input[name=startDate]')).toHaveValue('2027-07-02');
  await expect(popup.locator('input[name=startTime]')).toHaveValue('17:00');
  await expect(popup.locator('input[name=endTime]')).toHaveValue('23:00');
  await expect(popup.frameLocator('iframe').first().locator('body')).toContainText('Grillen, Musik und gute Laune.');
  // Adjust before saving, choose another calendar.
  const subject = `Sommerfest (importiert) ${Date.now()}`;
  await popup.locator('input[name=subject]').fill(subject);
  await popup.locator('input[name=startTime]').fill('18:00');
  const options = popup.locator('select[name=calendar] option');
  expect(await options.count()).toBeGreaterThan(1);
  const target = await options.nth(1).getAttribute('value');
  await popup.locator('select[name=calendar]').selectOption(target);
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=ev-save]').click();
  await closed;
  const ev = (await eventsOn(page, '2027-07-02')).find((e) => e.subject === subject);
  expect(ev).toBeTruthy();
  expect(ev.calendarId).toBe(target);
  expect(new Date(ev.start).toISOString()).toBe('2027-07-02T16:00:00.000Z');
});

test('ICS-Datei mit mehreren Terminen im Kalender importieren', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  const tag = Date.now();
  const ics = [
    'BEGIN:VCALENDAR', 'VERSION:2.0',
    'BEGIN:VEVENT', `UID:a-${tag}`, `SUMMARY:Messe Tag 1 ${tag}`, 'DTSTART:20270915T070000Z', 'DTEND:20270915T150000Z', 'END:VEVENT',
    'BEGIN:VEVENT', `UID:b-${tag}`, `SUMMARY:Messe Tag 2 ${tag}`, 'DTSTART;VALUE=DATE:20270916', 'DTEND;VALUE=DATE:20270917', 'END:VEVENT',
    'BEGIN:VEVENT', `UID:c-${tag}`, `SUMMARY:Messe Abbau ${tag}`, 'DTSTART:20270917T070000Z', 'DTEND:20270917T090000Z', 'END:VEVENT',
    'END:VCALENDAR', '',
  ].join('\r\n');
  const [chooser] = await Promise.all([page.waitForEvent('filechooser'), page.locator('[data-testid=tb-import-ics]').click()]);
  await chooser.setFiles({ name: 'messe.ics', mimeType: 'text/calendar', buffer: Buffer.from(ics) });
  const list = page.locator('[data-testid=ics-list]');
  await expect(list.locator('.ics-row')).toHaveCount(3);
  await expect(list).toContainText('16.09.2027 (ganztägig)');
  // The last one is left out.
  await list.locator('.ics-row', { hasText: 'Messe Abbau' }).locator('input[type=checkbox]').uncheck();
  await page.locator('.modal-footer button', { hasText: 'Importieren' }).click();
  await expect(page.locator('.toast', { hasText: '2 Termine importiert.' })).toBeVisible();
  expect((await eventsOn(page, '2027-09-15')).some((e) => e.subject === `Messe Tag 1 ${tag}`)).toBe(true);
  const day2 = (await eventsOn(page, '2027-09-16')).find((e) => e.subject === `Messe Tag 2 ${tag}`);
  expect(day2 && day2.isAllDay).toBe(true);
  expect((await eventsOn(page, '2027-09-17')).some((e) => e.subject === `Messe Abbau ${tag}`)).toBe(false);
});

test('Ungültige Kalenderdatei meldet einen Fehler', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  const [chooser] = await Promise.all([page.waitForEvent('filechooser'), page.locator('[data-testid=tb-import-ics]').click()]);
  await chooser.setFiles({ name: 'kaputt.ics', mimeType: 'text/calendar', buffer: Buffer.from('kein Kalender') });
  await expect(page.locator('.toast', { hasText: 'keinen Termin' })).toBeVisible();
});
