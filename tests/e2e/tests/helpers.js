const { expect } = require('@playwright/test');

async function openApp(page) {
  await page.goto('/');
  await expect(page.locator('tr.msg').first()).toBeVisible();
}

const row = (page, subject) => page.locator('tr.msg', { hasText: subject });

/** Folder node in the mailbox tree (not in favorites). */
function folder(page, name, mailboxIndex = 0) {
  return page.locator('[data-mailbox-root]').nth(mailboxIndex).locator('.tree-node', { has: page.locator('.fname', { hasText: new RegExp(`^${name}$`) }) });
}

async function openFolder(page, name, mailboxIndex = 0) {
  const node = folder(page, name, mailboxIndex);
  if (!(await node.count())) {
    // expand Posteingang to reveal sub folders
    await folder(page, 'Posteingang', mailboxIndex).locator('.twisty').click();
  }
  await folder(page, name, mailboxIndex).click();
  await expect(page.locator('.listpane .folder-name')).toHaveText(name);
}

async function popupFrom(page, action) {
  const [popup] = await Promise.all([page.waitForEvent('popup'), action()]);
  await popup.waitForLoadState('domcontentloaded');
  return popup;
}

module.exports = { openApp, row, folder, openFolder, popupFrom };
