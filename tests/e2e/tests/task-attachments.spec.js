// Files attached to a task ("Aufgabe (Mail)") – also when it is delegated.
const { test, expect } = require('@playwright/test');
const { openApp, row, select, popupFrom, openFolder } = require('./helpers');

async function newTask(page) {
  const popup = await popupFrom(page, () => page.locator('[data-testid=new-task]').click());
  await expect(popup.locator('body[data-ready="1"]')).toHaveCount(1);
  return popup;
}

const PDF = { name: 'Angebot.pdf', mimeType: 'application/pdf', buffer: Buffer.from('%PDF-1.4\n%%EOF\n') };
const TXT = { name: 'Notizen.txt', mimeType: 'text/plain', buffer: Buffer.from('Preise prüfen') };

test('Aufgabe mit Anhängen: hinzufügen, wieder entfernen, anlegen', async ({ page }) => {
  await openApp(page);
  const popup = await newTask(page);
  const subject = `Angebot mit Anhang ${Date.now()}`;
  await popup.locator('#subject').fill(subject);
  const list = popup.getByTestId('task-attachments');
  await expect(list).toBeHidden();
  await popup.getByTestId('task-file-input').setInputFiles([PDF, TXT]);
  await expect(list.getByTestId('attachment-chip')).toHaveCount(2);
  await list.getByTestId('attachment-chip').filter({ hasText: 'Notizen.txt' }).locator('.att-x').click();
  await expect(list.getByTestId('attachment-chip')).toHaveCount(1);
  await expect(list).toContainText('Angebot.pdf');
  const closed = popup.waitForEvent('close');
  await popup.getByTestId('task-create').click();
  await closed;
  const r = row(page, subject);
  await expect(r).toBeVisible({ timeout: 15_000 });
  await select(page, subject);
  const atts = page.locator('#readpane [data-testid=rp-attachment]');
  await expect(atts).toHaveCount(1);
  await expect(atts).toContainText('Angebot.pdf');
});

test('Anhänge per Drag & Drop in das Aufgabenfenster', async ({ page }) => {
  await openApp(page);
  const popup = await newTask(page);
  const dt = await popup.evaluateHandle(() => {
    const d = new DataTransfer();
    d.items.add(new File(['hallo'], 'Skizze.png', { type: 'image/png' }));
    return d;
  });
  await popup.locator('#form').dispatchEvent('dragover', { dataTransfer: dt });
  await expect(popup.locator('body')).toHaveClass(/dropping/);
  await popup.locator('#form').dispatchEvent('drop', { dataTransfer: dt });
  await expect(popup.getByTestId('task-attachments')).toContainText('Skizze.png');
  await expect(popup.locator('body')).not.toHaveClass(/dropping/);
});

test('Delegierte Aufgabe: Anhang geht mit der Mail raus und bleibt an der eigenen Kopie', async ({ page }) => {
  await openApp(page);
  const popup = await newTask(page);
  const subject = `Vertrag gegenlesen ${Date.now()}`;
  await popup.locator('#subject').fill(subject);
  await popup.getByTestId('task-for-other').click();
  await popup.locator('[data-testid=task-delegate] .rcpt-input').fill('jonas.weber@contoso.example');
  await popup.locator('[data-testid=task-delegate] .rcpt-input').press('Enter');
  await popup.getByTestId('task-file-input').setInputFiles([PDF]);
  const closed = popup.waitForEvent('close');
  await popup.getByTestId('task-create').click();
  await closed;
  // The own copy waits in the inbox (no due time).
  await select(page, subject);
  await expect(page.locator('#readpane [data-testid=rp-attachment]')).toContainText('Angebot.pdf');
  // The mail that went out carries it too.
  await openFolder(page, 'Gesendete Elemente');
  await select(page, subject);
  await expect(page.locator('#readpane [data-testid=rp-attachment]')).toContainText('Angebot.pdf');
});
