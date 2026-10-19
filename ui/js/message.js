// Standalone message window.
import { call, params, openWindow, closeWindow, isTauri, on } from './api.js';
import { h, $ } from './util.js';
import { modal, reportError, toast, showMenuAt, confirmDialog } from './dialogs.js';
import { renderMessage } from './mailview.js';
import { getCategories, presetHex, swatchHtml } from './categories.js';
import { tbBtn, group } from './toolbar.js';
import { state, bootstrapSecondary, accountOfMailbox } from './state.js';

const P = params();
let msg = null;

async function load() {
  msg = await call('get_message', { mailboxId: P.mailboxId, messageId: P.messageId });
  renderMessage($('#msg'), msg, { accountId: accountOfMailbox(P.mailboxId), config: state.config, onChanged: () => load().catch(reportError) });
  document.title = `${msg.subject || '(Kein Betreff)'} – Nachricht`;
  if (isTauri && window.__TAURI__.window) { try { window.__TAURI__.window.getCurrentWindow().setTitle(document.title); } catch { /* ignore */ } }
  if (!msg.isRead) call('set_read', { mailboxId: P.mailboxId, ids: [msg.id], isRead: true }, { silent: true }).catch(() => {});
}

function respond(mode) { openWindow('compose', { mailboxId: P.mailboxId, mode, sourceMessageId: P.messageId }).catch(reportError); }

async function setCats(cats) {
  await call('set_categories', { mailboxId: P.mailboxId, ids: [msg.id], categories: cats });
  await load();
}

function renderToolbar() {
  const tb = $('#toolbar');
  tb.append(
    group(
      tbBtn('Antworten', 'reply', () => respond('reply'), { testid: 'msg-reply' }),
      tbBtn('Allen antworten', 'replyAll', () => respond('replyAll')),
      tbBtn('Weiterleiten', 'forward', () => respond('forward')),
    ),
    group(
      tbBtn('Löschen', 'delete', async () => {
        await call('delete_messages', { mailboxId: P.mailboxId, ids: [msg.id], permanent: false });
        await closeWindow();
      }),
      tbBtn('Ungelesen', 'mailClosed', async () => {
        await call('set_read', { mailboxId: P.mailboxId, ids: [msg.id], isRead: false });
        toast('Als ungelesen markiert.');
      }, { title: 'Als ungelesen markieren' }),
    ),
    group(
      tbBtn('Kategorisieren', 'tag', (b) => {
        const acc = accountOfMailbox(P.mailboxId);
        const cur = msg.categories || [];
        showMenuAt(b, [
          ...getCategories(acc).map((c) => ({
            label: c.displayName, iconHtml: swatchHtml(presetHex(c.color), 14), checked: cur.includes(c.displayName),
            onClick: () => setCats(cur.includes(c.displayName) ? cur.filter((x) => x !== c.displayName) : [...cur, c.displayName]),
          })),
          { separator: true },
          { label: 'Alle Kategorien löschen', disabled: !cur.length, onClick: () => setCats([]) },
        ]);
      }, { caret: true }),
      tbBtn('Nachverfolgung', 'flag', (b) => showMenuAt(b, [
        { label: 'Zur Nachverfolgung kennzeichnen', icon: 'flag', onClick: () => setFlag('flagged') },
        { label: 'Als erledigt markieren', icon: 'check', onClick: () => setFlag('complete') },
        { label: 'Kennzeichnung löschen', icon: 'flagOutline', onClick: () => setFlag('notFlagged') },
      ]), { caret: true }),
    ),
    group(
      tbBtn('Drucken', 'print', () => {
        const f = document.querySelector('.mail-frame');
        if (f && f.contentWindow) f.contentWindow.print(); else window.print();
      }),
      tbBtn('Quelltext', 'source', async () => {
        const src = await call('get_message_source', { mailboxId: P.mailboxId, messageId: msg.id });
        const ta = h('textarea.input.selectable', { readOnly: true, style: { width: '100%', height: '60vh', fontFamily: 'Consolas, monospace', fontSize: '11px' } });
        ta.value = src || '';
        await modal({ title: 'Nachrichtenquelle', body: ta, width: 820, buttons: [{ label: 'Schließen', value: true, primary: true }] });
      }),
    ),
  );
}

async function setFlag(flag) {
  await call('set_flag', { mailboxId: P.mailboxId, ids: [msg.id], flag });
  await load();
}

async function init() {
  renderToolbar();
  try { await bootstrapSecondary(); } catch (e) { reportError(e); }
  try { await load(); } catch (e) { reportError(e); $('#msg').appendChild(h('div.empty', 'Die Nachricht konnte nicht geladen werden.')); }
  on('categories-changed', () => load().catch(() => {}));
  window.addEventListener('keydown', (e) => {
    if (document.querySelector('.modal-overlay')) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key.toLowerCase();
    if (e.key === 'Escape') closeWindow();
    else if (ctrl && e.shiftKey && k === 'r') { e.preventDefault(); respond('replyAll'); }
    else if (ctrl && k === 'r') { e.preventDefault(); respond('reply'); }
    else if (ctrl && k === 'f') { e.preventDefault(); respond('forward'); }
  });
}

init().catch(reportError);
export { confirmDialog };
