// Compose window (new mail, reply, reply all, forward, edit draft).
import { call, params, closeWindow, isTauri } from './api.js';
import { h, $, esc, fmtSize, fileToBase64, initials, avatarColor } from './util.js';
import { icon } from './icons.js';
import { modal, reportError, toast, confirmDialog, showMenuAt, setPending } from './dialogs.js';
import { recipientField } from './recipients.js';
import { createEditor } from './editor.js';
import { splitBtn } from './toolbar.js';
import { state, bootstrapSecondary, mailboxes, mailboxById, accountOfMailbox } from './state.js';
import { duePicker } from './due.js';
import { loadSignatures, defaultSignatureFor, applySignature, currentSignatureId, signatureMenuItems, signaturesPanel } from './signatures.js';

const P = params();
const MODE_TITLE = { new: 'Neue Nachricht', reply: 'Antwort', replyAll: 'Antwort', forward: 'Weiterleitung' };

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

let to, cc, bcc, subject, editor, ccRow, bccRow, attList, fromSel, impBtn, titleEl, ccLink, sigBtn, remind, sendSplit;

function markDirty() { st.dirty = true; }

function renderHead() {
  const head = $('#head');
  const more = h('button.icon-btn', { type: 'button', title: 'Weitere Optionen', 'aria-label': 'Weitere Optionen', dataset: { testid: 'compose-more' }, html: icon('menu', 18) });
  more.addEventListener('click', () => showMenuAt(more, [
    { label: 'Als Entwurf speichern', icon: 'save', shortcut: 'Strg+S', onClick: () => save(false), testid: 'compose-save' },
    { label: 'Verwerfen', icon: 'delete', onClick: () => discard(), testid: 'compose-discard' },
    { separator: true },
    { label: 'Wichtigkeit: hoch', icon: 'important', checked: st.importance === 'high', onClick: () => setImportance(st.importance === 'high' ? 'normal' : 'high'), testid: 'imp-high' },
    { label: 'Wichtigkeit: niedrig', icon: 'lowImportance', checked: st.importance === 'low', onClick: () => setImportance(st.importance === 'low' ? 'normal' : 'low'), testid: 'imp-low' },
    { label: 'Lesebestätigung anfordern', icon: 'receipt', checked: st.receipt, onClick: () => { st.receipt = !st.receipt; markDirty(); syncToggles(); }, testid: 'receipt' },
  ]));
  titleEl = h('div.cw-title', 'Neue Nachricht');
  head.append(more, titleEl, h('span.cw-flags'));
}

function setImportance(v) { st.importance = v; markDirty(); syncToggles(); }

function syncToggles() {
  if (impBtn) {
    impBtn.classList.toggle('on', st.importance === 'high');
    impBtn.title = st.importance === 'high' ? 'Wichtigkeit: hoch (aktiv)' : 'Wichtigkeit: hoch';
  }
  const flags = $('.cw-flags');
  if (flags) {
    flags.innerHTML = '';
    if (st.importance === 'high') flags.appendChild(h('span.pill.warn', { html: icon('important', 13) + ' Wichtig' }));
    if (st.importance === 'low') flags.appendChild(h('span.pill', { html: icon('lowImportance', 13) + ' Niedrig' }));
    if (st.receipt) flags.appendChild(h('span.pill', { html: icon('receipt', 13) + ' Lesebestätigung' }));
  }
}

function renderForm() {
  const form = $('#form');
  const mbId = () => st.mailboxId;
  to = recipientField({ mailboxId: mbId, name: 'to', onChange: markDirty });
  cc = recipientField({ mailboxId: mbId, name: 'cc', onChange: markDirty });
  bcc = recipientField({ mailboxId: mbId, name: 'bcc', onChange: markDirty });
  subject = h('input.cw-input#subject', { type: 'text', name: 'subject', 'aria-label': 'Betreff', placeholder: '' });
  subject.addEventListener('input', () => { markDirty(); updateTitle(); });

  // send row: [Senden ▾] + Von
  const sendrow = $('#sendrow');
  const send = splitBtn('Senden', 'send', () => send_().catch(reportError), (b) => showMenuAt(b, [
    { label: 'Senden', icon: 'send', shortcut: 'Strg+Eingabe', onClick: () => send_() },
    { label: 'Speichern & schließen', icon: 'save', onClick: async () => { await save(true); await finish(); } },
  ]), { testid: 'compose-send', menuTestid: 'compose-send-menu', primary: true, title: 'Senden (Strg+Eingabe)' });
  const mbs = mailboxes();
  const fromChip = h('div.from-chip#from-chip');
  const paintFrom = () => {
    const mb = mailboxById(st.mailboxId) || mbs[0];
    fromChip.innerHTML = '';
    if (!mb) return;
    fromChip.append(...[h('span.avatar.xs', { style: { background: avatarColor(mb.address) } }, initials(mb.displayName, mb.address)),
      h('span.from-text', `${mb.displayName} <${mb.address}>`), mbs.length > 1 && !(st.draft && st.draft.id) ? h('span', { html: icon('chevronDown', 14) }) : null].filter(Boolean));
  };
  if (mbs.length > 1) {
    fromChip.classList.add('clickable');
    fromChip.addEventListener('click', () => {
      if (st.draft && st.draft.id) { toast('Der Absender kann für einen vorhandenen Entwurf nicht geändert werden.'); return; }
      showMenuAt(fromChip, mbs.map((m) => ({ label: `${m.displayName} <${m.address}>`, checked: m.id === st.mailboxId, onClick: async () => {
        const changedAccount = accountOfMailbox(m.id) !== accountOfMailbox(st.mailboxId);
        st.mailboxId = m.id; markDirty(); paintFrom();
        if (changedAccount) await useDefaultSignature();
      } })));
    });
  }
  renderForm.paintFrom = paintFrom;
  sendSplit = send;
  sendrow.append(send, h('span.cw-from-lbl', 'Von'), fromChip);

  // fields
  ccLink = h('button.linkbtn.cc-link', { type: 'button', dataset: { testid: 'add-ccbcc' }, onclick: () => showCcBcc(true) }, 'Cc & Bcc hinzufügen');
  const row = (label, field, extra) => h('div.cw-row', h('label.cw-lbl', { onclick: () => field.focus() }, label), h('div.cw-val', field.el || field, extra || null));
  const toRow = row('An', to, ccLink);
  ccRow = row('Cc', cc);
  bccRow = row('Bcc', bcc);
  ccRow.classList.add('hidden'); bccRow.classList.add('hidden');
  remind = reminderRow();
  form.append(toRow, ccRow, bccRow, remind.el, row('Betreff', subject));

  attList = $('#atts');

  // editor with attach + importance at the front of the toolbar
  const fileInput = h('input', { type: 'file', multiple: true, style: { display: 'none' }, id: 'file-input' });
  fileInput.addEventListener('change', async () => {
    for (const f of fileInput.files) await addFile(f).catch(reportError);
    fileInput.value = '';
  });
  const attachBtn = h('button.ed-btn', { type: 'button', title: 'Datei anfügen', 'aria-label': 'Datei anfügen', dataset: { testid: 'compose-attach' }, html: icon('attach', 18), onclick: () => fileInput.click() });
  impBtn = h('button.ed-btn.imp', { type: 'button', title: 'Wichtigkeit: hoch', 'aria-label': 'Wichtigkeit hoch', dataset: { testid: 'compose-importance' }, html: icon('important', 18), onclick: () => setImportance(st.importance === 'high' ? 'normal' : 'high') });
  sigBtn = h('button.ed-btn.ed-sig', { type: 'button', title: 'Signatur', 'aria-label': 'Signatur', dataset: { testid: 'compose-signature' }, html: icon('signature', 18) + `<span class="ed-sig-label">Signatur</span>` + icon('chevronDown', 12) });
  sigBtn.addEventListener('mousedown', (e) => e.preventDefault());
  sigBtn.addEventListener('click', () => openSignatureMenu().catch(reportError));
  editor = createEditor({ onInput: markDirty, lead: [attachBtn, fileInput, impBtn, sigBtn] });
  $('#editor').append(editor.toolbar, editor.area);

  // drag & drop files onto the window
  document.addEventListener('dragover', (e) => { if (e.dataTransfer && [...e.dataTransfer.types].includes('Files')) { e.preventDefault(); document.body.classList.add('dropping'); } });
  document.addEventListener('dragleave', (e) => { if (!e.relatedTarget) document.body.classList.remove('dropping'); });
  document.addEventListener('drop', async (e) => {
    document.body.classList.remove('dropping');
    if (!e.dataTransfer || !e.dataTransfer.files.length) return;
    e.preventDefault();
    for (const f of e.dataTransfer.files) await addFile(f).catch(reportError);
  });
}

// ------------------------------------------------------------------ signatures
function composeMode() { return (st.draft && st.draft.mode) || P.mode || 'new'; }

async function openSignatureMenu() {
  const settings = await loadSignatures(accountOfMailbox(st.mailboxId));
  showMenuAt(sigBtn, signatureMenuItems({
    settings,
    current: currentSignatureId(editor.doc()),
    onPick: (sig) => { applySignature(editor.doc(), sig, composeMode()); editor.changed(); },
    onEdit: () => editSignatures(),
  }));
}

/** Replaces the signature with the default of the current account (respecting useOn* flags). */
async function useDefaultSignature() {
  const settings = await loadSignatures(accountOfMailbox(st.mailboxId));
  await editor.ready;
  applySignature(editor.doc(), defaultSignatureFor(settings, composeMode()), composeMode());
  editor.changed();
}

async function editSignatures() {
  const accountId = accountOfMailbox(st.mailboxId);
  const panel = signaturesPanel(state.accounts, accountId);
  const res = await modal({
    title: 'Signaturen', body: panel.el, width: 760, className: 'sig-modal',
    buttons: [{ label: 'Speichern', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: async () => { await panel.save(); return 'ok'; },
  });
  if (res === 'ok') toast('Signaturen gespeichert.');
}

function showCcBcc(focus) {
  ccRow.classList.remove('hidden');
  bccRow.classList.remove('hidden');
  ccLink.classList.add('hidden');
  if (focus) cc.focus();
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
  attList.classList.toggle('hidden', !all.length);
  for (const a of all) {
    const x = h('button.att-x', { type: 'button', title: 'Entfernen', 'aria-label': 'Anhang entfernen', html: icon('close', 13) });
    x.addEventListener('click', () => {
      if (a.kind === 'existing') st.removed.push(a.id);
      else st.newAttachments = st.newAttachments.filter((n) => n.tmpId !== a.id);
      markDirty();
      renderAttachments();
    });
    attList.appendChild(h('span.attachment', { title: a.name },
      h('span.att-ic', { html: icon(/^image\//.test(a.contentType) ? 'image' : 'file', 18) }),
      h('span.att-text', h('span.att-name', a.name), h('span.att-size', fmtSize(a.size))), x));
  }
}

function updateTitle() {
  const s = subject.value.trim() || MODE_TITLE[(st.draft && st.draft.mode) || P.mode || 'new'];
  document.title = s;
  if (titleEl) titleEl.textContent = s;
  if (isTauri && window.__TAURI__.window) { try { window.__TAURI__.window.getCurrentWindow().setTitle(document.title); } catch { /* ignore */ } }
}

function applyDraft(d) {
  st.draft = d;
  st.mailboxId = d.mailboxId || st.mailboxId;
  st.importance = d.importance || 'normal';
  st.receipt = !!d.requestReadReceipt;
  st.attachments = d.attachments || [];
  // Keep anything the user typed while the draft was loading.
  const merge = (field, list) => {
    const typed = field.get();
    const have = new Set((list || []).map((a) => a.address.toLowerCase()));
    field.set([...(list || []), ...typed.filter((a) => !have.has(a.address.toLowerCase()))]);
  };
  merge(to, d.to); merge(cc, d.cc); merge(bcc, d.bcc);
  if ((d.bcc || []).length || (d.cc || []).length) showCcBcc(false);
  if (!subject.value) subject.value = d.subject || '';
  syncToggles();
  renderAttachments();
  updateTitle();
}

/** "Erinnerung an mich: Aus | Ein [Zeitpunkt]" – a follow-up copy in Wiedervorlage after sending. */
function reminderRow() {
  let on = false;
  const picker = duePicker({ config: state.config, variant: 'select', testid: 'remind-when' });
  const seg = h('div.segmented.remind-seg', { role: 'radiogroup', 'aria-label': 'Erinnerung an mich' });
  const mk = (val, label) => {
    const b = h('button.seg', { type: 'button', role: 'radio', dataset: { testid: 'remind-' + (val ? 'on' : 'off') } }, label);
    b.addEventListener('click', () => { on = val; paint(); });
    seg.appendChild(b);
    return b;
  };
  const off = mk(false, 'Aus');
  const onBtn = mk(true, 'Ein');
  const paint = () => {
    off.classList.toggle('on', !on); onBtn.classList.toggle('on', on);
    off.setAttribute('aria-checked', String(!on)); onBtn.setAttribute('aria-checked', String(on));
    picker.el.classList.toggle('hidden', !on);
    el.classList.toggle('active', on);
  };
  const el = h('div.cw-row.remind-row', { dataset: { testid: 'remind-row' } },
    h('span.cw-lbl.remind-lbl'),
    h('div.cw-val.remind-val', { title: 'Nach dem Senden liegt eine Kopie zum Nachfassen auf Wiedervorlage – „Antworten“ geht an die Empfänger.' },
      h('span.remind-text', { html: icon('bell', 15) + '<span>Erinnerung an mich:</span>' }), seg, picker.el));
  paint();
  return {
    el,
    /** null = off; { date } or { error }. */
    get() { return on ? picker.get() : null; },
  };
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

async function send_() {
  if (st.busy) return;
  if (!validate()) return;
  if (!subject.value.trim()) {
    const ok = await confirmDialog('Diese Nachricht hat keinen Betreff. Möchten Sie sie trotzdem senden?', { title: 'Kein Betreff', ok: 'Trotzdem senden', cancel: 'Nicht senden' });
    if (!ok) { subject.focus(); return; }
  }
  const r = remind ? remind.get() : null;
  if (r && r.error) { toast(r.error, 'error'); return; }
  const remindAt = r && r.date ? r.date : null;
  st.busy = true;
  setSending(true);
  try {
    const draft = buildInput();
    if (remindAt) draft.remindAt = remindAt.toISOString();
    await call('send_draft', { draft });
    st.dirty = false; st.serverCreated = false;
    await finish();
  } catch (e) {
    reportError(e);
  } finally {
    st.busy = false;
    setSending(false);
  }
}

/** Locks the send button and shows a spinner while the message goes out. */
function setSending(on) {
  document.body.style.cursor = on ? 'progress' : '';
  if (!sendSplit) return;
  const [main, drop] = sendSplit.querySelectorAll('.tb-btn');
  main.disabled = drop.disabled = !!on;
  setPending(main, on);
  const label = main.querySelector('.tb-label');
  if (label) label.textContent = on ? 'Wird gesendet…' : 'Senden';
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
    if (renderForm.paintFrom) renderForm.paintFrom();
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
      title: 'Nachricht schließen',
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
    if (ctrl && e.key === 'Enter') { e.preventDefault(); send_().catch(reportError); }
    else if (ctrl && e.key.toLowerCase() === 's') { e.preventDefault(); save(false).catch(reportError); }
    else if (e.key === 'Escape') { e.preventDefault(); confirmClose().then((ok) => ok && closeWindow()); }
  });
}

async function init() {
  renderHead();
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
    if (renderForm.paintFrom) renderForm.paintFrom();
    await editor.setHtml(d.bodyHtml || '');
    st.dirty = !!P.to;
    // Loading takes a moment; never pull the cursor out of a field the user
    // already started typing in.
    const untouched = !document.activeElement || document.activeElement === document.body;
    if (untouched) { if ((d.to || []).length) await editor.focus(); else to.focus(); }
  } catch (e) {
    reportError(e);
    applyDraft({ id: null, mailboxId: st.mailboxId, mode: 'new', to: [], cc: [], bcc: [], subject: '', bodyHtml: '', importance: 'normal', requestReadReceipt: false, attachments: [] });
  }
}

init().catch(reportError);
export { state, esc };
