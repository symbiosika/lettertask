const { test, expect } = require('@playwright/test');
const { openApp, select } = require('./helpers');

async function openSettings(page, tab) {
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-settings]').click();
  await expect(page.locator('.modal', { hasText: 'Einstellungen' })).toBeVisible();
  if (tab) await page.locator(`[data-testid=settings-tab-${tab}]`).click();
}

test('Kategorie im Kategorienmanager hinzufügen', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-testid=app-menu]').click();
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
  // available in the tag picker
  await select(page, 'Wartungsfenster Mailserver');
  await page.locator('[data-testid=tag-add]').click();
  await page.locator('.tagpop-filter').fill('E2E Kat');
  await expect(page.locator('[data-testid="tag-opt-E2E Kategorie"]')).toBeVisible();
});

test('Einstellungen speichern (Lesebereich unten)', async ({ page }) => {
  await openApp(page);
  await openSettings(page, 'reading');
  await page.locator('select[name=readingPane]').selectOption('bottom');
  await page.locator('#settings-ok').click();
  await expect(page.locator('#mailview')).toHaveClass(/bottom/);
  await page.reload();
  await expect(page.locator('#mailview')).toHaveClass(/bottom/);
  await openSettings(page, 'reading');
  await page.locator('select[name=readingPane]').selectOption('right');
  await page.locator('#settings-ok').click();
  await expect(page.locator('#mailview')).not.toHaveClass(/bottom/);
});

test('Wiedervorlage-Optionen bearbeiten', async ({ page }) => {
  await openApp(page);
  await openSettings(page, 'snooze');
  await page.locator('input[name=snoozeLabel0]').fill('Morgen früh');
  await page.locator('input[name=snoozeTime0]').fill('07:30');
  await page.locator('#settings-ok').click();
  await page.reload();
  await select(page, 'Wartungsfenster Mailserver');
  await page.locator('[data-testid=tb-snooze]').click();
  await expect(page.locator('[data-testid=snooze-opt-0]')).toContainText('Morgen früh');
  await expect(page.locator('[data-testid=snooze-opt-0]')).toContainText('07:30');
  await page.keyboard.press('Escape');
  // the menu links to the settings
  await page.locator('[data-testid=tb-snooze]').click();
  await page.locator('[data-testid=snooze-edit]').click();
  await expect(page.locator('[data-testid=settings-tab-snooze]')).toHaveClass(/on/);
  await page.locator('.snooze-editor .btn', { hasText: 'Standard wiederherstellen' }).click();
  await expect(page.locator('input[name=snoozeLabel0]')).toHaveValue('Morgen 8 Uhr');
  await page.locator('#settings-ok').click();
});
