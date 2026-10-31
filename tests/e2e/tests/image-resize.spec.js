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
  // The click lands on the click area in the outer window (WebKit delivers no
  // events from the editor frame), lying exactly over the picture.
  const spot = popup.getByTestId('img-spot');
  await expect(spot).toHaveCount(1);
  const [ib, sb] = [await img.boundingBox(), await spot.boundingBox()];
  expect(Math.abs(ib.x - sb.x) + Math.abs(ib.y - sb.y) + Math.abs(ib.width - sb.width)).toBeLessThan(2);
  await spot.click();
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
  await frame.locator('body').click({ position: { x: 400, y: 300 } });
  await expect(popup.getByTestId('img-resize')).toHaveCount(0);
  expect(await frame.evaluate(() => document.body.innerHTML)).not.toContain('img-resize');
  await popup.close({ runBeforeUnload: false });
});

async function editorWith(page, html) {
  await openApp(page);
  const popup = await popupFrom(page, () => page.locator('[data-testid=tb-new]').click());
  await expect(popup.locator('body[data-ready="1"]')).toHaveCount(1);
  const frame = await (await popup.locator('iframe.editor-frame').elementHandle()).contentFrame();
  const url = await pngDataUrl(popup);
  await frame.evaluate(([u, x]) => { document.body.innerHTML = x.replace('SRC', u); }, [url, html]);
  return { popup, frame };
}

test('Bild per Drag an eine andere Stelle im Text verschieben', async ({ page }) => {
  const { popup, frame } = await editorWith(page,
    '<p id="a">Erster Absatz mit Text</p><p id="b"><img id="pic" src="SRC" width="80"></p><p id="c">Zweiter Absatz Ende</p>');
  const spot = popup.getByTestId('img-spot');
  await expect(spot).toHaveCount(1);
  const s = await spot.boundingBox();
  const target = await frame.locator('#c').boundingBox();
  // Drop right after "Zweiter" in the last paragraph (boxes are page coordinates).
  const tx = target.x + 50, ty = target.y + target.height / 2;
  await popup.mouse.move(s.x + s.width / 2, s.y + s.height / 2);
  await popup.mouse.down();
  await popup.mouse.move(tx, ty, { steps: 8 });
  await popup.mouse.up();
  await expect(frame.locator('#c #pic')).toHaveCount(1);
  // The empty paragraph it came from is gone, the text is intact.
  await expect(frame.locator('#b')).toHaveCount(0);
  await expect(frame.locator('#c')).toHaveText('Zweiter Absatz Ende');
  await expect(frame.locator('#a')).toHaveText('Erster Absatz mit Text');
  // The click area follows the picture.
  const ib = await frame.locator('#pic').boundingBox();
  const sb = await popup.getByTestId('img-spot').boundingBox();
  expect(Math.abs(ib.x - sb.x) + Math.abs(ib.y - sb.y)).toBeLessThan(3);
  await popup.close({ runBeforeUnload: false });
});

test('Bild entfernen über den Rahmen', async ({ page }) => {
  const { popup, frame } = await editorWith(page, '<p>Text</p><p><img id="pic" src="SRC"></p>');
  await popup.getByTestId('img-spot').click();
  await popup.getByTestId('img-remove').click();
  await expect(frame.locator('#pic')).toHaveCount(0);
  await expect(popup.getByTestId('img-spot')).toHaveCount(0);
  await expect(popup.getByTestId('img-resize')).toHaveCount(0);
  await popup.close({ runBeforeUnload: false });
});
