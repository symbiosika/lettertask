// Hover card for e-mail addresses (message header, recipient lines, list rows):
// name and address with copy buttons. Any element with data-addr (and
// optionally data-name) inside a bound root gets the card.
import { h, esc, initials, avatarColor, touchInput } from './util.js';
import { icon } from './icons.js';
import { toast } from './dialogs.js';

const SHOW_DELAY = 450;
const HIDE_DELAY = 250;
let card = null, showTimer = null, hideTimer = null, anchor = null;

async function copy(text, what) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Older WebViews: copy through a temporary text field.
    const ta = h('textarea', { style: { position: 'fixed', opacity: '0' } }, text);
    document.body.appendChild(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
  }
  toast(`${what} kopiert.`);
}

function hide() {
  clearTimeout(showTimer);
  if (card) { card.remove(); card = null; }
  anchor = null;
}

function scheduleHide() {
  clearTimeout(hideTimer);
  hideTimer = setTimeout(hide, HIDE_DELAY);
}

function show(el, opts) {
  const address = el.dataset.addr || '';
  const name = el.dataset.name && el.dataset.name !== address ? el.dataset.name : '';
  if (!address) return;
  hide();
  anchor = el;
  const btn = (label, ic, testid, fn) => h('button.cc-btn', { type: 'button', dataset: { testid }, html: icon(ic, 14) + `<span>${esc(label)}</span>`, onclick: (e) => { e.stopPropagation(); fn(); } });
  const full = name ? `${name} <${address}>` : address;
  card = h('div.contact-card', { dataset: { testid: 'contact-card' }, role: 'dialog' },
    h('div.cc-head',
      h('div.avatar.lg', { style: { background: avatarColor(address || name) } }, initials(name, address)),
      h('div.cc-who', name ? h('div.cc-name', name) : null, h('div.cc-addr', address))),
    h('div.cc-actions',
      btn('E-Mail kopieren', 'copy', 'cc-copy-addr', () => copy(address, 'E-Mail-Adresse')),
      name ? btn('Name kopieren', 'copy', 'cc-copy-name', () => copy(name, 'Name')) : null,
      name ? btn('Beides kopieren', 'copy', 'cc-copy-full', () => copy(full, 'Name und Adresse')) : null,
      opts.onMail ? btn('E-Mail schreiben', 'mail', 'cc-mail', () => { hide(); opts.onMail({ name, address }); }) : null));
  card.addEventListener('mouseenter', () => clearTimeout(hideTimer));
  card.addEventListener('mouseleave', scheduleHide);
  document.body.appendChild(card);
  // Below the address, kept inside the window.
  const r = el.getBoundingClientRect();
  const cw = card.offsetWidth, ch = card.offsetHeight;
  let x = Math.min(r.left, window.innerWidth - cw - 8);
  let y = r.bottom + 6;
  if (y + ch > window.innerHeight - 8) y = Math.max(8, r.top - ch - 6);
  card.style.left = Math.max(8, x) + 'px';
  card.style.top = y + 'px';
}

/** Enables hover cards for [data-addr] elements inside `root`. opts: { onMail(addr) } */
export function bindContactCards(root, opts = {}) {
  root.addEventListener('mouseover', (e) => {
    if (touchInput()) return;
    const el = e.target.closest && e.target.closest('[data-addr]');
    if (!el || !root.contains(el)) return;
    clearTimeout(hideTimer);
    if (el === anchor) return;
    clearTimeout(showTimer);
    showTimer = setTimeout(() => show(el, opts), SHOW_DELAY);
  });
  root.addEventListener('mouseout', (e) => {
    const el = e.target.closest && e.target.closest('[data-addr]');
    if (!el || (e.relatedTarget && el.contains(e.relatedTarget))) return;
    clearTimeout(showTimer);
    if (anchor) scheduleHide();
  });
  // Touch has no hover: a tap on a name opens the card instead.
  root.addEventListener('click', (e) => {
    if (!touchInput()) return;
    const el = e.target.closest && e.target.closest('[data-addr]');
    if (el && root.contains(el) && el !== anchor) show(el, opts);
  });
  // Scrolling or clicking elsewhere closes the card.
  root.addEventListener('scroll', hide, true);
  root.addEventListener('mousedown', (e) => { if (card && !card.contains(e.target)) hide(); });
}

/** data-* attributes for an address span (HTML string context). */
export function addrAttrs(a) {
  if (!a || !a.address) return '';
  return ` data-addr="${esc(a.address)}" data-name="${esc(a.name || '')}"`;
}
