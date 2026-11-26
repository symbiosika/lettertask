// Import of iCalendar files (.ics): picked from disk, dropped onto the
// calendar or clicked as a mail attachment. One event opens in the event
// window, filled in and still unsaved, so it can be adjusted and its calendar
// chosen before it is saved. Several events come up in a list first.
import { call, openWindow } from './api.js';
import { h, toLocalIso, toDateInput, fmtDateTime, fmtDate, addDays } from './util.js';
import { modal, toast, reportError, loadingPill } from './dialogs.js';
import { state } from './state.js';
import { isImap, isGoogle } from './accounts.js';

/** localStorage key prefix the event window reads the imported event from. */
export const IMPORT_KEY = 'lt-ics-import:';
const TZ = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';

/** Attachment or file that holds iCalendar data. */
export function isIcs(a) {
  return /\.(ics|ical|ifb|icalendar)$/i.test((a && a.name) || '') || /^text\/calendar/i.test((a && a.contentType) || '');
}

/** Accounts with a calendar (Microsoft 365 and Google). */
function calendarAccounts() { return state.accounts.filter((a) => !isImap(a) || isGoogle(a)); }
export function canImportIcs() { return calendarAccounts().length > 0; }

/** Events of a mail attachment ({ mailboxId, messageId, attachmentId }). */
export async function importIcsAttachment(ref) {
  const stop = loadingPill('Kalenderdatei wird gelesen…', 150);
  let events;
  try { events = await call('ics_from_attachment', ref); } finally { stop(); }
  return importEvents(events);
}

/** Events of a local file (picked or dropped). */
export async function importIcsFile(file) {
  if (file.size > 10 * 1024 * 1024) { toast(`„${file.name}" ist zu groß für einen Kalenderimport.`, 'error'); return; }
  const text = await file.text();
  return importEvents(await call('parse_ics', { ics: text }));
}

/** File dialog for one or more .ics files. */
export function pickIcsFile() {
  const input = h('input', { type: 'file', accept: '.ics,.ical,.ifb,.icalendar,text/calendar', multiple: true, style: { display: 'none' } });
  input.addEventListener('change', async () => {
    const files = [...(input.files || [])];
    input.remove();
    for (const f of files) await importIcsFile(f).catch(reportError);
  });
  document.body.appendChild(input);
  input.click();
}

async function importEvents(events) {
  if (!canImportIcs()) { toast('Für den Import wird ein Microsoft-365- oder Google-Konto mit Kalender benötigt.', 'error'); return; }
  if (!events || !events.length) { toast('Die Datei enthält keinen Termin, der importiert werden kann.', 'error'); return; }
  if (events.length === 1) { editImported(events[0]); return; }
  await chooseEvents(events);
}

/** Opens the event window with the event filled in (nothing saved yet). */
export function editImported(ev) {
  purgeOld();
  const key = IMPORT_KEY + Date.now() + '-' + Math.random().toString(36).slice(2, 8);
  try { localStorage.setItem(key, JSON.stringify(ev)); } catch (e) { reportError(e); return; }
  openWindow('event', { importKey: key }).catch(reportError);
}

/** Import entries nobody picked up (window closed before loading). */
function purgeOld() {
  try {
    const limit = Date.now() - 3600 * 1000;
    for (let i = localStorage.length - 1; i >= 0; i--) {
      const k = localStorage.key(i);
      if (k && k.startsWith(IMPORT_KEY) && Number(k.slice(IMPORT_KEY.length).split('-')[0]) < limit) localStorage.removeItem(k);
    }
  } catch { /* storage unavailable */ }
}

function whenText(ev) {
  if (ev.isAllDay) {
    const s = new Date(ev.start), e = new Date(ev.end);
    const sd = new Date(s.getUTCFullYear(), s.getUTCMonth(), s.getUTCDate());
    const last = addDays(new Date(e.getUTCFullYear(), e.getUTCMonth(), e.getUTCDate()), -1);
    return last > sd ? `${fmtDate(sd)} – ${fmtDate(last)} (ganztägig)` : `${fmtDate(sd)} (ganztägig)`;
  }
  return fmtDateTime(new Date(ev.start));
}

/** EventInput of an imported event for saving it without the window. */
export function importInput(ev, google) {
  let start, end, first;
  if (ev.isAllDay) {
    start = ev.start.slice(0, 10) + 'T00:00:00';
    end = ev.end.slice(0, 10) + 'T00:00:00';
    first = ev.start.slice(0, 10);
  } else {
    const s = new Date(ev.start);
    start = toLocalIso(s); end = toLocalIso(new Date(ev.end)); first = toDateInput(s);
  }
  return {
    subject: ev.subject || '', start, end, timeZone: TZ, isAllDay: !!ev.isAllDay, location: ev.location || '',
    bodyHtml: ev.bodyHtml || '', attendees: [], showAs: ev.showAs || 'busy', categories: google ? [] : (ev.categories || []),
    isReminderOn: !!ev.isReminderOn, reminderMinutes: ev.reminderMinutes || 0, isOnlineMeeting: false,
    recurrence: ev.recurrence ? { ...ev.recurrence, startDate: first } : null,
    sensitivity: ev.sensitivity || 'normal', importance: ev.importance || 'normal',
  };
}

/** Several events: pick which ones go into which calendar, or open one to adjust it first. */
async function chooseEvents(events) {
  const accounts = calendarAccounts();
  const lists = await Promise.all(accounts.map((a) => call('list_calendars', { accountId: a.id, cached: true }, { silent: true }).catch(() => [])));
  const groups = accounts.map((account, i) => ({ account, calendars: (lists[i] || []).filter((c) => c.canEdit) })).filter((g) => g.calendars.length);
  if (!groups.length) { toast('Es gibt keinen Kalender, in den importiert werden kann.', 'error'); return; }
  const select = h('select.input', { 'aria-label': 'Kalender', dataset: { testid: 'ics-calendar' } });
  const many = groups.length > 1;
  for (const g of groups) {
    const parent = many ? h('optgroup', { label: g.account.email || g.account.displayName || g.account.id }) : select;
    for (const c of g.calendars) parent.appendChild(h('option', { value: c.id, dataset: { account: g.account.id }, selected: c.isDefault && g === groups[0] }, c.name + (c.isDefault ? ' (Standard)' : '')));
    if (many) select.appendChild(parent);
  }
  const checks = [];
  const rows = events.map((ev) => {
    const cb = h('input', { type: 'checkbox', checked: true });
    checks.push(cb);
    const edit = h('button.linkbtn', { type: 'button', title: 'Vor dem Import anpassen', dataset: { testid: 'ics-edit' } }, 'Bearbeiten…');
    edit.addEventListener('click', () => { cb.checked = false; editImported(ev); });
    return h('div.ics-row', { style: { display: 'flex', alignItems: 'center', gap: '8px', padding: '4px 0', borderBottom: '1px solid var(--border, #e5e5e5)' } },
      h('label', { style: { display: 'flex', alignItems: 'center', gap: '8px', flex: '1', minWidth: '0', cursor: 'pointer' } }, cb,
        h('span', { style: { minWidth: '0' } },
          h('div', { style: { fontWeight: '600', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' } }, ev.subject || '(ohne Betreff)'),
          h('div.dim', { style: { fontSize: '12px' } }, whenText(ev) + (ev.recurrence ? ' · Serie' : '') + (ev.location ? ' · ' + ev.location : '')))),
      edit);
  });
  const body = h('div',
    h('p', { style: { marginTop: 0 } }, `Die Datei enthält ${events.length} Termine. Wählen Sie die Termine und den Kalender – oder passen Sie einen Termin mit „Bearbeiten…" vorher an.`),
    h('div', { style: { maxHeight: '320px', overflow: 'auto', marginBottom: '10px' }, dataset: { testid: 'ics-list' } }, ...rows),
    h('label', { style: { display: 'flex', alignItems: 'center', gap: '8px' } }, 'Kalender:', select));
  await modal({
    title: 'Termine importieren', body, width: 520,
    buttons: [{ label: 'Importieren', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: async (v) => {
      if (v !== 'ok') return v;
      const picked = events.filter((_, i) => checks[i].checked);
      if (!picked.length) { toast('Bitte mindestens einen Termin wählen.', 'error'); return false; }
      const opt = select.selectedOptions[0];
      const accountId = opt.dataset.account;
      const google = isGoogle(state.accounts.find((a) => a.id === accountId));
      let done = 0;
      for (const ev of picked) {
        await call('create_event', { accountId, calendarId: select.value, input: importInput(ev, google) });
        checks[events.indexOf(ev)].checked = false; // not twice after an error
        done++;
      }
      toast(done === 1 ? '1 Termin importiert.' : `${done} Termine importiert.`);
      return v;
    },
  });
}
