const { test, expect } = require('@playwright/test');
const { openApp, popupFrom } = require('./helpers');

// 200×100 red PNG.
async function pngDataUrl(page) {
  return page.evaluate(() => {
    const c = document.createElement('canvas');
    c.width = 200; c.height = 100;
    const g = c.getContext('2d'); g.fillStyle = '#d33'; g.fillRect(0, 0, 200, 100);
    return c.toDataURL('image/png');
  });
}

test('Eingefügtes Bild im Mailtext lässt sich in der Größe ändern', async ({ page }) => {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('#subject')).toBeVisible();
  // The draft loads into the editor afterwards and replaces its document.
  await expect(popup.locator('body[data-ready="1"]')).toHaveCount(1);
  const frame = await (await popup.locator('iframe.editor-frame').elementHandle()).contentFrame();
  const url = await pngDataUrl(popup);
  await frame.evaluate((u) => { document.body.insertAdjacentHTML('afterbegin', `<p><img id="pic" src="${u}"></p>`); }, url);
  const img = frame.locator('#pic');
  await expect(img).toBeVisible();
  await img.click();
  await expect(popup.getByTestId('img-resize')).toBeVisible();

  // Quick size.
  await popup.getByTestId('img-size-50').click();
  await expect(img).toHaveAttribute('width', '100');
  // Drag the corner handle 60 px to the right.
  const hb = await popup.getByTestId('img-resize-handle').boundingBox();
  await popup.mouse.move(hb.x + hb.width / 2, hb.y + hb.height / 2);
  await popup.mouse.down();
  await popup.mouse.move(hb.x + hb.width / 2 + 60, hb.y + hb.height / 2, { steps: 5 });
  await popup.mouse.up();
  await expect(img).toHaveAttribute('width', '160');
  const box = await img.boundingBox();
  expect(Math.round(box.height)).toBe(80); // aspect ratio kept

  // The frame is not part of the mail.
  await frame.locator('body').click({ position: { x: 300, y: 300 } });
  await expect(popup.getByTestId('img-resize')).toHaveCount(0);
  expect(await frame.evaluate(() => document.body.innerHTML)).not.toContain('img-resize');
  await popup.close({ runBeforeUnload: false });
});
