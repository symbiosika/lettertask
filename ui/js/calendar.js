// Calendar module of the main window: date navigator, calendar list, day/week/month views.
import { call, openWindow } from './api.js';
import {
  h, esc, load, store, startOfDay, addDays, addMinutes, sameDay, startOfWeek, startOfMonth, fmtTime, fmtDateLong,
  WEEKDAYS, WEEKDAYS_SHORT, MONTHS, toLocalIso, pad, displayName,
} from './util.js';
import { icon } from './icons.js';
import { modal, reportError, toast, confirmDialog, showMenu } from './dialogs.js';
import { categoryHex, textOn } from './categories.js';
import { state } from './state.js';

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
};
const els = {};
let onStatus = () => {};

export function initCalendar({ navHost, contentHost, onCounts }) {
  onStatus = onCounts || onStatus;
  els.nav = navHost;
  els.view = h('div.calview.hidden#calview');
  els.body = h('div.cal-body');
  els.view.append(els.body);
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
  await Promise.all(state.accounts.map(async (a) => {
    try { cal.calendars.set(a.id, (await call('list_calendars', { accountId: a.id }, { silent: true })) || []); }
    catch (e) { cal.calendars.set(a.id, []); console.warn(e); }
  }));
  for (const id of [...cal.calendars.keys()]) if (!state.accounts.some((a) => a.id === id)) cal.calendars.delete(id);
}

function calendarById(accountId, id) { return (cal.calendars.get(accountId) || []).find((c) => c.id === id) || null; }

function range() {
  const d = cal.date;
  if (cal.view === 'day') return { start: d, end: addDays(d, 1), days: 1 };
  if (cal.view === 'workweek') { const s = startOfWeek(d); return { start: s, end: addDays(s, 5), days: 5 }; }
  if (cal.view === 'week') { const s = startOfWeek(d); return { start: s, end: addDays(s, 7), days: 7 }; }
  const first = startOfMonth(d);
  const s = startOfWeek(first);
  return { start: s, end: addDays(s, 42), days: 42 };
}

export async function refresh() {
  const token = ++cal.loadToken;
  if (!cal.calendars.size) await loadCalendars();
  const { start, end } = range();
  const results = await Promise.all(state.accounts.map(async (a) => {
    const ids = (cal.calendars.get(a.id) || []).filter((c) => !cal.hidden.has(c.id)).map((c) => c.id);
    if (!ids.length) return [];
    try {
      return (await call('list_events', { accountId: a.id, calendarIds: ids, start: start.toISOString(), end: end.toISOString() }, { silent: true })) || [];
    } catch (e) { reportError(e); return []; }
  }));
  if (token !== cal.loadToken) return;
  cal.events = results.flat();
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
  for (const a of state.accounts) {
    const sec = h('div.cal-list');
    sec.appendChild(h('div.tree-root.static', h('span.tr-label', 'Meine Kalender'), h('span.tr-sub', a.email)));
    for (const c of cal.calendars.get(a.id) || []) {
      const color = c.color || '#2f6fe4';
      const cb = h('input.cal-check', { type: 'checkbox', checked: !cal.hidden.has(c.id) });
      cb.style.setProperty('--cc', color);
      cb.addEventListener('change', () => {
        if (cb.checked) cal.hidden.delete(c.id); else cal.hidden.add(c.id);
        store('hiddenCalendars', [...cal.hidden]);
        refresh().catch(reportError);
      });
      sec.appendChild(h('label.cal-item', { title: c.name }, cb, h('span.cal-name', c.name), c.canEdit ? null : h('span.cal-ro', { html: icon('lock', 13), title: 'Schreibgeschützt' })));
    }
    els.nav.appendChild(sec);
  }
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
    dataset: { id: ev.id, accountId: ev.accountId }, title: `${eventTitle(ev)}${ev.location ? '\n' + ev.location : ''}\n${ev.isAllDay ? 'Ganztägig' : fmtTime(s) + ' – ' + fmtTime(e)}`,
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

// ------------------------------------------------------------- selection / peek
function selectEvent(ev, el, peek = true) {
  cal.selectedId = ev.id;
  els.body.querySelectorAll('.ev.selected').forEach((x) => x.classList.remove('selected'));
  els.body.querySelectorAll(`.ev[data-id="${CSS.escape(ev.id)}"]`).forEach((x) => x.classList.add('selected'));
  if (peek) showPeek(ev, el);
}

let peekEl = null;
function closePeek() { if (peekEl) { peekEl.remove(); peekEl = null; } }
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

function showPeek(ev, anchor) {
  closePeek();
  const color = evColor(ev);
  const p = h('div.peek', { role: 'dialog', 'aria-label': 'Termindetails' });
  p.appendChild(h('div.peek-bar', { style: { background: color } }));
  p.appendChild(h('div.peek-title', eventTitle(ev)));
  p.appendChild(h('div.peek-row', { html: icon('clock', 14) + ' ' + esc(eventTimeText(ev)) }));
  if (ev.location) p.appendChild(h('div.peek-row', { html: icon('location', 14) + ' ' + esc(ev.location) }));
  if (ev.isOnlineMeeting && ev.onlineMeetingUrl) {
    const a = h('a.peek-link', { href: '#', html: icon('teams', 14) + ' An Microsoft Teams-Besprechung teilnehmen' });
    a.addEventListener('click', (e) => { e.preventDefault(); import('./api.js').then((m) => m.openUrl(ev.onlineMeetingUrl)); });
    p.appendChild(h('div.peek-row', a));
  }
  if (ev.organizer) p.appendChild(h('div.peek-row', { html: icon('user', 14) + ' Organisator: ' + esc(displayName(ev.organizer)) }));
  if (ev.attendees && ev.attendees.length) {
    const att = h('div.peek-att');
    for (const a of ev.attendees.slice(0, 8)) att.appendChild(h('div', { html: `<span class="resp ${esc(a.response)}"></span>${esc(displayName(a))} <span class="dim">– ${esc(RESPONSE_TEXT[a.response] || '')}${a.type === 'optional' ? ', optional' : ''}</span>` }));
    if (ev.attendees.length > 8) att.appendChild(h('div.dim', `… und ${ev.attendees.length - 8} weitere`));
    p.appendChild(att);
  }
  if (ev.categories && ev.categories.length) p.appendChild(h('div.peek-row', { html: ev.categories.map((c) => { const bg = categoryHex(ev.accountId, c); return `<span class="cat-chip" style="background:${bg};color:${textOn(bg)}">${esc(c)}</span>`; }).join(' ') }));
  if (!ev.isOrganizer && ev.attendees && ev.attendees.length) {
    p.appendChild(h('div.peek-row.dim', `Ihre Antwort: ${RESPONSE_TEXT[ev.responseStatus] || '–'} · Anzeigen als: ${SHOWAS_TEXT[ev.showAs] || ''}`));
  }
  const actions = h('div.peek-actions');
  if (!ev.isOrganizer && !ev.isCancelled) {
    actions.append(
      h('button.btn.small', { html: icon('accept', 12) + ' Annehmen', onclick: () => respond(ev, 'accept') }),
      h('button.btn.small', { html: icon('tentative', 12) + ' Vorbehalt', onclick: () => respond(ev, 'tentativelyAccept') }),
      h('button.btn.small', { html: icon('decline', 12) + ' Ablehnen', onclick: () => respond(ev, 'decline') }),
    );
  }
  actions.append(h('span.spacer'),
    h('button.btn.small', { onclick: () => { closePeek(); openEvent(ev); } }, 'Öffnen'),
    h('button.btn.small', { onclick: () => { closePeek(); deleteEvent(ev).catch(reportError); } }, 'Löschen'));
  p.appendChild(actions);
  document.body.appendChild(p);
  const r = anchor.getBoundingClientRect();
  const pr = p.getBoundingClientRect();
  let left = r.right + 8;
  if (left + pr.width > innerWidth - 8) left = Math.max(8, r.left - pr.width - 8);
  let top = Math.min(r.top, innerHeight - pr.height - 30);
  p.style.left = left + 'px';
  p.style.top = Math.max(8, top) + 'px';
  peekEl = p;
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
  openWindow('event', { accountId: ev.accountId, eventId: ev.id }).catch(reportError);
}

function defaultCalendar() {
  for (const a of state.accounts) {
    const list = (cal.calendars.get(a.id) || []).filter((c) => c.canEdit);
    const c = list.find((x) => x.isDefault) || list[0];
    if (c) return { accountId: a.id, calendarId: c.id };
  }
  return null;
}

export function newEvent(opts = {}) {
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
