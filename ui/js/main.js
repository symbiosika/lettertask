// Main window shell: menu bar, toolbar, navigation pane, status bar, events, shortcuts.
import { call, on, onBusy } from './api.js';
import { h, $, load, store, fmtTime, esc } from './util.js';
import { icon } from './icons.js';
import { showMenu, showMenuAt, reportError, toast, busyIndicator } from './dialogs.js';
import { state, mailboxes, loadConfig, loadAccounts, loadFolders, loadCategories, wellKnown, folderById } from './state.js';
import { tbBtn, group, stack } from './toolbar.js';
import * as mail from './mail.js';
import * as calendar from './calendar.js';
import { hooks, openSettings, openCategoryManager, openAutoReply, addSharedMailbox, removeAccountDialog, addAccountFlow, welcomeDialog, aboutDialog } from './settings.js';

const els = {
  menubar: $('#menubar'), toolbar: $('#toolbar'), nav: $('#navpane'), navTitle: $('#nav-title'), tree: $('#folder-tree'),
  calNav: $('#cal-nav'), navButtons: $('#nav-buttons'), content: $('#content'), status: $('#statusbar'),
};

// ------------------------------------------------------------------ menu bar
function renderMenubar() {
  const busy = h('div.busy', { title: 'Wird ausgeführt…' });
  onBusy(busyIndicator(busy));
  const item = (label, items, cls = '') => {
    const el = h('div.mb-item' + cls, { role: 'menuitem', tabindex: '0' }, label);
    el.addEventListener('click', () => showMenuAt(el, items()));
    return el;
  };
  els.menubar.append(
    item('Datei', fileMenu, '.file'),
    item('Start', () => [
      { label: 'Neue E-Mail', icon: 'mailNew', shortcut: 'Strg+N', onClick: () => mail.newMail() },
      { label: 'Neuer Termin', icon: 'calendarNew', shortcut: 'Strg+Umschalt+A', onClick: () => calendar.newEvent() },
      { separator: true },
      { label: 'Senden/Empfangen', icon: 'sync', shortcut: 'F9', onClick: () => syncAll() },
    ]),
    item('Ansicht', viewMenu),
    item('Hilfe', () => [
      { label: 'Über Outlook Classic…', icon: 'info', onClick: () => aboutDialog() },
    ]),
    h('span.mb-title', { id: 'mb-title' }),
    busy,
  );
}

function fileMenu() {
  const acc = state.accounts[0] && state.accounts[0].id;
  return [
    { header: 'Kontoinformationen' },
    { label: 'Konto hinzufügen…', icon: 'user', onClick: () => addAccountFlow(), testid: 'menu-add-account' },
    { label: 'Freigegebenes Postfach hinzufügen…', icon: 'shared', onClick: () => addSharedMailbox(), disabled: !acc },
    { label: 'Konto entfernen…', icon: 'delete', onClick: () => removeAccountDialog(), disabled: !acc },
    { separator: true },
    { label: 'Automatische Antworten (Abwesenheit)…', icon: 'outOfOffice', disabled: !acc, onClick: () => pickAccount((id) => openAutoReply(id)) },
    { label: 'Kategorien verwalten…', icon: 'tag', disabled: !acc, onClick: () => pickAccount((id) => openCategoryManager(id)), testid: 'menu-categories' },
    { separator: true },
    { label: 'Optionen', icon: 'settings', onClick: () => openSettings(), testid: 'menu-settings' },
  ];
}

function pickAccount(fn) {
  if (state.accounts.length <= 1) return fn(state.accounts[0] && state.accounts[0].id);
  const r = $('.mb-item.file').getBoundingClientRect();
  showMenu(state.accounts.map((a) => ({ label: a.email, icon: 'user', onClick: () => fn(a.id) })), r.left + 200, r.bottom);
}

function viewMenu() {
  const c = state.config || {};
  const setCfg = async (patch) => {
    state.config = await call('save_config', { config: { ...state.config, ...patch } });
    applyConfig();
  };
  return [
    { label: 'E-Mail', icon: 'mail', shortcut: 'Strg+1', checked: state.mode === 'mail', onClick: () => setMode('mail') },
    { label: 'Kalender', icon: 'calendar', shortcut: 'Strg+2', checked: state.mode === 'calendar', onClick: () => setMode('calendar') },
    { separator: true },
    { label: 'Lesebereich', icon: 'dayView', submenu: () => [
      { label: 'Rechts', checked: c.readingPane === 'right', onClick: () => setCfg({ readingPane: 'right' }) },
      { label: 'Unten', checked: c.readingPane === 'bottom', onClick: () => setCfg({ readingPane: 'bottom' }) },
      { label: 'Aus', checked: c.readingPane === 'off', onClick: () => setCfg({ readingPane: 'off' }) },
    ] },
    { label: 'Nachrichtenliste', icon: 'workWeek', submenu: () => [
      { label: 'Spaltenansicht (klassisch)', checked: c.listMode !== 'compact', onClick: () => setCfg({ listMode: 'columns' }) },
      { label: 'Kompakt, zweizeilig', checked: c.listMode === 'compact', onClick: () => setCfg({ listMode: 'compact' }) },
    ] },
    { label: 'Ansicht zurücksetzen', onClick: () => { for (const k of ['colWidths', 'listWidth', 'listHeight', 'navWidth']) store(k, null); location.reload(); } },
  ];
}

// ------------------------------------------------------------------ toolbar
function renderToolbar() {
  const tb = els.toolbar;
  tb.innerHTML = '';
  if (state.mode === 'mail') {
    const newBtn = tbBtn('Neue E-Mail', 'mailNew', () => mail.newMail(), { testid: 'tb-new' });
    const newDrop = tbBtn('', 'chevronDown', (b) => showMenuAt(b, [
      { label: 'E-Mail-Nachricht', icon: 'mailNew', onClick: () => mail.newMail() },
      { label: 'Termin', icon: 'calendarNew', onClick: () => calendar.newEvent() },
    ]), { title: 'Neue Elemente', small: true });
    newDrop.classList.add('tb-split');
    tb.append(
      group(newBtn, newDrop),
      group(tbBtn('Löschen', 'delete', () => mail.deleteSelected(false), { testid: 'tb-delete' }), tbBtn('Archivieren', 'archive', () => mail.archiveSelected(), { testid: 'tb-archive' })),
      group(
        tbBtn('Antworten', 'reply', () => mail.respond('reply'), { testid: 'tb-reply' }),
        tbBtn('Allen antworten', 'replyAll', () => mail.respond('replyAll'), { testid: 'tb-replyall' }),
        tbBtn('Weiterleiten', 'forward', () => mail.respond('forward'), { testid: 'tb-forward' }),
      ),
      group(
        tbBtn('Verschieben', 'move', (b) => showMenuAt(b, mail.moveMenuItems('move')), { caret: true, testid: 'tb-move' }),
        tbBtn('Kategorisieren', 'tag', (b) => showMenuAt(b, mail.categoryMenuItems()), { caret: true, testid: 'tb-categorize' }),
        tbBtn('Nachverfolgung', 'flag', (b) => showMenuAt(b, mail.flagMenuItems()), { caret: true, testid: 'tb-flag', title: 'Zur Nachverfolgung' }),
        tbBtn('Gelesen/Ungelesen', 'readUnread', () => mail.toggleRead(), { testid: 'tb-read' }),
      ),
      group(tbBtn('Senden/Empfangen', 'sync', () => syncAll(), { testid: 'tb-sync', title: 'Alle Ordner senden/empfangen (F9)' })),
      searchBox(),
    );
  } else {
    const view = calendar.getView();
    const vb = (label, ic, v) => { const b = tbBtn(label, ic, () => { calendar.setView(v); renderToolbar(); }, { testid: 'tb-view-' + v }); if (view === v) b.classList.add('active'); return b; };
    tb.append(
      group(tbBtn('Neuer Termin', 'calendarNew', () => calendar.newEvent(), { testid: 'tb-new-event' }), tbBtn('Neue E-Mail', 'mailNew', () => mail.newMail())),
      group(tbBtn('Heute', 'today', () => calendar.goToday(), { testid: 'tb-today' }),
        stack(tbBtn('Zurück', 'prev', () => calendar.step(-1), { small: true }), tbBtn('Vor', 'next', () => calendar.step(1), { small: true }))),
      group(vb('Tag', 'dayView', 'day'), vb('Arbeitswoche', 'workWeek', 'workweek'), vb('Woche', 'weekView', 'week'), vb('Monat', 'monthView', 'month')),
      group(tbBtn('Löschen', 'delete', () => calendar.deleteSelected(), { testid: 'tb-delete-event' })),
      group(tbBtn('Senden/Empfangen', 'sync', () => syncAll())),
    );
  }
}

function searchBox() {
  const input = h('input#search-input', { type: 'search', placeholder: 'Aktuellen Ordner durchsuchen', 'aria-label': 'Suchen' });
  input.addEventListener('input', () => { if (!input.value) mail.clearSearch(); else mail.setLocalFilter(input.value); });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { e.preventDefault(); mail.serverSearch(input.value).catch(reportError); }
    else if (e.key === 'Escape') { input.value = ''; mail.clearSearch(); mail.focusList(); }
  });
  const btn = h('button', { type: 'button', title: 'Suchen (Server)', html: icon('search', 14), onclick: () => mail.serverSearch(input.value).catch(reportError) });
  return h('div.tb-search', h('div.searchbox', input, btn));
}

// ------------------------------------------------------------------ nav buttons / mode
function renderNavButtons() {
  els.navButtons.innerHTML = '';
  const mk = (mode, label, ic) => {
    const b = h('div.nav-btn' + (state.mode === mode ? '.active' : ''), { role: 'button', tabindex: '0', dataset: { testid: 'nav-' + mode } });
    b.innerHTML = icon(ic, 22) + `<span>${label}</span>`;
    b.addEventListener('click', () => setMode(mode));
    return b;
  };
  els.navButtons.append(mk('mail', 'E-Mail', 'mail'), mk('calendar', 'Kalender', 'calendar'));
}

function setMode(mode) {
  state.mode = mode;
  store('mode', mode);
  els.navTitle.textContent = mode === 'mail' ? 'E-Mail' : 'Kalender';
  els.tree.classList.toggle('hidden', mode !== 'mail');
  els.calNav.classList.toggle('hidden', mode !== 'calendar');
  mail.showMailView(mode === 'mail');
  calendar.showCalendarView(mode === 'calendar');
  renderNavButtons();
  renderToolbar();
  mail.renderFolderTree();
  renderStatus();
}

// ------------------------------------------------------------------ status bar
function renderStatus() {
  const s = state.sync;
  const parts = [];
  if (state.mode === 'mail') {
    const st = mail.listStats();
    parts.push(h('span', { dataset: { testid: 'status-count' } }, `Elemente: ${st.total}`));
    if (st.unread) parts.push(h('span', `Ungelesen: ${st.unread}`));
  } else {
    parts.push(h('span', `Termine: ${calendar.stats().count}`));
  }
  const statusText = {
    syncing: 'Ordner werden synchronisiert…', idle: 'Alle Ordner sind auf dem neuesten Stand.', offline: 'Offline – Verbindung getrennt', error: 'Fehler bei der Synchronisierung',
  }[s.state] || '';
  els.status.innerHTML = '';
  els.status.append(...[...parts.flatMap((p, i) => (i ? [h('span.sb-sep'), p] : [p])),
    h('span.spacer'),
    h('span.sb-status', { title: s.message || '' }, h('span.dot.' + (s.state || 'idle')), statusText + (s.state === 'error' && s.message ? ': ' + s.message : '')),
    s.lastSync ? h('span', `Letzte Aktualisierung: ${fmtTime(new Date(s.lastSync))}`) : null,
    h('span.sb-sep'),
    state.config && state.config.demo ? h('span.badge', { title: 'Es wird der eingebaute Demo-Server verwendet.' }, 'Demo-Modus') : h('span', state.accounts.length ? 'Verbunden mit: Microsoft Exchange (Graph)' : 'Kein Konto'),
  ].filter(Boolean));
}

// ------------------------------------------------------------------ sync
async function syncAll() {
  state.sync = { ...state.sync, state: 'syncing' };
  renderStatus();
  try {
    await call('sync_all', {});
    await Promise.all(mailboxes().map((m) => loadFolders(m.id).catch(() => {})));
    mail.renderFolderTree();
    await mail.reloadList();
    if (state.mode === 'calendar') await calendar.refresh();
  } catch (e) { reportError(e); }
}

// ------------------------------------------------------------------ data bootstrap
async function loadAll() {
  await loadAccounts();
  await Promise.all([
    ...mailboxes().map((m) => loadFolders(m.id).catch((e) => { console.warn(e); return []; })),
    ...state.accounts.map((a) => loadCategories(a.id).catch(() => [])),
  ]);
  // Fill missing folder lists from the server.
  await Promise.all(mailboxes().filter((m) => !(state.folders.get(m.id) || []).length).map(async (m) => {
    try { state.folders.set(m.id, await call('sync_folders', { mailboxId: m.id }, { silent: true })); } catch (e) { console.warn(e); }
  }));
  $('#mb-title').textContent = state.accounts.length ? state.accounts.map((a) => a.email).join(', ') : '';
}

async function selectInitialFolder() {
  const last = load('lastFolder', null);
  if (last && folderById(last.mailboxId, last.folderId)) return mail.selectFolder(last.mailboxId, last.folderId);
  const mb = mailboxes()[0];
  if (!mb) { mail.renderFolderTree(); await mail.reloadList(); return; }
  const inbox = wellKnown(mb.id, 'inbox') || (state.folders.get(mb.id) || [])[0];
  if (inbox) await mail.selectFolder(mb.id, inbox.id);
}

function applyConfig() {
  mail.applyReadingPaneLayout();
  mail.reloadList().catch(reportError);
  renderStatus();
}

async function onAccountsChanged() {
  await loadAll();
  mail.renderFolderTree();
  await calendar.onAccountsChanged();
  if (!state.current.mailboxId || !mailboxes().some((m) => m.id === state.current.mailboxId)) {
    state.current = { mailboxId: null, folderId: null };
    await selectInitialFolder();
  }
  renderStatus();
}

// ------------------------------------------------------------------ keyboard
function setupShortcuts() {
  window.addEventListener('keydown', (e) => {
    if (document.querySelector('.modal-overlay')) return;
    const t = e.target;
    const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable);
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key.toLowerCase();
    if (e.key === 'F9') { e.preventDefault(); syncAll(); return; }
    if (ctrl && k === '1') { e.preventDefault(); setMode('mail'); return; }
    if (ctrl && k === '2') { e.preventDefault(); setMode('calendar'); return; }
    if (ctrl && e.shiftKey && k === 'a') { e.preventDefault(); calendar.newEvent(); return; }
    if (ctrl && k === 'n') { e.preventDefault(); if (state.mode === 'calendar') calendar.newEvent(); else mail.newMail(); return; }
    if (ctrl && k === 'e') { e.preventDefault(); $('#search-input') && $('#search-input').focus(); return; }
    if (typing || state.mode !== 'mail') {
      if (state.mode === 'calendar' && !typing && e.key === 'Delete') { e.preventDefault(); calendar.deleteSelected().catch(reportError); }
      return;
    }
    if (ctrl && e.shiftKey && k === 'r') { e.preventDefault(); mail.respond('replyAll'); }
    else if (ctrl && k === 'r') { e.preventDefault(); mail.respond('reply'); }
    else if (ctrl && k === 'f') { e.preventDefault(); mail.respond('forward'); }
    else if (ctrl && k === 'q') { e.preventDefault(); mail.markSelected(true).catch(reportError); }
    else if (ctrl && k === 'u') { e.preventDefault(); mail.markSelected(false).catch(reportError); }
    else if (e.key === 'Delete' && t === document.body) { e.preventDefault(); mail.deleteSelected(e.shiftKey).catch(reportError); }
  });
}

// ------------------------------------------------------------------ events
function setupEvents() {
  on('sync-status', (p) => { state.sync = { ...state.sync, ...p }; renderStatus(); });
  on('mail-changed', (p) => mail.onMailChanged(p).catch((e) => console.warn(e)));
  on('calendar-changed', () => calendar.onCalendarChanged().catch((e) => console.warn(e)));
  on('accounts-changed', () => onAccountsChanged().catch(reportError));
  on('categories-changed', async (p) => {
    if (p && p.accountId) await loadCategories(p.accountId).catch(() => {});
    mail.reloadList().catch(() => {});
    calendar.onCalendarChanged().catch(() => {});
  });
  on('new-mail', (p) => {
    if (!p) return;
    toast(p.count > 1 ? `${p.count} neue Nachrichten` : `Neue Nachricht von ${p.from}: ${p.subject}`);
  });
}

// ------------------------------------------------------------------ init
async function init() {
  renderMenubar();
  hooks.onConfigSaved = applyConfig;
  hooks.onAccountsChanged = () => onAccountsChanged().catch(reportError);
  mail.initMail({ navHost: els.tree, contentHost: els.content, onCounts: renderStatus });
  calendar.initCalendar({ navHost: els.calNav, contentHost: els.content, onCounts: renderStatus });
  const navW = load('navWidth', null);
  if (navW) els.nav.style.width = navW + 'px';
  mail.makeSplitter($('#nav-split'), els.nav, 'x', 'navWidth');
  setupShortcuts();
  setupEvents();
  try {
    await loadConfig();
  } catch (e) {
    reportError(e);
    state.config = { clientId: '', tenant: 'organizations', syncIntervalSecs: 120, loadRemoteImages: false, signatureHtml: '', readingPane: 'right', markReadDelaySecs: 0, listMode: 'columns', demo: false };
  }
  mail.applyReadingPaneLayout();
  try { await loadAll(); } catch (e) { reportError(e); }
  setMode(load('mode', 'mail') === 'calendar' ? 'calendar' : 'mail');
  await selectInitialFolder().catch(reportError);
  renderStatus();
  if (!state.accounts.length) welcomeDialog().catch(reportError);
}

init().catch(reportError);
