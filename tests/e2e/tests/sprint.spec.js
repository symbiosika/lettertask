// "14 Tage" overview: appointments and due tasks side by side, each switchable.
const { test, expect } = require('@playwright/test');
const { openApp, api } = require('./helpers');

test('14 Tage: Termine und Aufgaben pro Tag, einzeln abwählbar', async ({ page }) => {
  await openApp(page);
  const [acc] = await api(page, 'list_accounts', {});
  const mb = acc.mailboxes[0].id;
  const at = (days, hh, mm = 0) => { const d = new Date(); d.setDate(d.getDate() + days); d.setHours(hh, mm, 0, 0); return d.toISOString(); };
  const tasks = [
    ['Angebot an Kunden schicken', 3, 9],
    ['Rückruf Steuerberater', 4, 12],
    ['Reisekosten einreichen', 1, 8],
    ['Vertrag gegenlesen', 2, 14],
    ['Präsentation vorbereiten', 5, 10],
    ['Quartalsbericht abschließen', 9, 8],
  ];
  for (const t of tasks) t[0] += ` #${Date.now() % 10000}`;
  for (const [subject, d, hh, mm] of tasks) await api(page, 'create_task', { mailboxId: mb, subject, bodyHtml: '', due: at(d, hh, mm) });

  await page.locator('[data-testid=nav-calendar]').click();
  await page.locator('[data-testid=tb-view-sprint]').click();
  await page.locator('[data-testid=tb-today]').click();
  const days = page.locator('[data-testid=sprint-day]');
  await expect(days).toHaveCount(14);
  const today = days.first();
  await expect(today).toHaveClass(/today/);
  // the test runner is not in the app's time zone: only check the day, not the time
  for (const [subject] of tasks) await expect(page.locator('[data-testid=sprint-task]', { hasText: subject })).toHaveCount(1);
  await expect(days.nth(1).locator('.sp-item', { hasText: 'Reisekosten einreichen' })).toBeVisible();
  await expect(page.locator('[data-testid=sprint-grid] .ev').first()).toBeVisible();
  await page.screenshot({ path: process.env.SPRINT_SHOT || 'test-results/sprint.png' });

  await page.locator('[data-testid=sprint-show-tasks]').click();
  await expect(page.locator('[data-testid=sprint-task]')).toHaveCount(0);
  await expect(page.locator('[data-testid=sprint-grid] .ev').first()).toBeVisible();
  await page.locator('[data-testid=sprint-show-events]').click();
  await expect(page.locator('[data-testid=sprint-grid] .ev')).toHaveCount(0);
  await page.locator('[data-testid=sprint-show-tasks]').click();
  await page.locator('[data-testid=sprint-show-events]').click();
  await expect(page.locator('[data-testid=sprint-task]').first()).toBeVisible();
});
