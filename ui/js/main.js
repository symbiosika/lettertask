// Main window shell: top bar with search, action bar, sidebar, events, shortcuts.
import { call, on, onBusy, isMobile } from './api.js';
import { h, $, load, store, fmtTime, esc, addDays, startOfDay } from './util.js';
import { icon } from './icons.js';
import { showMenu, showMenuAt, reportError, toast, busyIndicator, modal } from './dialogs.js';
import { state, mailboxes, loadConfig, loadAccounts, loadFolders, loadCategories, loadLanes, wellKnown, folderById } from './state.js';
import { openWindow } from './api.js';
import { onBroadcast } from './bus.js';
import { tbBtn, newBtns, sep, setPressed } from './toolbar.js';
import * as mail from './mail.js';
import * as calendar from './calendar.js';
import * as contacts from './contacts.js';
import { initCalPanel, toggleCalPanel, calPanelOpen, showCalPanelHost, onCalPanelChanged } from './calpanel.js';
import { isImap, isGoogle, accountAvatar } from './accounts.js';
import { snoozeOptions, computeSnooze, fmtSnoozeTarget } from './snooze.js';
import { dateTimeField } from './due.js';
import * as mobile from './mobile.js';
import { hooks, openSettings, openCategoryManager, openAutoReply, removeAccountDialog, addAccountFlow, welcomeDialog, aboutDialog } from './settings.js';

const els = {
  topbar: $('#topbar'), toolbar: $('#toolbar'), nav: $('#navpane'), navTitle: $('#nav-title'), tree: $('#folder-tree'),
  calNav: $('#cal-nav'), ctNav: $('#contacts-nav'), navButtons: $('#nav-buttons'), content: $('#content'), status: $('#statusbar'),
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
  const openPanel = () => {
    if (state.mode === 'contacts') { input.blur(); contacts.focusSearch(); return; }
    if (state.mode !== 'mail') return;
    renderPanel(); panel.classList.remove('hidden'); wrap.classList.add('open'); };
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

  const right = h('div.tb-right', focusButton(), h('span.spacer'), busy, h('div.sync-ind#sync-ind'), accountChip());
  els.topbar.append(h('div.tb-left', menuBtn, h('span.brand#brand', { html: icon('mail', 18) + '<span>Mail</span>' })), wrap, right);
}

// ------------------------------------------------------------------ Fokuszeit
const focus = { until: null, timer: null };
function focusActive() { return !!(focus.until && new Date(focus.until) > new Date()); }

function focusButton() {
  const b = h('button.focus-btn#focus-btn', { type: 'button', dataset: { testid: 'focus-btn' } });
  b.addEventListener('click', () => showMenuAt(b, focusMenu()));
  focusButton.el = b;
  paintFocus();
  return b;
}

function focusMenu() {
  if (focusActive()) {
    return [
      { header: `Fokuszeit bis ${fmtTime(new Date(focus.until))}` },
      { label: 'Fokuszeit beenden', icon: 'close', onClick: () => setFocus(0), testid: 'focus-end' },
      { separator: true },
      ...[1, 2, 3, 4].map((n) => ({ label: `Auf ${n} ${n === 1 ? 'Stunde' : 'Stunden'} ab jetzt ändern`, onClick: () => setFocus(n * 60), testid: 'focus-' + n })),
    ];
  }
  return [
    { header: 'Fokuszeit – keine automatischen Abrufe' },
    ...[[1, '1 Stunde Pause'], [2, '2 Stunden'], [3, '3 Stunden'], [4, '4 Stunden']].map(([n, label]) => ({ label, icon: n === 1 ? 'focus' : null, onClick: () => setFocus(n * 60), testid: 'focus-' + n })),
  ];
}

async function setFocus(minutes) {
  const r = await call('set_focus', { minutes });
  applyFocus(r && r.until);
  toast(focusActive() ? `Fokuszeit bis ${fmtTime(new Date(focus.until))} – neue Mails werden erst danach abgerufen.` : 'Fokuszeit beendet.');
}

function applyFocus(until) {
  focus.until = until || null;
  clearTimeout(focus.timer);
  if (focusActive()) {
    // local fallback in case the expiry event gets lost
    focus.timer = setTimeout(() => applyFocus(null), Math.min(2 ** 31 - 1, new Date(focus.until) - new Date() + 1500));
  }
  paintFocus();
  renderStatus();
}

function paintFocus() {
  const b = focusButton.el;
  if (!b) return;
  const on = focusActive();
  b.classList.toggle('on', on);
  b.title = on ? 'Fokuszeit aktiv – automatischer Abruf pausiert' : 'Fokuszeit: automatischen Abruf für eine Weile pausieren';
  b.innerHTML = icon('focus', 16) + `<span>${on ? `Fokus bis ${fmtTime(new Date(focus.until))}` : 'Fokuszeit'}</span>` + icon('chevronDown', 12);
}

function accountChip() {
  const acc = state.accounts[0];
  const el = h('button.account-chip#account-chip', { type: 'button', title: acc ? acc.email : 'Kein Konto' });
  const paint = () => {
    const a = state.accounts[0];
    el.title = a ? `${a.displayName} <${a.email}>` : 'Kein Konto';
    el.innerHTML = a ? '' : icon('user', 18);
    if (a) el.appendChild(accountAvatar(a, 'xs'));
  };
  paint();
  el.addEventListener('click', () => showMenuAt(el, appMenu()));
  accountChip.paint = paint;
  return el;
}

function appMenu() {
  const acc = state.accounts[0] && state.accounts[0].id;
  const o365 = state.accounts.find((a) => !isImap(a));
  const withCalendar = state.accounts.find((a) => !isImap(a) || isGoogle(a));
  const c = state.config || {};
  const setCfg = async (patch) => {
    state.config = await call('save_config', { config: { ...state.config, ...patch } });
    applyConfig();
  };
  return [
    { label: 'Neue E-Mail', icon: 'mail', shortcut: 'Strg+N', onClick: () => mail.newMail() },
    { label: 'Neue Aufgabe', icon: 'task', shortcut: 'Strg+Umschalt+N', disabled: !acc, onClick: () => newTask() },
    { label: 'Neuer Termin', icon: 'calendarNew', shortcut: 'Strg+Umschalt+A', disabled: !withCalendar, onClick: () => calendar.newEvent() },
    { label: 'Neuer Kontakt', icon: 'contacts', disabled: !acc, onClick: () => { setMode('contacts'); contacts.newContact().catch(reportError); } },
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
    { label: 'Signaturen…', icon: 'signature', disabled: !acc, onClick: () => openSettings('signatures'), testid: 'menu-signatures' },
    { label: 'Kategorien verwalten…', icon: 'tag', disabled: !acc, onClick: () => openCategoryManager(acc), testid: 'menu-categories' },
    { label: 'Abwesenheitsnotiz…', icon: 'outOfOffice', disabled: !o365, onClick: () => openAutoReply(o365.id), testid: 'menu-autoreply' },
    { separator: true },
    { label: 'Konto hinzufügen…', icon: 'user', onClick: () => addAccountFlow().catch(reportError), testid: 'menu-add-account' },
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
    if (!d || d <= now) return; // e.g. "Heute 18 Uhr" in the evening
    items.push({ label: o.label, shortcut: fmtSnoozeTarget(d), icon: i === 0 ? 'snooze' : null, onClick: () => mail.snoozeSelected(d), testid: 'snooze-opt-' + i });
  });
  items.push({ separator: true },
    { label: 'Datum und Uhrzeit wählen…', icon: 'calendar', onClick: () => customSnooze(), testid: 'snooze-custom' },
    { separator: true },
    { label: 'Optionen bearbeiten…', icon: 'settings', onClick: () => openSettings('snooze'), testid: 'snooze-edit' });
  return items;
}

/** Toggles the week calendar below list and reading pane ("hast du da Zeit?"). */
function calPanelBtn() {
  const b = tbBtn('', 'calendar', () => { toggleCalPanel(); }, { testid: 'tb-calpanel', title: 'Kalender unten ein-/ausblenden', iconOnly: true });
  setPressed(b, calPanelOpen());
  return b;
}
document.addEventListener('calpanel-toggled', (e) => {
  const b = document.querySelector('[data-testid=tb-calpanel]');
  if (b) setPressed(b, e.detail);
});

function newTask() {
  const mb = state.current.mailboxId || (mailboxes()[0] && mailboxes()[0].id);
  if (!mb) { toast('Bitte fügen Sie zuerst ein Konto hinzu.'); return; }
  openWindow('task', { mailboxId: mb }).catch(reportError);
}

/** Shift+S: first configured Wiedervorlage option directly. */
async function quickSnooze() {
  if (!mail.getSelection().ids.length) return;
  const o = snoozeOptions(state.config)[0];
  const d = o && computeSnooze(o.rule, new Date());
  if (d) await mail.snoozeSelected(d);
}

async function customSnooze() {
  if (!mail.getSelection().ids.length) { toast('Bitte wählen Sie zuerst ein Element aus.'); return; }
  const def = addDays(startOfDay(new Date()), 1); def.setHours(8);
  const input = dateTimeField(def, 'snoozeAt');
  const res = await modal({
    title: 'Auf Wiedervorlage', width: 360,
    body: h('div', h('label.lbl', 'Wieder im Posteingang am:'), input.el),
    buttons: [{ label: 'Wiedervorlage', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: () => {
      const d = input.value();
      if (!d || d <= new Date()) { input.el.classList.add('invalid'); toast('Bitte einen Zeitpunkt in der Zukunft wählen.', 'error'); return false; }
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
    lead.appendChild(newBtns([
      ['mail', 'Neue E-Mail (Strg+N)', () => mail.newMail(), 'tb-new'],
      ['task', 'Neue Aufgabe (Strg+Umschalt+N)', () => newTask(), 'new-task'],
      ['calendarNew', 'Neuer Termin (Strg+Umschalt+A)', () => calendar.newEvent(), 'new-event'],
    ]));
    lead.appendChild(tbBtn('', 'kanban', () => mail.openInboxKanban().catch(reportError), { testid: 'tb-kanban', title: 'Posteingang als Kanban öffnen', iconOnly: true }));
    tb.append(lead,
      h('div.ab-group',
        dep(tbBtn('Antworten', 'reply', () => mail.respond('reply'), { testid: 'tb-reply', title: 'Antworten (Strg+R)' })),
        dep(tbBtn('Allen antworten', 'replyAll', () => mail.respond('replyAll'), { testid: 'tb-replyall', title: 'Allen antworten (Strg+Umschalt+R)' })),
        dep(tbBtn('Weiterleiten', 'forward', () => mail.respond('forward'), { testid: 'tb-forward', title: 'Weiterleiten (Strg+F)' })),
        dep(tbBtn('Delegieren', 'people', () => mail.respond('delegate'), { testid: 'tb-delegate', title: 'Delegieren: weiterleiten und als Aufgabe nachverfolgen' }))),
      sep(),
      h('div.ab-group',
        dep(tbBtn('Archivieren', 'archive', () => mail.archiveSelected(), { testid: 'tb-archive', title: 'Archivieren (E)' })),
        dep(tbBtn('Auf Wiedervorlage', 'snooze', (b) => showMenuAt(b, snoozeItems()), { caret: true, testid: 'tb-snooze', title: 'Auf Wiedervorlage (S · Umschalt+S: erste Option)' })),
        dep(tbBtn('Löschen', 'delete', () => mail.deleteSelected(false), { testid: 'tb-delete', title: 'Löschen (Entf)' }))),
      h('span.spacer'),
      h('div.ab-group',
        dep(tbBtn('', 'tag', (b) => mail.openTagsForSelection(b), { testid: 'tb-tag', title: 'Kategorien (T)', iconOnly: true })),
        dep(tbBtn('', 'flag', () => mail.toggleFlagSelected(), { testid: 'tb-flag', title: 'Kennzeichnen', iconOnly: true })),
        dep(tbBtn('', 'mailOpen', () => mail.toggleRead(), { testid: 'tb-read', title: 'Gelesen/Ungelesen (Strg+Q / Strg+U)', iconOnly: true })),
        calPanelBtn(),
        tbBtn('', 'sync', () => syncAll(), { testid: 'tb-sync', title: 'Senden/Empfangen (F9)', iconOnly: true })),
    );
    updateActionState();
  } else if (state.mode === 'contacts') {
    lead.appendChild(newBtns([
      ['contacts', 'Neuer Kontakt (Strg+N)', () => contacts.newContact(), 'tb-new-contact'],
      ['mail', 'Neue E-Mail', () => mail.newMail(), 'new-mail'],
    ]));
    tb.append(lead,
      h('div.ab-group',
        dep(tbBtn('E-Mail schreiben', 'mail', () => { const c = contacts.selectedContact(); if (c) contacts.writeTo(c, c.emails[0]); }, { testid: 'tb-ct-write' })),
        dep(tbBtn('Bearbeiten', 'edit', () => contacts.editContact(contacts.selectedContact()), { testid: 'tb-ct-edit' })),
        dep(tbBtn('Löschen', 'delete', () => contacts.deleteContact(), { testid: 'tb-ct-delete' }))),
      h('span.spacer'),
      tbBtn('', 'sync', () => syncAll(), { testid: 'tb-sync', title: 'Kontakte synchronisieren (F9)', iconOnly: true }));
    updateActionState();
  } else {
    lead.appendChild(newBtns([
      ['calendarNew', 'Neuer Termin', () => calendar.newEvent(), 'tb-new-event'],
      ['mail', 'Neue E-Mail', () => mail.newMail(), 'new-mail'],
    ]));
    const view = calendar.getView();
    const seg = h('div.segmented', { role: 'tablist' });
    for (const [v, label] of [['day', 'Tag'], ['workweek', 'Arbeitswoche'], ['week', 'Woche'], ['sprint', '14 Tage'], ['month', 'Monat']]) {
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
  if (isMobile) mobile.update();
  if (state.mode === 'contacts') {
    const c = contacts.selectedContact();
    for (const b of selDependent) b.disabled = !c;
    const w = els.toolbar.querySelector('[data-testid=tb-ct-write]');
    if (w) w.disabled = !c || !c.emails.length;
    const d = els.toolbar.querySelector('[data-testid=tb-ct-delete]');
    if (d) d.disabled = !c || c.readOnly;
    return;
  }
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
    const b = h('button.nav-btn' + (state.mode === mode ? '.active' : ''), { type: 'button', title: `${label} (Strg+${{ mail: 1, calendar: 2, contacts: 3 }[mode]})`, 'aria-label': label, dataset: { testid: 'nav-' + mode } });
    b.innerHTML = icon(ic, 22);
    b.addEventListener('click', () => setMode(mode));
    return b;
  };
  els.navButtons.append(mk('mail', 'Mail', 'mail'), mk('calendar', 'Kalender', 'calendar'), mk('contacts', 'Kontakte', 'contacts'));
}

function setMode(mode) {
  state.mode = mode;
  store('mode', mode);
  document.body.dataset.mode = mode;
  const [title, ic] = { mail: ['Mail', 'mail'], calendar: ['Kalender', 'calendar'], contacts: ['Kontakte', 'contacts'] }[mode];
  els.navTitle.textContent = title;
  const brand = document.getElementById('brand');
  if (brand) brand.innerHTML = icon(ic, 18) + `<span>${title}</span>`;
  els.tree.classList.toggle('hidden', mode !== 'mail');
  els.calNav.classList.toggle('hidden', mode !== 'calendar');
  els.ctNav.classList.toggle('hidden', mode !== 'contacts');
  mail.showMailView(mode === 'mail');
  showCalPanelHost(mode === 'mail');
  calendar.showCalendarView(mode === 'calendar');
  contacts.showContactsView(mode === 'contacts');
  renderNavButtons();
  renderToolbar();
  mail.renderFolderTree();
  renderStatus();
}

/** Text of the Fokuszeit button (phone drawer). */
function focusLabel() { return focusActive() ? `Fokus bis ${fmtTime(new Date(focus.until))}` : 'Fokuszeit'; }

// ------------------------------------------------------------------ status (sidebar footer)
function renderStatus() {
  const s = state.sync;
  const text = focusActive() && s.state !== 'syncing' ? 'Fokuszeit – automatischer Abruf pausiert' : {
    syncing: 'Wird synchronisiert…', idle: s.lastSync ? `Aktualisiert ${fmtTime(new Date(s.lastSync))}` : 'Verbunden', offline: 'Offline', error: 'Synchronisierungsfehler',
  }[s.state] || '';
  const isError = s.state === 'error' && s.message;
  const st = h('span.st' + (isError ? '.err' : ''), { title: s.message || '' }, h('span.dot.' + (s.state || 'idle')),
    h('span.st-text', { dataset: { testid: 'sync-status' } }, state.accounts.length ? (isError ? `${text}: ${s.message}` : text) : 'Kein Konto'));
  if (isError) {
    st.setAttribute('role', 'button');
    st.addEventListener('click', () => showSyncError(s.message));
  }
  els.status.innerHTML = '';
  els.status.append(st);
  if (state.config && state.config.demo) els.status.append(h('span.badge', { title: 'Es wird der eingebaute Demo-Server verwendet.' }, 'Demo'));
  updateActionState();
}

/** Full text of a sync error (the status line only has room for the start). */
async function showSyncError(message) {
  const again = await modal({
    title: 'Synchronisierungsfehler',
    body: h('div', h('p', { style: { marginTop: 0 } }, 'Beim Abgleich mit dem Server ist ein Fehler aufgetreten:'),
      h('pre.selectable.err-detail', { dataset: { testid: 'sync-error-detail' } }, message)),
    buttons: [{ label: 'Erneut versuchen', value: true, primary: true }, { label: 'Schließen', value: false, cancel: true }],
    width: 520,
  });
  if (again) syncAll();
}

// ------------------------------------------------------------------ sync
async function syncAll() {
  state.sync = { ...state.sync, state: 'syncing' };
  renderStatus();
  try {
    if (state.mode === 'contacts') { await contacts.sync(); renderStatus(); return; }
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
  const failed = [];
  // Categories come from the server (Graph); the window must not wait for them.
  // The cache-backed rest is local and fast.
  Promise.all(state.accounts.map((a) => loadCategories(a.id).catch(() => [])))
    .then(() => { mail.reloadList().catch(() => {}); })
    .catch(() => {});
  await Promise.all([
    ...state.accounts.map((a) => loadCategories(a.id, { cached: true }).catch(() => [])),
    ...mailboxes().map((m) => loadFolders(m.id).catch((e) => { failed.push(`${m.address}: ${e.message || e}`); return []; })),
    ...state.accounts.map((a) => loadLanes(a.id)),
  ]);
  await Promise.all(mailboxes().filter((m) => !(state.folders.get(m.id) || []).length).map(async (m) => {
    try { state.folders.set(m.id, await call('sync_folders', { mailboxId: m.id }, { silent: true })); } catch (e) { failed.push(`${m.address}: ${e.message || e}`); }
  }));
  if (failed.length) {
    state.sync = { ...state.sync, state: 'error', message: 'Ordner konnten nicht geladen werden – ' + failed.join('; ') };
    renderStatus();
    toast(state.sync.message, 'error', 9000);
  }
  if (accountChip.paint) accountChip.paint();
}

async function selectInitialFolder() {
  const last = load('lastFolder', null);
  if (last && last.folderId === mail.TODAY && mailboxes().length) return mail.selectFolder(mailboxes()[0].id, mail.TODAY);
  if (last && last.folderId === mail.DELEGATED && mailboxes().length) return mail.selectFolder(mailboxes()[0].id, mail.DELEGATED);
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
  contacts.onAccountsChanged();
  if (!state.current.mailboxId || !mailboxes().some((m) => m.id === state.current.mailboxId)) {
    state.current = { mailboxId: null, folderId: null };
    await selectInitialFolder();
  }
  renderStatus();
}

// ------------------------------------------------------------------ keyboard
function setupShortcuts() {
  // No page zoom: a touchpad pinch arrives as Ctrl+wheel.
  window.addEventListener('wheel', (e) => { if (e.ctrlKey) e.preventDefault(); }, { passive: false });
  window.addEventListener('keydown', (e) => {
    if (document.querySelector('.modal-overlay') || document.querySelector('.tagpop')) return;
    const t = e.target;
    const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable);
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key.toLowerCase();
    if (e.key === 'F9') { e.preventDefault(); syncAll(); return; }
    if (ctrl && k === '1') { e.preventDefault(); setMode('mail'); return; }
    if (ctrl && k === '2') { e.preventDefault(); setMode('calendar'); return; }
    if (ctrl && k === '3') { e.preventDefault(); setMode('contacts'); return; }
    if (ctrl && e.shiftKey && k === 'a') { e.preventDefault(); calendar.newEvent(); return; }
    if (ctrl && e.shiftKey && k === 'n') { e.preventDefault(); newTask(); return; }
    if (ctrl && e.shiftKey && k === 'k' && state.mode === 'mail') { e.preventDefault(); mail.toggleView(); renderToolbar(); return; }
    if (ctrl && k === 'n') { e.preventDefault(); if (state.mode === 'calendar') calendar.newEvent(); else if (state.mode === 'contacts') contacts.newContact().catch(reportError); else mail.newMail(); return; }
    if (ctrl && k === 'e') { e.preventDefault(); $('#search-input').focus(); return; }
    if (typing || state.mode !== 'mail') {
      if (state.mode === 'calendar' && !typing && e.key === 'Delete') { e.preventDefault(); calendar.deleteSelected().catch(reportError); }
      if (state.mode === 'contacts' && !typing && e.key === 'Delete') { e.preventDefault(); contacts.deleteContact().catch(reportError); }
      return;
    }
    if (ctrl && e.shiftKey && k === 'r') { e.preventDefault(); mail.respond('replyAll'); }
    else if (ctrl && k === 'r') { e.preventDefault(); mail.respond('reply'); }
    else if (ctrl && k === 'f') { e.preventDefault(); mail.respond('forward'); }
    else if (ctrl && k === 'q') { e.preventDefault(); mail.markSelected(true).catch(reportError); }
    else if (ctrl && k === 'u') { e.preventDefault(); mail.markSelected(false).catch(reportError); }
    else if (!ctrl && !e.altKey && k === 't') { e.preventDefault(); mail.openTagsForSelection(els.toolbar.querySelector('[data-testid=tb-tag]')); }
    else if (!ctrl && !e.altKey && k === 'e') { e.preventDefault(); mail.archiveSelected().catch(reportError); }
    else if (!ctrl && !e.altKey && k === 's' && e.shiftKey) { e.preventDefault(); quickSnooze().catch(reportError); }
    else if (!ctrl && !e.altKey && k === 's') {
      e.preventDefault();
      if (!mail.getSelection().ids.length) { toast('Bitte wählen Sie zuerst ein Element aus.'); return; }
      showMenuAt(els.toolbar.querySelector('[data-testid=tb-snooze]'), snoozeItems());
    }
    else if (!ctrl && !e.altKey && /^[1-9]$/.test(e.key)) { e.preventDefault(); mail.setLaneByIndex(Number(e.key)).catch(reportError); }
    else if (e.key === 'Escape' && mail.isSearching()) { mail.clearSearch(); }
    else if (e.key === 'Delete' && t === document.body) { e.preventDefault(); mail.deleteSelected(e.shiftKey).catch(reportError); }
  });
}

// ------------------------------------------------------------------ events
function setupEvents() {
  let lastError = null;
  on('sync-status', (p) => {
    state.sync = { ...state.sync, ...p };
    renderStatus();
    // Show a new error once, not only as a dot in the status line.
    if (p && p.state === 'error' && p.message && p.message !== lastError) toast(p.message, 'error', 9000);
    lastError = p && p.state === 'error' ? p.message : null;
  });
  on('mail-changed', (p) => mail.onMailChanged(p).catch((e) => console.warn(e)));
  on('calendar-changed', () => { calendar.onCalendarChanged().catch((e) => console.warn(e)); onCalPanelChanged(); });
  on('accounts-changed', () => onAccountsChanged().catch(reportError));
  on('contacts-changed', () => contacts.onContactsChanged().catch((e) => console.warn(e)));
  on('categories-changed', async (p) => {
    if (p && p.accountId) await loadCategories(p.accountId).catch(() => {});
    mail.reloadList().catch(() => {});
    calendar.onCalendarChanged().catch(() => {});
  });
  on('rules-applied', (p) => {
    if (!p || !p.count) return;
    const subjects = (p.subjects || []).join('\n');
    toast(p.count === 1 ? '1 Mail automatisch auf Wiedervorlage gelegt.' : `${p.count} Mails automatisch auf Wiedervorlage gelegt.`, 'info', 6000, { title: subjects });
    if (p.mailboxId) mail.refreshFolders(p.mailboxId).catch(() => {});
  });
  on('focus-changed', (p) => applyFocus(p && p.until));
  on('followup-resolved', (p) => {
    if (!p) return;
    toast(`Nachfassen erledigt – Antwort von ${p.from || 'unbekannt'}`, 'info', 6000, { title: p.subject || '' });
  });
  on('recurrence-scheduled', (p) => {
    if (!p) return;
    const d = p.due ? new Date(p.due) : null;
    toast(d && !isNaN(d) ? `Nächste Wiederholung: ${fmtSnoozeTarget(d)}` : 'Nächste Wiederholung eingeplant.', 'info', 6000, { title: p.subject || '' });
  });
  // Mails go out in the background once the compose window has closed.
  on('send-failed', (p) => {
    if (!p) return;
    const drafts = p.mailboxId && wellKnown(p.mailboxId, 'drafts');
    toast(`Nicht gesendet: „${p.subject || '(Kein Betreff)'}“ – ${p.message}`, 'error', 15000, drafts ? {
      action: { label: 'Entwürfe öffnen', onClick: () => { setMode('mail'); return mail.selectFolder(p.mailboxId, drafts.id); } },
    } : {});
  });
  on('lanes-changed', (p) => { if (p && p.accountId) mail.onLanesChanged(p.accountId).catch(() => {}); });
  onBroadcast((m) => {
    if (m.type === 'task-created' && m.delegate) toast(`Aufgabe an ${m.delegate.name || m.delegate.address} gesendet${m.due ? ` – Wiedervorlage ${m.due}` : ''}.`);
    else if (m.type === 'delegation-done') toast(`Aufgabe „${m.subject}“ erledigt – archiviert.`);
    else if (m.type === 'task-created') toast(m.due ? `Aufgabe angelegt – fällig ${m.due}.` : 'Aufgabe angelegt – sie liegt im Posteingang.');
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
  mail.hooks.openLaneSettings = (accountId) => openSettings('kanban', { accountId });
  mail.initMail({ navHost: els.tree, contentHost: els.content, onCounts: renderStatus, onSelection: updateActionState });
  calendar.initCalendar({ navHost: els.calNav, contentHost: els.content, onCounts: updateActionState });
  initCalPanel(els.content, $('#mailview'));
  contacts.initContacts({ navHost: els.ctNav, contentHost: els.content, onSelection: updateActionState });
  if (isMobile) mobile.initMobile({ appMenu, snoozeItems, focusMenu, focusLabel, setMode, syncAll, newTask });
  const navW = load('navWidth2', null);
  if (navW) document.documentElement.style.setProperty('--nav-w', navW + 'px');
  setupNavSplitter();
  setupShortcuts();
  setupEvents();
  try {
    await loadConfig();
  } catch (e) {
    reportError(e);
    state.config = { clientId: '', tenant: '', syncIntervalSecs: 120, loadRemoteImages: false, readingPane: 'right', markReadDelaySecs: 2, listMode: 'comfortable', showDateSeparators: true, snoozeOptions: [], demo: false };
  }
  mail.applyReadingPaneLayout();
  try { await loadAll(); } catch (e) { reportError(e); }
  const m = load('mode', 'mail');
  setMode(['mail', 'calendar', 'contacts'].includes(m) ? m : 'mail');
  await selectInitialFolder().catch(reportError);
  try { const f = await call('get_focus', {}, { silent: true }); applyFocus(f && f.until); } catch { /* older backend */ }
  mail.refreshTodayCount();
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
