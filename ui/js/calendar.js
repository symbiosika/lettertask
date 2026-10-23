// Calendar module of the main window: date navigator, calendar list, day/week/month views
// and the "14 Tage" overview (appointments and due tasks/Wiedervorlagen per day).
import { call, openWindow } from './api.js';
import {
  h, esc, load, store, startOfDay, addDays, addMinutes, sameDay, startOfWeek, startOfMonth, fmtTime, fmtDateLong,
  WEEKDAYS, WEEKDAYS_SHORT, MONTHS, toLocalIso, pad, displayName, initials, avatarColor,
} from './util.js';
import { icon } from './icons.js';
import { modal, reportError, toast, confirmDialog, showMenu, busyClick } from './dialogs.js';
import { categoryHex, textOn } from './categories.js';
import { state } from './state.js';
import { isImap, accountLabel } from './accounts.js';
import { fmtSnoozeTarget, snoozeOptions, computeSnooze } from './snooze.js';
import { dateTimeField } from './due.js';
import { recipientField } from './recipients.js';

/** Accounts with a calendar (IMAP accounts have none). */
function calAccounts() { return state.accounts.filter((a) => !isImap(a)); }

const SLOT_PX = 24; // per 30 minutes
const DAY_START_WORK = 8, DAY_END_WORK = 17;

const cal = {
  view: load('calView', 'workweek'),
  date: startOfDay(new Date()),
  navMonth: startOfMonth(new Date()),
  calendars: new Map(), // accountId -> Calendar[]
  hidden: new Set(load('hiddenCalendars', [])),
  events: [],
  selectedId: null,
  slot: null, // {start: Date, end: Date}
  loadToken: 0,
  tasks: [], // "14 Tage": due mails (MessageSummary) in range, overdue ones included
  sprint: { events: load('sprintEvents', true), tasks: load('sprintTasks', true) },
};
const SPRINT_DAYS = 14;
const els = {};
let onStatus = () => {};

export function initCalendar({ navHost, contentHost, onCounts }) {
  onStatus = onCounts || onStatus;
  els.nav = navHost;
  els.view = h('div.calview.hidden#calview');
  els.body = h('div.cal-body');
  els.loading = h('div.view-loading', { dataset: { testid: 'cal-loading' }, role: 'status' }, h('span.busy.on'), 'Termine werden geladen…');
  els.view.append(els.body, els.loading);
  contentHost.appendChild(els.view);
  setInterval(() => { if (!els.view.classList.contains('hidden')) paintNowLine(); }, 60000);
}

export function showCalendarView(show) {
  els.view.classList.toggle('hidden', !show);
  if (show) { renderNav(); refresh().catch(reportError); }
}

export function getView() { return cal.view; }
export function periodTitle() { return headerTitle(); }

// ------------------------------------------------------------- data
export async function loadCalendars() {
  // Last known list first (instant); the server list follows in the background.
  await Promise.all(calAccounts().map(async (a) => {
    try { cal.calendars.set(a.id, (await call('list_calendars', { accountId: a.id, cached: true }, { silent: true })) || []); }
    catch (e) { cal.calendars.set(a.id, []); console.warn(e); }
  }));
  for (const id of [...cal.calendars.keys()]) if (!calAccounts().some((a) => a.id === id)) cal.calendars.delete(id);
  refreshCalendarList();
}

let calListRefresh = null;
function refreshCalendarList() {
  if (calListRefresh) return;
  const before = JSON.stringify([...cal.calendars.entries()]);
  calListRefresh = Promise.all(calAccounts().map(async (a) => {
    try { cal.calendars.set(a.id, (await call('list_calendars', { accountId: a.id }, { silent: true })) || []); }
    catch (e) { console.warn(e); }
  })).then(() => {
    if (JSON.stringify([...cal.calendars.entries()]) === before) return;
    renderNav();
    if (!els.view.classList.contains('hidden')) refresh().catch(reportError);
  }).finally(() => { calListRefresh = null; });
}

function calendarById(accountId, id) { return (cal.calendars.get(accountId) || []).find((c) => c.id === id) || null; }

function range() {
  const d = cal.date;
  if (cal.view === 'day') return { start: d, end: addDays(d, 1), days: 1 };
  if (cal.view === 'workweek') { const s = startOfWeek(d); return { start: s, end: addDays(s, 5), days: 5 }; }
  if (cal.view === 'week') { const s = startOfWeek(d); return { start: s, end: addDays(s, 7), days: 7 }; }
  if (cal.view === 'sprint') return { start: d, end: addDays(d, SPRINT_DAYS), days: SPRINT_DAYS };
  const first = startOfMonth(d);
  const s = startOfWeek(first);
  return { start: s, end: addDays(s, 42), days: 42 };
}

export async function refresh() {
  const token = ++cal.loadToken;
  // Pill over the grid while the events of the shown range load (only if it takes a moment).
  const slow = setTimeout(() => { if (token === cal.loadToken) els.loading.classList.add('on'); }, 200);
  try { await loadRange(token); } finally {
    clearTimeout(slow);
    if (token === cal.loadToken) els.loading.classList.remove('on');
  }
}

async function loadRange(token) {
  if (!cal.calendars.size) await loadCalendars();
  const { start, end } = range();
  const sprint = cal.view === 'sprint';
  const wantEvents = !sprint || cal.sprint.events;
  const tasksP = sprint && cal.sprint.tasks
    ? call('list_due', { end: end.toISOString() }, { silent: true }).catch((e) => { console.warn(e); return []; })
    : Promise.resolve([]);
  const results = await Promise.all(calAccounts().map(async (a) => {
    if (!wantEvents) return [];
    const ids = (cal.calendars.get(a.id) || []).filter((c) => !cal.hidden.has(c.id)).map((c) => c.id);
    if (!ids.length) return [];
    try {
      return (await call('list_events', { accountId: a.id, calendarIds: ids, start: start.toISOString(), end: end.toISOString() }, { silent: true })) || [];
    } catch (e) { reportError(e); return []; }
  }));
  const tasks = await tasksP;
  if (token !== cal.loadToken) return;
  cal.events = results.flat();
  cal.tasks = tasks || [];
  render();
  renderNav();
}

export async function onCalendarChanged() {
  if (els.view.classList.contains('hidden')) return;
  await refresh();
}
export async function onAccountsChanged() {
  cal.calendars.clear();
  if (!els.view.classList.contains('hidden')) { await loadCalendars(); renderNav(); await refresh(); }
}

// ------------------------------------------------------------- navigation
export function setView(v) { cal.view = v; store('calView', v); onStatus(); refresh().catch(reportError); }
export function goToday() { cal.date = startOfDay(new Date()); cal.navMonth = startOfMonth(cal.date); refresh().catch(reportError); }
export function step(dir) {
  const v = cal.view;
  if (v === 'day') cal.date = addDays(cal.date, dir);
  else if (v === 'sprint') cal.date = addDays(cal.date, SPRINT_DAYS * dir);
  else if (v === 'month') cal.date = new Date(cal.date.getFullYear(), cal.date.getMonth() + dir, 1);
  else cal.date = addDays(cal.date, 7 * dir);
  cal.navMonth = startOfMonth(cal.date);
  onStatus();
  refresh().catch(reportError);
}
export function goTo(date) { cal.date = startOfDay(date); cal.navMonth = startOfMonth(date); refresh().catch(reportError); }

// ------------------------------------------------------------- left pane
export function renderNav() {
  if (!els.nav) return;
  els.nav.innerHTML = '';
  els.nav.appendChild(miniCalendar());
  if (!calAccounts().length) {
    els.nav.appendChild(h('p.tree-hint.cal-none', { dataset: { testid: 'cal-none' } }, 'Kalender ist nur für Microsoft-365-Konten verfügbar.'));
  }
  for (const a of calAccounts()) {
    const sec = h('div.cal-list');
    sec.appendChild(h('div.tree-root.static', h('span.tr-label', 'Meine Kalender'), h('span.tr-sub', accountLabel(a))));
    // Outlook calendar groups, e.g. "Kalender von Personen" (calendars of colleagues).
    let group = null;
    for (const c of cal.calendars.get(a.id) || []) {
      if ((c.group || null) !== group) {
        group = c.group || null;
        if (group) sec.appendChild(h('div.cal-group', { dataset: { testid: 'cal-group' } }, group));
      }
      const color = c.color || '#2f6fe4';
      const cb = h('input.cal-check', { type: 'checkbox', checked: !cal.hidden.has(c.id) });
      cb.style.setProperty('--cc', color);
      cb.addEventListener('change', () => {
        if (cb.checked) cal.hidden.delete(c.id); else cal.hidden.add(c.id);
        store('hiddenCalendars', [...cal.hidden]);
        refresh().catch(reportError);
      });
      const who = c.owner && c.owner.address ? `${c.name}\n${c.owner.address}` : c.name;
      const item = h('label.cal-item', { title: who, dataset: { testid: 'cal-item' } }, cb, h('span.cal-name', c.name), c.canEdit ? null : h('span.cal-ro', { html: icon('lock', 13), title: 'Schreibgeschützt' }));
      if (c.id.startsWith('user:')) {
        item.addEventListener('contextmenu', (e) => {
          e.preventDefault();
          showMenu([{ label: 'Kalender entfernen', icon: 'trash', onClick: () => removePeopleCalendar(a.id, c.id) }], e.clientX, e.clientY);
        });
      }
      sec.appendChild(item);
    }
    sec.appendChild(h('button.cal-add', { type: 'button', dataset: { testid: 'cal-add-person' }, onclick: () => addPeopleCalendar(a) },
      h('span', { html: icon('plus', 13) }), 'Kalender einer Person hinzufügen…'));
    els.nav.appendChild(sec);
  }
}

/** Adds a colleague's calendar (read via their mailbox, else free/busy). */
async function addPeopleCalendar(acc) {
  const mb = (acc.mailboxes || [])[0];
  const field = recipientField({ mailboxId: () => (mb ? mb.id : null), name: 'Person', placeholder: 'Name oder E-Mail-Adresse' });
  const res = await modal({
    title: 'Kalender einer Person hinzufügen', width: 420,
    body: h('div', h('label.lbl', 'Person aus Ihrer Organisation:'), field.el,
      h('p.note', 'Ohne Freigabe sehen Sie nur, wann die Person gebucht ist.')),
    buttons: [{ label: 'Hinzufügen', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: (v) => {
      if (v !== 'ok') return v;
      const list = field.get().filter((x) => !x.invalid);
      if (!list.length) { toast('Bitte eine Person auswählen.', 'error'); return false; }
      return list;
    },
  });
  if (!Array.isArray(res)) return;
  try {
    let cals = null;
    for (const p of res) cals = await call('add_people_calendar', { accountId: acc.id, address: p.address, name: p.name || '' });
    if (cals) cal.calendars.set(acc.id, cals);
    renderNav();
    await refresh();
  } catch (e) { reportError(e); }
}

async function removePeopleCalendar(accountId, calendarId) {
  try {
    cal.calendars.set(accountId, await call('remove_people_calendar', { accountId, calendarId }));
    renderNav();
    await refresh();
  } catch (e) { reportError(e); }
}

function miniCalendar() {
  const m = cal.navMonth;
  const wrap = h('div.minical');
  const head = h('div.minical-head',
    h('button.mc-nav', { title: 'Vorheriger Monat', html: icon('chevronLeft', 12), onclick: () => { cal.navMonth = new Date(m.getFullYear(), m.getMonth() - 1, 1); renderNav(); } }),
    h('span.mc-title', `${MONTHS[m.getMonth()]} ${m.getFullYear()}`),
    h('button.mc-nav', { title: 'Nächster Monat', html: icon('chevronRight', 12), onclick: () => { cal.navMonth = new Date(m.getFullYear(), m.getMonth() + 1, 1); renderNav(); } }));
  wrap.appendChild(head);
  const grid = h('div.minical-grid');
  for (const wd of ['Mo', 'Di', 'Mi', 'Do', 'Fr', 'Sa', 'So']) grid.appendChild(h('div.mc-wd', wd));
  const { start: rs, end: re } = range();
  const s = startOfWeek(m);
  const today = startOfDay(new Date());
  for (let i = 0; i < 42; i++) {
    const d = addDays(s, i);
    const inRange = cal.view === 'month' ? d.getMonth() === cal.date.getMonth() && d.getFullYear() === cal.date.getFullYear() : d >= rs && d < re;
    const cls = 'div.mc-day' + (d.getMonth() !== m.getMonth() ? '.other' : '') + (sameDay(d, today) ? '.today' : '') + (inRange ? '.inrange' : '');
    const cell = h(cls, { dataset: { date: toLocalIso(d).slice(0, 10) } }, String(d.getDate()));
    cell.addEventListener('click', () => goTo(d));
    grid.appendChild(cell);
  }
  wrap.appendChild(grid);
  return wrap;
}

// ------------------------------------------------------------- helpers
function evColor(ev) {
  if (ev.categories && ev.categories.length) return categoryHex(ev.accountId, ev.categories[0]);
  const c = calendarById(ev.accountId, ev.calendarId);
  return (c && c.color) || '#9db7e8';
}
function shade(hex, f) {
  const n = parseInt(hex.replace('#', ''), 16);
  const r = Math.round(((n >> 16) & 255) * f), g = Math.round(((n >> 8) & 255) * f), b = Math.round((n & 255) * f);
  return `rgb(${Math.min(255, r)},${Math.min(255, g)},${Math.min(255, b)})`;
}
function lighten(hex, amt) {
  const n = parseInt(hex.replace('#', ''), 16);
  const mix = (v) => Math.round(v + (255 - v) * amt);
  return `rgb(${mix((n >> 16) & 255)},${mix((n >> 8) & 255)},${mix(n & 255)})`;
}

/** Local day range for all-day events: [startDay, endDayExclusive). */
export function allDayRange(ev) {
  const toDay = (iso) => {
    const d = new Date(iso);
    if (d.getUTCHours() === 0 && d.getUTCMinutes() === 0) return new Date(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
    const local = new Date(d.getTime() + 12 * 3600000); // round to nearest local day
    return startOfDay(local);
  };
  const s = toDay(ev.start);
  let e = toDay(ev.end);
  if (e <= s) e = addDays(s, 1);
  return { start: s, end: e };
}

function isAllDayLike(ev) {
  if (ev.isAllDay) return true;
  return new Date(ev.end) - new Date(ev.start) >= 24 * 3600000;
}

function eventTitle(ev) { return ev.subject || '(Kein Betreff)'; }

// ------------------------------------------------------------- shared with the mail panel
/** Events of all visible calendars in [start, end) (same calendars as the calendar view). */
export async function eventsBetween(start, end) {
  if (!cal.calendars.size) await loadCalendars();
  const res = await Promise.all(calAccounts().map(async (a) => {
    const ids = (cal.calendars.get(a.id) || []).filter((c) => !cal.hidden.has(c.id)).map((c) => c.id);
    if (!ids.length) return [];
    try { return (await call('list_events', { accountId: a.id, calendarIds: ids, start: start.toISOString(), end: end.toISOString() }, { silent: true })) || []; }
    catch (e) { console.warn(e); return []; }
  }));
  return res.flat();
}
/** Card colours of an event, as in the calendar view. */
export function eventColors(ev) {
  const color = evColor(ev);
  return { background: ev.showAs === 'free' ? '#fff' : lighten(color, 0.55), bar: shade(color, 0.78) };
}
export { isAllDayLike, eventTitle };

function headerTitle() {
  const { start, end } = range();
  if (cal.view === 'day') return fmtDateLong(cal.date);
  if (cal.view === 'month') return `${MONTHS[cal.date.getMonth()]} ${cal.date.getFullYear()}`;
  const last = addDays(end, -1);
  if (start.getMonth() === last.getMonth()) return `${start.getDate()}. – ${last.getDate()}. ${MONTHS[start.getMonth()]} ${start.getFullYear()}`;
  return `${start.getDate()}. ${MONTHS[start.getMonth()]} – ${last.getDate()}. ${MONTHS[last.getMonth()]} ${last.getFullYear()}`;
}

// ------------------------------------------------------------- render
function render() {
  closePeek();
  if (cal.view === 'sprint') { renderSprint(); onStatus(); return; }
  if (!calAccounts().length) {
    els.body.innerHTML = '';
    els.body.appendChild(h('div.empty.cal-empty', h('span.empty-ic', { html: icon('calendar', 48) }),
      h('div', { style: { fontWeight: '600', color: 'var(--text-2)' } }, 'Kein Kalender verfügbar'),
      h('div', 'Kalender ist nur für Microsoft-365-Konten verfügbar. IMAP-Konten unterstützen nur E-Mail.')));
    onStatus();
    return;
  }
  if (cal.view === 'month') renderMonth(); else renderTimeGrid();
  onStatus();
}

function renderTimeGrid() {
  const { start, days } = range();
  const dayList = Array.from({ length: days }, (_, i) => addDays(start, i));
  const prevScroll = els.body.querySelector('.tg-scroll') ? els.body.querySelector('.tg-scroll').scrollTop : null;
  els.body.innerHTML = '';
  const grid = h('div.timegrid', { style: { '--days': days } });

  // header
  const head = h('div.tg-head', h('div.tg-gutter'));
  const today = startOfDay(new Date());
  for (const d of dayList) {
    head.appendChild(h('div.tg-dayhead' + (sameDay(d, today) ? '.today' : ''), { onclick: () => { cal.date = d; setView('day'); } },
      h('span.dnum', String(d.getDate())), h('span.dname', WEEKDAYS[d.getDay()])));
  }
  grid.appendChild(head);

  // all-day band
  const allDay = cal.events.filter(isAllDayLike);
  const band = h('div.tg-allday', h('div.tg-gutter'));
  const bandInner = h('div.tg-allday-inner');
  const lanes = [];
  for (const ev of allDay.sort((a, b) => new Date(a.start) - new Date(b.start))) {
    const r = ev.isAllDay ? allDayRange(ev) : { start: startOfDay(new Date(ev.start)), end: addDays(startOfDay(new Date(new Date(ev.end) - 1)), 1) };
    let si = Math.round((r.start - start) / 86400000), ei = Math.round((r.end - start) / 86400000);
    if (ei <= 0 || si >= days) continue;
    si = Math.max(0, si); ei = Math.min(days, ei);
    let lane = lanes.findIndex((l) => l.every(([a, b]) => ei <= a || si >= b));
    if (lane < 0) { lane = lanes.length; lanes.push([]); }
    lanes[lane].push([si, ei]);
    const el = eventBlock(ev, 'allday');
    Object.assign(el.style, { left: `calc(${si} * 100% / ${days} + 2px)`, width: `calc(${ei - si} * 100% / ${days} - 4px)`, top: lane * 20 + 2 + 'px' });
    bandInner.appendChild(el);
  }
  bandInner.style.height = Math.max(22, lanes.length * 20 + 4) + 'px';
  for (let i = 1; i < days; i++) bandInner.appendChild(h('div.tg-allday-sep', { style: { left: `calc(${i} * 100% / ${days})` } }));
  bandInner.addEventListener('dblclick', (e) => {
    if (e.target !== bandInner) return;
    const idx = Math.floor((e.offsetX / bandInner.clientWidth) * days);
    newEvent({ start: dayList[idx], end: addDays(dayList[idx], 1), allDay: true });
  });
  band.appendChild(bandInner);
  grid.appendChild(band);

  // body
  const scroll = h('div.tg-scroll');
  const body = h('div.tg-body', { style: { height: SLOT_PX * 48 + 'px' } });
  const gutter = h('div.tg-times');
  for (let hr = 0; hr < 24; hr++) gutter.appendChild(h('div.tg-time', { style: { height: SLOT_PX * 2 + 'px' } }, h('span.hh', String(hr).padStart(2, '0')), h('span.mm', '00')));
  body.appendChild(gutter);
  const cols = h('div.tg-cols');
  dayList.forEach((d) => {
    const col = h('div.tg-col' + (sameDay(d, today) ? '.today' : ''), { dataset: { date: toLocalIso(d).slice(0, 10) } });
    const wd = d.getDay();
    for (let s = 0; s < 48; s++) {
      const hour = s / 2;
      const work = wd >= 1 && wd <= 5 && hour >= DAY_START_WORK && hour < DAY_END_WORK;
      const slot = h('div.tg-slot' + (work ? '.work' : '') + (s % 2 ? '.half' : ''), { style: { height: SLOT_PX + 'px' }, dataset: { slot: s } });
      col.appendChild(slot);
    }
    col.addEventListener('mousedown', (e) => {
      if (e.target.closest('.ev')) return;
      const s = Math.floor(e.offsetY / SLOT_PX);
      if (!(e.target.classList.contains('tg-slot'))) return;
      selectSlot(d, Number(e.target.dataset.slot ?? s), col);
    });
    col.addEventListener('dblclick', (e) => {
      if (e.target.closest('.ev')) return;
      const s = Number(e.target.dataset.slot);
      if (Number.isNaN(s)) return;
      const st = addMinutes(d, s * 30);
      newEvent({ start: st, end: addMinutes(st, 30) });
    });
    col.addEventListener('contextmenu', (e) => {
      if (e.target.closest('.ev')) return;
      e.preventDefault();
      const s = Number(e.target.dataset.slot);
      if (Number.isNaN(s)) return;
      selectSlot(d, s, col);
      const st = addMinutes(d, s * 30);
      showMenu([
        { label: 'Neuer Termin', icon: 'calendarNew', onClick: () => newEvent({ start: st, end: addMinutes(st, 30) }) },
        { label: 'Neuer ganztägiger Termin', icon: 'calendar', onClick: () => newEvent({ start: d, end: addDays(d, 1), allDay: true }) },
        { separator: true },
        { label: 'Gehe zu Heute', icon: 'today', onClick: () => goToday() },
      ], e.clientX, e.clientY);
    });
    // timed events of this day
    const dayStart = d, dayEnd = addDays(d, 1);
    const segs = [];
    for (const ev of cal.events) {
      if (isAllDayLike(ev)) continue;
      const s = new Date(ev.start), e = new Date(ev.end);
      if (e <= dayStart || s >= dayEnd) continue;
      const ss = s < dayStart ? dayStart : s;
      const ee = e > dayEnd ? dayEnd : e;
      segs.push({ ev, s: (ss - dayStart) / 60000, e: Math.max((ee - dayStart) / 60000, (ss - dayStart) / 60000 + 15) });
    }
    layoutOverlaps(segs);
    for (const sg of segs) {
      const el = eventBlock(sg.ev, 'timed');
      Object.assign(el.style, {
        top: (sg.s / 30) * SLOT_PX + 'px', height: Math.max(SLOT_PX - 2, ((sg.e - sg.s) / 30) * SLOT_PX - 2) + 'px',
        left: `calc(${sg.col} * (100% - 8px) / ${sg.cols} + 1px)`, width: `calc((100% - 8px) / ${sg.cols} - 2px)`,
      });
      col.appendChild(el);
    }
    cols.appendChild(col);
  });
  body.appendChild(cols);
  scroll.appendChild(body);
  grid.appendChild(scroll);
  els.body.appendChild(grid);
  scroll.scrollTop = prevScroll !== null ? prevScroll : SLOT_PX * 2 * 7;
  paintNowLine();
  if (cal.slot) {
    const col = cols.querySelector(`.tg-col[data-date="${toLocalIso(cal.slot.start).slice(0, 10)}"]`);
    if (col) selectSlot(cal.slot.start, (cal.slot.start.getHours() * 60 + cal.slot.start.getMinutes()) / 30, col, true);
  }
}

function selectSlot(day, s, col, keep) {
  els.body.querySelectorAll('.tg-slot.sel').forEach((x) => x.classList.remove('sel'));
  const slot = col.querySelector(`.tg-slot[data-slot="${s}"]`);
  if (slot) slot.classList.add('sel');
  const st = addMinutes(startOfDay(day), s * 30);
  cal.slot = { start: st, end: addMinutes(st, 30) };
  if (!keep) { cal.selectedId = null; els.body.querySelectorAll('.ev.selected').forEach((x) => x.classList.remove('selected')); closePeek(); }
}

function layoutOverlaps(segs) {
  segs.sort((a, b) => a.s - b.s || b.e - a.e);
  let cluster = [], clusterEnd = -1;
  const flush = () => {
    const colsEnd = [];
    for (const sg of cluster) {
      let c = colsEnd.findIndex((end) => end <= sg.s);
      if (c < 0) { c = colsEnd.length; colsEnd.push(sg.e); } else colsEnd[c] = sg.e;
      sg.col = c;
    }
    for (const sg of cluster) sg.cols = colsEnd.length;
    cluster = [];
  };
  for (const sg of segs) {
    if (cluster.length && sg.s >= clusterEnd) { flush(); clusterEnd = -1; }
    cluster.push(sg);
    clusterEnd = Math.max(clusterEnd, sg.e);
  }
  if (cluster.length) flush();
}

function paintNowLine() {
  els.body.querySelectorAll('.now-line').forEach((x) => x.remove());
  const now = new Date();
  const col = els.body.querySelector(`.tg-col[data-date="${toLocalIso(now).slice(0, 10)}"]`);
  if (!col) return;
  const min = now.getHours() * 60 + now.getMinutes();
  col.appendChild(h('div.now-line', { style: { top: (min / 30) * SLOT_PX + 'px' } }));
}

function eventBlock(ev, kind) {
  const color = evColor(ev);
  const cls = 'div.ev.' + kind + (ev.id === cal.selectedId ? '.selected' : '') + (ev.showAs === 'tentative' ? '.tentative' : '') + (ev.showAs === 'free' ? '.free' : '') + (ev.isCancelled ? '.cancelled' : '') + (ev.responseStatus === 'notResponded' || ev.responseStatus === 'none' && !ev.isOrganizer ? '.unanswered' : '');
  const s = new Date(ev.start), e = new Date(ev.end);
  const el = h(cls, {
    dataset: { id: ev.id, accountId: ev.accountId }, 'aria-label': `${eventTitle(ev)}, ${ev.isAllDay ? 'ganztägig' : fmtTime(s) + ' – ' + fmtTime(e)}`,
    style: { background: ev.showAs === 'free' ? '#fff' : lighten(color, 0.55), color: '#1f2328', '--bar': shade(color, 0.78) },
  });
  const icons = (ev.recurrence || ev.type === 'occurrence' ? icon('recurrence', 11) : '') + (ev.isOnlineMeeting ? icon('teams', 11) : '') + (ev.sensitivity === 'private' ? icon('lock', 11) : '');
  if (kind === 'timed') {
    el.innerHTML = `<div class="ev-subj">${esc(eventTitle(ev))}</div>${ev.location ? `<div class="ev-loc">${esc(ev.location)}</div>` : ''}<div class="ev-icons">${icons}</div>`;
  } else if (kind === 'allday') {
    el.innerHTML = `<span class="ev-subj">${esc(eventTitle(ev))}</span>${ev.location ? ` <span class="ev-loc">(${esc(ev.location)})</span>` : ''}<span class="ev-icons">${icons}</span>`;
  } else {
    el.innerHTML = `${ev.isAllDay ? '' : `<span class="ev-time">${fmtTime(s)}</span> `}<span class="ev-subj">${esc(eventTitle(ev))}</span>`;
  }
  el.addEventListener('mousedown', (x) => x.stopPropagation());
  attachHover(el, ev);
  el.addEventListener('click', (x) => { x.stopPropagation(); selectEvent(ev, el); });
  el.addEventListener('dblclick', (x) => { x.stopPropagation(); openEvent(ev); });
  el.addEventListener('contextmenu', (x) => { x.preventDefault(); x.stopPropagation(); selectEvent(ev, el, false); eventContextMenu(ev, x.clientX, x.clientY); });
  return el;
}

function renderMonth() {
  const { start } = range();
  els.body.innerHTML = '';
  const grid = h('div.monthgrid');
  for (const wd of [1, 2, 3, 4, 5, 6, 0]) grid.appendChild(h('div.mg-wd', WEEKDAYS[wd]));
  const today = startOfDay(new Date());
  const m = cal.date.getMonth();
  for (let i = 0; i < 42; i++) {
    const d = addDays(start, i);
    const cell = h('div.mg-cell' + (d.getMonth() !== m ? '.other' : '') + (sameDay(d, today) ? '.today' : ''), { dataset: { date: toLocalIso(d).slice(0, 10) } });
    cell.appendChild(h('div.mg-date', d.getDate() === 1 ? `${d.getDate()}. ${MONTHS[d.getMonth()].slice(0, 3)}` : String(d.getDate())));
    const evs = cal.events.filter((ev) => {
      if (ev.isAllDay) { const r = allDayRange(ev); return r.start <= d && r.end > d; }
      const s = new Date(ev.start), e = new Date(ev.end);
      return s < addDays(d, 1) && e > d && !(e.getTime() === d.getTime());
    }).sort((a, b) => (b.isAllDay - a.isAllDay) || (new Date(a.start) - new Date(b.start)));
    const max = 4;
    evs.slice(0, evs.length > max ? max - 1 : max).forEach((ev) => cell.appendChild(eventBlock(ev, 'month')));
    if (evs.length > max) {
      cell.appendChild(h('div.mg-more', { onclick: (e) => { e.stopPropagation(); cal.date = d; setView('day'); } }, `+${evs.length - max + 1} weitere`));
    }
    cell.addEventListener('dblclick', (e) => {
      if (e.target.closest('.ev')) return;
      const st = new Date(d); st.setHours(9);
      newEvent({ start: st, end: addMinutes(st, 30) });
    });
    cell.addEventListener('click', (e) => { if (!e.target.closest('.ev')) { cal.slot = { start: new Date(d.getTime() + 9 * 3600000), end: new Date(d.getTime() + 9.5 * 3600000) }; grid.querySelectorAll('.mg-cell.sel').forEach((x) => x.classList.remove('sel')); cell.classList.add('sel'); closePeek(); } });
    grid.appendChild(cell);
  }
  els.body.appendChild(grid);
}

// ------------------------------------------------------------- "14 Tage"
export function sprintFilter() { return { ...cal.sprint }; }
export function setSprintFilter(key, on) {
  cal.sprint[key] = on;
  store(key === 'events' ? 'sprintEvents' : 'sprintTasks', on);
  refresh().catch(reportError);
}

function eventsOn(d) {
  return cal.events.filter((ev) => {
    if (ev.isAllDay) { const r = allDayRange(ev); return r.start <= d && r.end > d; }
    const s = new Date(ev.start), e = new Date(ev.end);
    return s < addDays(d, 1) && (e > d || s >= d);
  });
}

function taskItem(m, overdue) {
  const due = new Date(m.dueAt);
  const el = h('div.sp-item.sp-task' + (overdue ? '.overdue' : ''), {
    dataset: { id: m.id, testid: 'sprint-task' },
    title: `${m.subject || '(Kein Betreff)'}\n${overdue ? 'Überfällig seit' : 'Fällig'} ${fmtSnoozeTarget(due)}`,
  });
  el.innerHTML = `<span class="sp-time">${icon('snooze', 11)}${overdue ? 'überfällig' : fmtTime(due)}</span> <span class="sp-subj">${esc(m.subject || '(Kein Betreff)')}</span>`;
  el.addEventListener('dblclick', () => openWindow('message', { mailboxId: m.mailboxId, messageId: m.id, folderId: m.folderId }).catch(reportError));
  el.addEventListener('click', (x) => { x.stopPropagation(); els.body.querySelectorAll('.sp-item.selected').forEach((y) => y.classList.remove('selected')); el.classList.add('selected'); });
  el.addEventListener('contextmenu', (x) => { x.preventDefault(); x.stopPropagation(); el.click(); taskMenu(m, x.clientX, x.clientY); });
  return el;
}

/** Right-click on a due task: open it or move its due time (Wiedervorlage). */
function taskMenu(m, x, y) {
  const now = new Date();
  const moves = [];
  snoozeOptions(state.config).forEach((o, i) => {
    const d = computeSnooze(o.rule, now);
    if (d && d > now) moves.push({ label: o.label, shortcut: fmtSnoozeTarget(d), icon: i === 0 ? 'snooze' : null, onClick: () => moveTask(m, d) });
  });
  moves.push({ separator: true }, { label: 'Datum und Uhrzeit wählen…', icon: 'calendar', onClick: () => moveTaskCustom(m) });
  showMenu([
    { label: 'Öffnen', icon: 'open', onClick: () => openWindow('message', { mailboxId: m.mailboxId, messageId: m.id, folderId: m.folderId }).catch(reportError) },
    { separator: true },
    { label: 'Verschieben', icon: 'snooze', submenu: () => moves, testid: 'sprint-task-move' },
  ], x, y);
}

async function moveTask(m, until) {
  try {
    await call('snooze_messages', { mailboxId: m.mailboxId, ids: [m.id], until: until.toISOString() });
    toast(`Verschoben auf ${fmtSnoozeTarget(until)}.`);
    await refresh();
  } catch (e) { reportError(e); }
}

async function moveTaskCustom(m) {
  const def = m.dueAt && new Date(m.dueAt) > new Date() ? new Date(m.dueAt) : addDays(startOfDay(new Date()), 1);
  if (!m.dueAt || new Date(m.dueAt) <= new Date()) def.setHours(8);
  const input = dateTimeField(def, 'moveAt');
  const res = await modal({
    title: 'Aufgabe verschieben', width: 360,
    body: h('div', h('label.lbl', 'Neu fällig am:'), input.el),
    buttons: [{ label: 'Verschieben', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: () => {
      const d = input.value();
      if (!d || d <= new Date()) { input.el.classList.add('invalid'); toast('Bitte einen Zeitpunkt in der Zukunft wählen.', 'error'); return false; }
      return d;
    },
  });
  if (res instanceof Date) await moveTask(m, res);
}

function sprintEvent(ev, day) {
  const el = eventBlock(ev, 'sprint');
  const s = new Date(ev.start);
  if (!isAllDayLike(ev)) el.innerHTML = `<span class="sp-time">${sameDay(s, day) ? fmtTime(s) : '…'}</span> <span class="sp-subj">${esc(eventTitle(ev))}</span>`;
  return el;
}

function renderSprint() {
  const { start } = range();
  els.body.innerHTML = '';
  const today = startOfDay(new Date());
  const toggle = (key, label, testid) => {
    const on = cal.sprint[key];
    const b = h('button.chip.sp-filter' + (on ? '.on' : ''), { type: 'button', role: 'switch', 'aria-checked': on ? 'true' : 'false', dataset: { testid } },
      h('span.sp-check', { html: on ? icon('check', 12) : '' }), label);
    b.addEventListener('click', () => setSprintFilter(key, !on));
    return b;
  };
  const counts = h('span.dim.sp-count');
  const bar = h('div.sp-bar', h('span.sp-title', `${SPRINT_DAYS} Tage`), toggle('events', 'Termine', 'sprint-show-events'), toggle('tasks', 'Aufgaben & Wiedervorlagen', 'sprint-show-tasks'), h('span.spacer'), counts);
  const grid = h('div.sprintgrid', { dataset: { testid: 'sprint-grid' } });
  let nEv = 0, nTask = 0;
  for (let i = 0; i < SPRINT_DAYS; i++) {
    const d = addDays(start, i);
    const wd = d.getDay();
    const isToday = sameDay(d, today);
    const col = h('div.sp-day' + (wd === 0 || wd === 6 ? '.weekend' : '') + (isToday ? '.today' : '') + (d < today ? '.past' : ''), { dataset: { date: toLocalIso(d).slice(0, 10), testid: 'sprint-day' } });
    col.appendChild(h('div.sp-head', { onclick: () => { cal.date = d; setView('day'); } }, h('span.sp-wd', WEEKDAYS_SHORT[wd].toUpperCase()), h('span.sp-date', `${pad(d.getDate())}.${pad(d.getMonth() + 1)}.`)));
    const evs = cal.sprint.events ? eventsOn(d) : [];
    const allDay = evs.filter(isAllDayLike);
    // Timed appointments and tasks interleaved by time; overdue tasks lead today's column.
    const timed = evs.filter((ev) => !isAllDayLike(ev)).map((ev) => ({ t: Math.max(new Date(ev.start), d), el: () => sprintEvent(ev, d) }));
    const tasks = cal.sprint.tasks ? cal.tasks.filter((m) => {
      const due = new Date(m.dueAt);
      return sameDay(due, d) || (isToday && due < today);
    }) : [];
    for (const m of tasks) {
      const due = new Date(m.dueAt);
      const overdue = due < new Date();
      timed.push({ t: overdue ? new Date(0) : due, el: () => taskItem(m, overdue) });
    }
    timed.sort((a, b) => a.t - b.t);
    for (const ev of allDay) col.appendChild(sprintEvent(ev, d));
    for (const x of timed) col.appendChild(x.el());
    nEv += evs.length; nTask += tasks.length;
    col.addEventListener('dblclick', (e) => {
      if (e.target.closest('.ev,.sp-item,.sp-head')) return;
      const st = new Date(d); st.setHours(9);
      newEvent({ start: st, end: addMinutes(st, 30) });
    });
    grid.appendChild(col);
  }
  counts.textContent = [cal.sprint.events ? `${nEv} Termine` : '', cal.sprint.tasks ? `${nTask} Aufgaben` : ''].filter(Boolean).join(' · ');
  els.body.append(bar, grid);
}

// ------------------------------------------------------------- selection / peek
function selectEvent(ev, el, peek = true) {
  cal.selectedId = ev.id;
  els.body.querySelectorAll('.ev.selected').forEach((x) => x.classList.remove('selected'));
  els.body.querySelectorAll(`.ev[data-id="${CSS.escape(ev.id)}"]`).forEach((x) => x.classList.add('selected'));
  if (peek) showPeek(ev, el);
}

let peekEl = null;
function closePeek() { hideHover(true); if (peekEl) { peekEl.remove(); peekEl = null; } }
document.addEventListener('mousedown', (e) => { if (peekEl && !peekEl.contains(e.target) && !e.target.closest('.ev')) closePeek(); });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closePeek(); });

const RESPONSE_TEXT = { accepted: 'Zugesagt', tentativelyAccepted: 'Mit Vorbehalt', declined: 'Abgelehnt', organizer: 'Organisator', notResponded: 'Keine Antwort', none: 'Keine Antwort' };
const SHOWAS_TEXT = { free: 'Frei', tentative: 'Mit Vorbehalt', busy: 'Gebucht', oof: 'Abwesend', workingElsewhere: 'An anderem Ort tätig', unknown: '' };

export function eventTimeText(ev) {
  if (ev.isAllDay) {
    const r = allDayRange(ev);
    const last = addDays(r.end, -1);
    return sameDay(r.start, last) ? `${fmtDateLong(r.start)} (ganztägig)` : `${fmtDateLong(r.start)} – ${fmtDateLong(last)} (ganztägig)`;
  }
  const s = new Date(ev.start), e = new Date(ev.end);
  return sameDay(s, e) ? `${fmtDateLong(s)}, ${fmtTime(s)} – ${fmtTime(e)}` : `${fmtDateLong(s)} ${fmtTime(s)} – ${fmtDateLong(e)} ${fmtTime(e)}`;
}

const DAY_MS = 86400000;

/** "in 2 Std. 10 Min." / "läuft · noch 20 Min." / "morgen" / "vor 3 Tagen" relative to now. */
function relativeWhen(ev, now = new Date()) {
  const s = ev.isAllDay ? allDayRange(ev).start : new Date(ev.start);
  const e = ev.isAllDay ? allDayRange(ev).end : new Date(ev.end);
  const dur = (ms) => {
    const m = Math.round(ms / 60000);
    if (m < 60) return `${m} Min.`;
    const hh = Math.floor(m / 60), mm = m % 60;
    return mm ? `${hh} Std. ${mm} Min.` : `${hh} Std.`;
  };
  if (s <= now && e > now) return { text: ev.isAllDay ? 'heute' : `läuft · noch ${dur(e - now)}`, cls: 'now' };
  const days = Math.round((startOfDay(s) - startOfDay(now)) / DAY_MS);
  if (s > now) {
    if (days === 0) return { text: `in ${dur(s - now)}`, cls: s - now <= 30 * 60000 ? 'soon' : 'today' };
    if (days === 1) return { text: 'morgen', cls: 'later' };
    if (days < 7) return { text: `in ${days} Tagen`, cls: 'later' };
    return { text: `in ${Math.round(days / 7)} Wo.`, cls: 'later' };
  }
  const ago = -Math.round((startOfDay(e) - startOfDay(now)) / DAY_MS);
  return { text: ago <= 0 ? 'vorbei' : ago === 1 ? 'gestern' : `vor ${ago} Tagen`, cls: 'past' };
}

function durationText(ev) {
  if (ev.isAllDay) {
    const r = allDayRange(ev);
    const n = Math.round((r.end - r.start) / DAY_MS);
    return n > 1 ? `${n} Tage` : 'ganztägig';
  }
  const m = Math.round((new Date(ev.end) - new Date(ev.start)) / 60000);
  if (m < 60) return `${m} Min.`;
  return m % 60 ? `${Math.floor(m / 60)} Std. ${m % 60} Min.` : `${m / 60} Std.`;
}

function whenLine(ev) {
  const s = new Date(ev.start);
  if (ev.isAllDay) {
    const r = allDayRange(ev), last = addDays(r.end, -1);
    return sameDay(r.start, last) ? `${WEEKDAYS[r.start.getDay()]}, ${fmtDateShortLong(r.start)}` : `${fmtDateShortLong(r.start)} – ${fmtDateShortLong(last)}`;
  }
  const e = new Date(ev.end);
  return sameDay(s, e)
    ? `${WEEKDAYS[s.getDay()]}, ${fmtDateShortLong(s)} · ${fmtTime(s)} – ${fmtTime(e)}`
    : `${fmtDateShortLong(s)} ${fmtTime(s)} – ${fmtDateShortLong(e)} ${fmtTime(e)}`;
}
function fmtDateShortLong(d) { return `${d.getDate()}. ${MONTHS[d.getMonth()]}`; }

/** Teams/Zoom/Meet link: the online meeting url, else the first meeting link in location/preview. */
function joinUrl(ev) {
  if (ev.onlineMeetingUrl) return ev.onlineMeetingUrl;
  const m = /(https:\/\/(?:teams\.microsoft\.com|[\w.-]*zoom\.us|meet\.google\.com)\/\S+)/i.exec(`${ev.location || ''} ${ev.bodyPreview || ''}`);
  return m ? m[1] : null;
}

function avatar(a, cls = '') {
  const name = displayName(a);
  return `<span class="bc-av ${cls} ${esc(a.response || '')}" style="background:${avatarColor(a.address || name)}" title="${esc(name)} – ${esc(RESPONSE_TEXT[a.response] || '')}">${esc(initials(a.name, a.address))}</span>`;
}

/**
 * "Briefing card" for an appointment: when, where, how to join, who and what.
 * Shared by the hover card and the click peek (which adds the actions).
 */
function briefingCard(ev, { actions = false } = {}) {
  const color = evColor(ev);
  const calObj = calendarById(ev.accountId, ev.calendarId);
  const p = h('div.peek.briefing', { role: 'dialog', 'aria-label': 'Termindetails', dataset: { testid: 'briefing' } });
  p.style.setProperty('--bc', color);
  const when = relativeWhen(ev);
  const owner = calObj && calObj.owner ? calObj.owner.name || calObj.owner.address : null;
  p.appendChild(h('div.bc-head',
    h('span.bc-cal', h('span.bc-dot'), owner ? `Kalender von ${owner}` : calObj ? calObj.name : 'Kalender'),
    h('span.spacer'),
    h('span.bc-when.' + when.cls, { dataset: { testid: 'briefing-when' } }, when.text)));
  p.appendChild(h('div.peek-title' + (ev.isCancelled ? '.cancelled' : ''), (ev.isCancelled ? 'Abgesagt: ' : '') + eventTitle(ev)));

  const facts = h('div.bc-facts');
  const fact = (ic, html) => facts.appendChild(h('div.bc-fact', { html: icon(ic, 15) + `<span>${html}</span>` }));
  fact('clock', `${esc(whenLine(ev))} <span class="dim">· ${esc(durationText(ev))}</span>${ev.recurrence || ev.type === 'occurrence' ? ` <span class="bc-tag">${icon('recurrence', 11)} Serie</span>` : ''}`);
  const url = joinUrl(ev);
  const loc = ev.location && !/^Microsoft Teams/i.test(ev.location) ? ev.location : '';
  if (loc) fact('location', esc(loc));
  p.appendChild(facts);
  if (url) {
    const join = h('button.btn.primary.bc-join', { type: 'button', dataset: { testid: 'briefing-join' }, html: icon('teams', 15) + (/teams\.microsoft/i.test(url) || ev.isOnlineMeeting ? ' Teams-Besprechung beitreten' : ' Online-Besprechung beitreten') });
    join.addEventListener('click', () => import('./api.js').then((m) => m.openUrl(url)));
    p.appendChild(join);
  }

  const people = (ev.attendees || []).filter((a) => a.type !== 'resource');
  if (ev.organizer || people.length) {
    const sec = h('div.bc-sec');
    const count = (r) => people.filter((a) => a.response === r).length;
    const open = people.length - count('accepted') - count('declined') - count('tentativelyAccepted') - count('organizer');
    const parts = [count('accepted') && `${count('accepted')} zugesagt`, count('tentativelyAccepted') && `${count('tentativelyAccepted')} vielleicht`, count('declined') && `${count('declined')} abgesagt`, open > 0 && `${open} offen`].filter(Boolean);
    sec.appendChild(h('div.bc-label', people.length ? `${people.length + (ev.organizer && !people.some((a) => a.address === ev.organizer.address) ? 1 : 0)} Personen` : 'Organisator', parts.length ? h('span.dim', ' · ' + parts.join(' · ')) : null));
    const row = h('div.bc-people');
    if (ev.organizer) row.insertAdjacentHTML('beforeend', avatar({ ...ev.organizer, response: 'organizer' }, 'org'));
    const shown = people.filter((a) => !ev.organizer || a.address !== ev.organizer.address);
    row.insertAdjacentHTML('beforeend', shown.slice(0, 9).map((a) => avatar(a)).join(''));
    if (shown.length > 9) row.appendChild(h('span.bc-av.more', `+${shown.length - 9}`));
    if (ev.organizer) row.appendChild(h('span.bc-orgname', { html: `<b>${esc(displayName(ev.organizer))}</b> <span class="dim">organisiert</span>` }));
    sec.appendChild(row);
    if (actions && people.length) {
      const att = h('div.peek-att');
      for (const a of people.slice(0, 12)) att.appendChild(h('div', { html: `<span class="resp ${esc(a.response)}"></span>${esc(displayName(a))} <span class="dim">– ${esc(RESPONSE_TEXT[a.response] || '')}${a.type === 'optional' ? ', optional' : ''}</span>` }));
      if (people.length > 12) att.appendChild(h('div.dim', `… und ${people.length - 12} weitere`));
      sec.appendChild(att);
    }
    p.appendChild(sec);
  }

  const agenda = (ev.bodyPreview || '').replace(/_{5,}[\s\S]*$/, '').replace(/\s+/g, ' ').trim();
  if (agenda && ev.sensitivity !== 'private') {
    p.appendChild(h('div.bc-sec', h('div.bc-label', 'Worum geht’s'), h('div.bc-agenda', { dataset: { testid: 'briefing-agenda' } }, agenda)));
  }

  const meta = [];
  if (ev.categories && ev.categories.length) meta.push(...ev.categories.map((c) => { const bg = categoryHex(ev.accountId, c); return `<span class="cat-chip" style="background:${bg};color:${textOn(bg)}">${esc(c)}</span>`; }));
  if (!ev.isOrganizer && people.length && !owner) meta.push(`<span class="bc-tag">Ihre Antwort: ${esc(RESPONSE_TEXT[ev.responseStatus] || '–')}</span>`);
  if (ev.showAs && SHOWAS_TEXT[ev.showAs] && ev.showAs !== 'busy') meta.push(`<span class="bc-tag">${esc(SHOWAS_TEXT[ev.showAs])}</span>`);
  if (ev.importance === 'high') meta.push(`<span class="bc-tag imp">Wichtig</span>`);
  if (ev.sensitivity === 'private') meta.push(`<span class="bc-tag">${icon('lock', 11)} Privat</span>`);
  if (ev.isReminderOn && !ev.isAllDay && !owner) meta.push(`<span class="bc-tag">${icon('bell', 11)} ${ev.reminderMinutes} Min. vorher</span>`);
  if (meta.length) p.appendChild(h('div.bc-meta', { html: meta.join(' ') }));

  if (actions) {
    const bar = h('div.peek-actions');
    if (!ev.isOrganizer && !ev.isCancelled && people.length && !owner) {
      bar.append(
        h('button.btn.small', { html: icon('accept', 12) + ' Annehmen', onclick: busyClick(() => respond(ev, 'accept'), bar) }),
        h('button.btn.small', { html: icon('tentative', 12) + ' Vorbehalt', onclick: busyClick(() => respond(ev, 'tentativelyAccept'), bar) }),
        h('button.btn.small', { html: icon('decline', 12) + ' Ablehnen', onclick: busyClick(() => respond(ev, 'decline'), bar) }),
      );
    }
    bar.append(h('span.spacer'), h('button.btn.small', { onclick: () => { closePeek(); openEvent(ev); } }, 'Öffnen'));
    if (!calObj || calObj.canEdit) bar.append(h('button.btn.small', { onclick: () => { closePeek(); deleteEvent(ev).catch(reportError); } }, 'Löschen'));
    p.appendChild(bar);
  }
  return p;
}

function placeCard(p, anchor) {
  document.body.appendChild(p);
  const r = anchor.getBoundingClientRect();
  const pr = p.getBoundingClientRect();
  let left = r.right + 8;
  if (left + pr.width > innerWidth - 8) left = Math.max(8, r.left - pr.width - 8);
  const top = Math.min(r.top, innerHeight - pr.height - 12);
  p.style.left = left + 'px';
  p.style.top = Math.max(8, top) + 'px';
}

function showPeek(ev, anchor) {
  closePeek();
  hideHover(true);
  const p = briefingCard(ev, { actions: true });
  placeCard(p, anchor);
  peekEl = p;
}

// Hover: the same card without actions, after a short delay; it stays while the
// pointer is on the card (so "Beitreten" can be clicked).
let hoverEl = null, hoverTimer = 0, hoverHide = 0;
function hideHover(now = false) {
  clearTimeout(hoverTimer);
  clearTimeout(hoverHide);
  const go = () => { if (hoverEl) { hoverEl.remove(); hoverEl = null; } };
  if (now) go(); else hoverHide = setTimeout(go, 180);
}
function attachHover(el, ev) {
  el.addEventListener('mouseenter', () => {
    if (peekEl) return;
    clearTimeout(hoverHide);
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(() => {
      if (peekEl || !el.isConnected) return;
      hideHover(true);
      const p = briefingCard(ev);
      p.classList.add('hover');
      p.addEventListener('mouseenter', () => clearTimeout(hoverHide));
      p.addEventListener('mouseleave', () => hideHover());
      placeCard(p, el);
      hoverEl = p;
    }, 380);
  });
  el.addEventListener('mouseleave', () => hideHover());
  el.addEventListener('mousedown', () => hideHover(true));
}

async function respond(ev, response) {
  closePeek();
  try {
    await call('respond_event', { accountId: ev.accountId, eventId: ev.id, response, comment: '', sendResponse: true });
    toast('Antwort wurde gesendet.');
    await refresh();
  } catch (e) { reportError(e); }
}

function eventContextMenu(ev, x, y) {
  showMenu([
    { label: 'Öffnen', icon: 'open', onClick: () => openEvent(ev) },
    !ev.isOrganizer ? { separator: true } : null,
    !ev.isOrganizer ? { label: 'Annehmen', icon: 'accept', onClick: () => respond(ev, 'accept') } : null,
    !ev.isOrganizer ? { label: 'Mit Vorbehalt', icon: 'tentative', onClick: () => respond(ev, 'tentativelyAccept') } : null,
    !ev.isOrganizer ? { label: 'Ablehnen', icon: 'decline', onClick: () => respond(ev, 'decline') } : null,
    { separator: true },
    { label: 'Löschen', icon: 'delete', onClick: () => deleteEvent(ev) },
  ].filter(Boolean), x, y);
}

// ------------------------------------------------------------- actions
export function openEvent(ev) {
  if (String(ev.id).startsWith('fb:')) { toast('Für diesen Kalender sind nur die Frei/Gebucht-Zeiten freigegeben.'); return; }
  openWindow('event', { accountId: ev.accountId, eventId: ev.id }).catch(reportError);
}

function defaultCalendar() {
  for (const a of calAccounts()) {
    const list = (cal.calendars.get(a.id) || []).filter((c) => c.canEdit);
    const c = list.find((x) => x.isDefault) || list[0];
    if (c) return { accountId: a.id, calendarId: c.id };
  }
  return null;
}

export function newEvent(opts = {}) {
  if (!calAccounts().length) { toast('Termine sind nur mit einem Microsoft-365-Konto möglich.', 'error'); return; }
  if (!cal.calendars.size) {
    loadCalendars().then(() => { if (cal.calendars.size) newEvent(opts); else toast('Kein bearbeitbarer Kalender vorhanden.', 'error'); }).catch(reportError);
    return;
  }
  const target = defaultCalendar();
  if (!target) { toast('Kein bearbeitbarer Kalender vorhanden.', 'error'); return; }
  let start = opts.start, end = opts.end;
  if (!start) {
    if (cal.slot) { start = cal.slot.start; end = cal.slot.end; }
    else {
      const now = new Date();
      start = new Date(cal.date); start.setHours(now.getHours() + 1, 0, 0, 0);
      end = addMinutes(start, 30);
    }
  }
  openWindow('event', { ...target, start: toLocalIso(start), end: toLocalIso(end || addMinutes(start, 30)), allDay: opts.allDay ? '1' : null }).catch(reportError);
}

export async function deleteSelected() {
  const ev = cal.events.find((e) => e.id === cal.selectedId);
  if (!ev) { toast('Bitte wählen Sie zuerst einen Termin aus.'); return; }
  await deleteEvent(ev);
}

async function deleteEvent(ev) {
  let targetId = ev.id;
  if (ev.type === 'occurrence' && ev.seriesMasterId) {
    const choice = await modal({
      title: 'Serienelement löschen', width: 400,
      body: h('div', h('p', { style: { marginTop: 0 } }, `„${eventTitle(ev)}" ist ein Serientermin. Was möchten Sie löschen?`),
        h('label.checkrow', h('input', { type: 'radio', name: 'delocc', value: 'one', checked: true }), 'Nur dieses Vorkommen'),
        h('label.checkrow', h('input', { type: 'radio', name: 'delocc', value: 'series' }), 'Die gesamte Serie')),
      buttons: [{ label: 'OK', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
      onBeforeClose: () => document.querySelector('input[name=delocc]:checked').value,
    });
    if (!choice) return;
    if (choice === 'series') targetId = ev.seriesMasterId;
  } else {
    const hasAtt = ev.isOrganizer && ev.attendees && ev.attendees.length;
    const ok = await confirmDialog(hasAtt ? `Die Besprechung „${eventTitle(ev)}" wird abgesagt und die Teilnehmer werden benachrichtigt. Fortfahren?` : `Möchten Sie „${eventTitle(ev)}" löschen?`, { danger: true, ok: hasAtt ? 'Absage senden' : 'Löschen' });
    if (!ok) return;
  }
  await call('delete_event', { accountId: ev.accountId, eventId: targetId });
  cal.selectedId = null;
  await refresh();
}

export function stats() {
  const { start, end } = range();
  return { count: cal.events.length, start, end };
}

export function viewRange() { return range(); }

export { pad, WEEKDAYS_SHORT };
