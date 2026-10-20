// "Aufgabe (Mail)": a mail from yourself to yourself – only subject, notes and due time.
// Created directly on the server (create_task): "Sofort" → inbox, otherwise → Wiedervorlage.
import { call, params, closeWindow, isTauri } from './api.js';
import { h, $, initials, avatarColor } from './util.js';
import { icon } from './icons.js';
import { reportError, toast, confirmDialog, showMenuAt } from './dialogs.js';
import { createEditor } from './editor.js';
import { state, bootstrapSecondary, mailboxes, mailboxById } from './state.js';
import { duePicker } from './due.js';
import { broadcast } from './bus.js';

const P = params();
const st = { mailboxId: P.mailboxId || null, dirty: false, busy: false, closing: false };
let subject, editor, due, createBtn;

function markDirty() { st.dirty = true; }

function render() {
  $('#head').append(h('span', { html: icon('task', 18) }), h('div.cw-title', 'Neue Aufgabe'), h('span.cw-flags'));

  const form = $('#form');
  const mbs = mailboxes();
  const fromChip = h('div.from-chip#from-chip', { dataset: { testid: 'task-account' } });
  const paintFrom = () => {
    const mb = mailboxById(st.mailboxId) || mbs[0];
    fromChip.innerHTML = '';
    if (!mb) return;
    fromChip.append(...[h('span.avatar.xs', { style: { background: avatarColor(mb.address) } }, initials(mb.displayName, mb.address)),
      h('span.from-text', mb.address), mbs.length > 1 ? h('span', { html: icon('chevronDown', 14) }) : null].filter(Boolean));
  };
  if (mbs.length > 1) {
    fromChip.classList.add('clickable');
    fromChip.addEventListener('click', () => showMenuAt(fromChip, mbs.map((m) => ({
      label: `${m.displayName} <${m.address}>`, checked: m.id === st.mailboxId, onClick: () => { st.mailboxId = m.id; paintFrom(); },
    }))));
  }
  paintFrom();

  subject = h('input.task-subject#subject', { type: 'text', name: 'subject', placeholder: 'Was ist zu tun?', 'aria-label': 'Betreff', autocomplete: 'off' });
  subject.addEventListener('input', () => { markDirty(); document.title = subject.value.trim() ? `Aufgabe: ${subject.value.trim()}` : 'Neue Aufgabe'; });
  due = duePicker({ config: state.config, allowNow: true, variant: 'chips', testid: 'task-due', onChange: markDirty });

  form.append(
    h('div.task-row.task-meta', h('span.task-lbl', 'Für'), fromChip, h('span.task-hint', 'Aufgabe an mich selbst – ohne Empfänger')),
    h('div.task-row', subject),
    h('div.task-row.task-due', h('span.task-lbl', { html: icon('snooze', 15) + '<span>Fällig</span>' }), due.el),
  );

  editor = createEditor({ onInput: markDirty, minHeight: 180 });
  $('#editor').append(editor.toolbar, editor.area);

  createBtn = h('button.btn.primary', { type: 'button', dataset: { testid: 'task-create' }, title: 'Aufgabe anlegen (Strg+Eingabe)', html: icon('task', 16) + '<span>Aufgabe anlegen</span>' });
  createBtn.addEventListener('click', () => create().catch(reportError));
  const cancel = h('button.btn', { type: 'button', dataset: { testid: 'task-cancel' } }, 'Abbrechen');
  cancel.addEventListener('click', () => confirmClose().then((ok) => ok && closeWindow()));
  $('#foot').append(h('span.task-kbd', 'Strg+Eingabe: anlegen · Esc: abbrechen'), h('span.spacer'), cancel, createBtn);
}

async function create() {
  if (st.busy) return;
  const text = subject.value.trim();
  if (!text) { toast('Bitte beschreiben Sie die Aufgabe im Betreff.', 'error'); subject.focus(); return; }
  const d = due.get();
  if (d.error) { toast(d.error, 'error'); return; }
  st.busy = true;
  createBtn.disabled = true;
  try {
    await call('create_task', { mailboxId: st.mailboxId, subject: text, bodyHtml: editor.getHtml(), due: d.date ? d.date.toISOString() : null });
    broadcast({ type: 'task-created', subject: text, due: d.date ? due.label() : null });
    st.dirty = false; st.closing = true;
    await closeWindow();
  } finally {
    st.busy = false;
    createBtn.disabled = false;
  }
}

async function confirmClose() {
  if (!st.dirty || st.closing) return true;
  const ok = await confirmDialog('Die Aufgabe wurde noch nicht angelegt. Verwerfen?', { title: 'Aufgabe verwerfen', ok: 'Verwerfen', danger: true });
  if (ok) st.closing = true;
  return ok;
}

function setupClose() {
  if (isTauri && window.__TAURI__.window && window.__TAURI__.window.getCurrentWindow) {
    const w = window.__TAURI__.window.getCurrentWindow();
    if (w.onCloseRequested) {
      w.onCloseRequested(async (event) => {
        if (st.closing || !st.dirty) return;
        event.preventDefault();
        if (await confirmClose()) await (w.destroy ? w.destroy() : w.close());
      });
    }
  } else {
    window.addEventListener('beforeunload', (e) => { if (!st.closing && st.dirty) { e.preventDefault(); e.returnValue = ''; } });
  }
  window.addEventListener('keydown', (e) => {
    if (document.querySelector('.modal-overlay')) return;
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') { e.preventDefault(); create().catch(reportError); }
    else if (e.key === 'Escape') { e.preventDefault(); confirmClose().then((ok) => ok && closeWindow()); }
  });
}

async function init() {
  try { await bootstrapSecondary(); } catch (e) { reportError(e); }
  if (!st.mailboxId || !mailboxById(st.mailboxId)) st.mailboxId = mailboxes()[0] && mailboxes()[0].id;
  render();
  setupClose();
  subject.focus();
  await editor.setHtml('');
  document.body.dataset.ready = '1';
  if (document.activeElement !== subject) subject.focus();
}

init().catch(reportError);
