const { test, expect } = require('@playwright/test');
const { openApp, popupFrom } = require('./helpers');

// The demo has one Microsoft 365 account: two more are simulated.
const EXTRA = [
  { id: 'm365:b', email: 'b@fabrikam.example', displayName: 'B', kind: 'graph', avatar: null, mailboxes: [] },
  { id: 'm365:c', email: 'c@northwind.example', displayName: 'C', kind: 'graph', avatar: null, mailboxes: [] },
];
const CALS = {
  'm365:b': [{ id: 'cal-b1', accountId: 'm365:b', name: 'Kalender', color: '#3a7', isDefault: true, canEdit: true },
    { id: 'cal-b2', accountId: 'm365:b', name: 'Projekt Fabrikam', color: '#a73', isDefault: false, canEdit: true }],
  'm365:c': [{ id: 'cal-c1', accountId: 'm365:c', name: 'Northwind Team', color: '#37a', isDefault: true, canEdit: true },
    { id: 'cal-c2', accountId: 'm365:c', name: 'Nur lesen', color: '#777', isDefault: false, canEdit: false }],
};

test('Neuer Termin: Kalender aller Konten wählbar, gespeichert im gewählten Konto', async ({ page, context }) => {
  await context.route('**/api/call/list_accounts', async (route) => {
    const res = await route.fetch();
    await route.fulfill({ response: res, json: [...(await res.json()), ...EXTRA] });
  });
  await context.route('**/api/call/list_calendars', async (route) => {
    const { accountId } = route.request().postDataJSON();
    if (CALS[accountId]) return route.fulfill({ json: CALS[accountId] });
    return route.fallback();
  });
  let created = null;
  await context.route('**/api/call/create_event', async (route) => {
    created = route.request().postDataJSON();
    await route.fulfill({ json: { id: 'new', accountId: created.accountId, calendarId: created.calendarId, subject: 'x', start: created.input.start, end: created.input.end } });
  });
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new-event]').click());
  const sel = popup.locator('select[name=calendar]');
  await expect(sel.locator('optgroup')).toHaveCount(3);
  await expect(sel.locator('optgroup[label="b@fabrikam.example"] option')).toHaveText(['Kalender (Standard)', 'Projekt Fabrikam']);
  // read-only calendars are not offered
  await expect(sel.locator('optgroup[label="c@northwind.example"] option')).toHaveText(['Northwind Team (Standard)']);
  // the own default stays preselected
  await expect(sel.locator('option:checked')).toHaveAttribute('data-account', /^(?!m365:)/);
  await sel.selectOption({ label: 'Northwind Team (Standard)' });
  await popup.locator('input[name=subject]').fill('Termin im dritten Konto');
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=ev-save]').click();
  await closed;
  expect(created).toMatchObject({ accountId: 'm365:c', calendarId: 'cal-c1' });
});
