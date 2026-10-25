// "Aufgabe (Mail)": a mail from yourself to yourself – only subject, notes and due time.
// Created directly on the server (create_task): "Sofort" → inbox, otherwise → Wiedervorlage.
// "Andere Person" (Delegation): the task goes out as a mail ("ToDo: …"); the copy for
// yourself carries the hidden marker "[D_<address>]" and is followed up like a task.
import { call, params, closeWindow, currentWindow } from './api.js';
import { h, $ } from './util.js';
import { icon } from './icons.js';
import { reportError, toast, confirmDialog, setPending } from './dialogs.js';
import { createEditor } from './editor.js';
import { state, bootstrapSecondary, mailboxes, mailboxById } from './state.js';
import { duePicker } from './due.js';
import { broadcast } from './bus.js';
import { RECURRENCE_OPTIONS } from './recurrence.js';
import { recipientField } from './recipients.js';

const P = params();
const st = { mailboxId: P.mailboxId || null, dirty: false, busy: false, closing: false };
let subject, editor, due, createBtn, repeat, assignee, todoPrefix;
let delegated = false;

function markDirty() { st.dirty = true; }

function render() {
  $('#head').append(h('span', { html: icon('task', 18) }), h('div.cw-title', 'Neue Aufgabe'), h('span.cw-flags'));

  const form = $('#form');

  subject = h('input.task-subject#subject', { type: 'text', name: 'subject', placeholder: 'Was ist zu tun?', 'aria-label': 'Betreff', autocomplete: 'off' });
  subject.addEventListener('input', () => { markDirty(); document.title = subject.value.trim() ? `Aufgabe: ${subject.value.trim()}` : 'Neue Aufgabe'; });
  due = duePicker({ config: state.config, allowNow: true, variant: 'chips', testid: 'task-due', onChange: markDirty });

  repeat = h('select.input.task-repeat', { name: 'recurrence', dataset: { testid: 'task-repeat' }, 'aria-label': 'Wiederholen' });
  for (const [k, label] of RECURRENCE_OPTIONS) repeat.appendChild(h('option', { value: k || '' }, label));
  repeat.addEventListener('change', () => { markDirty(); repeatHint.textContent = repeat.value ? 'Beim Archivieren wird die nächste Wiederholung automatisch auf Wiedervorlage gelegt.' : ''; });
  const repeatHint = h('span.task-hint');
  const repeatRow = h('div.task-row', h('span.task-lbl', { html: icon('repeat', 15) + '<span>Wiederholen</span>' }), repeat, repeatHint);

  // "Für: An mich | Andere Person" – a delegated task is sent to the assignee.
  assignee = recipientField({ mailboxId: () => st.mailboxId, placeholder: 'E-Mail-Adresse der zuständigen Person', name: 'assignee', onChange: markDirty });
  todoPrefix = h('input', { type: 'checkbox', checked: true, dataset: { testid: 'task-todo-prefix' } });
  todoPrefix.addEventListener('change', markDirty);
  const delegateRow = h('div.task-row.task-delegate.hidden', { dataset: { testid: 'task-delegate' } },
    h('span.task-lbl', { html: icon('send', 15) + '<span>An</span>' }), assignee.el,
    h('label.task-check', { title: 'Betreff der gesendeten Mail beginnt mit „ToDo: “' }, todoPrefix, h('span', '„ToDo: “ voranstellen')));
  const seg = h('div.segmented.remind-seg', { role: 'radiogroup', 'aria-label': 'Zuständig' });
  const mk = (val, label, testid) => {
    const b = h('button.seg', { type: 'button', role: 'radio', dataset: { testid } }, label);
    b.addEventListener('click', () => { delegated = val; markDirty(); paint(); if (val) assignee.el.querySelector('input').focus(); });
    seg.appendChild(b);
    return b;
  };
  const meBtn = mk(false, 'An mich', 'task-for-me');
  const otherBtn = mk(true, 'Andere Person', 'task-for-other');
  const paint = () => {
    meBtn.classList.toggle('on', !delegated); otherBtn.classList.toggle('on', delegated);
    meBtn.setAttribute('aria-checked', String(!delegated)); otherBtn.setAttribute('aria-checked', String(delegated));
    delegateRow.classList.toggle('hidden', !delegated);
    // The copy is a reminder for yourself; a recurring delegation would need a new mail each time.
    repeatRow.classList.toggle('hidden', delegated);
    $('#head .cw-title').textContent = delegated ? 'Neue Aufgabe delegieren' : 'Neue Aufgabe';
    createBtn.querySelector('span').textContent = delegated ? 'Aufgabe senden' : 'Aufgabe anlegen';
  };

  form.append(
    h('div.task-row', subject),
    h('div.task-row', h('span.task-lbl', { html: icon('user', 15) + '<span>Für</span>' }), seg),
    delegateRow,
    h('div.task-row.task-due', h('span.task-lbl', { html: icon('snooze', 15) + '<span>Fällig</span>' }), due.el),
    repeatRow,
  );

  editor = createEditor({ onInput: markDirty, minHeight: 180 });
  $('#editor').append(editor.toolbar, editor.area);

  createBtn = h('button.btn.primary', { type: 'button', dataset: { testid: 'task-create' }, title: 'Aufgabe anlegen (Strg+Eingabe)', html: icon('task', 16) + '<span>Aufgabe anlegen</span>' });
  createBtn.addEventListener('click', () => create().catch(reportError));
  const cancel = h('button.btn', { type: 'button', dataset: { testid: 'task-cancel' } }, 'Abbrechen');
  cancel.addEventListener('click', () => confirmClose().then((ok) => ok && closeWindow()));
  $('#foot').append(h('span.task-kbd', 'Strg+Eingabe: anlegen · Esc: abbrechen'), h('span.spacer'), cancel, createBtn);
  paint();
}

async function create() {
  if (st.busy) return;
  const text = subject.value.trim();
  if (!text) { toast('Bitte beschreiben Sie die Aufgabe im Betreff.', 'error'); subject.focus(); return; }
  const d = due.get();
  if (d.error) { toast(d.error, 'error'); return; }
  let delegate = null;
  if (delegated) {
    const bad = assignee.invalid();
    const list = assignee.get();
    if (bad.length) { toast(`Ungültige Adresse: ${bad[0]}`, 'error'); return; }
    if (list.length !== 1) { toast('Bitte genau eine zuständige Person angeben.', 'error'); assignee.el.querySelector('input').focus(); return; }
    delegate = list[0];
  }
  st.busy = true;
  createBtn.disabled = true;
  setPending(createBtn, true);
  try {
    await call('create_task', { mailboxId: st.mailboxId, subject: text, bodyHtml: editor.getHtml(), due: d.date ? d.date.toISOString() : null, recurrence: delegated ? null : repeat.value || null, delegate, todoPrefix: todoPrefix.checked });
    broadcast({ type: 'task-created', subject: text, due: d.date ? due.label() : null, delegate });
    st.dirty = false; st.closing = true;
    await closeWindow();
  } finally {
    st.busy = false;
    createBtn.disabled = false;
    setPending(createBtn, false);
  }
}

/** Nothing typed yet: closing loses nothing worth asking about. */
function isBlank() {
  return !subject.value.trim() && !editor.getHtml().replace(/<[^>]*>|&nbsp;/g, '').trim();
}

async function confirmClose() {
  if (!st.dirty || st.closing || isBlank()) return true;
  const ok = await confirmDialog('Die Aufgabe wurde noch nicht angelegt. Verwerfen?', { title: 'Aufgabe verwerfen', ok: 'Verwerfen', danger: true });
  if (ok) st.closing = true;
  return ok;
}

function setupClose() {
  const w = currentWindow();
  if (w) {
    if (w.onCloseRequested) {
      w.onCloseRequested(async (event) => {
        if (st.closing || !st.dirty) return;
        event.preventDefault();
        if (await confirmClose()) await (w.destroy ? w.destroy() : w.close());
      });
    }
  } else {
    window.__ltBeforeClose = confirmClose;
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
  // The editor starts empty; reloading it here would replace the document the
  // user may already be typing into.
  await editor.ready;
  document.body.dataset.ready = '1';
  if (document.activeElement !== subject) subject.focus();
}

init().catch(reportError);
