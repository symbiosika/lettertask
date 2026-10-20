// "Auf Wiedervorlage": rule evaluation and labels. Rule grammar (docs/API.md):
//   "<base>@HH:MM"  base = tomorrow | days:N | weekend (next Saturday) | nextWeek (next Monday)
import { WEEKDAYS_SHORT, fmtTime, pad, startOfDay, addDays, sameDay } from './util.js';

export const DEFAULT_SNOOZE = [
  { label: 'Morgen 8 Uhr', rule: 'tomorrow@08:00' },
  { label: 'Übermorgen 8 Uhr', rule: 'days:2@08:00' },
  { label: 'Am Wochenende 8 Uhr', rule: 'weekend@08:00' },
  { label: 'Nächste Woche', rule: 'nextWeek@08:00' },
  { label: 'In 14 Tagen', rule: 'days:14@08:00' },
  { label: 'In 30 Tagen', rule: 'days:30@08:00' },
];

export function parseRule(rule) {
  const m = /^(tomorrow|weekend|nextWeek|days:(\d+))(?:@(\d{1,2}):(\d{2}))?$/.exec(String(rule || '').trim());
  if (!m) return null;
  const base = m[1].startsWith('days:') ? 'days' : m[1];
  return { base, days: m[2] ? Number(m[2]) : base === 'tomorrow' ? 1 : 0, hour: m[3] ? Number(m[3]) : 8, minute: m[4] ? Number(m[4]) : 0 };
}

export function formatRule({ base, days, hour, minute }) {
  const t = `${pad(hour)}:${pad(minute)}`;
  if (base === 'days') return `days:${Math.max(1, days || 1)}@${t}`;
  return `${base}@${t}`;
}

/** Target time (local Date) for a rule, relative to `now`. Returns null for invalid rules. */
export function computeSnooze(rule, now = new Date()) {
  const r = parseRule(rule);
  if (!r) return null;
  const today = startOfDay(now);
  let day;
  if (r.base === 'tomorrow') day = addDays(today, 1);
  else if (r.base === 'days') day = addDays(today, Math.max(1, r.days));
  else if (r.base === 'weekend') {
    // next Saturday strictly after today
    const delta = ((6 - today.getDay() + 7) % 7) || 7;
    day = addDays(today, delta);
  } else if (r.base === 'nextWeek') {
    // next Monday strictly after today
    const delta = ((1 - today.getDay() + 7) % 7) || 7;
    day = addDays(today, delta);
  } else return null;
  const d = new Date(day);
  d.setHours(r.hour, r.minute, 0, 0);
  return d;
}

/** "Mi, 21.10. 08:00" */
export function fmtSnoozeTarget(d) {
  return `${WEEKDAYS_SHORT[d.getDay()]}, ${pad(d.getDate())}.${pad(d.getMonth() + 1)}. ${fmtTime(d)}`;
}

/** Short "bis …" label for the list: today → "08:00", this week → "Mi 08:00", else date. */
export function fmtDueShort(iso, now = new Date()) {
  const d = new Date(iso);
  if (sameDay(d, now)) return fmtTime(d);
  const diff = (startOfDay(d) - startOfDay(now)) / 86400000;
  if (diff > 0 && diff < 7) return `${WEEKDAYS_SHORT[d.getDay()]} ${fmtTime(d)}`;
  return `${pad(d.getDate())}.${pad(d.getMonth() + 1)}. ${fmtTime(d)}`;
}

export function snoozeOptions(config) {
  const list = config && Array.isArray(config.snoozeOptions) && config.snoozeOptions.length ? config.snoozeOptions : DEFAULT_SNOOZE;
  return list.filter((o) => o && parseRule(o.rule));
}

export const BASE_LABELS = [
  ['tomorrow', 'Morgen'],
  ['days', 'In N Tagen'],
  ['weekend', 'Am Wochenende (Sa)'],
  ['nextWeek', 'Nächste Woche (Mo)'],
];
