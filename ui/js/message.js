// Standalone message window.
import { call, params, openWindow, closeWindow, isTauri, on } from './api.js';
import { h, $ } from './util.js';
import { modal, reportError, toast, showMenuAt, confirmDialog } from './dialogs.js';
import { renderMessage } from './mailview.js';
import { tbBtn, sep } from './toolbar.js';
import { state, bootstrapSecondary, accountOfMailbox } from './state.js';
import { broadcast } from './bus.js';

const P = params();
let msg = null;

async function load() {
  // A reply to a delegated task (same thread) offers "Aufgabe erledigt & archivieren".
  const delegationP = call('find_delegation', { mailboxId: P.mailboxId, messageId: P.messageId }, { silent: true }).catch(() => null);
  msg = await call('get_message', { mailboxId: P.mailboxId, messageId: P.messageId });
  const delegation = await delegationP;
  const assignee = delegation && (delegation.to || [])[0];
  const accountId = accountOfMailbox(P.mailboxId);
  renderMessage($('#msg'), msg, {
    accountId, config: state.config, onChanged: () => load().catch(reportError),
    // An answered invitation is done: archive it and close the window.
    onResponded: async () => {
      try { await call('archive_messages', { mailboxId: P.mailboxId, ids: [msg.id] }); } catch (e) { console.warn(e); }
      await closeWindow();
    },
    delegation: delegation ? { name: (assignee && (assignee.name || assignee.address)) || '', subject: delegation.subject } : null,
    onDelegationDone: async () => {
      await call('archive_messages', { mailboxId: P.mailboxId, ids: [msg.id, delegation.id] });
      broadcast({ type: 'delegation-done', subject: delegation.subject });
      await closeWindow();
    },
    setCategories: (cats) => call('set_categories', { mailboxId: P.mailboxId, ids: [msg.id], categories: cats }, { silent: true }).then(() => { msg.categories = cats; }),
    onManageCategories: () => import('./settings.js').then((m) => m.openCategoryManager(accountId)),
  });
  document.title = msg.subject || '(Kein Betreff)';
  if (isTauri && window.__TAURI__.window) { try { window.__TAURI__.window.getCurrentWindow().setTitle(document.title); } catch { /* ignore */ } }
  if (!msg.isRead) call('set_read', { mailboxId: P.mailboxId, ids: [msg.id], isRead: true }, { silent: true }).catch(() => {});
}

function respond(mode) {
  const p = mode === 'delegate' ? { mailboxId: P.mailboxId, mode: 'forward', delegate: '1', sourceMessageId: P.messageId } : { mailboxId: P.mailboxId, mode, sourceMessageId: P.messageId };
  openWindow('compose', p).catch(reportError);
}

async function setCats(cats) {
  await call('set_categories', { mailboxId: P.mailboxId, ids: [msg.id], categories: cats });
  await load();
}

function renderToolbar() {
  const tb = $('#toolbar');
  tb.append(
    tbBtn('Antworten', 'reply', () => respond('reply'), { testid: 'msg-reply' }),
    tbBtn('Allen antworten', 'replyAll', () => respond('replyAll')),
    tbBtn('Weiterleiten', 'forward', () => respond('forward')),
    tbBtn('Delegieren', 'people', () => respond('delegate'), { testid: 'msg-delegate' }),
    sep(),
    tbBtn('Archivieren', 'archive', async () => {
      await call('archive_messages', { mailboxId: P.mailboxId, ids: [msg.id] });
      await closeWindow();
    }),
    tbBtn('Löschen', 'delete', async () => {
      await call('delete_messages', { mailboxId: P.mailboxId, ids: [msg.id], permanent: false });
      await closeWindow();
    }),
    h('span.spacer'),
    tbBtn('', 'flag', (b) => showMenuAt(b, [
      { label: 'Kennzeichnen', icon: 'flag', onClick: () => setFlag('flagged') },
      { label: 'Als erledigt markieren', icon: 'check', onClick: () => setFlag('complete') },
      { label: 'Kennzeichnung entfernen', onClick: () => setFlag('notFlagged') },
    ]), { title: 'Kennzeichnung', iconOnly: true }),
    tbBtn('', 'mail', async () => {
      await call('set_read', { mailboxId: P.mailboxId, ids: [msg.id], isRead: false });
      toast('Als ungelesen markiert.');
    }, { title: 'Als ungelesen markieren', iconOnly: true }),
    tbBtn('', 'print', () => {
      const f = document.querySelector('.mail-frame');
      if (f && f.contentWindow) f.contentWindow.print(); else window.print();
    }, { title: 'Drucken', iconOnly: true }),
    tbBtn('', 'source', async () => {
      const src = await call('get_message_source', { mailboxId: P.mailboxId, messageId: msg.id });
      const ta = h('textarea.input.selectable.source', { readOnly: true });
      ta.value = src || '';
      await modal({ title: 'Nachrichtenquelle', body: ta, width: 820, buttons: [{ label: 'Schließen', value: true, primary: true }] });
    }, { title: 'Quelltext anzeigen', iconOnly: true }),
  );
}

async function setFlag(flag) {
  await call('set_flag', { mailboxId: P.mailboxId, ids: [msg.id], flag });
  await load();
}

async function init() {
  renderToolbar();
  try { await bootstrapSecondary(); } catch (e) { reportError(e); }
  $('#msg').appendChild(h('div.loading-state', h('span.busy'), h('div', 'Nachricht wird geladen…')));
  try { await load(); } catch (e) { reportError(e); $('#msg').innerHTML = ''; $('#msg').appendChild(h('div.empty', 'Die Nachricht konnte nicht geladen werden.')); }
  on('categories-changed', () => load().catch(() => {}));
  window.addEventListener('keydown', (e) => {
    if (document.querySelector('.modal-overlay')) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key.toLowerCase();
    if (e.key === 'Escape' && !document.querySelector('.tagpop')) closeWindow();
    else if (!ctrl && k === 't' && !(e.target && (e.target.tagName === 'INPUT' || e.target.isContentEditable)) && $('#msg')._openTagPicker) { e.preventDefault(); $('#msg')._openTagPicker(); }
    else if (ctrl && e.shiftKey && k === 'r') { e.preventDefault(); respond('replyAll'); }
    else if (ctrl && k === 'r') { e.preventDefault(); respond('reply'); }
    else if (ctrl && k === 'f') { e.preventDefault(); respond('forward'); }
  });
}

init().catch(reportError);
export { confirmDialog };
