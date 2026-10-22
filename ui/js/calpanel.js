// "Kalender unten": a 7-day week strip (07–20 Uhr) below message list and
// reading pane, to answer "hast du da Zeit?" without leaving the mail.
import { h, esc, load, store, startOfDay, addDays, addMinutes, sameDay, fmtTime, WEEKDAYS_SHORT, pad } from './util.js';
import { icon } from './icons.js';
import { reportError } from './dialogs.js';
import { eventsBetween, eventColors, isAllDayLike, eventTitle, openEvent, newEvent } from './calendar.js';

const FIRST_HOUR = 7;
const LAST_HOUR = 20; // grid ends at 20:00
const DAYS = 7;

const st = { open: load('calPanelOpen', false), start: startOfDay(new Date()), events: [], token: 0 };
const els = {};

/** Builds the panel below `mailview` inside `host`; returns the toggle function. */
export function initCalPanel(host, mailview) {
  els.col = h('div.mailcol');
  host.insertBefore(els.col, mailview);
  els.split = h('div.splitter.h.cp-split');
  els.panel = h('div.calpanel', { dataset: { testid: 'cal-panel' } });
  els.col.append(mailview, els.split, els.panel);
  els.panel.style.height = load('calPanelHeight', 300) + 'px';
  els.split.addEventListener('mousedown', resize);
  apply();
}

/** Dragging the bar up makes the panel taller. */
function resize(e) {
  e.preventDefault();
  const start = e.clientY, startH = els.panel.offsetHeight;
  const frames = document.querySelectorAll('iframe');
  frames.forEach((f) => (f.style.pointerEvents = 'none'));
  document.body.classList.add('resizing');
  const move = (ev) => { els.panel.style.height = Math.max(160, Math.min(window.innerHeight - 220, startH - (ev.clientY - start))) + 'px'; };
  const up = () => {
    document.removeEventListener('mousemove', move);
    document.removeEventListener('mouseup', up);
    frames.forEach((f) => (f.style.pointerEvents = ''));
    document.body.classList.remove('resizing');
    store('calPanelHeight', els.panel.offsetHeight);
  };
  document.addEventListener('mousemove', move);
  document.addEventListener('mouseup', up);
}

export function calPanelOpen() { return st.open; }

export function toggleCalPanel(on = !st.open) {
  st.open = on;
  store('calPanelOpen', on);
  apply();
}

/** Mail view hidden (calendar/contacts mode) → panel hidden too. */
export function showCalPanelHost(show) {
  if (els.col) els.col.classList.toggle('hidden', !show);
}

/** Events changed elsewhere (sync, new appointment). */
export function onCalPanelChanged() {
  if (st.open) load_().catch(reportError);
}

function apply() {
  if (!els.panel) return;
  els.panel.classList.toggle('hidden', !st.open);
  els.split.classList.toggle('hidden', !st.open);
  if (st.open) { render(); load_().catch(reportError); }
  document.dispatchEvent(new CustomEvent('calpanel-toggled', { detail: st.open }));
}

function step(n) {
  st.start = n === 0 ? startOfDay(new Date()) : addDays(st.start, n);
  render();
  load_().catch(reportError);
}

async function load_() {
  const token = ++st.token;
  els.panel.classList.add('loading');
  try {
    const events = await eventsBetween(st.start, addDays(st.start, DAYS));
    if (token !== st.token) return;
    st.events = events;
    render();
  } finally {
    if (token === st.token) els.panel.classList.remove('loading');
  }
}

function render() {
  const p = els.panel;
  p.innerHTML = '';
  const end = addDays(st.start, DAYS - 1);
  const title = `${pad(st.start.getDate())}.${pad(st.start.getMonth() + 1)}. – ${pad(end.getDate())}.${pad(end.getMonth() + 1)}.${end.getFullYear()}`;
  const navBtn = (ic, label, n, testid) => h('button.icon-btn', { type: 'button', title: label, 'aria-label': label, dataset: { testid }, html: icon(ic, 16), onclick: () => step(n) });
  p.appendChild(h('div.cp-bar',
    h('span.cp-title', { html: icon('calendar', 15) + ' Kalender' }),
    h('button.btn.small', { type: 'button', dataset: { testid: 'cp-today' }, onclick: () => step(0) }, 'Heute'),
    navBtn('chevronLeft', 'Vorherige Woche', -DAYS, 'cp-prev'),
    navBtn('chevronRight', 'Nächste Woche', DAYS, 'cp-next'),
    h('span.cp-range', { dataset: { testid: 'cp-range' } }, title),
    h('span.busy.cp-busy'),
    h('span.spacer'),
    h('button.icon-btn', { type: 'button', title: 'Kalender ausblenden', 'aria-label': 'Kalender ausblenden', html: icon('close', 14), onclick: () => toggleCalPanel(false) })));

  const today = startOfDay(new Date());
  const days = Array.from({ length: DAYS }, (_, i) => addDays(st.start, i));
  const head = h('div.cp-head', h('div.cp-gutter'));
  const allday = h('div.cp-allday', h('div.cp-gutter.cp-adlabel', 'ganzt.'));
  const grid = h('div.cp-grid');
  const hours = h('div.cp-hours');
  for (let hr = FIRST_HOUR; hr < LAST_HOUR; hr++) hours.appendChild(h('div.cp-hour', `${pad(hr)}:00`));
  grid.appendChild(hours);
  let anyAllDay = false;
  for (const d of days) {
    const wd = d.getDay();
    const cls = (wd === 0 || wd === 6 ? '.weekend' : '') + (sameDay(d, today) ? '.today' : '');
    head.appendChild(h('div.cp-dayhead' + cls, h('span.cp-wd', WEEKDAYS_SHORT[wd]), ` ${pad(d.getDate())}.${pad(d.getMonth() + 1)}.`));
    const evs = st.events.filter((ev) => onDay(ev, d));
    const adCell = h('div.cp-adcell' + cls);
    for (const ev of evs.filter(isAllDayLike)) { adCell.appendChild(block(ev, null)); anyAllDay = true; }
    allday.appendChild(adCell);
    const col = h('div.cp-col' + cls, { dataset: { date: d.toISOString().slice(0, 10) } });
    for (let hr = FIRST_HOUR; hr < LAST_HOUR; hr++) col.appendChild(h('div.cp-slot'));
    col.addEventListener('dblclick', (e) => {
      if (e.target.closest('.cp-ev')) return;
      const r = col.getBoundingClientRect();
      const mins = Math.floor(((e.clientY - r.top) / r.height) * (LAST_HOUR - FIRST_HOUR) * 2) * 30;
      const s = addMinutes(d, FIRST_HOUR * 60 + mins);
      newEvent({ start: s, end: addMinutes(s, 30) });
    });
    layout(evs.filter((ev) => !isAllDayLike(ev)), d).forEach(({ ev, top, height, lane, lanes }) => {
      const el = block(ev, d);
      el.style.top = top + '%';
      el.style.height = height + '%';
      el.style.left = `calc(${(lane / lanes) * 100}% + 1px)`;
      el.style.width = `calc(${100 / lanes}% - 2px)`;
      col.appendChild(el);
    });
    if (sameDay(d, today)) {
      const now = new Date();
      const m = now.getHours() * 60 + now.getMinutes() - FIRST_HOUR * 60;
      if (m >= 0 && m <= (LAST_HOUR - FIRST_HOUR) * 60) col.appendChild(h('div.cp-now', { style: { top: (m / ((LAST_HOUR - FIRST_HOUR) * 60)) * 100 + '%' } }));
    }
    grid.appendChild(col);
  }
  allday.classList.toggle('empty', !anyAllDay);
  p.append(head, allday, h('div.cp-scroll', grid));
}

function onDay(ev, d) {
  const s = new Date(ev.start), e = new Date(ev.end), next = addDays(d, 1);
  if (ev.isAllDay) {
    // All-day events are stored as UTC dates.
    const sd = new Date(ev.start.slice(0, 10) + 'T00:00:00'), ed = new Date(ev.end.slice(0, 10) + 'T00:00:00');
    return sd < next && ed > d;
  }
  return s < next && e > d;
}

/** Position within 07–20 Uhr (clamped) and side-by-side lanes for overlaps. */
function layout(evs, d) {
  const span = (LAST_HOUR - FIRST_HOUR) * 60;
  const items = evs.map((ev) => {
    const s = new Date(ev.start), e = new Date(ev.end);
    const from = Math.max(0, (s - d) / 60000 - FIRST_HOUR * 60);
    const to = Math.min(span, (e - d) / 60000 - FIRST_HOUR * 60);
    return { ev, from: Math.min(from, span - 15), to: Math.max(to, Math.min(from, span - 15) + 15) };
  }).filter((x) => x.to > 0 && x.from < span).sort((a, b) => a.from - b.from || b.to - a.to);
  const out = [];
  let group = [], groupEnd = -1;
  const flush = () => {
    const laneEnds = [];
    for (const x of group) {
      let lane = laneEnds.findIndex((end) => end <= x.from);
      if (lane < 0) { lane = laneEnds.length; laneEnds.push(0); }
      laneEnds[lane] = x.to;
      x.lane = lane;
    }
    for (const x of group) out.push({ ev: x.ev, top: (x.from / span) * 100, height: ((x.to - x.from) / span) * 100, lane: x.lane, lanes: laneEnds.length });
    group = [];
  };
  for (const x of items) {
    if (group.length && x.from >= groupEnd) flush();
    group.push(x);
    groupEnd = Math.max(groupEnd, x.to);
  }
  if (group.length) flush();
  return out;
}

function block(ev, day) {
  const c = eventColors(ev);
  const s = new Date(ev.start);
  const timed = day && !isAllDayLike(ev);
  const el = h('div.cp-ev' + (timed ? '' : '.allday') + (ev.showAs === 'free' ? '.free' : '') + (ev.showAs === 'tentative' ? '.tentative' : ''), {
    dataset: { testid: 'cp-event', id: ev.id },
    title: `${eventTitle(ev)}${timed ? `\n${fmtTime(s)} – ${fmtTime(new Date(ev.end))}` : ''}${ev.location ? `\n${ev.location}` : ''}`,
    style: { background: c.background, '--bar': c.bar },
  });
  el.innerHTML = timed
    ? `<span class="cp-time">${sameDay(s, day) ? fmtTime(s) : '…'}</span> <span class="cp-subj">${esc(eventTitle(ev))}</span>`
    : `<span class="cp-subj">${esc(eventTitle(ev))}</span>`;
  el.addEventListener('dblclick', (e) => { e.stopPropagation(); openEvent(ev); });
  return el;
}
