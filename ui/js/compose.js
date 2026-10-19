// Compose window (new mail, reply, reply all, forward, edit draft).
import { call, params, closeWindow, isTauri } from './api.js';
import { h, $, esc, fmtSize, fileToBase64 } from './util.js';
import { icon } from './icons.js';
import { modal, reportError, toast, confirmDialog } from './dialogs.js';
import { recipientField } from './recipients.js';
import { createEditor } from './editor.js';
import { tbBtn, group, setPressed } from './toolbar.js';
import { state, bootstrapSecondary, mailboxes, mailboxById } from './state.js';

const P = params();
const MODE_TITLE = { new: 'Unbenannt', reply: 'Antwort', replyAll: 'Antwort', forward: 'Weiterleitung' };

const st = {
  draft: null,            // last Draft from the server
  mailboxId: P.mailboxId,
  importance: 'normal',
  receipt: false,
  attachments: [],        // existing (server) attachments
  newAttachments: [],     // {name, contentType, contentBase64, size, tmpId}
  removed: [],
  dirty: false,
  serverCreated: false,   // create_draft created a server draft we must clean up if abandoned
  userSaved: false,
  busy: false,
  closing: false,
};

let to, cc, bcc, subject, editor, ccRow, bccRow, attRow, attList, fromSel;

function markDirty() { st.dirty = true; }

function renderToolbar() {
  const tb = $('#toolbar');
  const hi = tbBtn('Wichtigkeit: hoch', 'important', () => { st.importance = st.importance === 'high' ? 'normal' : 'high'; syncToggles(); markDirty(); }, { toggle: true, testid: 'imp-high' });
  const lo = tbBtn('Wichtigkeit: niedrig', 'lowImportance', () => { st.importance = st.importance === 'low' ? 'normal' : 'low'; syncToggles(); markDirty(); }, { toggle: true, testid: 'imp-low' });
  const rr = tbBtn('Lesebestätigung', 'receipt', () => { st.receipt = !st.receipt; syncToggles(); markDirty(); }, { toggle: true, testid: 'receipt' });
  const bccBtn = tbBtn('Bcc', 'people', () => { bccRow.classList.toggle('hidden'); if (!bccRow.classList.contains('hidden')) bcc.focus(); }, { title: 'Bcc-Feld ein-/ausblenden' });
  const fileInput = h('input', { type: 'file', multiple: true, style: { display: 'none' }, id: 'file-input' });
  fileInput.addEventListener('change', async () => {
    for (const f of fileInput.files) await addFile(f).catch(reportError);
    fileInput.value = '';
  });
  tb.append(
    group(
      tbBtn('Datei anfügen', 'attach', () => fileInput.click(), { testid: 'compose-attach' }), fileInput,
      tbBtn('Speichern', 'save', () => save(false), { testid: 'compose-save', title: 'Speichern (Strg+S)' }),
      tbBtn('Verwerfen', 'delete', () => discard(), { testid: 'compose-discard' }),
    ),
    group(bccBtn, hi, lo, rr),
  );
  function syncToggles() {
    setPressed(hi, st.importance === 'high');
    setPressed(lo, st.importance === 'low');
    setPressed(rr, st.receipt);
  }
  renderToolbar.sync = syncToggles;
}

function renderForm() {
  const form = $('#form');
  const mbId = () => st.mailboxId;
  to = recipientField({ mailboxId: mbId, name: 'to', onChange: markDirty });
  cc = recipientField({ mailboxId: mbId, name: 'cc', onChange: markDirty });
  bcc = recipientField({ mailboxId: mbId, name: 'bcc', onChange: markDirty });
  subject = h('input.input#subject', { type: 'text', name: 'subject', 'aria-label': 'Betreff' });
  subject.addEventListener('input', () => { markDirty(); updateTitle(); });

  const rows = h('div.form-rows');
  const mbs = mailboxes();
  if (mbs.length > 1) {
    fromSel = h('select.input#from', { 'aria-label': 'Von' }, ...mbs.map((m) => h('option', { value: m.id, selected: m.id === st.mailboxId }, `${m.displayName} <${m.address}>`)));
    fromSel.disabled = !!(st.draft && st.draft.id);
    fromSel.title = fromSel.disabled ? 'Der Absender kann für einen vorhandenen Entwurf nicht geändert werden.' : '';
    fromSel.addEventListener('change', () => { st.mailboxId = fromSel.value; markDirty(); });
    rows.appendChild(h('div.frow', h('span.flabel', 'Von:'), h('div.fval', fromSel)));
  }
  const lblBtn = (label, field) => h('button.flabel', { type: 'button', tabindex: '-1', onclick: () => field.focus() }, label);
  rows.appendChild(h('div.frow', lblBtn('An…', to), h('div.fval', to.el)));
  ccRow = h('div.frow', lblBtn('Cc…', cc), h('div.fval', cc.el));
  bccRow = h('div.frow.hidden', lblBtn('Bcc…', bcc), h('div.fval', bcc.el));
  rows.append(ccRow, bccRow);
  rows.appendChild(h('div.frow', h('span.flabel', 'Betreff:'), h('div.fval', subject)));
  attList = h('div.att-row');
  attRow = h('div.frow.hidden', h('span.flabel', 'Angefügt:'), h('div.fval', attList));
  rows.appendChild(attRow);

  const sendBig = h('button.send-big', { type: 'button', dataset: { testid: 'compose-send' }, title: 'Senden (Strg+Eingabe)', html: icon('send', 30) + '<span>Senden</span>', onclick: () => send().catch(reportError) });
  form.append(h('div.form-send', sendBig), rows);

  editor = createEditor({ onInput: markDirty });
  $('#editor').append(editor.toolbar, editor.area);

  // drag & drop files onto the window
  document.addEventListener('dragover', (e) => { if (e.dataTransfer && [...e.dataTransfer.types].includes('Files')) e.preventDefault(); });
  document.addEventListener('drop', async (e) => {
    if (!e.dataTransfer || !e.dataTransfer.files.length) return;
    e.preventDefault();
    for (const f of e.dataTransfer.files) await addFile(f).catch(reportError);
  });
}

async function addFile(file) {
  if (file.size > 150 * 1024 * 1024) { toast(`„${file.name}" ist größer als 150 MB und kann nicht angefügt werden.`, 'error'); return; }
  const contentBase64 = await fileToBase64(file);
  st.newAttachments.push({ name: file.name, contentType: file.type || 'application/octet-stream', contentBase64, size: file.size, tmpId: Math.random().toString(36).slice(2) });
  markDirty();
  renderAttachments();
}

function renderAttachments() {
  attList.innerHTML = '';
  const all = [
    ...st.attachments.filter((a) => !st.removed.includes(a.id)).map((a) => ({ ...a, kind: 'existing' })),
    ...st.newAttachments.map((a) => ({ ...a, id: a.tmpId, kind: 'new' })),
  ];
  attRow.classList.toggle('hidden', !all.length);
  for (const a of all) {
    const x = h('button.att-x', { type: 'button', title: 'Entfernen', html: icon('close', 10) });
    x.addEventListener('click', () => {
      if (a.kind === 'existing') st.removed.push(a.id);
      else st.newAttachments = st.newAttachments.filter((n) => n.tmpId !== a.id);
      markDirty();
      renderAttachments();
    });
    attList.appendChild(h('span.attachment', { title: a.name }, h('span', { html: icon(/^image\//.test(a.contentType) ? 'image' : 'file', 16) }), h('span.att-name', a.name), h('span.att-size', `(${fmtSize(a.size)})`), x));
  }
}

function updateTitle() {
  const s = subject.value.trim() || MODE_TITLE[(st.draft && st.draft.mode) || P.mode || 'new'];
  document.title = `${s} – Nachricht (HTML)`;
  if (isTauri && window.__TAURI__.window) { try { window.__TAURI__.window.getCurrentWindow().setTitle(document.title); } catch { /* ignore */ } }
}

function applyDraft(d) {
  st.draft = d;
  st.mailboxId = d.mailboxId || st.mailboxId;
  st.importance = d.importance || 'normal';
  st.receipt = !!d.requestReadReceipt;
  st.attachments = d.attachments || [];
  to.set(d.to); cc.set(d.cc); bcc.set(d.bcc);
  if ((d.bcc || []).length) bccRow.classList.remove('hidden');
  subject.value = d.subject || '';
  renderToolbar.sync();
  renderAttachments();
  updateTitle();
}

function buildInput() {
  return {
    id: st.draft ? st.draft.id : null,
    mailboxId: st.mailboxId,
    mode: (st.draft && st.draft.mode) || P.mode || 'new',
    to: to.get(), cc: cc.get(), bcc: bcc.get(),
    subject: subject.value,
    bodyHtml: editor.getHtml(),
    importance: st.importance,
    requestReadReceipt: st.receipt,
    newAttachments: st.newAttachments.map(({ name, contentType, contentBase64 }) => ({ name, contentType, contentBase64 })),
    removedAttachmentIds: [...st.removed],
  };
}

function validate() {
  const invalid = [...to.invalid(), ...cc.invalid(), ...bcc.invalid()];
  if (invalid.length) { toast(`Ungültige Adresse: ${invalid.join(', ')}`, 'error'); return false; }
  const input = buildInput();
  if (!input.to.length && !input.cc.length && !input.bcc.length) {
    toast('Geben Sie mindestens einen Empfänger an.', 'error');
    to.focus();
    return false;
  }
  return true;
}

async function send() {
  if (st.busy) return;
  if (!validate()) return;
  if (!subject.value.trim()) {
    const ok = await confirmDialog('Diese Nachricht hat keinen Betreff. Möchten Sie sie trotzdem senden?', { title: 'Kein Betreff', ok: 'Trotzdem senden', cancel: 'Nicht senden' });
    if (!ok) { subject.focus(); return; }
  }
  st.busy = true;
  document.body.style.cursor = 'progress';
  try {
    await call('send_draft', { draft: buildInput() });
    st.dirty = false; st.serverCreated = false;
    await finish();
  } catch (e) {
    reportError(e);
  } finally {
    st.busy = false;
    document.body.style.cursor = '';
  }
}

async function save(quiet) {
  if (st.busy) return;
  st.busy = true;
  try {
    const d = await call('save_draft', { draft: buildInput() });
    st.newAttachments = []; st.removed = [];
    st.draft = { ...(st.draft || {}), ...d };
    st.attachments = d.attachments || st.attachments;
    st.userSaved = true; st.dirty = false;
    if (fromSel) { fromSel.disabled = true; }
    renderAttachments();
    if (!quiet) toast('Entwurf gespeichert.');
  } finally { st.busy = false; }
}

async function discard() {
  const ok = await confirmDialog('Möchten Sie diese Nachricht verwerfen?', { title: 'Verwerfen', ok: 'Verwerfen', danger: true });
  if (!ok) return;
  if (st.draft && st.draft.id) await call('discard_draft', { mailboxId: st.mailboxId, draftId: st.draft.id }).catch(reportError);
  st.dirty = false; st.serverCreated = false;
  await finish();
}

async function finish() {
  st.closing = true;
  await closeWindow();
}

/** Called before the window closes. Returns true when closing may proceed. */
async function confirmClose() {
  if (st.closing) return true;
  if (st.dirty) {
    const choice = await modal({
      title: 'Outlook Classic',
      body: h('div.confirm-body', h('span', { html: icon('warning', 32) }), h('div', 'Möchten Sie die Änderungen speichern?')),
      buttons: [{ label: 'Ja', value: 'yes', primary: true }, { label: 'Nein', value: 'no' }, { label: 'Abbrechen', value: 'cancel', cancel: true }],
    });
    if (choice === 'cancel' || choice === undefined) return false;
    if (choice === 'yes') {
      try { await save(true); } catch (e) { reportError(e); return false; }
      st.closing = true;
      return true;
    }
  }
  if (st.serverCreated && !st.userSaved && st.draft && st.draft.id) {
    await call('discard_draft', { mailboxId: st.mailboxId, draftId: st.draft.id }, { silent: true }).catch(() => {});
  }
  st.closing = true;
  return true;
}

function setupClose() {
  if (isTauri && window.__TAURI__.window && window.__TAURI__.window.getCurrentWindow) {
    const w = window.__TAURI__.window.getCurrentWindow();
    if (w.onCloseRequested) {
      w.onCloseRequested(async (event) => {
        if (st.closing) return;
        event.preventDefault();
        if (await confirmClose()) { st.closing = true; await (w.destroy ? w.destroy() : w.close()); }
      });
    }
  } else {
    window.addEventListener('beforeunload', (e) => {
      if (st.closing) return;
      if (st.dirty) { e.preventDefault(); e.returnValue = ''; return; }
      if (st.serverCreated && !st.userSaved && st.draft && st.draft.id && navigator.sendBeacon) {
        navigator.sendBeacon('/api/call/discard_draft', new Blob([JSON.stringify({ mailboxId: st.mailboxId, draftId: st.draft.id })], { type: 'application/json' }));
      }
    });
  }
  window.addEventListener('keydown', (e) => {
    if (document.querySelector('.modal-overlay')) return;
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && e.key === 'Enter') { e.preventDefault(); send().catch(reportError); }
    else if (ctrl && e.key.toLowerCase() === 's') { e.preventDefault(); save(false).catch(reportError); }
    else if (e.key === 'Escape') { e.preventDefault(); confirmClose().then((ok) => ok && closeWindow()); }
  });
}

async function init() {
  renderToolbar();
  try { await bootstrapSecondary(); } catch (e) { reportError(e); }
  if (!st.mailboxId) st.mailboxId = mailboxes()[0] && mailboxes()[0].id;
  renderForm();
  setupClose();
  try {
    let d;
    if (P.draftId) {
      d = await call('get_draft', { mailboxId: st.mailboxId, messageId: P.draftId });
      st.userSaved = true;
    } else {
      d = await call('create_draft', { mailboxId: st.mailboxId, mode: P.mode || 'new', sourceMessageId: P.sourceMessageId || null });
      st.serverCreated = !!d.id;
      if (P.to) { const extra = P.to.split(/[;,]/).map((x) => x.trim()).filter(Boolean).map((a) => ({ name: a, address: a })); d.to = [...(d.to || []), ...extra]; }
    }
    applyDraft(d);
    // From: show mailbox address even when only one exists
    if (fromSel) fromSel.value = st.mailboxId;
    await editor.setHtml(d.bodyHtml || '');
    st.dirty = !!P.to;
    if ((d.to || []).length) await editor.focus(); else to.focus();
  } catch (e) {
    reportError(e);
    applyDraft({ id: null, mailboxId: st.mailboxId, mode: 'new', to: [], cc: [], bcc: [], subject: '', bodyHtml: '', importance: 'normal', requestReadReceipt: false, attachments: [] });
  }
  const mb = mailboxById(st.mailboxId);
  if (mb && !fromSel) $('#form').querySelector('.form-rows').prepend(h('div.frow', h('span.flabel', 'Von:'), h('div.fval', { style: { paddingTop: '4px', color: '#333' } }, `${mb.displayName} <${mb.address}>`)));
}

init().catch(reportError);
export { state, esc };
