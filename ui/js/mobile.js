// Phone shell: app bar, drawer, bottom navigation, full-screen reader and
// sheets, swipe actions. Desktop never loads this (see main.js).
//
// Layers (drawer, reader, sheets, dialogs, menus) sit on the browser history,
// so the Android back button closes the topmost one.
import { on } from './api.js';
import { h, esc, load, store } from './util.js';
import { icon } from './icons.js';
import { reportError, showSheetMenu } from './dialogs.js';
import { state, mailboxes, wellKnown } from './state.js';
import { accountAvatar } from './accounts.js';
import * as mail from './mail.js';
import * as calendar from './calendar.js';
import * as contacts from './contacts.js';

// ------------------------------------------------------------------ layers
const layers = [];
history.replaceState({ ltDepth: 0 }, '');
// Our own history.back() calls still on their way. Chrome fixes the target of
// back() when it is called, so a layer opened meanwhile waits for it.
let backs = 0;
const waiting = [];

/**
 * Registers an open layer; `close` runs on back. Returns `pop(run = true)` that
 * closes it from code – `run = false` when the caller already closed it.
 */
function pushLayer(close) {
  const entry = { close };
  layers.push(entry);
  if (backs) waiting.push(entry); else history.pushState({ ltDepth: layers.length }, '');
  return (run = true) => {
    const i = layers.indexOf(entry);
    if (i < 0) return;
    layers.splice(i, 1);
    const w = waiting.indexOf(entry);
    if (w >= 0) waiting.splice(w, 1); else { backs++; history.back(); }
    if (run) close();
  };
}

window.addEventListener('popstate', (e) => {
  if (backs) {
    if (--backs === 0) {
      for (const entry of waiting.splice(0)) {
        if (layers.includes(entry)) history.pushState({ ltDepth: layers.indexOf(entry) + 1 }, '');
      }
    }
    return;
  }
  const depth = (e.state && e.state.ltDepth) || 0;
  while (layers.length > depth) {
    const l = layers.pop();
    try { l.close(); } catch (err) { console.error(err); }
  }
});

// ------------------------------------------------------------------ sheets (the former windows)
const TITLES = { compose: 'Neue Nachricht', message: 'Nachricht', event: 'Termin', task: 'Aufgabe' };
const frames = [];

function openSheet(kind, query) {
  const frame = h('iframe.m-frame-doc', { src: `${kind}.html${query ? '?' + query : ''}`, title: TITLES[kind] || '' });
  const wrap = h('div.m-frame', { dataset: { kind, testid: 'm-frame-' + kind } }, frame);
  document.body.appendChild(wrap);
  requestAnimationFrame(() => wrap.classList.add('in'));
  const entry = { wrap, frame, pop: null };
  const arm = () => { entry.pop = pushLayer(() => requestClose(entry)); };
  // Back button: the page may object (unsaved changes) – then the layer stays.
  const requestClose = async (en) => {
    let ok = true;
    try {
      const ask = en.frame.contentWindow && en.frame.contentWindow.__ltBeforeClose;
      if (ask) ok = await ask();
    } catch (e) { console.warn(e); }
    if (ok) remove(en); else arm();
  };
  arm();
  frames.push(entry);
}

function remove(entry) {
  const i = frames.indexOf(entry);
  if (i >= 0) frames.splice(i, 1);
  entry.wrap.classList.remove('in');
  setTimeout(() => entry.wrap.remove(), 220);
}

/** Called by a frame's closeWindow(). */
function closeSheet(win) {
  const entry = frames.find((f) => f.frame.contentWindow === win);
  if (!entry) return;
  if (entry.pop) entry.pop(false);
  remove(entry);
}

window.__lt = { pushLayer, openSheet, closeSheet };

// ------------------------------------------------------------------ shell
const ui = {};
let hooks = {};
const st = { reader: null, select: null, search: null, drawer: null, ctDetail: null, fabOpen: null };

/**
 * hooks: { appMenu(), snoozeItems(), focusMenu(), focusLabel(), setMode(mode), syncAll(), newTask() }
 */
export function initMobile(h0) {
  hooks = h0;
  document.body.classList.add('m-app');
  ui.appbar = h('header.m-appbar#m-appbar');
  ui.searchrow = h('div.m-searchrow.hidden');
  ui.readbar = h('div.m-readbar', { dataset: { testid: 'm-readbar' } });
  ui.readactions = h('div.m-readactions');
  ui.ctbar = h('div.m-ctbar');
  ui.bottomnav = h('nav.m-bottomnav', { 'aria-label': 'Bereiche' });
  ui.fab = h('div.m-fab-wrap');
  ui.scrim = h('div.m-scrim');
  ui.ptr = h('div.m-ptr', { html: icon('sync', 20) });
  document.body.prepend(ui.appbar, ui.searchrow);
  document.body.append(ui.readbar, ui.readactions, ui.ctbar, ui.bottomnav, ui.fab, ui.scrim);
  $('#msglist').before(ui.ptr);
  ui.scrim.addEventListener('click', () => { if (st.drawer) st.drawer(); if (st.fabOpen) st.fabOpen(); });

  setupDrawer();
  setupList();
  setupBoard();
  setupContacts();
  renderReadbar();
  // Work week and week are too narrow for a phone: start with the agenda once.
  if (!load('mCalViewSet', false)) {
    store('mCalViewSet', true);
    if (['workweek', 'week'].includes(calendar.getView())) calendar.setView('sprint');
  }
  on('open-window', (p) => { if (p && p.kind) openSheet(p.kind, p.query || ''); });
  update();
}

function $(sel) { return document.querySelector(sel); }

/** Repaints app bar, bottom navigation and FAB for the current state. */
export function update() {
  if (!ui.appbar) return;
  renderAppbar();
  renderBottomNav();
  if (ui.fabMode !== state.mode) renderFab();
}

const btn = (ic, label, onClick, testid, extra = '') => {
  const b = h('button.m-icon' + extra, { type: 'button', title: label, 'aria-label': label, dataset: testid ? { testid } : {}, html: icon(ic, 22) });
  b.addEventListener('click', (e) => { e.stopPropagation(); Promise.resolve().then(() => onClick(b)).catch(reportError); });
  return b;
};

// ------------------------------------------------------------------ app bar
function folderTitle() {
  if (mail.isToday()) return 'Heute';
  if (mail.isDelegated()) return 'Delegiert';
  if (mail.isAllInbox()) return 'Geteilter Posteingang';
  const f = mail.currentFolder();
  return f ? f.displayName : 'Mail';
}

function renderAppbar() {
  const bar = ui.appbar;
  // Typing in the search field or toggling rows must not rebuild the bar.
  if (state.mode === 'mail' && st.search && bar.classList.contains('searching')) return;
  if (state.mode === 'mail' && st.select && bar.classList.contains('contextual')) {
    bar.querySelector('[data-testid=m-select-count]').textContent = `${mail.getSelection().ids.length} ausgewählt`;
    return;
  }
  bar.innerHTML = '';
  bar.className = 'm-appbar';
  const append = bar.append.bind(bar);
  bar.append = (...nodes) => append(...nodes.filter(Boolean));
  if (state.mode === 'mail' && st.select) {
    const n = mail.getSelection().ids.length;
    bar.classList.add('contextual');
    bar.append(
      btn('close', 'Auswahl beenden', () => st.select(), 'm-select-close'),
      h('div.m-title', h('span.m-title-main', { dataset: { testid: 'm-select-count' } }, `${n} ausgewählt`)),
      btn('archive', 'Archivieren', () => mail.archiveSelected().then(endSelect), 'm-sel-archive'),
      btn('snooze', 'Auf Wiedervorlage', () => snoozeSheet(endSelect), 'm-sel-snooze'),
      btn('delete', 'Löschen', () => mail.deleteSelected(false).then(endSelect), 'm-sel-delete'),
      btn('more', 'Weitere Aktionen', () => showSheetMenu([
        ...mail.moreItems(false).filter((it) => it && !it.separator && !/Fenster|Quelltext/.test(it.label || '')),
        { label: 'Kategorien', icon: 'tag', submenu: () => mail.categoryMenuItems() },
        { label: 'Kanban-Lane', icon: 'kanban', submenu: () => mail.laneMenuItems() },
      ]), 'm-sel-more'));
    return;
  }
  if (state.mode === 'mail' && st.search) {
    bar.classList.add('searching');
    const input = h('input.m-search-input', { type: 'search', placeholder: 'In Mails suchen', enterkeyhint: 'search', autocomplete: 'off', spellcheck: false, dataset: { testid: 'm-search-input' } });
    input.value = st.searchText || '';
    input.addEventListener('input', () => {
      st.searchText = input.value;
      if (!input.value) mail.clearSearch(); else mail.setLocalFilter(input.value);
    });
    input.addEventListener('keydown', (e) => {
      if (e.key !== 'Enter') return;
      e.preventDefault();
      const t = input.value.trim();
      if (!t) return;
      input.blur();
      mail.serverSearch(t, { scope: st.searchScope || 'all', field: 'all', hasAttachments: false }).catch(reportError);
    });
    bar.append(btn('chevronLeft', 'Suche schließen', () => st.search(), 'm-search-close'), input);
    renderSearchRow();
    setTimeout(() => input.focus(), 30);
    return;
  }
  const menu = btn('menu', 'Menü', () => openDrawer(), 'm-menu');
  if (state.mode === 'mail') {
    const stats = mail.listStats();
    const kanban = mail.getView() === 'kanban';
    bar.append(menu,
      h('div.m-title', h('span.m-title-main', { dataset: { testid: 'm-title' } }, folderTitle()),
        h('span.m-title-sub', stats.unread ? `${stats.unread} ungelesen` : stats.total ? `${stats.total} Elemente` : '')),
      btn('search', 'Suchen', () => openSearch(), 'm-search'),
      mail.isVirtual() ? null : btn(kanban ? 'bullets' : 'kanban', kanban ? 'Als Liste' : 'Als Kanban', () => { mail.toggleView(); update(); }, 'm-view'),
      accountButton());
  } else if (state.mode === 'calendar') {
    bar.append(menu,
      btn('chevronLeft', 'Zurück', () => { calendar.step(-1); setTimeout(update, 50); }, 'm-cal-prev'),
      h('div.m-title.center', h('span.m-title-main', { dataset: { testid: 'm-cal-title' } }, calTitle())),
      btn('chevronRight', 'Vor', () => { calendar.step(1); setTimeout(update, 50); }, 'm-cal-next'),
      btn('today', 'Heute', () => { calendar.goToday(); setTimeout(update, 50); }, 'm-cal-today'),
      btn(viewIcon(calendar.getView()), 'Ansicht', () => showSheetMenu(calViewItems(), { title: 'Ansicht' }), 'm-cal-view'));
  } else {
    bar.append(menu,
      h('div.m-title', h('span.m-title-main', 'Kontakte')),
      btn('sync', 'Synchronisieren', () => hooks.syncAll(), 'm-ct-sync'),
      accountButton());
  }
}

function accountButton() {
  const a = state.accounts[0];
  const b = h('button.m-avatar', { type: 'button', title: a ? a.email : 'Konto', 'aria-label': 'Konto und Einstellungen', dataset: { testid: 'm-account' } });
  if (a) b.appendChild(accountAvatar(a, 'sm')); else b.innerHTML = icon('user', 22);
  b.addEventListener('click', () => showSheetMenu(hooks.appMenu().filter((it) => !it || !/^Ansicht$/.test(it.label || '')), { title: a ? a.email : 'lettertask' }));
  return b;
}

const MONTHS_SHORT = ['Jan.', 'Feb.', 'März', 'Apr.', 'Mai', 'Juni', 'Juli', 'Aug.', 'Sep.', 'Okt.', 'Nov.', 'Dez.'];
const WD_SHORT = ['So', 'Mo', 'Di', 'Mi', 'Do', 'Fr', 'Sa'];
/** Short period title for the narrow app bar. */
function calTitle() {
  const v = calendar.getView();
  const { start, end } = calendar.viewRange();
  const last = new Date(end.getTime() - 86400000);
  const dm = (d) => `${d.getDate()}. ${MONTHS_SHORT[d.getMonth()]}`;
  if (v === 'day') return `${WD_SHORT[start.getDay()]}, ${dm(start)}`;
  if (v === 'month') return calendar.periodTitle();
  if (start.getMonth() === last.getMonth()) return `${start.getDate()}.–${dm(last)}`;
  return `${dm(start)} – ${dm(last)}`;
}

const VIEWS = [['sprint', 'Agenda (14 Tage)', 'bullets'], ['day', 'Tag', 'dayView'], ['week', 'Woche', 'weekView'], ['month', 'Monat', 'monthView']];
function viewIcon(v) { return (VIEWS.find((x) => x[0] === v) || VIEWS[0])[2]; }
function calViewItems() {
  const cur = calendar.getView();
  return VIEWS.map(([v, label, ic]) => ({ label, icon: ic, checked: cur === v, onClick: () => { calendar.setView(v); setTimeout(update, 50); } }));
}

// ------------------------------------------------------------------ search
function openSearch() {
  st.searchText = '';
  st.searchScope = st.searchScope || 'all';
  const pop = pushLayer(() => closeSearch());
  st.search = () => pop();
  document.body.classList.add('m-searching');
  update();
}

function closeSearch() {
  st.search = null;
  st.searchText = '';
  document.body.classList.remove('m-searching');
  ui.searchrow.classList.add('hidden');
  mail.clearSearch();
  update();
}

function renderSearchRow() {
  const row = ui.searchrow;
  row.innerHTML = '';
  row.classList.remove('hidden');
  const chip = (v, label) => {
    const b = h('button.m-chip' + (st.searchScope === v ? '.on' : ''), { type: 'button' }, label);
    b.addEventListener('click', () => { st.searchScope = v; renderSearchRow(); });
    return b;
  };
  row.append(chip('all', 'Alle Ordner'), chip('folder', 'Dieser Ordner'), h('span.m-search-hint', 'Eingabe: auf dem Server suchen'));
}

// ------------------------------------------------------------------ drawer (the sidebar)
function setupDrawer() {
  const nav = $('#navpane');
  nav.classList.add('m-drawer');
  ui.drawerHead = h('div.m-drawer-head');
  ui.drawerMenu = h('div.m-drawer-menu');
  nav.prepend(ui.drawerHead);
  $('#statusbar').before(ui.drawerMenu);
  // Choosing a folder (or calendar) closes the drawer.
  nav.addEventListener('click', (e) => {
    const node = e.target.closest('.tree-node');
    if (!node || e.target.closest('.twisty')) return;
    if (st.drawer) setTimeout(() => st.drawer && st.drawer(), 120);
  });
}

function paintDrawerHead() {
  if (!ui.drawerHead) return;
  const a = state.accounts[0];
  ui.drawerHead.innerHTML = '';
  const focusBtn = h('button.m-drawer-focus', { type: 'button', dataset: { testid: 'm-focus' }, html: icon('focus', 18) + `<span>${esc(hooks.focusLabel ? hooks.focusLabel() : 'Fokuszeit')}</span>` });
  focusBtn.addEventListener('click', () => showSheetMenu(hooks.focusMenu(), { title: 'Fokuszeit' }));
  ui.drawerHead.append(
    h('div.m-drawer-brand', { html: icon('mail', 20) + '<span>lettertask</span>' }),
    a ? h('div.m-drawer-account', accountAvatar(a, 'lg'), h('div.m-drawer-who', h('div.m-drawer-name', a.displayName || a.email), h('div.m-drawer-mail', a.email))) : null,
    focusBtn);
  ui.drawerMenu.innerHTML = '';
  const items = hooks.appMenu().filter((it) => it && !it.separator && !it.header && /Einstellungen|Signaturen|Kategorien|Abwesenheit|Konto hinzufügen|Über/.test(it.label || ''));
  for (const it of items) {
    const b = h('button.m-drawer-item', { type: 'button', disabled: !!it.disabled, html: icon(it.icon || 'settings', 20) + `<span>${esc(it.label.replace(/…$/, ''))}</span>` });
    b.addEventListener('click', () => { if (st.drawer) st.drawer(); Promise.resolve().then(it.onClick).catch(reportError); });
    ui.drawerMenu.appendChild(b);
  }
}

function openDrawer() {
  if (st.drawer) return;
  paintDrawerHead();
  document.body.classList.add('m-drawer-open');
  const pop = pushLayer(() => closeDrawer());
  st.drawer = () => pop();
}

function closeDrawer() {
  st.drawer = null;
  document.body.classList.remove('m-drawer-open');
}

// ------------------------------------------------------------------ bottom navigation
function renderBottomNav() {
  const nav = ui.bottomnav;
  nav.innerHTML = '';
  const mb = mailboxes()[0];
  const inbox = mb && wellKnown(mb.id, 'inbox');
  const inboxUnread = inbox ? inbox.unreadItemCount || 0 : 0;
  const todayN = mail.todayCount ? mail.todayCount() : 0;
  const active = state.mode === 'mail' ? (mail.isToday() ? 'today' : 'mail') : state.mode;
  const tab = (key, label, ic, badge, onClick) => {
    const b = h('button.m-tab' + (active === key ? '.on' : ''), { type: 'button', dataset: { testid: 'm-tab-' + key } },
      h('span.m-tab-ic', { html: icon(ic, 22) }, badge ? h('span.m-badge', badge > 99 ? '99+' : String(badge)) : null),
      h('span.m-tab-label', label));
    b.addEventListener('click', () => Promise.resolve().then(onClick).then(update).catch(reportError));
    return b;
  };
  nav.append(
    tab('mail', 'Posteingang', 'inbox', inboxUnread, async () => {
      if (state.mode !== 'mail') hooks.setMode('mail');
      if (mb && inbox && (state.current.folderId !== inbox.id || state.current.mailboxId !== mb.id)) await mail.selectFolder(mb.id, inbox.id);
      else scrollListTop();
    }),
    tab('today', 'Heute', 'sun', todayN, async () => {
      if (state.mode !== 'mail') hooks.setMode('mail');
      if (mb && !mail.isToday()) await mail.selectFolder(mb.id, mail.TODAY);
      else scrollListTop();
    }),
    tab('calendar', 'Kalender', 'calendar', 0, () => { if (state.mode !== 'calendar') hooks.setMode('calendar'); else calendar.goToday(); }),
    tab('contacts', 'Kontakte', 'contacts', 0, () => { if (state.mode !== 'contacts') hooks.setMode('contacts'); }),
  );
}

function scrollListTop() {
  const l = $('#msglist');
  if (l) l.scrollTo({ top: 0, behavior: 'smooth' });
}

// ------------------------------------------------------------------ FAB
function renderFab() {
  ui.fabMode = state.mode;
  const wrap = ui.fab;
  wrap.innerHTML = '';
  closeFab();
  const main = (ic, label, onClick, testid) => {
    const b = h('button.m-fab', { type: 'button', 'aria-label': label, dataset: { testid }, html: icon(ic, 22) + `<span>${esc(label)}</span>` });
    b.addEventListener('click', () => Promise.resolve().then(onClick).catch(reportError));
    return b;
  };
  if (state.mode === 'calendar') { wrap.append(main('plus', 'Termin', () => calendar.newEvent(), 'm-fab')); return; }
  if (state.mode === 'contacts') { wrap.append(main('plus', 'Kontakt', () => contacts.newContact(), 'm-fab')); return; }
  const dial = h('div.m-dial');
  const opt = (ic, label, onClick, testid) => {
    const b = h('button.m-dial-opt', { type: 'button', dataset: { testid } }, h('span.m-dial-label', label), h('span.m-dial-ic', { html: icon(ic, 20) }));
    b.addEventListener('click', () => { closeFab(); Promise.resolve().then(onClick).catch(reportError); });
    return b;
  };
  dial.append(
    opt('calendarNew', 'Termin', () => calendar.newEvent(), 'm-new-event'),
    opt('task', 'Aufgabe', () => hooks.newTask(), 'm-new-task'),
    opt('mail', 'E-Mail', () => mail.newMail(), 'm-new-mail'));
  const fab = main('edit', 'Schreiben', () => {
    if (wrap.classList.contains('open')) { closeFab(); mail.newMail(); return; }
    wrap.classList.add('open');
    ui.scrim.classList.add('fab');
    const pop = pushLayer(() => closeFabNow());
    st.fabOpen = () => pop();
  }, 'm-fab');
  wrap.append(dial, fab);
}

function closeFab() { if (st.fabOpen) st.fabOpen(); }
function closeFabNow() {
  st.fabOpen = null;
  ui.fab.classList.remove('open');
  ui.scrim.classList.remove('fab');
}

// ------------------------------------------------------------------ reader
function renderReadbar() {
  ui.readbar.innerHTML = '';
  ui.readbar.append(
    btn('chevronLeft', 'Zurück', () => closeReader(), 'm-read-back'),
    h('span.spacer'),
    btn('archive', 'Archivieren', () => mail.archiveSelected().then(closeReader), 'm-read-archive'),
    btn('snooze', 'Auf Wiedervorlage', () => snoozeSheet(closeReader), 'm-read-snooze'),
    btn('delete', 'Löschen', () => mail.deleteSelected(false).then(closeReader), 'm-read-delete'),
    btn('more', 'Weitere Aktionen', () => showSheetMenu([
      ...mail.moreItems(false).filter((it) => it && !/Fenster|Quelltext/.test(it.label || '')),
      { separator: true },
      { label: 'Kategorien', icon: 'tag', submenu: () => mail.categoryMenuItems() },
      { label: 'Kanban-Lane', icon: 'kanban', submenu: () => mail.laneMenuItems() },
    ]), 'm-read-more'));
  ui.readactions.innerHTML = '';
  const act = (ic, label, mode) => {
    const b = h('button.m-readact', { type: 'button', dataset: { testid: 'm-' + mode }, html: icon(ic, 20) + `<span>${label}</span>` });
    b.addEventListener('click', () => mail.respond(mode));
    return b;
  };
  ui.readactions.append(act('reply', 'Antworten', 'reply'), act('replyAll', 'Allen', 'replyAll'), act('forward', 'Weiterleiten', 'forward'));
}

function openReader() {
  if (st.reader) return;
  document.body.classList.add('m-reading');
  const pop = pushLayer(() => hideReader());
  st.reader = () => pop();
}

function hideReader() {
  st.reader = null;
  document.body.classList.remove('m-reading');
}

function closeReader() { if (st.reader) st.reader(); }

/** Wiedervorlage options as a sheet; `after` runs once a time was picked. */
function snoozeSheet(after) {
  const items = hooks.snoozeItems().map((it) => (it && it.onClick ? { ...it, onClick: () => Promise.resolve(it.onClick()).then(() => { if (!/Optionen|Datum/.test(it.label)) after && after(); }) } : it));
  showSheetMenu(items, { title: 'Auf Wiedervorlage' });
}

// ------------------------------------------------------------------ selection mode
function startSelect(id) {
  if (!st.select) {
    document.body.classList.add('m-select');
    const pop = pushLayer(() => endSelectNow());
    st.select = () => pop();
  }
  mail.selectIds([id]);
  if (navigator.vibrate) { try { navigator.vibrate(15); } catch { /* ignore */ } }
  update();
}

function endSelect() { if (st.select) st.select(); }
function endSelectNow() {
  st.select = null;
  document.body.classList.remove('m-select');
  mail.selectIds([]);
  update();
}

// ------------------------------------------------------------------ list: tap, long press, swipe, pull to refresh
const SWIPE_OPEN = 88;

function setupList() {
  const L = $('#msglist');
  // Taps in selection mode toggle rows instead of selecting one.
  const inSelect = (e) => st.select && e.target.closest('.msg');
  L.addEventListener('mousedown', (e) => { if (inSelect(e)) e.stopPropagation(); }, true);
  L.addEventListener('click', (e) => {
    const row = e.target.closest('.msg');
    if (!row) return;
    if (suppressClick) { suppressClick = false; e.stopPropagation(); e.preventDefault(); return; }
    if (st.select) {
      e.stopPropagation();
      mail.toggleSelectId(row.dataset.id);
      if (!mail.getSelection().ids.length) endSelect(); else update();
      return;
    }
    if (e.target.closest('.qa, .flagcell, .readcell')) return;
    openReader();
  });
  L.addEventListener('contextmenu', (e) => { if (e.target.closest('.msg')) { e.preventDefault(); e.stopPropagation(); } }, true);
  // Two quick taps are no desktop double click (that would open a second window).
  L.addEventListener('dblclick', (e) => e.stopPropagation(), true);

  let g = null; // current gesture
  let suppressClick = false;
  let bg = null;
  const reset = (row, animate = true) => {
    if (!row) return;
    row.style.transition = animate ? 'transform .18s ease' : '';
    row.style.transform = '';
    setTimeout(() => { row.style.transition = ''; if (bg) { bg.remove(); bg = null; } }, animate ? 190 : 0);
  };
  L.addEventListener('pointerdown', (e) => {
    if (e.pointerType === 'mouse') return;
    const row = e.target.closest('.msg');
    g = { x: e.clientX, y: e.clientY, row, dx: 0, mode: null, pull: 0, top: L.scrollTop <= 0 };
    if (row && !st.select) {
      g.timer = setTimeout(() => {
        if (!g || g.mode) return;
        g.mode = 'long';
        suppressClick = true;
        startSelect(row.dataset.id);
      }, 480);
    }
  });
  L.addEventListener('pointermove', (e) => {
    if (!g) return;
    const dx = e.clientX - g.x, dy = e.clientY - g.y;
    if (!g.mode) {
      if (Math.abs(dx) < 10 && Math.abs(dy) < 10) return;
      clearTimeout(g.timer);
      if (g.row && !st.select && Math.abs(dx) > Math.abs(dy) * 1.3) {
        g.mode = 'swipe';
        bg = h('div.m-swipe-bg');
        bg.style.top = g.row.offsetTop + 'px';
        bg.style.height = g.row.offsetHeight + 'px';
        L.appendChild(bg);
      } else if (dy > 0 && g.top && L.scrollTop <= 0) {
        g.mode = 'pull';
      } else {
        g.mode = 'scroll';
      }
    }
    if (g.mode === 'swipe') {
      g.dx = dx;
      g.row.style.transform = `translateX(${dx}px)`;
      const right = dx > 0;
      bg.className = 'm-swipe-bg ' + (right ? 'archive' : 'snooze') + (Math.abs(dx) > SWIPE_OPEN ? ' armed' : '');
      bg.innerHTML = right ? icon('archive', 22) + '<span>Archivieren</span>' : '<span>Wiedervorlage</span>' + icon('snooze', 22);
    } else if (g.mode === 'pull') {
      g.pull = Math.max(0, Math.min(110, dy * 0.5));
      ui.ptr.style.transform = `translateY(${g.pull}px) rotate(${g.pull * 3}deg)`;
      ui.ptr.classList.toggle('armed', g.pull > 60);
      ui.ptr.classList.add('show');
    }
  });
  const end = () => {
    if (!g) return;
    clearTimeout(g.timer);
    const gg = g;
    g = null;
    if (gg.mode === 'swipe') {
      suppressClick = true;
      setTimeout(() => { suppressClick = false; }, 350);
      const id = gg.row.dataset.id;
      if (gg.dx > SWIPE_OPEN) {
        gg.row.style.transition = 'transform .16s ease';
        gg.row.style.transform = `translateX(${L.clientWidth}px)`;
        mail.selectIds([id]);
        setTimeout(() => { mail.archiveSelected().catch(reportError).finally(() => { if (bg) { bg.remove(); bg = null; } }); }, 140);
      } else if (gg.dx < -SWIPE_OPEN) {
        reset(gg.row);
        mail.selectIds([id]);
        snoozeSheet(null);
      } else {
        reset(gg.row);
      }
    } else if (gg.mode === 'pull') {
      const go = gg.pull > 60;
      ui.ptr.classList.toggle('spin', go);
      ui.ptr.style.transition = 'transform .2s ease';
      ui.ptr.style.transform = go ? 'translateY(56px)' : '';
      const done = () => { ui.ptr.style.transform = ''; ui.ptr.classList.remove('spin', 'armed', 'show'); setTimeout(() => { ui.ptr.style.transition = ''; }, 220); };
      if (go) Promise.resolve(hooks.syncAll()).finally(() => setTimeout(done, 300));
      else done();
    }
  };
  L.addEventListener('pointerup', end);
  L.addEventListener('pointercancel', () => {
    if (g && g.mode === 'swipe') reset(g.row);
    if (g && g.mode === 'pull') { ui.ptr.style.transform = ''; ui.ptr.classList.remove('show', 'armed'); }
    if (g) clearTimeout(g.timer);
    g = null;
  });
}

// Kanban: a tap opens the card (desktop: double click).
function setupBoard() {
  const board = $('#board');
  if (!board) return;
  board.addEventListener('dblclick', (e) => e.stopPropagation(), true);
  board.addEventListener('click', (e) => {
    if (e.target.closest('button, .kb-head, input')) return;
    if (e.target.closest('.kb-card')) openReader();
  });
}

// ------------------------------------------------------------------ contacts: detail as a layer
function setupContacts() {
  const list = $('.ct-list');
  if (!list) return;
  ui.ctbar.append(btn('chevronLeft', 'Zurück', () => { if (st.ctDetail) st.ctDetail(); }, 'm-ct-back'), h('span.m-ctbar-title', 'Kontakt'));
  list.addEventListener('click', (e) => {
    if (!e.target.closest('.ct-row')) return;
    if (st.ctDetail) return;
    document.body.classList.add('m-ct-detail');
    const pop = pushLayer(() => { st.ctDetail = null; document.body.classList.remove('m-ct-detail'); });
    st.ctDetail = () => pop();
  });
}

export function isReaderOpen() { return !!st.reader; }
