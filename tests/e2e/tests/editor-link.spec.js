const { test, expect } = require('@playwright/test');
const { openApp, popupFrom } = require('./helpers');

test('Link im Mailtext einfügen und bearbeiten', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('#subject')).toBeVisible();
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
