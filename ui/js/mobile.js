// Phone shell: app bar, drawer, bottom navigation, full-screen reader and
// sheets, swipe actions. Desktop never loads this (see main.js).
//
// Layers (drawer, reader, sheets, dialogs, menus) sit on the browser history,
// so the Android back button closes the topmost one.
import { on, openUrl } from './api.js';
import { h, esc, load, store } from './util.js';
import { icon } from './icons.js';
import { reportError, showSheetMenu } from './dialogs.js';
import { state, mailboxes, wellKnown } from './state.js';
import { accountAvatar } from './accounts.js';
import { isTaskMail } from './categories.js';
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
const st = { reader: null, select: null, search: null, drawer: null, ctDetail: null };

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
  ui.scrim = h('div.m-scrim');
  ui.ptr = h('div.m-ptr', { html: icon('sync', 20) });
  document.body.prepend(ui.appbar, ui.searchrow);
  document.body.append(ui.readbar, ui.readactions, ui.ctbar, ui.bottomnav, ui.scrim);
  $('#msglist').before(ui.ptr);
  ui.scrim.addEventListener('click', () => { if (st.drawer) st.drawer(); });

  setupDrawer();
  setupList();
  // Large titles above the mail and contact lists; they fold into the app bar on scroll.
  ui.large = h('div.m-largetitle', { dataset: { testid: 'm-largetitle' } });
  const lp = $('.listpane');
  if (lp) lp.prepend(ui.large);
  ui.largeCt = h('div.m-largetitle', h('span.m-lt-main', 'Kontakte'));
  const cp = $('.ct-listpane');
  if (cp) cp.prepend(ui.largeCt);
  document.addEventListener('scroll', (e) => {
    const t = e.target;
    if (!t || !t.closest || !t.closest('.msglist, .ct-list, .kb-cols')) return;
    document.body.classList.toggle('m-scrolled', t.scrollTop > 8);
  }, true);
  setupBoard();
  setupContacts();
  renderReadbar();
  // Work week and week are too narrow for a phone: start with the agenda once.
  store('mCalViewSet', true);
  if (['workweek', 'week'].includes(calendar.getView())) calendar.setView('sprint');
  on('open-window', (p) => { if (p && p.kind) openSheet(p.kind, p.query || ''); });
  update();
}

function $(sel) { return document.querySelector(sel); }

/** Repaints app bar, bottom navigation and FAB for the current state. */
export function update() {
  if (!ui.appbar) return;
  renderAppbar();
  if (ui.large && state.mode === 'mail') {
    const stats = mail.listStats();
    ui.large.innerHTML = '';
    ui.large.append(h('span.m-lt-main', folderTitle()), h('span.m-lt-sub', stats.unread ? `${stats.unread} ungelesen` : stats.total ? `${stats.total} Elemente` : ''));
  }
  // The app-bar title hides only while a large title is really on screen (not on the board).
  const large = (state.mode === 'mail' && mail.getView() !== 'kanban') || state.mode === 'contacts';
  document.body.classList.toggle('m-has-large', large);
  if (st.lastMode !== state.mode) {
    st.lastMode = state.mode;
    // title and list belong together: the title stays folded while the list is still scrolled
    requestAnimationFrame(() => {
      const list = [...document.querySelectorAll('.msglist, .ct-list, .kb-cols')].find((e) => e.offsetParent);
      document.body.classList.toggle('m-scrolled', !!list && list.scrollTop > 8);
    });
  }
  renderBottomNav();
  renderTodayAgenda();
}

// ------------------------------------------------------------------ "Heute": today's meetings above the list
let agendaDay = '';
let agendaEvents = null;
async function renderTodayAgenda() {
  const show = state.mode === 'mail' && mail.isToday() && !st.select && !st.search;
  if (!ui.agenda) { ui.agenda = h('div.m-agenda', { dataset: { testid: 'm-today-agenda' } }); $('#msglist').before(ui.agenda); }
  ui.agenda.classList.toggle('hidden', !show);
  if (!show) return;
  const now = new Date();
  const day = now.toDateString();
  if (agendaDay !== day || !agendaEvents) {
    agendaDay = day;
    agendaEvents = [];
    const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const end = new Date(start.getTime() + 86400000);
    try { agendaEvents = (await calendar.eventsBetween(start, end, { own: true })).filter((e) => !e.isCancelled); } catch (e) { console.warn(e); }
    setTimeout(() => { agendaEvents = null; }, 5 * 60000); // refreshed every few minutes
  }
  const evs = agendaEvents
    .filter((e) => (e.isAllDay ? calendar.allDayRange(e).end > new Date(now.getFullYear(), now.getMonth(), now.getDate()) : new Date(e.end) > now))
    .sort((a, b) => (b.isAllDay - a.isAllDay) || (new Date(a.start) - new Date(b.start)));
  const pad = (n) => String(n).padStart(2, '0');
  const time = (e) => (e.isAllDay ? 'ganztägig' : `${pad(new Date(e.start).getHours())}:${pad(new Date(e.start).getMinutes())}`);
  ui.agenda.innerHTML = '';
  const head = h('div.m-agenda-head', h('span', evs.length ? `Termine heute (${evs.length})` : 'Termine heute'));
  const all = h('button.m-agenda-all', { type: 'button' }, 'Kalender');
  all.addEventListener('click', () => { hooks.setMode('calendar'); calendar.goToday(); update(); });
  head.appendChild(all);
  ui.agenda.appendChild(head);
  if (!evs.length) ui.agenda.appendChild(h('div.m-agenda-none', 'Keine weiteren Termine'));
  for (const e of evs.slice(0, 3)) {
    const row = h('div.m-agenda-row', { role: 'button', tabindex: '0' },
      h('span.m-agenda-time', time(e)),
      h('span.m-agenda-title', e.subject || '(Kein Betreff)'));
    row.addEventListener('click', () => calendar.openEvent(e));
    if (e.onlineMeetingUrl && !e.isAllDay) {
      const join = h('button.m-agenda-join', { type: 'button', html: icon('teams', 16) + '<span>Beitreten</span>' });
      join.addEventListener('click', (ev) => { ev.stopPropagation(); openUrl(e.onlineMeetingUrl); });
      row.appendChild(join);
    }
    ui.agenda.appendChild(row);
  }
  if (evs.length > 3) {
    const more = h('button.m-agenda-more', { type: 'button' }, `${evs.length - 3} weitere im Kalender`);
    more.addEventListener('click', () => all.click());
    ui.agenda.appendChild(more);
  }
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
      (() => { const b = h('button.m-textbtn', { type: 'button', dataset: { testid: 'm-sel-all-btn' } }, 'Alle'); b.addEventListener('click', () => { mail.selectAllVisible(); update(); }); return b; })(),
      btn('archive', 'Archivieren', () => mail.archiveSelected().then(endSelect), 'm-sel-archive'),
      btn('snooze', 'Auf Wiedervorlage', () => snoozeSheet(endSelect), 'm-sel-snooze'),
      btn('more', 'Weitere Aktionen', () => showSheetMenu([
        { label: 'Alle auswählen', icon: 'check', onClick: () => { mail.selectAllVisible(); update(); }, testid: 'm-sel-all' },
        { separator: true },
        ...mail.moreItems(false).filter((it) => it && !it.separator && !/Fenster|Quelltext/.test(it.label || '')),
        { label: 'Kategorien', icon: 'tag', submenu: () => mail.categoryMenuItems() },
        { label: 'Kanban-Lane', icon: 'kanban', submenu: () => mail.laneMenuItems() },
      ]), 'm-sel-more'),
      btn('delete', 'Löschen', () => mail.deleteSelectedUndoable().then(endSelect), 'm-sel-delete'));
    return;
  }
  if (state.mode === 'mail' && st.search) {
    bar.classList.add('searching');
    const input = h('input.m-search-input', { type: 'search', placeholder: 'Suchen (Eingabe: auf dem Server)', enterkeyhint: 'search', autocomplete: 'off', spellcheck: false, dataset: { testid: 'm-search-input' } });
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
    input.focus();
    setTimeout(() => { if (document.activeElement !== input) input.focus(); }, 30);
    return;
  }
  // The folder/calendar list is a page of its own ("Postfächer"), opened from the left.
  // Same leading glyph in every area: it opens the area's list page (Postfächer / Kalender / Adressbücher).
  const menu = btn('folder', state.mode === 'mail' ? 'Postfächer' : state.mode === 'calendar' ? 'Kalender auswählen' : 'Adressbücher', () => openDrawer(), 'm-menu');
  if (state.mode === 'mail') {
    const stats = mail.listStats();
    const kanban = mail.getView() === 'kanban';
    bar.append(menu,
      h('div.m-title', h('span.m-title-main', { dataset: { testid: 'm-title' } }, folderTitle()),
        h('span.m-title-sub', stats.unread ? `${stats.unread} ungelesen` : stats.total ? `${stats.total} Elemente` : '')),
      btn('search', 'Suchen', () => openSearch(), 'm-search'),
      composeButton());
    void kanban;
  } else if (state.mode === 'calendar') {
    bar.append(menu,
      btn('chevronLeft', 'Zurück', () => { calendar.step(-1); setTimeout(update, 50); }, 'm-cal-prev'),
      (() => {
        // a tap on the period jumps to today (the separate "Heute" icon made the title too short)
        const { start, end } = calendar.viewRange();
        const now = new Date();
        const away = now < start || now >= end;
        const t = h('button.m-title.center.m-cal-titlebtn', { type: 'button', 'aria-label': 'Zu heute springen', dataset: { testid: 'm-cal-today' } },
          h('span.m-title-main', { dataset: { testid: 'm-cal-title' } }, calTitle()),
          // shown while today is out of view – a visible way back
          away ? h('span.m-title-sub.m-cal-back', 'Zu heute') : null);
        t.addEventListener('click', () => { calendar.goToday(); setTimeout(update, 50); });
        return t;
      })(),
      btn('chevronRight', 'Vor', () => { calendar.step(1); setTimeout(update, 50); }, 'm-cal-next'),
      viewButton(),
      composeButton());
  } else {
    bar.append(menu,
      h('div.m-title', h('span.m-title-main', 'Kontakte')),
      composeButton());
  }
}

/**
 * "Neu" in the app bar (replaces the floating button): a tap creates what the
 * current area is about – mail, a task on "Heute", an event, a contact; a long
 * press offers all three kinds.
 */
function composeButton() {
  const today = state.mode === 'mail' && mail.isToday();
  const [ic, label, run] = state.mode === 'calendar' ? ['plus', 'Neuer Termin', () => calendar.newEvent()]
    : state.mode === 'contacts' ? ['plus', 'Neuer Kontakt', () => contacts.newContact()]
    : today ? ['task', 'Neue Aufgabe', () => hooks.newTask()]
    : ['edit', 'Neue E-Mail', () => mail.newMail()];
  let longPressed = false;
  let timer = null;
  const b = btn(ic, label, () => { if (longPressed) { longPressed = false; return; } return run(); }, 'm-compose', '.m-compose');
  // labelled: "Neu" next to the icon (an icon alone was a guess for some testers)
  // calendar: icon only – the period title needs the room on 375–393pt phones
  if (state.mode !== 'calendar') b.insertAdjacentHTML('beforeend', '<span class="m-compose-label">Neu</span>');
  const sheet = () => showSheetMenu([
    { label: 'E-Mail', icon: 'mail', onClick: () => mail.newMail(), testid: 'm-new-mail' },
    { label: 'Aufgabe', icon: 'task', onClick: () => hooks.newTask(), testid: 'm-new-task' },
    { label: 'Termin', icon: 'calendarNew', onClick: () => calendar.newEvent(), testid: 'm-new-event' },
  ], { title: 'Neu' });
  b.addEventListener('pointerdown', () => { longPressed = false; timer = setTimeout(() => { longPressed = true; sheet(); }, 480); });
  for (const ev of ['pointerup', 'pointercancel', 'pointerleave']) b.addEventListener(ev, () => clearTimeout(timer));
  b.addEventListener('contextmenu', (e) => e.preventDefault());
  return b;
}

/** Calendar view as a labelled button ("Agenda ▾") instead of a bare icon. */
function viewButton() {
  const cur = VIEWS.find((x) => x[0] === calendar.getView()) || VIEWS[0];
  const b = h('button.m-textbtn', { type: 'button', 'aria-label': 'Ansicht wählen', dataset: { testid: 'm-cal-view' }, html: `<span>${cur[0] === 'sprint' ? 'Agenda' : cur[1]}</span>` + icon('chevronDown', 14) });
  b.addEventListener('click', () => showSheetMenu(calViewItems(), { title: 'Ansicht' }));
  return b;
}

/** Account menu (settings, accounts, about …) – opened from the account row on the folder page. */
function openAccountSheet() {
  const a = state.accounts[0];
  if (st.drawer) st.drawer();
  showSheetMenu(hooks.appMenu().filter((it) => !it || !/^Ansicht$/.test(it.label || '')), { title: a ? a.email : 'lettertask' });
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
  if (v === 'month') return `${MONTHS_SHORT[start.getMonth() === last.getMonth() ? start.getMonth() : new Date(start.getTime() + 15 * 86400000).getMonth()]} ${new Date(start.getTime() + 15 * 86400000).getFullYear()}`;
  if (start.getMonth() === last.getMonth()) return `${start.getDate()}.–${last.getDate()}.${last.getMonth() + 1}.`;
  return `${start.getDate()}.${start.getMonth() + 1}.–${last.getDate()}.${last.getMonth() + 1}.`;
}

// No week view on the phone: seven columns of 40px leave no room for a title.
const VIEWS = [['sprint', 'Agenda (14 Tage)', 'bullets'], ['day', 'Tag', 'dayView'], ['month', 'Monat', 'monthView']];
function viewIcon(v) { return (VIEWS.find((x) => x[0] === v) || VIEWS[0])[2]; }
function calViewItems() {
  const cur = calendar.getView();
  return [
    ...VIEWS.map(([v, label, ic]) => ({ label, icon: ic, checked: cur === v, onClick: () => { calendar.setView(v); setTimeout(update, 50); } })),
    { separator: true },
    { label: 'Heute', icon: 'today', onClick: () => { calendar.goToday(); setTimeout(update, 50); } },
  ];
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
    b.addEventListener('click', () => { st.searchScope = v; mail.setMobileSearchScope(v); renderSearchRow(); });
    return b;
  };
  mail.setMobileSearchScope(st.searchScope || 'all');
  row.append(chip('all', 'Alle Ordner'), chip('folder', 'Dieser Ordner'));
}

// ------------------------------------------------------------------ drawer (the sidebar)
function setupDrawer() {
  const nav = $('#navpane');
  nav.classList.add('m-drawer');
  ui.drawerHead = h('div.m-drawer-head');
  ui.drawerMenu = h('div.m-drawer-menu');
  // only the title bar stays pinned; account and folders scroll below it
  ui.drawerBar = h('div.m-drawer-bar');
  nav.prepend(ui.drawerBar, ui.drawerHead);
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
  ui.drawerBar.innerHTML = '';
  const done = h('button.m-nav-action.m-drawer-done', { type: 'button', dataset: { testid: 'm-drawer-done' } }, 'Fertig');
  done.addEventListener('click', () => { if (st.drawer) st.drawer(); });
  ui.drawerBar.append(h('span.m-drawer-title', state.mode === 'mail' ? 'Postfächer' : state.mode === 'calendar' ? 'Kalender' : 'Adressbücher'), done);
  // The account row opens the account menu (it replaced the avatar in the app bar).
  const acc = a ? h('button.m-drawer-account', { type: 'button', dataset: { testid: 'm-account' }, 'aria-label': 'Konto und Einstellungen' },
    accountAvatar(a, 'lg'), h('div.m-drawer-who', h('div.m-drawer-name', a.displayName || a.email), h('div.m-drawer-mail', a.email)), h('span.m-row-chev', { html: icon('chevronRight', 16) })) : null;
  if (acc) acc.addEventListener('click', () => openAccountSheet());
  // Fokuszeit belongs to mail: a row in the mail list page only.
  const focusRow = state.mode === 'mail' ? h('button.m-drawer-item.m-drawer-focusrow', { type: 'button', dataset: { testid: 'm-focus' }, html: icon('focus', 20) + `<span>Fokuszeit</span><span class="m-row-value">${esc(hooks.focusLabel ? hooks.focusLabel().replace(/^Fokuszeit:?\s*/, '') || 'Aus' : 'Aus')}</span>` }) : null;
  if (focusRow) focusRow.addEventListener('click', () => showSheetMenu(hooks.focusMenu(), { title: 'Fokuszeit' }));
  ui.drawerHead.append(h('div.m-drawer-group', acc, focusRow));
  ui.drawerMenu.innerHTML = '';
  ui.drawerMenu.classList.toggle('hidden', state.mode !== 'mail');
  // Phone: the Kanban/list switch lives with the folders (the app bar has no avatar menu any more).
  if (state.mode === 'mail' && !mail.isVirtual()) {
    const kanban = mail.getView() === 'kanban';
    const vb = h('button.m-drawer-item', { type: 'button', dataset: { testid: 'm-view' }, html: icon(kanban ? 'bullets' : 'kanban', 20) + `<span>${kanban ? 'Als Liste anzeigen' : 'Als Kanban-Board anzeigen'}</span>` });
    vb.addEventListener('click', () => { if (st.drawer) st.drawer(); mail.toggleView(); update(); });
    ui.drawerMenu.appendChild(vb);
  }
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
  // A folder that is also a favourite is marked once (in Favoriten) – not twice.
  const sel = [...document.querySelectorAll('#navpane .tree-node.selected')];
  sel.forEach((n, i) => n.classList.toggle('m-dup', i > 0));
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

// ------------------------------------------------------------------ reader
function renderReadbar() {
  ui.readbar.innerHTML = '';
  ui.readbar.append(
    btn('chevronLeft', 'Zurück', () => closeReader(), 'm-read-back'),
    h('span.spacer'),
    btn('archive', 'Archivieren', () => mail.archiveSelected().then(closeReader), 'm-read-archive'),
    btn('snooze', 'Auf Wiedervorlage', () => snoozeSheet(closeReader), 'm-read-snooze'),
    btn('more', 'Weitere Aktionen', () => showSheetMenu([
      ...mail.moreItems(false).filter((it) => it && !/Fenster|Quelltext/.test(it.label || '')),
      { separator: true },
      { label: 'Kategorien', icon: 'tag', submenu: () => mail.categoryMenuItems() },
      { label: 'Kanban-Lane', icon: 'kanban', submenu: () => mail.laneMenuItems() },
    ]), 'm-read-more'),
    btn('delete', 'Löschen', () => mail.deleteSelectedUndoable().then(closeReader), 'm-read-delete'));
  renderReadActions(false);
  // A task gets "Erledigt" (archive) and "Später" instead of reply buttons.
  document.addEventListener('lt-detail', (e) => {
    const m = e.detail && e.detail.message;
    const task = isTaskMail(m);
    renderReadActions(task);
    // A task: no sender/recipient block (it is from you, to you) – a status line instead.
    document.body.classList.toggle('m-task-open', task);
    requestAnimationFrame(() => {
      const pane = document.querySelector('.readpane');
      if (!pane) return;
      pane.querySelectorAll('.m-task-meta').forEach((x) => x.remove());
      if (!task) return;
      const row = pane.querySelector('.rp-fromrow');
      const due = m.dueAt ? new Date(m.dueAt) : null;
      const when = due ? due.toLocaleString('de-DE', { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }) : '';
      const meta = h('div.m-task-meta', { dataset: { testid: 'm-task-meta' }, html: icon('task', 18) + `<span>Aufgabe · ${due ? 'fällig ' + esc(when) : 'ohne Fälligkeit'}</span>` });
      if (row) row.before(meta);
    });
  });
}

function renderReadActions(task) {
  if (ui.readactions.dataset.task === String(task)) return;
  ui.readactions.dataset.task = String(task);
  ui.readactions.innerHTML = '';
  const act = (ic, label, testid, onClick, cls = '') => {
    const b = h('button.m-readact' + cls, { type: 'button', dataset: { testid }, html: icon(ic, 20) + `<span>${label}</span>` });
    b.addEventListener('click', () => Promise.resolve().then(onClick).catch(reportError));
    return b;
  };
  if (task) {
    ui.readactions.append(
      act('check', 'Erledigt', 'm-done', () => mail.completeSelected().then(closeReader), '.m-done'),
      act('snooze', 'Später', 'm-later', () => snoozeSheet(closeReader)),
      act('delete', 'Löschen', 'm-task-delete', () => mail.deleteSelectedUndoable().then(closeReader), '.m-readact-danger'));
  } else {
    // A toolbar as in Mail: archive, the three answers, delete (red, at the far end).
    const tool = (ic, label, testid, onClick, cls = '') => {
      const b = h('button.m-tool' + cls, { type: 'button', dataset: { testid }, 'aria-label': label, html: icon(ic, 22) + `<span>${label}</span>` });
      b.addEventListener('click', () => Promise.resolve().then(onClick).catch(reportError));
      return b;
    };
    ui.readactions.append(
      tool('archive', 'Archiv', 'm-archive', () => mail.archiveSelected().then(closeReader)),
      tool('reply', 'Antworten', 'm-reply', () => mail.respond('reply')),
      tool('replyAll', 'Allen', 'm-replyAll', () => mail.respond('replyAll')),
      tool('forward', 'Weiterleiten', 'm-forward', () => mail.respond('forward')),
      tool('delete', 'Löschen', 'm-delete', () => mail.deleteSelectedUndoable().then(closeReader), '.m-tool-danger'));
  }
  ui.readactions.classList.toggle('m-toolbar', !task);
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
  // A–Z index on the right edge: tap or slide to jump to a letter.
  const LETTERS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ#'.split('');
  const idx = h('div.m-az', { dataset: { testid: 'm-az' }, 'aria-label': 'Alphabet-Index' }, ...LETTERS.map((l) => h('span', l)));
  const bubble = h('div.m-az-bubble');
  list.parentElement.append(idx, bubble);
  const jump = (y) => {
    const r = idx.getBoundingClientRect();
    const i = Math.max(0, Math.min(LETTERS.length - 1, Math.floor((y - r.top) / (r.height / LETTERS.length))));
    const want = LETTERS[i];
    const heads = [...list.querySelectorAll('.ct-letter')];
    const target = heads.find((el) => el.textContent.trim().toUpperCase() >= want) || heads[heads.length - 1];
    if (target) list.scrollTop = target.offsetTop - list.offsetTop;
    idx.querySelectorAll('span').forEach((s, j) => s.classList.toggle('on', j === i));
    bubble.textContent = want;
    bubble.classList.add('show');
  };
  let sliding = false;
  idx.addEventListener('pointerdown', (e) => { sliding = true; idx.setPointerCapture(e.pointerId); jump(e.clientY); e.preventDefault(); });
  idx.addEventListener('pointermove', (e) => { if (sliding) jump(e.clientY); });
  const stop = () => { sliding = false; setTimeout(() => { idx.querySelectorAll('span.on').forEach((s) => s.classList.remove('on')); bubble.classList.remove('show'); }, 500); };
  idx.addEventListener('pointerup', stop);
  idx.addEventListener('pointercancel', stop);
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
