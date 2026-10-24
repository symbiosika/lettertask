// Appointment / meeting window.
import { call, params, closeWindow, currentWindow } from './api.js';
import {
  h, $, esc, addDays, startOfDay, toDateInput, toTimeInput, parseLocal, toLocalIso, DAY_KEYS, WEEKDAYS, MONTHS, displayName,
} from './util.js';
import { icon } from './icons.js';
import { modal, reportError, toast, confirmDialog, showMenuAt, field, loadingPill } from './dialogs.js';
import { recipientField } from './recipients.js';
import { createEditor } from './editor.js';
import { tbBtn, group, setPressed } from './toolbar.js';
import { getCategories, presetHex, swatchHtml, textOn, categoryHex } from './categories.js';
import { state, bootstrapSecondary } from './state.js';
import { isImap, isGoogle } from './accounts.js';

const P = params();
const TZ = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
// Reminder before the start – labelled "… vorher" so it is not read as the duration.
const REMINDERS = [[-1, 'Keine Erinnerung'], [0, 'Erinnerung bei Beginn'], [5, '5 Minuten vorher'], [10, '10 Minuten vorher'], [15, '15 Minuten vorher'], [30, '30 Minuten vorher'], [60, '1 Stunde vorher'], [120, '2 Stunden vorher'], [240, '4 Stunden vorher'], [720, '12 Stunden vorher'], [1440, '1 Tag vorher'], [2880, '2 Tage vorher'], [10080, '1 Woche vorher']];
const SHOWAS = [['free', 'Frei'], ['workingElsewhere', 'An anderem Ort tätig'], ['tentative', 'Mit Vorbehalt'], ['busy', 'Gebucht'], ['oof', 'Abwesend']];
const RESPONSE_TEXT = { accepted: 'Zugesagt', tentativelyAccepted: 'Mit Vorbehalt', declined: 'Abgelehnt', organizer: 'Organisator', notResponded: 'Keine Antwort', none: 'Keine Antwort' };

const st = {
  accountId: P.accountId,
  event: null,       // existing EventDetail
  readOnly: false,
  categories: [],
  recurrence: null,
  sensitivity: 'normal',
  importance: 'normal',
  online: false,
  showAs: 'busy',
  reminder: 15,
  attendeesVisible: false,
  dirty: false,
  closing: false,
};
const f = {}; // form controls
let editor, required, optional;

function dirty() { st.dirty = true; }

function allDayRange(ev) {
  const toDay = (iso) => {
    const d = new Date(iso);
    if (d.getUTCHours() === 0 && d.getUTCMinutes() === 0) return new Date(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
    return startOfDay(new Date(d.getTime() + 12 * 3600000));
  };
  const s = toDay(ev.start);
  let e = toDay(ev.end);
  if (e <= s) e = addDays(s, 1);
  return { start: s, end: e };
}

// --------------------------------------------------------------- toolbar
function renderToolbar() {
  const tb = $('#toolbar');
  tb.innerHTML = '';
  if (st.readOnly) {
    tb.append(
      group(
        tbBtn('Annehmen', 'accept', () => respond('accept'), { testid: 'ev-accept' }),
        tbBtn('Mit Vorbehalt', 'tentative', () => respond('tentativelyAccept')),
        tbBtn('Ablehnen', 'decline', () => respond('decline')),
      ),
      group(tbBtn('Löschen', 'delete', () => del())),
      st.event && st.event.onlineMeetingUrl ? group(tbBtn(meetingLabel() + ' beitreten', 'teams', () => import('./api.js').then((m) => m.openUrl(st.event.onlineMeetingUrl)))) : null,
    );
    return;
  }
  const hasAtt = st.attendeesVisible;
  const saveLabel = hasAtt && !(st.event && st.event.id) ? 'Senden' : 'Speichern & schließen';
  const showAs = h('select.input', { title: 'Anzeigen als', 'aria-label': 'Anzeigen als', dataset: { testid: 'ev-showas' } }, ...SHOWAS.map(([v, l]) => h('option', { value: v, selected: st.showAs === v }, l)));
  showAs.addEventListener('change', () => { st.showAs = showAs.value; dirty(); });
  const rem = h('select.input', { title: 'Erinnerung vor Beginn', 'aria-label': 'Erinnerung', dataset: { testid: 'ev-reminder' } }, ...REMINDERS.map(([v, l]) => h('option', { value: String(v), selected: st.reminder === v }, l)));
  rem.addEventListener('change', () => { st.reminder = Number(rem.value); dirty(); });
  const teams = tbBtn(google() ? 'Google Meet' : 'Teams-Besprechung', 'teams', () => { st.online = !st.online; setPressed(teams, st.online); dirty(); }, { toggle: true, testid: 'ev-teams' });
  setPressed(teams, st.online);
  const priv = tbBtn('Privat', 'lock', () => { st.sensitivity = st.sensitivity === 'private' ? 'normal' : 'private'; setPressed(priv, st.sensitivity === 'private'); dirty(); }, { toggle: true });
  setPressed(priv, st.sensitivity === 'private');
  const hi = tbBtn('Wichtig', 'important', () => { st.importance = st.importance === 'high' ? 'normal' : 'high'; setPressed(hi, st.importance === 'high'); dirty(); }, { toggle: true });
  setPressed(hi, st.importance === 'high');
  const series = tbBtn('Serientyp', 'recurrence', () => recurrenceDialog(), { testid: 'ev-series' });
  setPressed(series, !!st.recurrence);
  tb.append(
    group(tbBtn(saveLabel, hasAtt && !(st.event && st.event.id) ? 'send' : 'save', () => save(), { testid: 'ev-save', primary: true }), tbBtn('Löschen', 'delete', () => del(), { disabled: !(st.event && st.event.id), testid: 'ev-delete' })),
    group(tbBtn(hasAtt ? 'Einladung absagen' : 'Teilnehmer einladen', 'people', () => toggleAttendees(), { testid: 'ev-invite' }), teams),
    group(h('div.tb-stack', { style: { gap: '4px', padding: '2px 4px' } },
      h('label', { style: { display: 'flex', gap: '6px', alignItems: 'center' }, title: 'Anzeigen als' }, h('span', { html: icon('clock', 14) }), showAs),
      h('label', { style: { display: 'flex', gap: '6px', alignItems: 'center' }, title: 'Erinnerung vor Beginn' }, h('span', { html: icon('bell', 14) }), rem))),
    group(series),
    // Google calendars know no categories (and no importance).
    google() ? group(priv) : group(tbBtn('Kategorisieren', 'tag', (b) => showMenuAt(b, categoryItems()), { caret: true, testid: 'ev-categorize' }), priv, hi),
  );
}

/** Event of a Google account (Google Meet instead of Teams, no categories). */
function google() { const a = state.accounts.find((x) => x.id === st.accountId); return !!a && isGoogle(a); }
function meetingLabel() { return /meet\.google\.com/.test((st.event && st.event.onlineMeetingUrl) || '') ? 'Meet' : 'Teams'; }

function categoryItems() {
  return [
    ...getCategories(st.accountId).map((c) => ({
      label: c.displayName, iconHtml: swatchHtml(presetHex(c.color), 14), checked: st.categories.includes(c.displayName),
      onClick: () => { st.categories = st.categories.includes(c.displayName) ? st.categories.filter((x) => x !== c.displayName) : [...st.categories, c.displayName]; renderCats(); dirty(); },
    })),
    { separator: true },
    { label: 'Alle Kategorien löschen', disabled: !st.categories.length, onClick: () => { st.categories = []; renderCats(); dirty(); } },
  ];
}

function toggleAttendees() {
  if (st.attendeesVisible && (required.get().length || optional.get().length)) {
    confirmDialog('Die Teilnehmerliste wird entfernt. Fortfahren?').then((ok) => { if (ok) { required.set([]); optional.set([]); st.attendeesVisible = false; layoutRows(); renderToolbar(); dirty(); } });
    return;
  }
  st.attendeesVisible = !st.attendeesVisible;
  layoutRows();
  renderToolbar();
  if (st.attendeesVisible) required.focus();
}

// --------------------------------------------------------------- form
function renderForm() {
  const form = $('#form');
  form.innerHTML = '';
  const mb = () => {
    const acc = state.accounts.find((a) => a.id === st.accountId);
    return acc && acc.mailboxes[0] ? acc.mailboxes[0].id : null;
  };
  required = recipientField({ mailboxId: mb, name: 'required', onChange: dirty });
  optional = recipientField({ mailboxId: mb, name: 'optional', onChange: dirty });
  f.subject = h('input.input', { name: 'subject', 'aria-label': 'Betreff' });
  f.subject.addEventListener('input', () => { dirty(); updateTitle(); });
  f.location = h('input.input', { name: 'location', 'aria-label': 'Ort' });
  f.location.addEventListener('input', dirty);
  f.startDate = h('input.input', { type: 'date', name: 'startDate', 'aria-label': 'Beginn Datum' });
  f.startTime = h('input.input', { type: 'time', step: '300', name: 'startTime', 'aria-label': 'Beginn Uhrzeit' });
  f.endDate = h('input.input', { type: 'date', name: 'endDate', 'aria-label': 'Ende Datum' });
  f.endTime = h('input.input', { type: 'time', step: '300', name: 'endTime', 'aria-label': 'Ende Uhrzeit' });
  f.allDay = h('input', { type: 'checkbox', name: 'allDay' });
  f.calendar = h('select.input', { name: 'calendar', 'aria-label': 'Kalender' });
  let lastDuration = 30;
  const dur = () => (parseLocal(f.endDate.value, f.endTime.value) - parseLocal(f.startDate.value, f.startTime.value)) / 60000;
  const onStart = () => {
    // keep duration when start changes (Outlook behaviour)
    const s = parseLocal(f.startDate.value, f.allDay.checked ? '00:00' : f.startTime.value);
    if (f.allDay.checked) {
      const days = Math.max(0, Math.round((parseLocal(f.endDate.value) - parseLocal(f.startDate.value)) / 86400000));
      if (days < 0 || isNaN(days)) f.endDate.value = f.startDate.value;
    } else {
      const e = new Date(s.getTime() + lastDuration * 60000);
      f.endDate.value = toDateInput(e); f.endTime.value = toTimeInput(e);
    }
    dirty();
  };
  for (const el of [f.startDate, f.startTime]) el.addEventListener('change', onStart);
  for (const el of [f.endDate, f.endTime]) el.addEventListener('change', () => { const d = dur(); if (d > 0) lastDuration = d; dirty(); });
  f.startTime.addEventListener('focus', () => { const d = dur(); if (d > 0) lastDuration = d; });
  f.allDay.addEventListener('change', () => { syncAllDay(); dirty(); });
  f.calendar.addEventListener('change', dirty);

  const attLbl = (label, fld) => h('button.flabel.btn.small', { type: 'button', tabindex: '-1', onclick: () => fld.focus(), style: { justifyContent: 'flex-start' } }, label);
  f.rowsAtt = [attLbl('Erforderlich…', required), required.el, attLbl('Optional…', optional), optional.el];
  f.catRow = h('div.cat-picks');
  form.append(
    ...f.rowsAtt,
    h('span', 'Betreff:'), f.subject,
    h('span', 'Ort:'), f.location,
    h('span', 'Beginn:'), h('div.inline', f.startDate, f.startTime, h('label.checkrow', { style: { margin: '0 0 0 8px' } }, f.allDay, 'Ganztägiges Ereignis')),
    h('span', 'Ende:'), h('div.inline', f.endDate, f.endTime, f.recurText = h('span.dim')),
    h('span', 'Kalender:'), h('div.inline', f.calendar, f.catRow),
  );
  editor = createEditor({ onInput: dirty, minHeight: 160 });
  $('#editor').append(editor.toolbar, editor.area);
  layoutRows();
}

function layoutRows() {
  for (const el of f.rowsAtt) el.classList.toggle('hidden', !st.attendeesVisible);
}

function syncAllDay() {
  const on = f.allDay.checked;
  f.startTime.classList.toggle('hidden', on);
  f.endTime.classList.toggle('hidden', on);
  if (on && st.showAs === 'busy' && !(st.event && st.event.id)) { st.showAs = 'free'; renderToolbar(); }
}

function renderCats() {
  f.catRow.innerHTML = '';
  for (const c of st.categories) {
    const bg = categoryHex(st.accountId, c);
    f.catRow.appendChild(h('span.cat-chip', { style: { background: bg, color: textOn(bg) } }, c));
  }
}

function recurrenceText(r) {
  if (!r) return '';
  const every = r.interval > 1 ? `alle ${r.interval} ` : 'jede(n) ';
  let s = '';
  if (r.pattern === 'daily') s = r.interval > 1 ? `Alle ${r.interval} Tage` : 'Täglich';
  else if (r.pattern === 'weekly') s = (r.interval > 1 ? `Alle ${r.interval} Wochen am ` : 'Wöchentlich am ') + (r.daysOfWeek || []).map((d) => WEEKDAYS[DAY_KEYS.indexOf(d)]).join(', ');
  else if (r.pattern === 'absoluteMonthly') s = `Am ${r.dayOfMonth}. ${every}Monat${r.interval > 1 ? 'e' : ''}`;
  else if (r.pattern === 'absoluteYearly') s = `Jährlich am ${r.dayOfMonth}. ${MONTHS[(r.month || 1) - 1]}`;
  if (r.endDate) s += ` bis ${r.endDate.split('-').reverse().join('.')}`;
  else if (r.occurrences) s += `, ${r.occurrences} Termine`;
  return s;
}

function updateRecurText() {
  f.recurText.innerHTML = st.recurrence ? icon('recurrence', 13) + ' ' + esc(recurrenceText(st.recurrence)) : '';
}

function updateTitle() {
  const t = f.subject.value.trim() || 'Unbenannt';
  document.title = `${t} – ${st.attendeesVisible ? 'Besprechung' : 'Termin'}`;
  { const w = currentWindow(); if (w) w.setTitle(document.title).catch(() => {}); }
}

// --------------------------------------------------------------- recurrence dialog
async function recurrenceDialog() {
  const start = parseLocal(f.startDate.value || toDateInput(new Date()));
  const r = st.recurrence || {
    pattern: 'weekly', interval: 1, daysOfWeek: [DAY_KEYS[start.getDay()]], dayOfMonth: start.getDate(), month: start.getMonth() + 1,
    startDate: f.startDate.value, endDate: null, occurrences: null,
  };
  const radio = (name, value, checked, label) => h('label.checkrow', { style: { margin: '3px 0' } }, h('input', { type: 'radio', name, value, checked }), label);
  const pat = h('div', { style: { borderRight: '1px solid #ddd', paddingRight: '12px' } },
    radio('pat', 'daily', r.pattern === 'daily', 'Täglich'), radio('pat', 'weekly', r.pattern === 'weekly', 'Wöchentlich'),
    radio('pat', 'absoluteMonthly', r.pattern === 'absoluteMonthly', 'Monatlich'), radio('pat', 'absoluteYearly', r.pattern === 'absoluteYearly', 'Jährlich'));
  const interval = h('input.input', { type: 'number', min: '1', max: '99', value: String(r.interval || 1), style: { width: '56px' } });
  const days = h('div', { style: { display: 'grid', gridTemplateColumns: 'repeat(4, auto)', gap: '2px 12px', marginTop: '6px' } },
    ...[1, 2, 3, 4, 5, 6, 0].map((i) => h('label.checkrow', { style: { margin: 0 } }, h('input', { type: 'checkbox', value: DAY_KEYS[i], checked: (r.daysOfWeek || []).includes(DAY_KEYS[i]) }), WEEKDAYS[i])));
  const dom = h('input.input', { type: 'number', min: '1', max: '31', value: String(r.dayOfMonth || start.getDate()), style: { width: '56px' } });
  const month = h('select.input', ...MONTHS.map((m, i) => h('option', { value: String(i + 1), selected: (r.month || start.getMonth() + 1) === i + 1 }, m)));
  const detail = h('div', { style: { paddingLeft: '12px', minWidth: '320px' } });
  const unitLabel = { daily: 'Tag(e)', weekly: 'Woche(n) am:', absoluteMonthly: 'Monat(en)', absoluteYearly: 'Jahr(e)' };
  const renderDetail = () => {
    const p = pat.querySelector('input:checked').value;
    detail.innerHTML = '';
    detail.appendChild(h('div.inline', { style: { display: 'flex', gap: '6px', alignItems: 'center' } }, 'Jede(n)/alle', interval, unitLabel[p]));
    if (p === 'weekly') detail.appendChild(days);
    if (p === 'absoluteMonthly') detail.appendChild(h('div', { style: { display: 'flex', gap: '6px', alignItems: 'center', marginTop: '6px' } }, 'Am Tag', dom));
    if (p === 'absoluteYearly') detail.appendChild(h('div', { style: { display: 'flex', gap: '6px', alignItems: 'center', marginTop: '6px' } }, 'Am', dom, month));
  };
  pat.addEventListener('change', renderDetail);
  renderDetail();
  const rStart = h('input.input', { type: 'date', value: r.startDate || f.startDate.value });
  const endKind = r.endDate ? 'date' : r.occurrences ? 'count' : 'none';
  const occ = h('input.input', { type: 'number', min: '1', max: '999', value: String(r.occurrences || 10), style: { width: '60px' } });
  const endDate = h('input.input', { type: 'date', value: r.endDate || toDateInput(addDays(start, 70)) });
  const range = h('div',
    field('Beginn:', rStart),
    radio('end', 'none', endKind === 'none', 'Kein Enddatum'),
    h('div', { style: { display: 'flex', gap: '6px', alignItems: 'center' } }, radio('end', 'count', endKind === 'count', 'Endet nach:'), occ, 'Terminen'),
    h('div', { style: { display: 'flex', gap: '6px', alignItems: 'center' } }, radio('end', 'date', endKind === 'date', 'Endet am:'), endDate));
  const body = h('div',
    h('fieldset', h('legend', 'Serienmuster'), h('div', { style: { display: 'flex' } }, pat, detail)),
    h('fieldset', h('legend', 'Seriendauer'), range));
  const res = await modal({
    title: 'Terminserie', body, width: 560,
    buttons: [{ label: 'OK', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }, ...(st.recurrence ? [{ label: 'Serie entfernen', value: 'remove' }] : [])],
    onBeforeClose: (v) => {
      if (v !== 'ok') return v;
      const p = pat.querySelector('input:checked').value;
      const iv = Math.max(1, parseInt(interval.value, 10) || 1);
      const dow = [...days.querySelectorAll('input:checked')].map((x) => x.value);
      if (p === 'weekly' && !dow.length) { toast('Bitte mindestens einen Wochentag wählen.', 'error'); return false; }
      const ek = range.querySelector('input[name=end]:checked').value;
      return {
        pattern: p, interval: iv, daysOfWeek: p === 'weekly' ? dow : [], dayOfMonth: parseInt(dom.value, 10) || 1, month: parseInt(month.value, 10) || 1,
        startDate: rStart.value, endDate: ek === 'date' ? endDate.value : null, occurrences: ek === 'count' ? Math.max(1, parseInt(occ.value, 10) || 1) : null,
      };
    },
  });
  if (res === 'remove') st.recurrence = null;
  else if (res && typeof res === 'object') st.recurrence = res;
  else return;
  dirty();
  updateRecurText();
  renderToolbar();
}

// --------------------------------------------------------------- load / save
function fillCalendars(calendars, selectedId) {
  f.calendar.innerHTML = '';
  for (const c of calendars.filter((c) => c.canEdit || c.id === selectedId)) {
    f.calendar.appendChild(h('option', { value: c.id, selected: c.id === selectedId }, c.name + (c.isDefault ? ' (Standard)' : '')));
  }
}

function fillFromEvent(ev) {
  f.subject.value = ev.subject || '';
  f.location.value = ev.location || '';
  f.allDay.checked = !!ev.isAllDay;
  if (ev.isAllDay) {
    const r = allDayRange(ev);
    f.startDate.value = toDateInput(r.start); f.startTime.value = '08:00';
    f.endDate.value = toDateInput(addDays(r.end, -1)); f.endTime.value = '08:30';
  } else {
    const s = new Date(ev.start), e = new Date(ev.end);
    f.startDate.value = toDateInput(s); f.startTime.value = toTimeInput(s);
    f.endDate.value = toDateInput(e); f.endTime.value = toTimeInput(e);
  }
  st.categories = [...(ev.categories || [])];
  st.recurrence = ev.recurrence || null;
  st.sensitivity = ev.sensitivity || 'normal';
  st.importance = ev.importance || 'normal';
  st.online = !!ev.isOnlineMeeting;
  st.showAs = ev.showAs || 'busy';
  st.reminder = ev.isReminderOn ? (ev.reminderMinutes ?? 15) : -1;
  const atts = (ev.attendees || []).filter((a) => a.type !== 'resource');
  required.set(atts.filter((a) => a.type === 'required').map(({ name, address }) => ({ name, address })));
  optional.set(atts.filter((a) => a.type === 'optional').map(({ name, address }) => ({ name, address })));
  st.attendeesVisible = atts.length > 0;
  layoutRows();
  syncAllDay();
  renderCats();
  updateRecurText();
}

function buildInput() {
  const allDay = f.allDay.checked;
  let start, end;
  if (allDay) {
    const s = parseLocal(f.startDate.value);
    const e = addDays(parseLocal(f.endDate.value || f.startDate.value), 1);
    start = toLocalIso(s); end = toLocalIso(e);
  } else {
    start = toLocalIso(parseLocal(f.startDate.value, f.startTime.value));
    end = toLocalIso(parseLocal(f.endDate.value, f.endTime.value));
  }
  const attendees = st.attendeesVisible ? [
    ...required.get().map((a) => ({ ...a, type: 'required' })),
    ...optional.get().map((a) => ({ ...a, type: 'optional' })),
  ] : [];
  return {
    subject: f.subject.value.trim(), start, end, timeZone: TZ, isAllDay: allDay, location: f.location.value.trim(),
    bodyHtml: editor.getHtml(), attendees, showAs: st.showAs, categories: st.categories,
    isReminderOn: st.reminder >= 0, reminderMinutes: Math.max(0, st.reminder), isOnlineMeeting: st.online,
    recurrence: st.recurrence ? { ...st.recurrence, startDate: st.recurrence.startDate || f.startDate.value } : null,
    sensitivity: st.sensitivity, importance: st.importance,
  };
}

let saving = false;
async function save() {
  if (saving) return;
  saving = true;
  try { await saveNow(); } finally { saving = false; }
}

async function saveNow() {
  const input = buildInput();
  if (!f.startDate.value || (!input.isAllDay && (!f.startTime.value || !f.endTime.value))) { toast('Bitte Beginn und Ende angeben.', 'error'); return; }
  if (input.end <= input.start) { toast('Das Ende muss nach dem Beginn liegen.', 'error'); f.endDate.focus(); return; }
  const invalid = [...required.invalid(), ...optional.invalid()];
  if (invalid.length) { toast('Ungültige Adresse: ' + invalid.join(', '), 'error'); return; }
  if (!input.subject) {
    const ok = await confirmDialog('Der Termin hat keinen Betreff. Trotzdem speichern?', { ok: 'Speichern' });
    if (!ok) { f.subject.focus(); return; }
  }
  if (st.event && st.event.id) await call('update_event', { accountId: st.accountId, eventId: st.event.id, input });
  else await call('create_event', { accountId: st.accountId, calendarId: f.calendar.value, input });
  st.dirty = false;
  st.closing = true;
  await closeWindow();
}

async function del() {
  if (!st.event || !st.event.id) { st.closing = true; await closeWindow(); return; }
  const hasAtt = st.event.isOrganizer && (st.event.attendees || []).length;
  const ok = await confirmDialog(hasAtt ? 'Die Besprechung wird abgesagt und alle Teilnehmer werden benachrichtigt. Fortfahren?' : 'Möchten Sie diesen Termin löschen?', { danger: true, ok: hasAtt ? 'Absage senden' : 'Löschen' });
  if (!ok) return;
  await call('delete_event', { accountId: st.accountId, eventId: st.event.id });
  st.closing = true;
  await closeWindow();
}

async function respond(response) {
  const comment = h('textarea.input', { rows: 3, style: { width: '100%' }, placeholder: 'Kommentar (optional)' });
  const send = h('input', { type: 'checkbox', checked: true });
  const res = await modal({
    title: { accept: 'Annehmen', tentativelyAccept: 'Mit Vorbehalt annehmen', decline: 'Ablehnen' }[response], width: 420,
    body: h('div', comment, h('label.checkrow', send, 'Antwort an den Organisator senden')),
    buttons: [{ label: 'OK', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
  });
  if (res !== 'ok') return;
  await call('respond_event', { accountId: st.accountId, eventId: st.event.id, response, comment: comment.value, sendResponse: send.checked });
  toast('Antwort gesendet.');
  st.closing = true;
  await closeWindow();
}

function renderReadOnly(ev) {
  for (const el of document.querySelectorAll('#form input, #form select')) el.disabled = true;
  for (const el of document.querySelectorAll('#form .rcpt-box')) el.style.pointerEvents = 'none';
  editor.toolbar.classList.add('hidden');
  editor.ready.then(() => { try { editor.area.contentDocument.designMode = 'off'; } catch { /* ignore */ } });
  $('#banner').appendChild(h('div.readonly-banner', { html: icon('info', 14) + ` Organisator: <b>${esc(displayName(ev.organizer))}</b> · Ihre Antwort: ${esc(RESPONSE_TEXT[ev.responseStatus] || '–')}` }));
}

async function init() {
  try { await bootstrapSecondary(); } catch (e) { reportError(e); }
  if (!st.accountId) { const o = state.accounts.find((a) => !isImap(a) || isGoogle(a)) || state.accounts[0]; st.accountId = o && o.id; }
  renderForm();
  // Calendar list and event load in parallel; the form is already on screen.
  const calendarsP = call('list_calendars', { accountId: st.accountId, cached: true }, { silent: true }).catch((e) => { reportError(e); return []; });
  if (P.eventId) {
    const stop = loadingPill('Termin wird geladen…', 150);
    let ev, calendars;
    try { [ev, calendars] = await Promise.all([call('get_event', { accountId: st.accountId, eventId: P.eventId }), calendarsP]); } finally { stop(); }
    if (ev.type === 'occurrence' && ev.seriesMasterId && ev.isOrganizer) {
      const choice = await modal({
        title: 'Serienelement öffnen', width: 400,
        body: h('div', h('p', { style: { marginTop: 0 } }, `„${ev.subject}" ist ein Serientermin. Möchten Sie nur dieses Vorkommen oder die Serie öffnen?`),
          h('label.checkrow', h('input', { type: 'radio', name: 'occ', value: 'one', checked: true }), 'Nur dieses Vorkommen'),
          h('label.checkrow', h('input', { type: 'radio', name: 'occ', value: 'series' }), 'Die gesamte Serie')),
        buttons: [{ label: 'OK', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
        onBeforeClose: () => document.querySelector('input[name=occ]:checked').value,
      });
      if (!choice) { st.closing = true; await closeWindow(); return; }
      if (choice === 'series') ev = await call('get_event', { accountId: st.accountId, eventId: ev.seriesMasterId });
    }
    st.event = ev;
    st.readOnly = !ev.isOrganizer;
    fillCalendars(calendars || [], ev.calendarId);
    fillFromEvent(ev);
    await editor.setHtml(ev.bodyHtml || '');
    if (st.readOnly) renderReadOnly(ev);
  } else {
    const now = new Date();
    const s = P.start ? new Date(P.start) : new Date(now.getFullYear(), now.getMonth(), now.getDate(), now.getHours() + 1);
    const e = P.end ? new Date(P.end) : new Date(s.getTime() + 30 * 60000);
    f.startDate.value = toDateInput(s); f.startTime.value = toTimeInput(s);
    f.allDay.checked = P.allDay === '1';
    if (f.allDay.checked) { f.endDate.value = toDateInput(addDays(e, -1) < s ? s : addDays(e, -1)); f.endTime.value = toTimeInput(s); st.showAs = 'free'; }
    else { f.endDate.value = toDateInput(e); f.endTime.value = toTimeInput(e); }
    const calendars = (await calendarsP) || [];
    const def = calendars.find((c) => c.id === P.calendarId) || calendars.find((c) => c.isDefault) || calendars[0];
    fillCalendars(calendars, def && def.id);
    syncAllDay();
    if (P.attendees) { st.attendeesVisible = true; required.set(P.attendees.split(';').filter(Boolean).map((a) => ({ name: a, address: a }))); layoutRows(); }
  }
  renderToolbar();
  updateTitle();
  st.dirty = false;
  f.subject.focus();

  window.addEventListener('keydown', (e) => {
    if (document.querySelector('.modal-overlay')) return;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's' && !st.readOnly) { e.preventDefault(); save().catch(reportError); }
    else if (e.key === 'Escape') { e.preventDefault(); tryClose(); }
  });
  const w = currentWindow();
  if (w) {
    if (w.onCloseRequested) w.onCloseRequested(async (ev) => { if (st.closing || !st.dirty) return; ev.preventDefault(); if (await confirmDiscard()) { st.closing = true; await (w.destroy ? w.destroy() : w.close()); } });
  } else {
    window.__ltBeforeClose = async () => st.closing || !st.dirty || st.readOnly || confirmDiscard();
    window.addEventListener('beforeunload', (e) => { if (st.dirty && !st.closing) { e.preventDefault(); e.returnValue = ''; } });
  }
}

async function confirmDiscard() {
  const choice = await modal({
    title: 'lettertask',
    body: h('div.confirm-body', h('span', { html: icon('warning', 32) }), h('div', 'Möchten Sie die Änderungen speichern?')),
    buttons: [{ label: 'Ja', value: 'yes', primary: true }, { label: 'Nein', value: 'no' }, { label: 'Abbrechen', value: 'cancel', cancel: true }],
  });
  if (choice === 'yes') { await save().catch(reportError); return false; }
  return choice === 'no';
}

async function tryClose() {
  if (st.dirty && !st.readOnly && !(await confirmDiscard())) return;
  st.closing = true;
  await closeWindow();
}

init().catch(reportError);
