const { test, expect } = require('@playwright/test');
const { openApp, row, select, api } = require('./helpers');

test('Tags im Lesebereich zuweisen, entfernen und neu anlegen', async ({ page }) => {
  await openApp(page);
  await select(page, 'Neue Laptops sind da');
  await page.locator('[data-testid=tag-add]').click();
  const pop = page.locator('[data-testid=tag-popover]');
  await expect(pop).toBeVisible();
  expect(await pop.locator('.tag-opt').count()).toBeGreaterThanOrEqual(6);
  await pop.locator('[data-testid="tag-opt-Wichtig"]').click();
  await expect(pop.locator('[data-testid="tag-opt-Wichtig"]')).toHaveClass(/on/);
  await expect(page.locator('.rp-tags .tag-pill', { hasText: 'Wichtig' })).toBeVisible();
  await expect(row(page, 'Neue Laptops sind da').locator('.cat-chip', { hasText: 'Wichtig' })).toBeVisible();
  // keyboard: filter + Enter
  await pop.locator('.tagpop-filter').fill('Info');
  await pop.locator('.tagpop-filter').press('Enter');
  await expect(page.locator('.rp-tags .tag-pill', { hasText: 'Info' })).toBeVisible();
  // new category, created and assigned
  await pop.locator('.tagpop-filter').fill('');
  await pop.locator('[data-testid=tag-new]').click();
  await pop.locator('input[name=newTagName]').fill('E2E Tag');
  await pop.locator('.pcolor[data-color=preset19]').click();
  await pop.locator('[data-testid=tag-create]').click();
  await expect(page.locator('.rp-tags .tag-pill', { hasText: 'E2E Tag' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(pop).toHaveCount(0);
  await page.reload();
  await select(page, 'Neue Laptops sind da');
  for (const t of ['Wichtig', 'Info', 'E2E Tag']) await expect(page.locator('.rp-tags .tag-pill', { hasText: t })).toBeVisible();
  // remove via × on the pill
  const pill = page.locator('.rp-tags .tag-pill', { hasText: 'Info' });
  await pill.hover();
  await pill.locator('.tp-x').click();
  await expect(page.locator('.rp-tags .tag-pill', { hasText: 'Info' })).toHaveCount(0);
  await expect(row(page, 'Neue Laptops sind da').locator('.cat-chip', { hasText: 'Info' })).toHaveCount(0);
});

test('Tastenkürzel T und bis zu 20 Kategorien direkt sichtbar', async ({ page }) => {
  await openApp(page);
  const [acc] = await api(page, 'list_accounts');
  const existing = await api(page, 'list_categories', { accountId: acc.id });
  for (let i = existing.length; i < 25; i++) {
    await api(page, 'create_category', { accountId: acc.id, displayName: `Projekt ${String(i).padStart(2, '0')}`, color: `preset${i % 25}` });
  }
  await page.reload();
  await select(page, 'Wartungsfenster Mailserver');
  await page.locator('#msglist').focus();
  await page.keyboard.press('t');
  const pop = page.locator('[data-testid=tag-popover]');
  await expect(pop).toBeVisible();
  await expect(pop.locator('.tag-opt')).toHaveCount(20);
  await expect(pop.locator('.tagpop-hint')).toContainText('weitere');
  // the rest is reachable through the filter
  await pop.locator('.tagpop-filter').fill('Projekt 24');
  await expect(pop.locator('.tag-opt')).toHaveCount(1);
  await page.keyboard.press('Enter');
  await expect(page.locator('.rp-tags .tag-pill', { hasText: 'Projekt 24' })).toBeVisible();
  await page.keyboard.press('Escape');
  // clean up
  for (const c of await api(page, 'list_categories', { accountId: acc.id })) {
    if (/^Projekt \d\d$/.test(c.displayName)) await api(page, 'delete_category', { accountId: acc.id, id: c.id });
  }
});
