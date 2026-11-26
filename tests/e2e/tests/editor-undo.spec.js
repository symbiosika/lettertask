const { test, expect } = require('@playwright/test');
const { openApp, popupFrom } = require('./helpers');

/** New mail with an empty text, caret in the editor. */
async function emptyEditor(page) {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('body[data-ready="1"]')).toHaveCount(1);
  const frame = await (await popup.locator('iframe.editor-frame').elementHandle()).contentFrame();
  await frame.evaluate(() => { document.body.innerHTML = '<p><br></p>'; });
  // The cleared text is a step of its own; the test undoes only its own typing.
  await popup.waitForTimeout(1200);
  await frame.locator('body').click();
  const text = () => frame.evaluate(() => document.body.innerText.replace(/ /g, ' ').trim());
  return { popup, frame, text };
}

test('Strg+Z nimmt Tippen und Löschen schrittweise zurück, Strg+Y wiederholt', async ({ page }) => {
  const { popup, text } = await emptyEditor(page);
  await popup.keyboard.type('Hallo Welt');
  await popup.keyboard.press('Backspace');
  await popup.keyboard.press('Backspace');
  await popup.keyboard.press('Backspace');
  await popup.keyboard.press('Backspace');
  await expect.poll(text).toBe('Hallo');

  // Deleting is a step of its own: undo brings the deleted text back, not more.
  await popup.keyboard.press('Control+z');
  await expect.poll(text).toBe('Hallo Welt');
  await popup.keyboard.press('Control+z');
  await expect.poll(text).toBe('');

  await popup.keyboard.press('Control+y');
  await expect.poll(text).toBe('Hallo Welt');
  await popup.keyboard.press('Control+Shift+z');
  await expect.poll(text).toBe('Hallo');
  // After undoing the deletion the caret is where the text was deleted; typing
  // there drops the steps that could be redone.
  await popup.keyboard.press('Control+z');
  await popup.keyboard.type('!');
  await popup.keyboard.press('Control+y');
  await expect.poll(text).toBe('Hallo !Welt');
});

test('Strg+Z: eine Pause beim Schreiben beginnt einen neuen Schritt', async ({ page }) => {
  const { popup, text } = await emptyEditor(page);
  await popup.keyboard.type('Erster Satz.');
  await popup.waitForTimeout(1300);
  await popup.keyboard.type(' Zweiter Satz.');
  await popup.waitForTimeout(1300);
  await popup.keyboard.type(' Dritter.');
  await popup.keyboard.press('Control+z');
  await expect.poll(text).toBe('Erster Satz. Zweiter Satz.');
  await popup.keyboard.press('Control+z');
  await expect.poll(text).toBe('Erster Satz.');
});

test('Strg+Z macht eine markierte und gelöschte Passage und Formatierung rückgängig', async ({ page }) => {
  const { popup, frame, text } = await emptyEditor(page);
  await popup.keyboard.type('Bitte nicht löschen');
  await popup.waitForTimeout(1200);
  await frame.evaluate(() => {
    const t = document.body.querySelector('p').firstChild;
    const r = document.createRange(); r.setStart(t, 6); r.setEnd(t, 12);
    const s = document.getSelection(); s.removeAllRanges(); s.addRange(r);
  });
  await popup.locator('.ed-btn[data-cmd=bold]').click();
  await expect(frame.locator('b, strong, span[style*="bold"]')).toHaveCount(1);
  await popup.keyboard.press('Delete');
  await expect.poll(text).toBe('Bitte löschen');
  await popup.keyboard.press('Control+z');
  await expect.poll(text).toBe('Bitte nicht löschen');
  await expect(frame.locator('b, strong, span[style*="bold"]')).toHaveCount(1);
  await popup.keyboard.press('Control+z');
  await expect(frame.locator('b, strong, span[style*="bold"]')).toHaveCount(0);
  await expect.poll(text).toBe('Bitte nicht löschen');
});
