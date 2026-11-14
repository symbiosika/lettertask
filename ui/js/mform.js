// Phone form building blocks: iOS-style grouped list rows (Termin, Aufgabe,
// Kontakt, Einstellungen). Styles: css/mobile.css ("grouped rows").
import { h } from './util.js';
import { icon } from './icons.js';

/** White rounded group of rows; `title` above, `foot` (help text) below. */
export function mGroup(rows, { title, foot, testid } = {}) {
  return h('div.m-group-wrap', testid ? { dataset: { testid } } : {},
    title ? h('div.m-group-title', title) : null,
    h('div.m-group', ...rows.filter(Boolean)),
    foot ? h('div.m-group-foot', foot) : null);
}

/** Row: label on the left, `control` (node or text) on the right; tap runs onClick. */
export function mRow(label, control, { onClick, chevron, icon: ic, testid, className = '' } = {}) {
  const tag = onClick ? 'button' : 'div';
  const el = h(`${tag}.m-row${className ? '.' + className.split(' ').join('.') : ''}`, onClick ? { type: 'button' } : {},
    ic ? h('span.m-row-ic', { html: icon(ic, 20) }) : null,
    h('span.m-row-label', label),
    control == null ? null : h('span.m-row-value', typeof control === 'string' ? document.createTextNode(control) : control),
    chevron ? h('span.m-row-chev', { html: icon('chevronRight', 16) }) : null);
  if (testid) el.dataset.testid = testid;
  if (onClick) el.addEventListener('click', (e) => { if (e.target.closest('select, input')) return; onClick(el, e); });
  return el;
}

/** iOS switch (a real checkbox underneath). */
export function mSwitch(checked, onChange, { label, testid } = {}) {
  const input = h('input.m-switch', { type: 'checkbox', role: 'switch', 'aria-label': label || '' });
  input.checked = !!checked;
  if (testid) input.dataset.testid = testid;
  if (onChange) input.addEventListener('change', () => onChange(input.checked));
  return input;
}

/** Row with a switch; the whole row toggles it. */
export function mSwitchRow(label, checked, onChange, opts = {}) {
  const sw = mSwitch(checked, onChange, { label, testid: opts.testid });
  const row = h('label.m-row', opts.icon ? h('span.m-row-ic', { html: icon(opts.icon, 20) }) : null, h('span.m-row-label', label), h('span.m-row-value', sw));
  return { row, input: sw };
}

/** Row around a native <select>: value right-aligned, the system picker opens on tap. */
export function mSelectRow(label, select, opts = {}) {
  select.classList.add('m-select');
  return mRow(label, select, { chevron: true, ...opts, className: 'm-row-select' });
}

/** Full-width red action row (delete). */
export function mDanger(label, onClick, { testid } = {}) {
  const b = h('button.m-row.m-danger', { type: 'button' }, h('span.m-row-label', label));
  if (testid) b.dataset.testid = testid;
  b.addEventListener('click', onClick);
  return b;
}
