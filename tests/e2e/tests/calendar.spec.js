const { test, expect } = require('@playwright/test');
const { openApp, popupFrom } = require('./helpers');

const pad = (n) => String(n).padStart(2, '0');
const ymd = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

test('Wochenansicht zeigt Termine', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  await page.locator('[data-testid=tb-view-week]').click();
  await expect(page.locator('.tg-dayhead')).toHaveCount(7);
  await expect(page.locator('#ab-period')).toHaveText(/\d/);
  await expect(page.locator('.timegrid .ev').first()).toBeVisible();
  expect(await page.locator('.timegrid .ev').count()).toBeGreaterThan(2);
  // peek on click
  await page.locator('.tg-col .ev').first().click();
  await expect(page.locator('.peek .peek-title')).toBeVisible();
  // month view
  await page.locator('[data-testid=tb-view-month]').click();
  await expect(page.locator('.mg-cell')).toHaveCount(42);
  await page.locator('[data-testid=tb-view-workweek]').click();
  await expect(page.locator('.tg-dayhead')).toHaveCount(5);
});

test('Neuen Termin anlegen', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  await page.locator('[data-testid=tb-view-week]').click();
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new-event]').click());
  const subject = `E2E Termin ${Date.now()}`;
  await popup.locator('input[name=subject]').fill(subject);
  await popup.locator('input[name=location]').fill('Raum 1');
  const today = ymd(new Date());
  await popup.locator('input[name=startDate]').fill(today);
  await popup.locator('input[name=startTime]').fill('10:00');
  await popup.locator('input[name=endDate]').fill(today);
  await popup.locator('input[name=endTime]').fill('11:00');
  const closed = popup.waitForEvent('close');
  await popup.locator('[data-testid=ev-save]').click();
  await closed;
  await expect(page.locator('.timegrid .ev', { hasText: subject })).toBeVisible({ timeout: 15_000 });
  const edit = await popupFrom(page, () => page.locator('.timegrid .ev', { hasText: subject }).dblclick());
  await expect(edit.locator('input[name=subject]')).toHaveValue(subject);
  await expect(edit.locator('input[name=location]')).toHaveValue('Raum 1');
  await expect(edit.locator('input[name=startTime]')).toHaveValue('10:00');
  await edit.close({ runBeforeUnload: false });
  await page.locator('[data-testid=nav-mail]').click();
});

test('Briefing-Karte beim Überfahren eines Termins', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  await page.locator('[data-testid=tb-view-week]').click();
  await page.locator('[data-testid=tb-today]').click();
  const ev = page.locator('.tg-col .ev', { hasText: 'Daily Stand-up' }).first();
  await ev.hover();
  const card = page.locator('[data-testid=briefing].hover');
  await expect(card).toBeVisible();
  await expect(card.locator('.peek-title')).toHaveText('Daily Stand-up');
  await expect(card.locator('[data-testid=briefing-when]')).not.toBeEmpty();
  await expect(card.locator('[data-testid=briefing-join]')).toBeVisible();
  await expect(card.locator('.bc-av').first()).toBeVisible();
  // pointer onto the card keeps it open, leaving closes it
  await card.hover();
  await expect(card).toBeVisible();
  await page.mouse.move(5, 5);
  await expect(card).toHaveCount(0);
});

test('Kalender von Personen werden angezeigt', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  await expect(page.locator('[data-testid=cal-group]')).toHaveText(/Kalender von Personen/i);
  await expect(page.locator('[data-testid=cal-item]', { hasText: 'Laura Schmidt' })).toBeVisible();
  await page.locator('[data-testid=tb-view-week]').click();
  await page.locator('[data-testid=tb-today]').click();
  const ev = page.locator('.ev', { hasText: 'Lieferantengespräch Tailspin' }).first();
  await expect(ev).toBeVisible();
  await ev.hover();
  await expect(page.locator('[data-testid=briefing].hover .bc-cal')).toContainText('Kalender von Jonas Weber');
  // hiding the colleague's calendar removes its events
  await page.locator('[data-testid=cal-item]', { hasText: 'Jonas Weber' }).locator('input').uncheck();
  await expect(page.locator('.ev', { hasText: 'Lieferantengespräch Tailspin' })).toHaveCount(0);
});

test('Kalender einer Person hinzufügen und wieder entfernen', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  await page.locator('[data-testid=cal-add-person]').first().click();
  const input = page.locator('.modal .rcpt-input');
  await input.fill('lau');
  await expect(page.locator('.rcpt-pop')).toHaveCSS('position', 'fixed');
  await input.fill('kollegin@contoso.example');
  await page.locator('.modal button', { hasText: 'Hinzufügen' }).click();
  await expect(page.locator('.modal')).toHaveCount(0);
  const item = page.locator('[data-testid=cal-item]', { hasText: 'kollegin@contoso.example' });
  await expect(item).toBeVisible();
  await expect(page.locator('[data-testid=cal-group]', { hasText: 'Kalender von Personen' })).toBeVisible();
  // label and colour can be changed
  await item.click({ button: 'right' });
  await page.locator('[data-testid=cal-rename]').click();
  await page.locator('.modal input').fill('Kollegin K.');
  await page.locator('.modal button', { hasText: 'Speichern' }).click();
  const renamed = page.locator('[data-testid=cal-item]', { hasText: 'Kollegin K.' });
  await expect(renamed).toBeVisible();
  await renamed.click({ button: 'right' });
  await page.locator('[data-testid=cal-color]').hover();
  await page.locator('[data-testid=cal-color-2bb3a3]').click();
  await expect(renamed.locator('.cal-check')).toHaveCSS('--cc', '#2bb3a3');
  await renamed.click({ button: 'right' });
  await page.locator('[data-testid=cal-remove]').click();
  await expect(renamed).toHaveCount(0);
});

test('Kalender einer Person erneut hinzufügen blendet einen ausgeblendeten wieder ein', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  const add = async () => {
    await page.locator('[data-testid=cal-add-person]').first().click();
    await page.locator('.modal .rcpt-input').fill('erneut@contoso.example');
    await page.locator('.modal button', { hasText: 'Hinzufügen' }).click();
    await expect(page.locator('.modal')).toHaveCount(0);
  };
  await add();
  await expect(page.locator('.toast', { hasText: 'wurde hinzugefügt' })).toBeVisible();
  const item = page.locator('[data-testid=cal-item]', { hasText: 'erneut@contoso.example' });
  await item.locator('input').uncheck();
  await add();
  await expect(page.locator('.toast', { hasText: 'schon vorhanden und wird wieder angezeigt' })).toBeVisible();
  await expect(item.locator('input')).toBeChecked();
  await item.click({ button: 'right' });
  await page.locator('[data-testid=cal-remove]').click();
  await expect(item).toHaveCount(0);
});
