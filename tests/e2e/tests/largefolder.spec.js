const { test, expect } = require('@playwright/test');
const { openApp, folder } = require('./helpers');

// Demo folder "Archiv/Altablage": 2500 mails, all older than the initial sync window.
async function openAltablage(page) {
  if (!(await folder(page, 'Altablage').count())) await folder(page, 'Archiv').locator('.twisty').click();
  await folder(page, 'Altablage').click();
  await expect(page.locator('.listpane .folder-name')).toHaveText('Altablage');
}

test('Großer Ordner: Gesamtzahl vom Server, Mails werden seitenweise nachgeladen', async ({ page }) => {
  await openApp(page);
  const olderCalls = [];
  page.on('request', (r) => { if (r.url().endsWith('/api/call/load_older')) olderCalls.push(r.url()); });
  await openAltablage(page);
  // header shows the server total, not just the cached part
  await expect(page.locator('.listpane .lh-count')).toHaveText('2.500');
  const rows = page.locator('#msglist > .msg');
  await expect(rows.first()).toBeVisible();
  await expect(page.locator('.listpane .lh-busy')).not.toHaveClass(/on/); // background sync done
  const first = await rows.count();
  expect(first).toBeGreaterThan(0);
  expect(first).toBeLessThanOrEqual(200);

  // mark the first row: appending a page must not rebuild the list
  await page.locator('#msglist > .msg').first().evaluate((el) => { el.dataset.marker = 'kept'; });
  await page.locator('#msglist').evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await expect.poll(() => rows.count()).toBeGreaterThan(first);
  await expect(page.locator('#msglist > .msg[data-marker=kept]')).toHaveCount(1);
  // no burst: one scroll loads one page from the server
  expect(olderCalls.length).toBeLessThanOrEqual(2);
  await expect(page.locator('.listpane .lh-count')).toHaveText('2.500');
  await expect(page.locator('#msglist .msg', { hasText: 'Altablage Nr. 1' }).first()).toBeVisible();
});

test('Großer Ordner im Kanban: Spalte zeigt Karten in Schritten', async ({ page }) => {
  await openApp(page);
  await openAltablage(page);
  await expect(page.locator('#msglist > .msg').first()).toBeVisible();
  // fill the cache beyond one column step
  for (let i = 0; i < 3; i++) {
    const n = await page.locator('#msglist > .msg').count();
    await page.locator('#msglist').evaluate((el) => { el.scrollTop = el.scrollHeight; });
    await expect.poll(() => page.locator('#msglist > .msg').count()).toBeGreaterThan(n);
  }
  await page.locator('[data-testid=view-kanban]').first().click();
  const col = page.locator('[data-testid=kb-col-none]');
  await expect(col.locator('.kb-card').first()).toBeVisible();
  await expect(col.locator('.kb-card')).toHaveCount(100);
  const more = col.locator('[data-testid=kb-more]');
  await expect(more).toBeVisible();
  await more.click();
  await expect.poll(() => col.locator('.kb-card').count()).toBeGreaterThan(100);
  await page.locator('[data-testid=view-list]').first().click();
});
