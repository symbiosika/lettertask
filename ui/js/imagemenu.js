// "Bild kopieren" / "Bild speichern unter…" for pictures in mails and drafts.
// The WebView's own image menu copies a data: picture as its (base64) address
// text and cannot save it. In WebKitGTK no event reaches the page from the
// sandboxed mail frame, so there the app catches the right click natively and
// sends "image-menu" { label, uri, x, y } (src-tauri) to open this menu.
import { fileToBase64 } from './util.js';
import { call, on, isTauri, currentWindow, invokeNative } from './api.js';
import { showMenu, reportError, toast } from './dialogs.js';

/** The picture as PNG (other formats are converted; the clipboard takes PNG). */
async function imagePng(img) {
  const src = img.currentSrc || img.src;
  const toPng = (source, w, h_) => new Promise((resolve, reject) => {
    const c = document.createElement('canvas');
    c.width = w; c.height = h_;
    c.getContext('2d').drawImage(source, 0, 0);
    c.toBlob((b) => (b ? resolve(b) : reject(new Error('Bild nicht lesbar'))), 'image/png');
  });
  try {
    if (!img.complete) await img.decode();
    return await toPng(img, img.naturalWidth, img.naturalHeight);
  } catch {
    // Remote picture (canvas tainted): fetch it as data instead.
    const blob = await (await fetch(src)).blob();
    if (blob.type === 'image/png') return blob;
    const bmp = await createImageBitmap(blob);
    return toPng(bmp, bmp.width, bmp.height);
  }
}

/**
 * Copies a picture of a mail as image (pasted as picture elsewhere, e.g. in a chat).
 * `native`: through the app (GTK clipboard) instead of the WebView's clipboard API.
 */
export async function copyImage(img, { native = false } = {}) {
  try {
    if (native) {
      await invokeNative('copy_image', { pngBase64: await fileToBase64(await imagePng(img)) });
    } else {
      if (!window.ClipboardItem || !navigator.clipboard || !navigator.clipboard.write) throw new Error('Die Zwischenablage nimmt hier keine Bilder an.');
      // A promise inside the item keeps the user gesture (required by WebKit).
      await navigator.clipboard.write([new ClipboardItem({ 'image/png': imagePng(img) })]);
    }
    toast('Bild kopiert.');
  } catch (e) {
    reportError(new Error('Bild konnte nicht kopiert werden: ' + (e.message || e)));
  }
}

const EXT = { 'image/jpeg': 'jpg', 'image/svg+xml': 'svg', 'image/x-icon': 'ico' };

/** File name and content (base64) of a picture: data: pictures as they are, others as PNG. */
async function imageFile(img) {
  const src = img.currentSrc || img.src;
  // Inline pictures may carry their content id as fragment ("#cid=…").
  const m = /^data:([^;,]+)?((?:;[^;,]*)*?);base64,([^#]*)/i.exec(src);
  if (m) {
    const type = (m[1] || 'image/png').toLowerCase();
    return { name: 'Bild.' + (EXT[type] || type.split('/')[1] || 'png'), contentBase64: m[3] };
  }
  let base = 'Bild';
  try { base = decodeURIComponent(new URL(src).pathname.split('/').pop()).replace(/\.[^.]*$/, '') || base; } catch { /* keep */ }
  return { name: base + '.png', contentBase64: await fileToBase64(await imagePng(img)) };
}

/** Saves a picture through the save dialog. */
export async function saveImage(img) {
  try {
    const path = await call('save_file', await imageFile(img));
    if (path) toast('Gespeichert: ' + path);
  } catch (e) {
    reportError(new Error('Bild konnte nicht gespeichert werden: ' + (e.message || e)));
  }
}

/** The image menu at viewport position x/y. */
export function showImageMenu(img, x, y, { native = false } = {}) {
  showMenu([
    { label: 'Bild kopieren', icon: 'copy', onClick: () => copyImage(img, { native }), testid: 'img-copy' },
    { label: 'Bild speichern unter…', icon: 'save', onClick: () => saveImage(img), testid: 'img-save' },
  ], x, y);
}

/** The picture with address `uri` on this page or in one of its frames. */
function findImage(uri) {
  const docs = [document];
  for (const f of document.querySelectorAll('iframe')) {
    try { if (f.contentDocument) docs.push(f.contentDocument); } catch { /* other origin */ }
  }
  for (const d of docs) {
    for (const img of d.images) if (img.currentSrc === uri || img.src === uri) return img;
  }
  return null;
}

let installed = false;
/** Opens the image menu when the app reports a right click on a picture (WebKitGTK). */
export function installNativeImageMenu() {
  if (installed || !isTauri) return;
  installed = true;
  const w = currentWindow();
  if (!w) return;
  on('image-menu', (p) => {
    if (!p || p.label !== w.label) return;
    let img = findImage(p.uri);
    if (!img) { img = new Image(); img.src = p.uri; }
    showImageMenu(img, p.x, p.y, { native: true });
  });
}
