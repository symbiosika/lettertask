// Flat action-bar buttons shared by all windows.
import { h, esc } from './util.js';
import { icon } from './icons.js';
import { busyClick } from './dialogs.js';

/**
 * Icon + label button. opts: { title, caret, testid, small, disabled, toggle, primary, iconOnly }
 */
export function tbBtn(label, ic, onClick, { title, caret, testid, small, disabled, toggle, primary, iconOnly } = {}) {
  const cls = 'button.tb-btn' + (small ? '.small' : '') + (primary ? '.primary' : '') + (iconOnly || !label ? '.icon-only' : '');
  const b = h(cls, { type: 'button', title: title || label, 'aria-label': title || label, disabled: !!disabled, dataset: { testid: testid || '' } });
  b.innerHTML = (ic ? icon(ic, small ? 16 : 18) : '') + (label && !iconOnly ? `<span class="tb-label">${esc(label)}</span>` : '') + (caret ? `<span class="tb-caret">${icon('chevronDown', 12)}</span>` : '');
  if (toggle) b.setAttribute('aria-pressed', 'false');
  // A running action ignores further clicks (no double send/delete).
  b.addEventListener('click', busyClick((e) => onClick(b, e)));
  return b;
}

/** Split button: main action + chevron opening a menu. */
export function splitBtn(label, ic, onClick, onMenu, { testid, menuTestid, primary, title } = {}) {
  const main = tbBtn(label, ic, onClick, { testid, primary, title });
  const drop = tbBtn('', 'chevronDown', onMenu, { testid: menuTestid, primary, title: 'Weitere Optionen' });
  drop.classList.add('tb-drop');
  return h('div.split' + (primary ? '.primary' : ''), main, drop);
}

export function setPressed(b, on) {
  b.classList.toggle('active', !!on);
  b.setAttribute('aria-pressed', on ? 'true' : 'false');
}

export const group = (...children) => h('div.tb-group', ...children);
export const stack = (...children) => h('div.tb-stack', ...children);
export const sep = () => h('span.tb-sep');
