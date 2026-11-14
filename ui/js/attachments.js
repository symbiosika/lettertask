// Files to attach, shared by the compose and the task window: picked,
// dropped (browser and app), pasted from the file manager or dragged out of
// another mail. An attachment here is { name, contentType, contentBase64, size }.
import { call, currentWindow, isTauri, on, invokeNative } from './api.js';
import { h, fmtSize, fileToBase64 } from './util.js';
import { icon } from './icons.js';
import { toast, reportError } from './dialogs.js';

export const MAX_ATTACHMENT = 150 * 1024 * 1024;

const TYPES = {
  pdf: 'application/pdf', txt: 'text/plain', csv: 'text/csv', html: 'text/html', htm: 'text/html', ics: 'text/calendar', eml: 'message/rfc822',
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml', heic: 'image/heic',
  zip: 'application/zip', json: 'application/json', xml: 'application/xml', mp3: 'audio/mpeg', mp4: 'video/mp4',
  doc: 'application/msword', xls: 'application/vnd.ms-excel', ppt: 'application/vnd.ms-powerpoint',
  docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  pptx: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
};

/** Content type from the file extension (files dropped in the app come without one). */
export function contentTypeOf(name) {
  const ext = (String(name).match(/\.([^.]+)$/) || [])[1];
  return (ext && TYPES[ext.toLowerCase()]) || 'application/octet-stream';
}

/** A browser File; null (with a message) when it is too large. */
export async function readFile(file) {
  if (file.size > MAX_ATTACHMENT) { toast(`„${file.name}" ist größer als 150 MB und kann nicht angefügt werden.`, 'error'); return null; }
  const contentBase64 = await fileToBase64(file);
  return { name: file.name, contentType: file.type || contentTypeOf(file.name), contentBase64, size: file.size };
}

/** Attachment of another mail ({ mailboxId, messageId, attachmentId, name, contentType }). */
export async function readMailAttachment(ref) {
  const f = await call('attachment_content', { mailboxId: ref.mailboxId, messageId: ref.messageId, attachmentId: ref.attachmentId });
  return { name: f.name, contentType: ref.contentType || contentTypeOf(f.name), contentBase64: f.contentBase64, size: f.size };
}

function fromNative(res) {
  if ((res.skipped || []).length) toast(`Nicht angefügt (Ordner oder größer als 150 MB): ${res.skipped.join(', ')}`, 'error');
  return (res.files || []).map((f) => ({ name: f.name, contentType: contentTypeOf(f.name), contentBase64: f.contentBase64, size: f.size }));
}

/** Ctrl+V of files copied in the file manager: the page only gets their
 *  file:// addresses, the app reads the files. */
export async function readPastedPaths(paths) {
  if (!isTauri) { toast('Kopierte Dateien lassen sich nur in der Desktop-App einfügen – bitte per Drag & Drop anfügen.', 'error'); return []; }
  return fromNative(await invokeNative('read_local_files', { paths }));
}

/** Files dropped anywhere onto the window go to `add(att)`; body.dropping marks the drag. */
export function installFileDrop(add) {
  const dropKinds = ['Files', 'application/x-lt-attachment'];
  document.addEventListener('dragover', (e) => { if (e.dataTransfer && [...e.dataTransfer.types].some((t) => dropKinds.includes(t))) { e.preventDefault(); document.body.classList.add('dropping'); } });
  document.addEventListener('dragleave', (e) => { if (!e.relatedTarget) document.body.classList.remove('dropping'); });
  document.addEventListener('drop', async (e) => {
    document.body.classList.remove('dropping');
    if (!e.dataTransfer) return;
    // An attachment dragged out of a mail (lettertask in the browser).
    const ref = e.dataTransfer.getData('application/x-lt-attachment');
    if (ref) { e.preventDefault(); add(await readMailAttachment(JSON.parse(ref)).catch((err) => { reportError(err); return null; })); return; }
    if (!e.dataTransfer.files.length) return;
    e.preventDefault();
    for (const f of e.dataTransfer.files) add(await readFile(f).catch((err) => { reportError(err); return null; }));
  });
  // In the app, file drops arrive from the backend (Tauri takes them over natively).
  if (isTauri) {
    const w = currentWindow();
    const mine = (p) => p && w && p.label === w.label;
    on('files-drag', (p) => { if (mine(p)) document.body.classList.toggle('dropping', !!p.over); });
    on('files-dropped', (p) => { if (mine(p)) for (const a of fromNative(p)) add(a); });
  }
}

/** Chip of one attachment with a remove button. */
export function attachmentChip(a, onRemove) {
  const x = h('button.att-x', { type: 'button', title: 'Entfernen', 'aria-label': 'Anhang entfernen', html: icon('close', 13) });
  x.addEventListener('click', onRemove);
  return h('span.attachment', { title: a.name, dataset: { testid: 'attachment-chip' } },
    h('span.att-ic', { html: icon(/^image\//.test(a.contentType) ? 'image' : 'file', 18) }),
    h('span.att-text', h('span.att-name', a.name), h('span.att-size', fmtSize(a.size))), x);
}
