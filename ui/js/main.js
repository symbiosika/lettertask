// Main window shell: top bar with search, action bar, sidebar, events, shortcuts.
import { call, on, onBusy } from './api.js';
import { h, $, load, store, fmtTime, esc, initials, avatarColor, toLocalIso, addDays, startOfDay } from './util.js';
import { icon } from './icons.js';
import { showMenu, showMenuAt, reportError, toast, busyIndicator, modal } from './dialogs.js';
import { state, mailboxes, loadConfig, loadAccounts, loadFolders, loadCategories, wellKnown, folderById } from './state.js';
import { tbBtn, splitBtn, sep } from './toolbar.js';
import * as mail from './mail.js';
import * as calendar from './calendar.js';
import { snoozeOptions, computeSnooze, fmtSnoozeTarget } from './snooze.js';
import { hooks, openSettings, openCategoryManager, openAutoReply, removeAccountDialog, addAccountFlow, welcomeDialog, aboutDialog } from './settings.js';

const els = {
  topbar: $('#topbar'), toolbar: $('#toolbar'), nav: $('#navpane'), navTitle: $('#nav-title'), tree: $('#folder-tree'),
  calNav: $('#cal-nav'), navButtons: $('#nav-buttons'), content: $('#content'), status: $('#statusbar'),
};

// ------------------------------------------------------------------ search options
const search = {
  opts: { scope: load('searchScope', 'folder'), field: load('searchField', 'all'), hasAttachments: false },
  recent: load('recentSearches', []),
};

function rememberSearch(text) {
  search.recent = [text, ...search.recent.filter((t) => t !== text)].slice(0, 5);
  store('recentSearches', search.recent);
}

// ------------------------------------------------------------------ top bar
function renderTopbar() {
  const busy = h('div.busy', { title: 'Wird ausgeführt…' });
  onBusy(busyIndicator(busy));
  const menuBtn = h('button.icon-btn.hamburger', { type: 'button', title: 'Menü', 'aria-label': 'Menü', dataset: { testid: 'app-menu' }, html: icon('menu', 20) });
  menuBtn.addEventListener('click', () => showMenuAt(menuBtn, appMenu()));

  const input = h('input#search-input', { type: 'search', placeholder: 'Suchen', 'aria-label': 'Suchen', autocomplete: 'off', spellcheck: false });
  const panel = h('div.search-panel.hidden', { dataset: { testid: 'search-panel' } });
  const wrap = h('div.search-wrap', h('span.search-ic', { html: icon('search', 16) }), input,
    h('kbd.search-kbd', 'Strg+E'), panel);

  const renderPanel = () => {
    panel.innerHTML = '';
    const chip = (label, on, fn, testid) => {
      const b = h('button.chip' + (on ? '.on' : ''), { type: 'button', dataset: { testid } }, label);
      b.addEventListener('mousedown', (e) => e.preventDefault());
      b.addEventListener('click', () => { fn(); renderPanel(); });
      return b;
    };
    const setScope = (v) => { search.opts.scope = v; store('searchScope', v); };
    const setField = (v) => { search.opts.field = v; store('searchField', v); };
    const att = h('input', { type: 'checkbox', checked: search.opts.hasAttachments, dataset: { testid: 'search-att' } });
    att.addEventListener('mousedown', (e) => e.preventDefault());
    att.addEventListener('change', () => { search.opts.hasAttachments = att.checked; });
    panel.append(
      h('div.sp-row', h('span.sp-lbl', 'Suchen in'), h('div.sp-chips',
        chip('Aktueller Ordner', search.opts.scope !== 'all', () => setScope('folder'), 'scope-folder'),
        chip('Alle Ordner', search.opts.scope === 'all', () => setScope('all'), 'scope-all'))),
      h('div.sp-row', h('span.sp-lbl', 'Suchen nach'), h('div.sp-chips',
        ...[['all', 'Alles'], ['from', 'Von'], ['to', 'An'], ['subject', 'Betreff'], ['body', 'Text']].map(([v, l]) => chip(l, search.opts.field === v, () => setField(v), 'field-' + v)))),
      h('div.sp-row', h('span.sp-lbl', ''), h('label.checkrow', att, 'Nur Nachrichten mit Anhang')),
    );
    if (search.recent.length) {
      const rec = h('div.sp-recent', h('div.sp-head', 'Zuletzt gesucht'));
      for (const t of search.recent) {
        const r = h('div.sp-item', { html: icon('clock', 15) + `<span>${esc(t)}</span>` });
        r.addEventListener('mousedown', (e) => e.preventDefault());
        r.addEventListener('click', () => { input.value = t; runSearch(); });
        rec.appendChild(r);
      }
      panel.appendChild(rec);
    }
    panel.appendChild(h('div.sp-foot', 'Eingabetaste: auf dem Server suchen · Tippen: aktuelle Liste filtern · Esc: zurücksetzen'));
  };
  const openPanel = () => { if (state.mode !== 'mail') return; renderPanel(); panel.classList.remove('hidden'); wrap.classList.add('open'); };
  const closePanel = () => { panel.classList.add('hidden'); wrap.classList.remove('open'); };
  const runSearch = () => {
    const t = input.value.trim();
    if (!t) { mail.clearSearch(); return; }
    rememberSearch(t);
    closePanel();
    mail.serverSearch(t, { ...search.opts }).catch(reportError);
  };
  input.addEventListener('focus', openPanel);
  input.addEventListener('mousedown', () => { if (document.activeElement === input) openPanel(); });
  input.addEventListener('blur', () => setTimeout(closePanel, 120));
  input.addEventListener('input', () => {
    if (state.mode !== 'mail') return;
    if (!input.value) mail.clearSearch(); else mail.setLocalFilter(input.value);
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { e.preventDefault(); runSearch(); }
    else if (e.key === 'Escape') { e.preventDefault(); input.value = ''; mail.clearSearch(); closePanel(); input.blur(); mail.focusList(); }
  });

  const right = h('div.tb-right', busy, h('div.sync-ind#sync-ind'), accountChip());
  els.topbar.append(h('div.tb-left', menuBtn, h('span.brand', { html: icon('mail', 18) + '<span>Mail</span>' })), wrap, right);
}

function accountChip() {
  const acc = state.accounts[0];
  const el = h('button.account-chip#account-chip', { type: 'button', title: acc ? acc.email : 'Kein Konto' });
  const paint = () => {
    const a = state.accounts[0];
    el.title = a ? `${a.displayName} <${a.email}>` : 'Kein Konto';
    el.innerHTML = a ? `<span class="avatar xs" style="background:${avatarColor(a.email)}">${esc(initials(a.displayName, a.email))}</span>` : icon('user', 18);
  };
  paint();
  el.addEventListener('click', () => showMenuAt(el, appMenu()));
  accountChip.paint = paint;
  return el;
}

function appMenu() {
  const acc = state.accounts[0] && state.accounts[0].id;
  const c = state.config || {};
  const setCfg = async (patch) => {
    state.config = await call('save_config', { config: { ...state.config, ...patch } });
    applyConfig();
  };
  return [
    { label: 'Neue E-Mail', icon: 'mail', shortcut: 'Strg+N', onClick: () => mail.newMail() },
    { label: 'Neuer Termin', icon: 'calendarNew', shortcut: 'Strg+Umschalt+A', onClick: () => calendar.newEvent() },
    { separator: true },
    { label: 'Ansicht', icon: 'workWeek', submenu: () => [
      { header: 'Lesebereich' },
      { label: 'Rechts', checked: c.readingPane === 'right', onClick: () => setCfg({ readingPane: 'right' }) },
      { label: 'Unten', checked: c.readingPane === 'bottom', onClick: () => setCfg({ readingPane: 'bottom' }) },
      { label: 'Aus', checked: c.readingPane === 'off', onClick: () => setCfg({ readingPane: 'off' }) },
      { header: 'Nachrichtenliste' },
      { label: 'Komfortabel (mit Vorschau)', checked: c.listMode !== 'compact', onClick: () => setCfg({ listMode: 'comfortable' }) },
      { label: 'Kompakt', checked: c.listMode === 'compact', onClick: () => setCfg({ listMode: 'compact' }) },
      { label: 'Datumstrenner anzeigen', checked: c.showDateSeparators !== false, onClick: () => setCfg({ showDateSeparators: c.showDateSeparators === false }) },
      { separator: true },
      { label: 'Spaltenbreiten zurücksetzen', onClick: () => { for (const k of ['listWidth2', 'listHeight', 'navWidth2']) store(k, null); location.reload(); } },
    ] },
    { separator: true },
    { label: 'Einstellungen…', icon: 'settings', onClick: () => openSettings(), testid: 'menu-settings' },
    { label: 'Kategorien verwalten…', icon: 'tag', disabled: !acc, onClick: () => openCategoryManager(acc), testid: 'menu-categories' },
    { label: 'Abwesenheitsnotiz…', icon: 'outOfOffice', disabled: !acc, onClick: () => openAutoReply(acc), testid: 'menu-autoreply' },
    { separator: true },
    { label: 'Konto hinzufügen…', icon: 'user', onClick: () => addAccountFlow(), testid: 'menu-add-account' },
    { label: 'Konto entfernen…', icon: 'signOut', disabled: !acc, onClick: () => removeAccountDialog() },
    { separator: true },
    { label: 'Über…', icon: 'info', onClick: () => aboutDialog() },
  ];
}

// ------------------------------------------------------------------ action bar
function snoozeItems() {
  const items = [];
  if (mail.inSnoozedFolder()) {
    items.push({ label: 'Wiedervorlage aufheben (jetzt in den Posteingang)', icon: 'inbox', onClick: () => mail.unsnoozeSelected(), testid: 'snooze-unsnooze' }, { separator: true });
  }
  const now = new Date();
  snoozeOptions(state.config).forEach((o, i) => {
    const d = computeSnooze(o.rule, now);
    if (!d) return;
    items.push({ label: o.label, shortcut: fmtSnoozeTarget(d), icon: i === 0 ? 'snooze' : null, onClick: () => mail.snoozeSelected(d), testid: 'snooze-opt-' + i });
  });
  items.push({ separator: true },
    { label: 'Datum und Uhrzeit wählen…', icon: 'calendar', onClick: () => customSnooze(), testid: 'snooze-custom' },
    { separator: true },
    { label: 'Optionen bearbeiten…', icon: 'settings', onClick: () => openSettings('snooze'), testid: 'snooze-edit' });
  return items;
}

async function customSnooze() {
  if (!mail.getSelection().ids.length) { toast('Bitte wählen Sie zuerst ein Element aus.'); return; }
  const def = addDays(startOfDay(new Date()), 1); def.setHours(8);
  const input = h('input.input', { type: 'datetime-local', value: toLocalIso(def).slice(0, 16), name: 'snoozeAt', style: { width: '100%' } });
  const res = await modal({
    title: 'Auf Wiedervorlage', width: 360,
    body: h('div', h('label.lbl', 'Wieder im Posteingang am:'), input),
    buttons: [{ label: 'Wiedervorlage', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: () => {
      const d = new Date(input.value);
      if (isNaN(d) || d <= new Date()) { input.classList.add('invalid'); toast('Bitte einen Zeitpunkt in der Zukunft wählen.', 'error'); return false; }
      return d;
    },
  });
  if (res instanceof Date) await mail.snoozeSelected(res);
}

const selDependent = [];
function renderToolbar() {
  const tb = els.toolbar;
  tb.innerHTML = '';
  selDependent.length = 0;
  const dep = (b) => { selDependent.push(b); return b; };
  const lead = h('div.ab-lead');
  if (state.mode === 'mail') {
    lead.appendChild(splitBtn('Neu', 'plus', () => mail.newMail(), (b) => showMenuAt(b, [
      { label: 'E-Mail', icon: 'mail', shortcut: 'Strg+N', onClick: () => mail.newMail(), testid: 'new-mail' },
      { label: 'Termin', icon: 'calendarNew', shortcut: 'Strg+Umschalt+A', onClick: () => calendar.newEvent(), testid: 'new-event' },
    ]), { testid: 'tb-new', menuTestid: 'tb-new-menu', primary: true, title: 'Neue E-Mail (Strg+N)' }));
    tb.append(lead,
      h('div.ab-group',
        dep(tbBtn('Antworten', 'reply', () => mail.respond('reply'), { testid: 'tb-reply', title: 'Antworten (Strg+R)' })),
        dep(tbBtn('Allen antworten', 'replyAll', () => mail.respond('replyAll'), { testid: 'tb-replyall', title: 'Allen antworten (Strg+Umschalt+R)' })),
        dep(tbBtn('Weiterleiten', 'forward', () => mail.respond('forward'), { testid: 'tb-forward', title: 'Weiterleiten (Strg+F)' }))),
      sep(),
      h('div.ab-group',
        dep(tbBtn('Archivieren', 'archive', () => mail.archiveSelected(), { testid: 'tb-archive' })),
        dep(tbBtn('Auf Wiedervorlage', 'snooze', (b) => showMenuAt(b, snoozeItems()), { caret: true, testid: 'tb-snooze' })),
        dep(tbBtn('Löschen', 'delete', () => mail.deleteSelected(false), { testid: 'tb-delete', title: 'Löschen (Entf)' }))),
      h('span.spacer'),
      h('div.ab-group',
        dep(tbBtn('', 'tag', (b) => mail.openTagsForSelection(b), { testid: 'tb-tag', title: 'Kategorien (T)', iconOnly: true })),
        dep(tbBtn('', 'flag', () => mail.toggleFlagSelected(), { testid: 'tb-flag', title: 'Kennzeichnen', iconOnly: true })),
        dep(tbBtn('', 'mailOpen', () => mail.toggleRead(), { testid: 'tb-read', title: 'Gelesen/Ungelesen (Strg+Q / Strg+U)', iconOnly: true })),
        tbBtn('', 'sync', () => syncAll(), { testid: 'tb-sync', title: 'Senden/Empfangen (F9)', iconOnly: true })),
    );
    updateActionState();
  } else {
    lead.appendChild(splitBtn('Neu', 'plus', () => calendar.newEvent(), (b) => showMenuAt(b, [
      { label: 'Termin', icon: 'calendarNew', onClick: () => calendar.newEvent(), testid: 'new-event' },
      { label: 'E-Mail', icon: 'mail', onClick: () => mail.newMail(), testid: 'new-mail' },
    ]), { testid: 'tb-new-event', menuTestid: 'tb-new-menu', primary: true, title: 'Neuer Termin' }));
    const view = calendar.getView();
    const seg = h('div.segmented', { role: 'tablist' });
    for (const [v, label] of [['day', 'Tag'], ['workweek', 'Arbeitswoche'], ['week', 'Woche'], ['month', 'Monat']]) {
      const b = h('button.seg' + (view === v ? '.on' : ''), { type: 'button', role: 'tab', 'aria-selected': view === v ? 'true' : 'false', dataset: { testid: 'tb-view-' + v } }, label);
      b.addEventListener('click', () => { calendar.setView(v); renderToolbar(); });
      seg.appendChild(b);
    }
    tb.append(lead,
      h('div.ab-group',
        tbBtn('Heute', 'today', () => calendar.goToday(), { testid: 'tb-today' }),
        tbBtn('', 'chevronLeft', () => calendar.step(-1), { title: 'Zurück', iconOnly: true, testid: 'tb-prev' }),
        tbBtn('', 'chevronRight', () => calendar.step(1), { title: 'Vor', iconOnly: true, testid: 'tb-next' }),
        h('span.ab-period#ab-period', calendar.periodTitle())),
      h('span.spacer'),
      seg,
      sep(),
      tbBtn('Löschen', 'delete', () => calendar.deleteSelected(), { testid: 'tb-delete-event' }),
      tbBtn('', 'sync', () => syncAll(), { testid: 'tb-sync', title: 'Aktualisieren (F9)', iconOnly: true }),
    );
  }
}

function updateActionState() {
  if (state.mode !== 'mail') {
    const p = $('#ab-period');
    if (p) p.textContent = calendar.periodTitle();
    return;
  }
  const n = mail.getSelection().ids.length;
  for (const b of selDependent) b.disabled = n === 0;
  for (const id of ['tb-reply', 'tb-replyall', 'tb-forward']) {
    const b = els.toolbar.querySelector(`[data-testid=${id}]`);
    if (b) b.disabled = n !== 1;
  }
}

// ------------------------------------------------------------------ nav / mode
function renderNavButtons() {
  els.navButtons.innerHTML = '';
  const mk = (mode, label, ic) => {
    const b = h('button.nav-btn' + (state.mode === mode ? '.active' : ''), { type: 'button', title: `${label} (Strg+${mode === 'mail' ? 1 : 2})`, 'aria-label': label, dataset: { testid: 'nav-' + mode } });
    b.innerHTML = icon(ic, 22);
    b.addEventListener('click', () => setMode(mode));
    return b;
  };
  els.navButtons.append(mk('mail', 'Mail', 'mail'), mk('calendar', 'Kalender', 'calendar'));
}

function setMode(mode) {
  state.mode = mode;
  store('mode', mode);
  document.body.dataset.mode = mode;
  els.navTitle.textContent = mode === 'mail' ? 'Mail' : 'Kalender';
  els.tree.classList.toggle('hidden', mode !== 'mail');
  els.calNav.classList.toggle('hidden', mode !== 'calendar');
  mail.showMailView(mode === 'mail');
  calendar.showCalendarView(mode === 'calendar');
  renderNavButtons();
  renderToolbar();
  mail.renderFolderTree();
  renderStatus();
}

// ------------------------------------------------------------------ status (sidebar footer)
function renderStatus() {
  const s = state.sync;
  const text = {
    syncing: 'Wird synchronisiert…', idle: s.lastSync ? `Aktualisiert ${fmtTime(new Date(s.lastSync))}` : 'Verbunden', offline: 'Offline', error: 'Synchronisierungsfehler',
  }[s.state] || '';
  els.status.innerHTML = '';
  els.status.append(
    h('span.st', { title: s.message || '' }, h('span.dot.' + (s.state || 'idle')), h('span.st-text', { dataset: { testid: 'sync-status' } }, state.accounts.length ? text : 'Kein Konto')),
    state.config && state.config.demo ? h('span.badge', { title: 'Es wird der eingebaute Demo-Server verwendet.' }, 'Demo') : null,
  );
  updateActionState();
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
  await Promise.all(mailboxes().filter((m) => !(state.folders.get(m.id) || []).length).map(async (m) => {
    try { state.folders.set(m.id, await call('sync_folders', { mailboxId: m.id }, { silent: true })); } catch (e) { console.warn(e); }
  }));
  if (accountChip.paint) accountChip.paint();
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
    if (document.querySelector('.modal-overlay') || document.querySelector('.tagpop')) return;
    const t = e.target;
    const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable);
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key.toLowerCase();
    if (e.key === 'F9') { e.preventDefault(); syncAll(); return; }
    if (ctrl && k === '1') { e.preventDefault(); setMode('mail'); return; }
    if (ctrl && k === '2') { e.preventDefault(); setMode('calendar'); return; }
    if (ctrl && e.shiftKey && k === 'a') { e.preventDefault(); calendar.newEvent(); return; }
    if (ctrl && k === 'n') { e.preventDefault(); if (state.mode === 'calendar') calendar.newEvent(); else mail.newMail(); return; }
    if (ctrl && k === 'e') { e.preventDefault(); $('#search-input').focus(); return; }
    if (typing || state.mode !== 'mail') {
      if (state.mode === 'calendar' && !typing && e.key === 'Delete') { e.preventDefault(); calendar.deleteSelected().catch(reportError); }
      return;
    }
    if (ctrl && e.shiftKey && k === 'r') { e.preventDefault(); mail.respond('replyAll'); }
    else if (ctrl && k === 'r') { e.preventDefault(); mail.respond('reply'); }
    else if (ctrl && k === 'f') { e.preventDefault(); mail.respond('forward'); }
    else if (ctrl && k === 'q') { e.preventDefault(); mail.markSelected(true).catch(reportError); }
    else if (ctrl && k === 'u') { e.preventDefault(); mail.markSelected(false).catch(reportError); }
    else if (!ctrl && !e.altKey && k === 't') { e.preventDefault(); mail.openTagsForSelection(els.toolbar.querySelector('[data-testid=tb-tag]')); }
    else if (e.key === 'Escape' && mail.isSearching()) { mail.clearSearch(); }
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
  renderTopbar();
  hooks.onConfigSaved = applyConfig;
  hooks.onAccountsChanged = () => onAccountsChanged().catch(reportError);
  mail.hooks.snoozeItems = snoozeItems;
  mail.initMail({ navHost: els.tree, contentHost: els.content, onCounts: renderStatus, onSelection: updateActionState });
  calendar.initCalendar({ navHost: els.calNav, contentHost: els.content, onCounts: updateActionState });
  const navW = load('navWidth2', null);
  if (navW) document.documentElement.style.setProperty('--nav-w', navW + 'px');
  setupNavSplitter();
  setupShortcuts();
  setupEvents();
  try {
    await loadConfig();
  } catch (e) {
    reportError(e);
    state.config = { clientId: '', tenant: 'organizations', syncIntervalSecs: 120, loadRemoteImages: false, signatureHtml: '', readingPane: 'right', markReadDelaySecs: 2, listMode: 'comfortable', showDateSeparators: true, snoozeOptions: [], demo: false };
  }
  mail.applyReadingPaneLayout();
  try { await loadAll(); } catch (e) { reportError(e); }
  setMode(load('mode', 'mail') === 'calendar' ? 'calendar' : 'mail');
  await selectInitialFolder().catch(reportError);
  renderStatus();
  if (!state.accounts.length) welcomeDialog().catch(reportError);
}

function setupNavSplitter() {
  const split = $('#nav-split');
  split.onmousedown = (e) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = els.nav.offsetWidth;
    document.body.classList.add('resizing');
    const move = (ev) => document.documentElement.style.setProperty('--nav-w', Math.min(420, Math.max(180, startW + ev.clientX - startX)) + 'px');
    const up = () => {
      document.removeEventListener('mousemove', move); document.removeEventListener('mouseup', up);
      document.body.classList.remove('resizing');
      store('navWidth2', els.nav.offsetWidth);
    };
    document.addEventListener('mousemove', move); document.addEventListener('mouseup', up);
  };
}

init().catch(reportError);
export { showMenu };
