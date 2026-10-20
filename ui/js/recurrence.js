// Recurring tasks: hidden "[#<rule>]" categories (docs/API.md "Recurring tasks").
import { icon } from './icons.js';
import { esc } from './util.js';

export const RECURRENCE_OPTIONS = [
  [null, 'Nie'],
  ['daily', 'Täglich'],
  ['weekdays', 'Werktags'],
  ['weekly', 'Wöchentlich'],
  ['biweekly', 'Alle 2 Wochen'],
  ['monthly', 'Monatlich'],
  ['quarterly', 'Vierteljährlich'],
  ['yearly', 'Jährlich'],
];
const LABELS = Object.fromEntries(RECURRENCE_OPTIONS.filter(([k]) => k));

export function recurrenceLabel(rule) { return (rule && LABELS[rule]) || ''; }

/** Category marker for a rule. */
export function recurrenceCategory(rule) { return rule ? `[#${rule}]` : null; }
export function isRecurrenceCategory(c) { return typeof c === 'string' && c.startsWith('[#') && c.endsWith(']'); }

/** Rule of a message: MessageSummary.recurrence, else derived from the categories. */
export function recurrenceOf(m) {
  if (!m) return null;
  if (m.recurrence) return m.recurrence;
  const c = (m.categories || []).find(isRecurrenceCategory);
  return c ? c.slice(2, -1) : null;
}

/** Small "↻ Monatlich" badge; `short` = icon only (list rows). */
export function recurrenceBadgeHtml(rule, { short = false } = {}) {
  const label = recurrenceLabel(rule);
  if (!label) return '';
  return `<span class="rec-badge${short ? ' short' : ''}" title="Wiederkehrende Aufgabe: ${esc(label)}">${icon('repeat', short ? 13 : 13)}${short ? '' : `<span>${esc(label)}</span>`}</span>`;
}

/** Menu items to change the rule; `current` is the active rule (null = none). */
export function recurrenceMenuItems(current, onPick) {
  return RECURRENCE_OPTIONS.map(([k, label]) => ({
    label, checked: (current || null) === k, testid: 'rec-' + (k || 'none'),
    onClick: () => onPick(k),
  }));
}
