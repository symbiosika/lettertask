// Ribbon-style toolbar buttons shared by all windows.
import { h, esc } from './util.js';
import { icon } from './icons.js';
import { reportError } from './dialogs.js';

export function tbBtn(label, ic, onClick, { title, caret, testid, small, disabled, toggle } = {}) {
  const b = h('button.tb-btn' + (small ? '.small' : ''), { type: 'button', title: title || label, disabled: !!disabled, dataset: { testid: testid || '' } });
  b.innerHTML = icon(ic, small ? 16 : 24) + `<span>${esc(label)}${caret ? ' <span class="caret">▼</span>' : ''}</span>`;
  if (toggle) b.setAttribute('aria-pressed', 'false');
  b.addEventListener('click', (e) => {
    Promise.resolve().then(() => onClick(b, e)).catch(reportError);
  });
  return b;
}

export function setPressed(b, on) {
  b.classList.toggle('active', !!on);
  b.setAttribute('aria-pressed', on ? 'true' : 'false');
}

export const group = (...children) => h('div.tb-group', ...children);
export const stack = (...children) => h('div.tb-stack', ...children);
