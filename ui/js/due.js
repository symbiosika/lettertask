// "Fällig" / "Erinnerung" time picker shared by the task window and the compose window.
// Offers "Sofort" (optional), the configured Wiedervorlage options and a custom date/time.
import { h, toLocalIso, addDays, startOfDay } from './util.js';
import { snoozeOptions, computeSnooze, fmtSnoozeTarget } from './snooze.js';

/**
 * opts: { config, allowNow, variant: 'chips'|'select', testid, onChange }
 * Returns { el, get(): { date: Date|null } | { error } , label() }.
 * `date` null means "Sofort".
 */
export function duePicker({ config, allowNow = false, variant = 'chips', testid = 'due', onChange } = {}) {
  const opts = [];
  if (allowNow) opts.push({ key: 'now', label: 'Sofort' });
  snoozeOptions(config).forEach((o, i) => opts.push({ key: 'opt' + i, label: o.label, rule: o.rule }));
  opts.push({ key: 'custom', label: 'Datum und Uhrzeit…' });
  let key = opts[0].key;

  const def = addDays(startOfDay(new Date()), 1); def.setHours(8);
  const custom = h('input.input.due-custom.hidden', { type: 'datetime-local', value: toLocalIso(def).slice(0, 16), name: testid + 'At', dataset: { testid: testid + '-at' } });
  const preview = h('span.due-preview', { dataset: { testid: testid + '-preview' } });
  const target = () => {
    const o = opts.find((x) => x.key === key);
    if (!o || o.key === 'now') return null;
    if (o.key === 'custom') { const d = new Date(custom.value); return isNaN(d) ? undefined : d; }
    return computeSnooze(o.rule, new Date());
  };
  const paint = () => {
    custom.classList.toggle('hidden', key !== 'custom');
    const d = target();
    preview.textContent = key === 'now' ? 'erscheint gleich im Posteingang' : d ? '→ ' + fmtSnoozeTarget(d) : '';
    if (variant === 'chips') chips.querySelectorAll('.chip').forEach((c) => c.classList.toggle('on', c.dataset.key === key));
    else sel.value = key;
    if (onChange) onChange();
  };

  let chips = null;
  let sel = null;
  let control;
  if (variant === 'chips') {
    chips = h('div.due-chips', { role: 'radiogroup', dataset: { testid } });
    for (const o of opts) {
      const d = o.rule ? computeSnooze(o.rule, new Date()) : null;
      const c = h('button.chip.due-chip', { type: 'button', role: 'radio', dataset: { key: o.key, testid: `${testid}-${o.key}` }, title: d ? fmtSnoozeTarget(d) : o.label }, o.label);
      c.addEventListener('click', () => { key = o.key; paint(); if (key === 'custom') custom.focus(); });
      chips.appendChild(c);
    }
    control = chips;
  } else {
    sel = h('select.input.due-select', { name: testid, dataset: { testid } });
    for (const o of opts) sel.appendChild(h('option', { value: o.key }, o.label));
    sel.addEventListener('change', () => { key = sel.value; paint(); if (key === 'custom') custom.focus(); });
    control = sel;
  }
  custom.addEventListener('input', paint);
  const el = h('div.due-picker.' + variant, control, custom, preview);
  paint();
  return {
    el,
    get() {
      const d = target();
      if (key === 'now') return { date: null };
      if (!d) return { error: 'Bitte Datum und Uhrzeit angeben.' };
      if (d <= new Date()) return { error: 'Der Zeitpunkt muss in der Zukunft liegen.' };
      return { date: d };
    },
    label() { const d = target(); return key === 'now' ? 'Sofort' : d ? fmtSnoozeTarget(d) : ''; },
    focus() { (sel || chips.querySelector('.chip.on') || chips).focus(); },
  };
}
