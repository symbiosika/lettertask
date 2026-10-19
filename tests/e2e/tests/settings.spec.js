const { test, expect } = require('@playwright/test');
const { openApp } = require('./helpers');

test('Kategorie im Kategorienmanager hinzufügen', async ({ page }) => {
  await openApp(page);
  await page.locator('.mb-item.file').click();
  await page.locator('[data-testid=menu-categories]').click();
  const dlg = page.locator('.modal', { hasText: 'Farbkategorien' });
  await expect(dlg).toBeVisible();
  await dlg.locator('[data-testid=cat-new]').click();
  const ed = page.locator('.modal', { hasText: 'Neue Kategorie hinzufügen' });
  await ed.locator('input[name=categoryName]').fill('E2E Kategorie');
  await ed.locator('.pcolor[data-color=preset19]').click();
  await ed.locator('.btn.primary').click();
  await expect(dlg.locator('.catrow', { hasText: 'E2E Kategorie' })).toBeVisible();
  await dlg.locator('.btn.primary', { hasText: 'Schließen' }).click();
  // available in the categorize menu
  await page.locator('tr.msg').first().locator('.c-subject').click();
  await page.locator('[data-testid=tb-categorize]').click();
  await expect(page.locator('[data-testid="cat-E2E Kategorie"]')).toBeVisible();
});

test('Einstellungen speichern (Lesebereich unten)', async ({ page }) => {
  await openApp(page);
  const openSettings = async () => {
    await page.locator('.mb-item.file').click();
    await page.locator('[data-testid=menu-settings]').click();
    await expect(page.locator('.modal', { hasText: 'Optionen' })).toBeVisible();
  };
  await openSettings();
  await page.locator('select[name=readingPane]').selectOption('bottom');
  await page.locator('#settings-ok').click();
  await expect(page.locator('#mailview')).toHaveClass(/bottom/);
  await page.reload();
  await expect(page.locator('#mailview')).toHaveClass(/bottom/);
  // restore
  await openSettings();
  await page.locator('select[name=readingPane]').selectOption('right');
  await page.locator('#settings-ok').click();
  await expect(page.locator('#mailview')).not.toHaveClass(/bottom/);
});
