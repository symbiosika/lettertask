const { test, expect } = require('@playwright/test');
const { openApp, popupFrom } = require('./helpers');

test('Link im Mailtext einfügen und bearbeiten', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('#subject')).toBeVisible();
  // The draft loads into the editor afterwards and replaces its document.
  await expect(popup.locator('body[data-ready="1"]')).toHaveCount(1);
  const frame = await (await popup.locator('iframe.editor-frame').elementHandle()).contentFrame();
  await frame.evaluate(() => {
    document.body.innerHTML = '<p id="p">Hier klicken bitte</p>';
    const t = document.getElementById('p').firstChild;
    const r = document.createRange(); r.setStart(t, 5); r.setEnd(t, 12);
    const s = document.getSelection(); s.removeAllRanges(); s.addRange(r);
  });
  const dialogInput = popup.locator('.modal input.input').last();

  // Selected text becomes the link; "www." gets https://.
  await popup.locator('.ed-btn[title^="Link"]').click();
  await dialogInput.fill('www.example.de');
  await dialogInput.press('Enter');
  const a = frame.locator('#p a');
  await expect(a).toHaveAttribute('href', 'https://www.example.de');
  await expect(a).toHaveText('klicken');

  // Caret inside the link: the dialog shows and changes its address.
  await frame.evaluate(() => {
    const t = document.querySelector('#p a').firstChild;
    const r = document.createRange(); r.setStart(t, 2); r.collapse(true);
    const s = document.getSelection(); s.removeAllRanges(); s.addRange(r);
  });
  await popup.locator('.ed-btn[title^="Link"]').click();
  await expect(dialogInput).toHaveValue('https://www.example.de');
  await dialogInput.fill('https://symbiosika.de');
  await dialogInput.press('Enter');
  await expect(a).toHaveAttribute('href', 'https://symbiosika.de');
  await expect(frame.locator('#p a')).toHaveCount(1);
  await popup.close({ runBeforeUnload: false });
});

async function openEditor(page) {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('body[data-ready="1"]')).toHaveCount(1);
  const frame = await (await popup.locator('iframe.editor-frame').elementHandle()).contentFrame();
  return { popup, frame };
}

test('Adressen werden im Editor verlinkt, Text und Adresse getrennt bearbeitbar, Link entfernbar', async ({ page }) => {
  const { popup, frame } = await openEditor(page);
  await popup.locator('iframe.editor-frame').click();
  await frame.evaluate(() => { document.body.innerHTML = '<p id="p"><br></p>'; const r = document.createRange(); r.setStart(document.getElementById('p'), 0); r.collapse(true); const s = document.getSelection(); s.removeAllRanges(); s.addRange(r); });
  await popup.keyboard.type('Video: https://example.com/v?a=1');
  // Caret still at the address: stays text while typing.
  await popup.waitForTimeout(700);
  await expect(frame.locator('#p a')).toHaveCount(0);
  await popup.keyboard.type(' danke');
  const a = frame.locator('#p a');
  await expect(a).toHaveAttribute('href', 'https://example.com/v?a=1');
  await expect(a).toHaveText('https://example.com/v?a=1');
  await expect(frame.locator('#p')).toHaveText('Video: https://example.com/v?a=1 danke');
  // Typing went on behind the link, the caret did not jump.
  await popup.keyboard.type('!');
  await expect(frame.locator('#p')).toHaveText('Video: https://example.com/v?a=1 danke!');

  // Caret in the link: bar with its address.
  await frame.evaluate(() => { const t = document.querySelector('#p a').firstChild; const r = document.createRange(); r.setStart(t, 3); r.collapse(true); const s = document.getSelection(); s.removeAllRanges(); s.addRange(r); });
  const bar = popup.locator('[data-testid=link-bar]');
  await expect(bar).toBeVisible();
  await expect(bar).toContainText('https://example.com/v?a=1');

  // Text and address apart.
  await bar.locator('[data-testid=link-bar-edit]').click();
  await expect(popup.locator('[data-testid=link-url]')).toHaveValue('https://example.com/v?a=1');
  await popup.locator('[data-testid=link-text]').fill('das Video');
  await popup.locator('[data-testid=link-url]').fill('example.org/film');
  await popup.locator('[data-testid=link-url]').press('Enter');
  await expect(a).toHaveText('das Video');
  await expect(a).toHaveAttribute('href', 'https://example.org/film');

  // Only the address changes; the text stays.
  await frame.evaluate(() => { const t = document.querySelector('#p a').firstChild; const r = document.createRange(); r.setStart(t, 2); r.collapse(true); const s = document.getSelection(); s.removeAllRanges(); s.addRange(r); });
  await popup.locator('.ed-btn[title^="Link"]').click();
  await expect(popup.locator('[data-testid=link-text]')).toHaveValue('das Video');
  await popup.locator('[data-testid=link-url]').fill('https://symbiosika.de');
  await popup.locator('[data-testid=link-url]').press('Enter');
  await expect(a).toHaveText('das Video');
  await expect(a).toHaveAttribute('href', 'https://symbiosika.de');
  await popup.close({ runBeforeUnload: false });
});

test('Entfernter Link bleibt Text, auch beim Weiterschreiben', async ({ page }) => {
  const { popup, frame } = await openEditor(page);
  await popup.locator('iframe.editor-frame').click();
  await frame.evaluate(() => { document.body.innerHTML = '<p id="p">Siehe <a href="https://example.com">https://example.com</a> hier</p>'; const t = document.querySelector('#p a').firstChild; const r = document.createRange(); r.setStart(t, 4); r.collapse(true); const s = document.getSelection(); s.removeAllRanges(); s.addRange(r); });
  const bar = popup.locator('[data-testid=link-bar]');
  await expect(bar).toBeVisible();
  await bar.locator('[data-testid=link-bar-remove]').click();
  await expect(frame.locator('#p a')).toHaveCount(0);
  await expect(bar).toBeHidden();
  await frame.evaluate(() => { const p = document.getElementById('p'); const r = document.createRange(); r.selectNodeContents(p); r.collapse(false); const s = document.getSelection(); s.removeAllRanges(); s.addRange(r); });
  await popup.keyboard.type(' und weiter');
  await popup.waitForTimeout(800);
  await expect(frame.locator('#p a')).toHaveCount(0);
  await expect(frame.locator('#p')).toHaveText('Siehe https://example.com hier und weiter');

  // Through the dialog too.
  await frame.evaluate(() => { document.getElementById('p').innerHTML = 'Mehr <a href="https://a.de">auf a.de</a>'; const t = document.querySelector('#p a').firstChild; const r = document.createRange(); r.setStart(t, 1); r.collapse(true); const s = document.getSelection(); s.removeAllRanges(); s.addRange(r); });
  await popup.locator('.ed-btn[title^="Link"]').click();
  await popup.locator('#link-remove').click();
  await expect(frame.locator('#p a')).toHaveCount(0);
  await expect(frame.locator('#p')).toHaveText('Mehr auf a.de');
  await popup.close({ runBeforeUnload: false });
});
