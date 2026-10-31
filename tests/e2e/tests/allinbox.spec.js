const { test, expect } = require('@playwright/test');
const { openApp, row } = require('./helpers');

// The demo has one mailbox: a second (IMAP) account with its own inbox is simulated.
const MB2 = 'imap:erika';
const INBOX2 = 'INBOX';
const now = Date.now();
const mail2 = (id, subject, minutesAgo, extra = {}) => ({
  id, mailboxId: MB2, folderId: INBOX2, subject, from: { name: 'Erika Beispiel', address: 'erika@imap.example' },
  to: [{ name: 'Erika', address: 'erika@imap.example' }], receivedAt: new Date(now - minutesAgo * 60000).toISOString(),
  isRead: false, flag: 'notFlagged', importance: 'normal', hasAttachments: false, categories: [], preview: 'Aus dem zweiten Postfach', isDraft: false, ...extra,
});
const ITEMS2 = [mail2('e1', 'Zweites Postfach: ganz neu', 1), mail2('e2', 'Zweites Postfach: älter', 60 * 24 * 40, { isRead: true })];

async function setup(page, calls) {
  await page.route('**/api/call/list_accounts', async (route) => {
    const res = await route.fetch();
    const imap = { id: MB2, email: 'erika@imap.example', displayName: 'Erika IMAP', kind: 'imap', label: null, badge: { color: '#c2410c' }, avatar: null,
      mailboxes: [{ id: MB2, accountId: MB2, address: 'erika@imap.example', displayName: 'Erika IMAP', kind: 'primary' }] };
    await route.fulfill({ response: res, json: [...(await res.json()), imap] });
  });
  await page.route('**/api/call/list_folders', async (route) => {
    if (route.request().postDataJSON().mailboxId !== MB2) return route.fallback();
    await route.fulfill({ json: [{ id: INBOX2, mailboxId: MB2, parentId: null, displayName: 'Posteingang', wellKnown: 'inbox', unreadCount: 1, totalCount: 2, childFolderCount: 0 },
      { id: 'Archive', mailboxId: MB2, parentId: null, displayName: 'Archiv', wellKnown: 'archive', unreadCount: 0, totalCount: 0, childFolderCount: 0 }] });
  });
  await page.route('**/api/call/list_messages', async (route) => {
    const a = route.request().postDataJSON();
    if (a.mailboxId !== MB2) return route.fallback();
    const items = ITEMS2.filter((m) => !a.unreadOnly || !m.isRead);
    await route.fulfill({ json: { items, total: items.length, unread: items.filter((m) => !m.isRead).length, canLoadOlder: false } });
  });
  await page.route('**/api/call/get_message', async (route) => {
    const a = route.request().postDataJSON();
    if (a.mailboxId !== MB2) return route.fallback();
    const m = ITEMS2.find((x) => x.id === a.messageId);
    await route.fulfill({ json: { ...m, cc: [], bcc: [], replyTo: [], sentAt: m.receivedAt, bodyHtml: '<p>Hallo aus dem zweiten Postfach</p>', attachments: [], internetMessageId: null, webLink: null, isMeetingRequest: false, eventId: null } });
  });
  for (const cmd of ['find_delegation', 'sync_folder', 'set_read', 'archive_messages', 'set_active_folder', 'sync_folders']) {
    await page.route(`**/api/call/${cmd}`, async (route) => {
      const a = route.request().postDataJSON();
      if (a.mailboxId !== MB2) return route.fallback();
      calls.push({ cmd, ...a });
      await route.fulfill({ json: cmd === 'sync_folder' ? { added: 0, updated: 0, removed: 0 } : null });
    });
  }
}

test('Geteilter Posteingang: alle Posteingänge in einer Liste, Farbbalken je Postfach', async ({ page }) => {
  const calls = [];
  await setup(page, calls);
  await openApp(page);
  const node = page.getByTestId('allinbox-node');
  await expect(node).toContainText('Geteilter Posteingang');
  await node.click();
  await expect(page.locator('.listpane .folder-name')).toHaveText('Geteilter Posteingang');
  // Mails of both mailboxes, newest first.
  await expect(row(page, 'Zweites Postfach: ganz neu')).toHaveCount(1);
  await expect(row(page, 'Rechnung 2026-1043')).toHaveCount(1);
  const subjects = await page.locator('.msg .m-subj').allTextContents();
  expect(subjects[0]).toBe('Zweites Postfach: ganz neu');
  expect(subjects.indexOf('Zweites Postfach: älter')).toBeGreaterThan(subjects.indexOf('Rechnung 2026-1043'));
  // Source mailbox as a coloured bar on the left; the account header shows the same colour.
  const bar2 = row(page, 'Zweites Postfach: ganz neu').getByTestId('m-src');
  await expect(bar2).toHaveCSS('background-color', 'rgb(194, 65, 12)');
  await expect(bar2).toHaveAttribute('title', 'erika@imap.example');
  const bar1 = row(page, 'Rechnung 2026-1043').getByTestId('m-src');
  expect(await bar1.evaluate((e) => getComputedStyle(e).backgroundColor)).not.toBe('rgb(194, 65, 12)');
  const box = await bar2.boundingBox(), rowBox = await row(page, 'Zweites Postfach: ganz neu').boundingBox();
  expect(box.x - rowBox.x).toBeLessThan(2);
  await expect(page.locator('[data-mailbox-root="' + MB2 + '"] [data-testid=mb-bar]')).toHaveCSS('background-color', 'rgb(194, 65, 12)');
  // Normal folders show no bar.
  await page.locator('[data-mailbox-root]').first().locator('.tree-node', { hasText: 'Posteingang' }).first().click();
  await expect(page.getByTestId('m-src')).toHaveCount(0);
});

test('Geteilter Posteingang: Aktionen gehen an das jeweilige Postfach', async ({ page }) => {
  const calls = [];
  await setup(page, calls);
  await openApp(page);
  await page.getByTestId('allinbox-node').click();
  await row(page, 'Zweites Postfach: ganz neu').locator('.m-subj').click();
  // The reading pane reads from the mail's own mailbox.
  await expect(page.locator('.rp-subject')).toHaveText('Zweites Postfach: ganz neu');
  // Mixed selection: one mail of each mailbox.
  await row(page, 'Rechnung 2026-1043').locator('.m-subj').click({ modifiers: ['Control'] });
  const sent = [];
  page.on('request', (r) => { if (r.url().endsWith('/archive_messages')) sent.push(r.postDataJSON()); });
  await page.keyboard.press('e');
  await expect(row(page, 'Zweites Postfach: ganz neu')).toHaveCount(0);
  await expect(row(page, 'Rechnung 2026-1043')).toHaveCount(0);
  await expect.poll(() => sent.length).toBe(2);
  const byMb = Object.fromEntries(sent.map((x) => [x.mailboxId, x.ids]));
  expect(byMb[MB2]).toEqual(['e1']);
  const other = Object.keys(byMb).find((k) => k !== MB2);
  expect(byMb[other]).toHaveLength(1);
  expect(byMb[other][0]).not.toBe('e1');
});

test('Ein einzelnes Postfach zeigt keinen geteilten Posteingang', async ({ page }) => {
  await openApp(page);
  await expect(page.getByTestId('today-node')).toBeVisible();
  await expect(page.getByTestId('allinbox-node')).toHaveCount(0);
});
