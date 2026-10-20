const { expect } = require('@playwright/test');

async function openApp(page) {
  await page.goto('/');
  await expect(page.locator('.msg').first()).toBeVisible();
}

const row = (page, subject) => page.locator('.msg', { hasText: subject });

/** Folder node in the account tree (not in favorites). */
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

/** Selects a message by clicking its subject line. */
async function select(page, subject) {
  await row(page, subject).first().locator('.m-subj').click();
  await expect(page.locator('.rp-subject')).toContainText(subject);
}

async function popupFrom(page, action) {
  const [popup] = await Promise.all([page.waitForEvent('popup'), action()]);
  await popup.waitForLoadState('domcontentloaded');
  return popup;
}

async function api(page, cmd, args = {}) {
  const res = await page.request.post('/api/call/' + cmd, { data: args });
  expect(res.ok(), `${cmd}: ${await res.text()}`).toBeTruthy();
  return res.json();
}

module.exports = { openApp, row, folder, openFolder, select, popupFrom, api };
