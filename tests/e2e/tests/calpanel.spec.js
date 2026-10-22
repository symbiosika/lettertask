const { test, expect } = require('@playwright/test');
const { openApp } = require('./helpers');

test('Kalender unten: 7 Tage einblenden, blättern, Zustand bleibt', async ({ page }) => {
  await openApp(page);
  const panel = page.locator('[data-testid=cal-panel]');
  await expect(panel).toBeHidden();
  await page.locator('[data-testid=tb-calpanel]').click();
  await expect(panel).toBeVisible();
  await expect(panel.locator('.cp-dayhead')).toHaveCount(7);
  await expect(panel.locator('.cp-hour').first()).toHaveText('07:00');
  await expect(panel.locator('.cp-hour').last()).toHaveText('19:00');
  await expect(panel.locator('.cp-dayhead.today')).toHaveCount(1);
  // the demo calendar has appointments this week
  await expect(panel.locator('[data-testid=cp-event]').first()).toBeVisible();
  // mail list and reading pane stay usable above it
  await expect(page.locator('#msglist')).toBeVisible();
  const range = await panel.locator('[data-testid=cp-range]').textContent();
  await panel.locator('[data-testid=cp-next]').click();
  await expect(panel.locator('[data-testid=cp-range]')).not.toHaveText(range);
  await expect(panel.locator('.cp-dayhead.today')).toHaveCount(0);
  await panel.locator('[data-testid=cp-today]').click();
  await expect(panel.locator('[data-testid=cp-range]')).toHaveText(range);
  await page.screenshot({ path: 'test-results/calpanel.png' });
  // remembered after reload
  await page.reload();
  await expect(page.locator('[data-testid=cal-panel]')).toBeVisible();
  await page.locator('[data-testid=tb-calpanel]').click();
  await expect(page.locator('[data-testid=cal-panel]')).toBeHidden();
});
