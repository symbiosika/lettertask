const { test, expect } = require('@playwright/test');
const { openApp, popupFrom } = require('./helpers');

const pad = (n) => String(n).padStart(2, '0');
const ymd = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

test('Wochenansicht zeigt Termine', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=nav-calendar]').click();
  await page.locator('[data-testid=tb-view-week]').click();
  await expect(page.locator('.tg-dayhead')).toHaveCount(7);
  await expect(page.locator('.timegrid .ev').first()).toBeVisible();
  expect(await page.locator('.timegrid .ev').count()).toBeGreaterThan(2);
  // peek on click
  await page.locator('.tg-col .ev').first().click();
  await expect(page.locator('.peek .peek-title')).toBeVisible();
  // month view
  await page.locator('[data-testid=tb-view-month]').click();
  await expect(page.locator('.mg-cell')).toHaveCount(42);
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
  // open again and verify values round-trip
  const edit = await popupFrom(page, () => page.locator('.timegrid .ev', { hasText: subject }).dblclick());
  await expect(edit.locator('input[name=subject]')).toHaveValue(subject);
  await expect(edit.locator('input[name=location]')).toHaveValue('Raum 1');
  await expect(edit.locator('input[name=startTime]')).toHaveValue('10:00');
  await edit.close({ runBeforeUnload: false });
});
