// Security regression tests: hostile mail content and server data must neither
// execute script nor load remote resources while external content is blocked.
const { test, expect } = require('@playwright/test');
const { openApp, row, select, api } = require('./helpers');

const GIF = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7';
const pwn = (tag, target = 'window') => `${target}.__pwned=(${target}.__pwned||[]).concat('${tag}')`;
const xss = (tag) => `<img src=x onerror="${pwn(tag)}">`;

async function account(page) { return (await api(page, 'list_accounts'))[0]; }

/** Records (and aborts) every request that leaves the local devserver. */
async function trackRemote(page) {
  const hits = [];
  await page.context().route(/^(?!https?:\/\/127\.0\.0\.1[:/])[a-z]+:\/\//i, (route) => {
    hits.push(route.request().url());
    return route.abort();
  });
  return hits;
}

/** Puts a mail into the inbox (as a task without due date: unread, from yourself). */
async function plantMail(page, subject, bodyHtml) {
  const acc = await account(page);
  await api(page, 'create_task', { mailboxId: acc.mailboxes[0].id, subject, bodyHtml });
}

const mailFrame = (page) => page.frameLocator('#readpane iframe.mail-frame');
const pwned = (page) => page.evaluate(() => window.__pwned || []);

const HOSTILE_BODY = [
  '<p id="marker">Feindlicher Inhalt</p>',
  // script and event handlers (all variants)
  `<script>${pwn('script', 'parent')}</script>`,
  `<img src=x onerror="${pwn('onerror', 'parent')}">`,
  `<svg onload="${pwn('svg', 'parent')}"><animate onbegin="${pwn('animate', 'parent')}" attributeName="x" dur="1s"/></svg>`,
  `<body onload="${pwn('body', 'parent')}">`,
  `<details open ontoggle="${pwn('toggle', 'parent')}"><summary>x</summary></details>`,
  `<iframe srcdoc="<script>${pwn('srcdoc', 'parent.parent')}</script>"></iframe>`,
  `<object data="data:text/html,<script>${pwn('object', 'parent.parent')}</script>"></object>`,
  `<embed src="data:text/html,<script>${pwn('embed', 'parent.parent')}</script>">`,
  // remote loads
  '<img src="https://evil.test/img.png">',
  '<img src="//evil.test/protocol-relative.png">',
  '<img srcset="https://evil.test/srcset.png 1x" src="' + GIF + '">',
  '<picture><source srcset="https://evil.test/picture.png"><img src="' + GIF + '"></picture>',
  '<video poster="https://evil.test/poster.png" src="https://evil.test/video.mp4"></video>',
  '<audio src="https://evil.test/audio.mp3" autoplay></audio>',
  '<svg><image href="https://evil.test/svg-image.png" width="10" height="10"/><image xlink:href="https://evil.test/svg-xlink.png" width="10" height="10"/></svg>',
  '<table background="https://evil.test/table-bg.png"><tr><td background="https://evil.test/td-bg.png">x</td></tr></table>',
  '<div style="background:url(https://evil.test/inline-css.png)">x</div>',
  '<div style="background-image:url(\'\\68ttps://evil.test/css-escape.png\')">x</div>',
  '<style>@import url(https://evil.test/import.css); @font-face{font-family:x;src:url(https://evil.test/font.woff)} p{font-family:x;background:url("//evil.test/style.png")}</style>',
  '<link rel="stylesheet" href="https://evil.test/link.css">',
  '<link rel="prefetch" href="https://evil.test/prefetch">',
  '<link rel="icon" href="https://evil.test/favicon.ico">',
  '<iframe src="https://evil.test/frame"></iframe>',
  '<object data="https://evil.test/object"></object>',
  '<embed src="https://evil.test/embed">',
  '<input type="image" src="https://evil.test/input.png">',
  // navigation of the reading pane
  '<meta http-equiv="refresh" content="0;url=https://evil.test/refresh">',
  '<meta http-equiv="&#x72;efresh" content="0;url=https://evil.test/refresh-entity">',
  '<meta http-equiv=" Refresh " content="0;url=https://evil.test/refresh-space">',
  '<base href="https://evil.test/base/"><img src="relative.png">',
  '<form action="https://evil.test/form" target="_self"><button id="formbtn">Absenden</button></form>',
  // links that must not be opened
  `<a id="jslink" href="javascript:${pwn('jslink', 'parent')}">JS</a>`,
  `<a id="jsentity" href="jav&#x09;ascript:${pwn('jsentity', 'parent')}">JS2</a>`,
  `<a id="datalink" href="data:text/html,<script>${pwn('datalink', 'opener')}</script>">Data</a>`,
  '<a id="filelink" href="file:///etc/passwd">Datei</a>',
  '<a id="selflink" href="https://evil.test/self" target="_self">Selbst</a>',
  `<svg width="100" height="20"><a id="svglink" xlink:href="https://evil.test/svg-link" href="https://evil.test/svg-link"><text x="0" y="15">SVG-Link</text></a></svg>`,
  `<map name="m"><area id="arealink" shape="rect" coords="0,0,300,40" href="https://evil.test/area" target="_self"></map><img usemap="#m" src="${GIF}" width="300" height="40">`,
].join('\n');

test('Mailtext: keine Skripte, keine externen Inhalte, keine Navigation', async ({ page }) => {
  const hits = await trackRemote(page);
  const popups = [];
  page.context().on('page', (p) => popups.push(p.url()));
  await openApp(page);
  const subject = `Sicherheit Mailtext ${Date.now()}`;
  await plantMail(page, subject, HOSTILE_BODY);
  await page.reload();
  await select(page, subject);
  await expect(mailFrame(page).locator('#marker')).toHaveText('Feindlicher Inhalt');
  // remote content was detected → banner with download button
  await expect(page.locator('.infobar.remote')).toBeVisible();

  // clicking every link or button in the mail opens nothing and runs nothing
  const stillThere = (step) => expect(mailFrame(page).locator('#marker'), step).toHaveText('Feindlicher Inhalt', { timeout: 3000 });
  for (const id of ['jslink', 'jsentity', 'datalink', 'filelink', 'formbtn']) {
    await mailFrame(page).locator('#' + id).click();
    await stillThere(id);
  }
  await mailFrame(page).locator('#svglink').click({ force: true });
  await stillThere('svglink');
  await mailFrame(page).locator('img[usemap]').click({ position: { x: 10, y: 10 } });
  await stillThere('arealink');
  for (const id of ['jslink', 'filelink', 'selflink']) {
    await mailFrame(page).locator('#' + id).click({ button: 'middle' });
    await stillThere('middle ' + id);
  }
  await page.waitForTimeout(1500);

  // the reading pane was not navigated away (meta refresh, _self links, forms)
  const frame = page.frames().find((f) => f.parentFrame() === page.mainFrame() && f.url().startsWith('about:'));
  expect(frame, 'Lesebereich-Frame').toBeTruthy();
  expect(frame.url()).toBe('about:srcdoc');
  await expect(mailFrame(page).locator('#marker')).toHaveText('Feindlicher Inhalt');

  expect(await pwned(page)).toEqual([]);
  // only the clicked http(s) links of the image map / SVG went out – as new browser windows
  const opened = new Set(['https://evil.test/area', 'https://evil.test/svg-link']);
  expect(hits.filter((u) => !opened.has(u))).toEqual([]);
  expect(hits.sort()).toEqual([...opened].sort());
  expect(popups.length, 'nur die zwei http(s)-Links öffnen ein Fenster').toBe(2);

  // area/self links are routed through the safe opener (http(s) only)
  const [popup] = await Promise.all([page.waitForEvent('popup'), mailFrame(page).locator('#selflink').click()]);
  await expect.poll(() => hits).toContain('https://evil.test/self');
  await popup.close();
});

test('Mailtext: Erkennung externer Inhalte ohne Lücken', async ({ page }) => {
  await trackRemote(page);
  await openApp(page);
  const vectors = {
    srcset: '<img srcset="https://evil.test/a.png 2x">',
    poster: '<video poster="https://evil.test/p.png"></video>',
    background: '<table background="https://evil.test/bg.png"><tr><td>x</td></tr></table>',
    link: '<link rel="stylesheet" href="https://evil.test/s.css">',
    import: '<style>@import "https://evil.test/i.css";</style>',
    entity: '<img src="&#104;ttps://evil.test/e.png">',
    cssEscape: '<div style="background:url(\\68ttps://evil.test/c.png)">x</div>',
    svgXlink: '<svg><image xlink:href="https://evil.test/x.png"/></svg>',
  };
  const stamp = Date.now();
  for (const [name, html] of Object.entries(vectors)) {
    await plantMail(page, `Extern ${name} ${stamp}`, `<p>Text</p>${html}`);
  }
  await page.reload();
  for (const name of Object.keys(vectors)) {
    await select(page, `Extern ${name} ${stamp}`);
    await expect(page.locator('.infobar.remote'), name).toBeVisible();
  }
  // plain mails do not show the banner
  await plantMail(page, `Ohne extern ${stamp}`, `<p>Nur Text <img src="${GIF}"></p>`);
  await page.reload();
  await select(page, `Ohne extern ${stamp}`);
  await expect(page.locator('.infobar.remote')).toHaveCount(0);
});

test('Serverdaten werden in der Oberfläche nie als HTML ausgeführt', async ({ page }) => {
  await trackRemote(page);
  await openApp(page);
  const acc = await account(page);
  const mb = acc.mailboxes[0].id;
  const stamp = Date.now();
  const subject = `XSS ${stamp} ${xss('subject')}`;
  await api(page, 'create_task', { mailboxId: mb, subject, bodyHtml: '<p>x</p>' });
  const cat = await api(page, 'create_category', { accountId: acc.id, displayName: `Kat ${stamp} ${xss('category')}`, color: 'preset1' });
  const inbox = (await api(page, 'list_folders', { mailboxId: mb })).find((f) => f.wellKnown === 'inbox');
  await api(page, 'create_folder', { mailboxId: mb, parentId: null, name: `Ordner ${stamp} ${xss('folder')}` });
  const msgs = (await api(page, 'list_messages', { mailboxId: mb, folderId: inbox.id, limit: 500 })).items;
  const m = msgs.find((x) => x.subject.startsWith(`XSS ${stamp}`));
  await api(page, 'set_categories', { mailboxId: mb, ids: [m.id], categories: [cat.displayName] });

  await page.reload();
  await expect(row(page, `XSS ${stamp}`)).toBeVisible();
  await row(page, `XSS ${stamp}`).first().locator('.m-subj').click();
  await expect(page.locator('.rp-subject')).toContainText('<img src=x');
  await expect(page.locator('.tag-pill', { hasText: `Kat ${stamp}` })).toBeVisible();
  await expect(page.locator('.fname', { hasText: `Ordner ${stamp}` })).toHaveCount(1);

  // calendar: event title and location
  const cals = await api(page, 'list_calendars', { accountId: acc.id });
  const start = new Date();
  start.setHours(10, 0, 0, 0);
  const end = new Date(start.getTime() + 3600_000);
  const iso = (d) => d.toISOString().slice(0, 19);
  await api(page, 'create_event', {
    accountId: acc.id,
    calendarId: cals[0].id,
    input: { subject: `Termin ${stamp} ${xss('event')}`, start: iso(start), end: iso(end), timeZone: 'UTC', location: xss('location'), categories: [cat.displayName] },
  });
  await page.locator('[data-testid=nav-calendar]').click();
  await page.locator('[data-testid=tb-view-week]').click();
  const ev = page.locator('.timegrid .ev', { hasText: `Termin ${stamp}` });
  await expect(ev).toBeVisible();
  await ev.click();
  await expect(page.locator('.peek .peek-title')).toContainText(`Termin ${stamp}`);

  // contacts
  await api(page, 'create_contact', {
    accountId: acc.id,
    input: { displayName: `Kontakt ${stamp} ${xss('contact')}`, givenName: xss('given'), surname: 'X', emails: [`k${stamp}@example.com`], company: xss('company'), jobTitle: xss('job'), phones: [], notes: xss('notes') },
  });
  await page.locator('[data-testid=nav-contacts]').click();
  await page.locator('[data-testid=contacts-search]').fill(`Kontakt ${stamp}`);
  await page.locator('[data-testid=contact-row]', { hasText: `Kontakt ${stamp}` }).click();
  await expect(page.locator('[data-testid=contact-name]')).toContainText(`Kontakt ${stamp}`);

  await page.waitForTimeout(500);
  expect(await pwned(page)).toEqual([]);

  // cleanup
  await api(page, 'delete_category', { accountId: acc.id, id: cat.id });
});

test('Abwesenheitsnotiz: HTML vom Server wird nicht ausgeführt', async ({ page }) => {
  await trackRemote(page);
  await openApp(page);
  const acc = await account(page);
  const before = await api(page, 'get_auto_reply', { accountId: acc.id });
  await api(page, 'set_auto_reply', {
    accountId: acc.id,
    autoReply: { ...before, status: 'alwaysEnabled', internalReplyMessage: `<p>Bin weg</p>${xss('autoreply')}`, externalReplyMessage: '<p>Out of office</p><img src="https://evil.test/ooo.png">' },
  });
  await page.locator('[data-testid=app-menu]').click();
  await page.locator('[data-testid=menu-autoreply]').click();
  await expect(page.locator('.modal textarea').first()).toHaveValue(/Bin weg/);
  await page.waitForTimeout(500);
  expect(await pwned(page)).toEqual([]);
  await page.keyboard.press('Escape');
  await api(page, 'set_auto_reply', { accountId: acc.id, autoReply: before });
});

test('Anwendungsfenster laufen unter einer Content-Security-Policy', async ({ page }) => {
  const res = await page.goto('/');
  const csp = res.headers()['content-security-policy'] || '';
  expect(csp).toContain("script-src 'self'");
  expect(csp).toContain("object-src 'none'");
  await expect(page.locator('.msg').first()).toBeVisible();
  // injected inline script (e.g. through an escaping bug) is refused
  const ran = await page.evaluate(() => {
    const s = document.createElement('script');
    s.textContent = 'window.__inline = 1';
    document.body.appendChild(s);
    const d = document.createElement('div');
    d.innerHTML = '<img src="data:," onerror="window.__inline = 2">';
    document.body.appendChild(d);
    return new Promise((r) => setTimeout(() => r(window.__inline || 0), 300));
  });
  expect(ran).toBe(0);
});

test('Links öffnen nur http(s) – auch im Browsermodus', async ({ page }) => {
  await openApp(page);
  const popups = [];
  page.context().on('page', (p) => popups.push(p));
  const res = await page.evaluate(async () => {
    const { openUrl } = await import('./js/api.js');
    const out = [];
    for (const u of ['javascript:window.__pwned=1', 'data:text/html,x', 'file:///etc/passwd', ' JAVASCRIPT:alert(1)']) {
      try { await openUrl(u); out.push('ok'); } catch { out.push('err'); }
    }
    return out;
  });
  expect(res).toEqual(['err', 'err', 'err', 'err']);
  await page.waitForTimeout(300);
  expect(popups).toEqual([]);
  expect(await pwned(page)).toEqual([]);
});

test('Dev-Server: fremde Websites dürfen die API nicht aufrufen', async ({ page }) => {
  const cross = await page.request.post('/api/call/list_accounts', { data: {}, headers: { Origin: 'https://evil.test' } });
  expect(cross.status()).toBe(403);
  const rebinding = await page.request.post('/api/call/list_accounts', { data: {}, headers: { Host: 'evil.test' } });
  expect(rebinding.status()).toBe(403);
  const own = await page.request.post('/api/call/list_accounts', { data: {} });
  expect(own.status()).toBe(200);
});
