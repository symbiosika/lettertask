// Generic helpers: escaping, DOM building, dates (German), persistence.

export function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

/** Tiny element builder: h('div.cls#id', {attrs}, children...) */
export function h(sel, attrs, ...children) {
  const m = sel.match(/^([a-z0-9-]+)?((?:[.#][\w-]+)*)$/i);
  const el = document.createElement((m && m[1]) || 'div');
  if (m && m[2]) {
    for (const part of m[2].match(/[.#][\w-]+/g)) {
      if (part[0] === '.') el.classList.add(part.slice(1));
      else el.id = part.slice(1);
    }
  }
  if (attrs && (typeof attrs !== 'object' || attrs instanceof Node || Array.isArray(attrs))) {
    children.unshift(attrs);
    attrs = null;
  }
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v === null || v === undefined || v === false) continue;
      if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
      else if (k === 'html') el.innerHTML = v;
      else if (k === 'text') el.textContent = v;
      else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
      else if (k === 'dataset') Object.assign(el.dataset, v);
      else if (k in el && typeof v !== 'string') el[k] = v;
      else el.setAttribute(k, v === true ? '' : v);
    }
  }
  appendChildren(el, children);
  return el;
}

function appendChildren(el, children) {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) appendChildren(el, c);
    else if (c instanceof Node) el.appendChild(c);
    else el.appendChild(document.createTextNode(String(c)));
  }
}

export const $ = (sel, root = document) => root.querySelector(sel);
export const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

export function debounce(fn, ms) {
  let t;
  return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
}

// ---------- persistence (per-viewer UI prefs) ----------
export function load(key, def) {
  try {
    const v = localStorage.getItem('oc.' + key);
    return v === null ? def : JSON.parse(v);
  } catch { return def; }
}
export function store(key, val) {
  try { localStorage.setItem('oc.' + key, JSON.stringify(val)); } catch { /* ignore */ }
}

// ---------- dates ----------
export const WEEKDAYS = ['Sonntag', 'Montag', 'Dienstag', 'Mittwoch', 'Donnerstag', 'Freitag', 'Samstag'];
export const WEEKDAYS_SHORT = ['So', 'Mo', 'Di', 'Mi', 'Do', 'Fr', 'Sa'];
export const MONTHS = ['Januar', 'Februar', 'März', 'April', 'Mai', 'Juni', 'Juli', 'August', 'September', 'Oktober', 'November', 'Dezember'];
export const DAY_KEYS = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'];

const pad = (n) => String(n).padStart(2, '0');
export { pad };

export function startOfDay(d) { const x = new Date(d); x.setHours(0, 0, 0, 0); return x; }
export function addDays(d, n) { const x = new Date(d); x.setDate(x.getDate() + n); return x; }
export function addMinutes(d, n) { return new Date(d.getTime() + n * 60000); }
export function sameDay(a, b) { return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate(); }
/** Monday-based start of week. */
export function startOfWeek(d) { const x = startOfDay(d); const wd = (x.getDay() + 6) % 7; return addDays(x, -wd); }
export function startOfMonth(d) { return new Date(d.getFullYear(), d.getMonth(), 1); }

export function fmtTime(d) { return `${pad(d.getHours())}:${pad(d.getMinutes())}`; }
export function fmtDate(d) { return `${pad(d.getDate())}.${pad(d.getMonth() + 1)}.${d.getFullYear()}`; }
export function fmtDateShort(d) { return `${pad(d.getDate())}.${pad(d.getMonth() + 1)}.`; }
export function fmtDateLong(d) { return `${WEEKDAYS[d.getDay()]}, ${d.getDate()}. ${MONTHS[d.getMonth()]} ${d.getFullYear()}`; }
export function fmtDateTime(d) { return `${WEEKDAYS_SHORT[d.getDay()]} ${fmtDate(d)} ${fmtTime(d)}`; }

/** Local wall time without zone, "YYYY-MM-DDTHH:MM:SS". */
export function toLocalIso(d) {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}:00`;
}
export function toDateInput(d) { return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`; }
export function toTimeInput(d) { return `${pad(d.getHours())}:${pad(d.getMinutes())}`; }
export function parseLocal(dateStr, timeStr = '00:00') {
  const [y, m, d] = dateStr.split('-').map(Number);
  const [hh, mm] = (timeStr || '00:00').split(':').map(Number);
  return new Date(y, m - 1, d, hh || 0, mm || 0);
}

/** Message list time column: today → time, this week → "Mo 14:02", older → date. */
export function fmtListDate(iso) {
  if (!iso) return '';
  const d = new Date(iso);
  const now = new Date();
  if (sameDay(d, now)) return fmtTime(d);
  const diff = (startOfDay(now) - startOfDay(d)) / 86400000;
  if (diff > 0 && diff < 7) return `${WEEKDAYS_SHORT[d.getDay()]} ${fmtTime(d)}`;
  if (d.getFullYear() === now.getFullYear()) return `${WEEKDAYS_SHORT[d.getDay()]}, ${fmtDateShort(d)}`;
  return fmtDate(d);
}

/** Date separators for the message list. Returns {key, label}. */
export function dateGroup(iso, now = new Date()) {
  const d = new Date(iso);
  const today = startOfDay(now);
  const day = startOfDay(d);
  const diff = Math.round((today - day) / 86400000);
  if (diff < 0) return { key: 'future', label: 'Zukünftig' };
  if (diff === 0) return { key: 'today', label: 'Heute' };
  if (diff === 1) return { key: 'yesterday', label: 'Gestern' };
  const weekStart = startOfWeek(now);
  if (day >= weekStart) return { key: 'thisweek', label: 'Diese Woche' };
  if (day >= addDays(weekStart, -7)) return { key: 'lastweek', label: 'Letzte Woche' };
  const monthStart = startOfMonth(now);
  if (day >= monthStart) return { key: 'earliermonth', label: 'Früher in diesem Monat' };
  if (day >= new Date(now.getFullYear(), now.getMonth() - 1, 1)) return { key: 'lastmonth', label: 'Letzter Monat' };
  return { key: 'older', label: 'Älter' };
}

export function fmtSize(n) {
  if (!n && n !== 0) return '';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  return `${(n / 1024 / 1024).toFixed(1).replace('.', ',')} MB`;
}

export function initials(name, address) {
  const src = (name || address || '?').replace(/[<>"()]/g, '').trim();
  const parts = src.split(/[\s.@_-]+/).filter(Boolean);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return (parts[0] || '?').slice(0, 2).toUpperCase();
}

export const AVATAR_COLORS = ['#5b8def', '#2fa67a', '#9b6bd8', '#e07b39', '#5c9e3a', '#d0567a', '#2b9bb5', '#7a6ee0', '#c08a2b', '#4f7cac', '#c25b4d', '#3f9e9a'];
export function avatarColor(s) {
  let hsh = 0;
  for (const ch of String(s || '')) hsh = (hsh * 31 + ch.charCodeAt(0)) >>> 0;
  return AVATAR_COLORS[hsh % AVATAR_COLORS.length];
}

export function fmtAddress(a) {
  if (!a) return '';
  if (a.name && a.address && a.name !== a.address) return `${a.name} <${a.address}>`;
  return a.name || a.address || '';
}
export function displayName(a) { return a ? (a.name || a.address || '') : ''; }

export const EMAIL_RE = /^[^\s@<>(),;:"]+@[^\s@<>(),;:"]+\.[^\s@<>(),;:"]+$/;

/** Parses "Name <a@b>" / "a@b" into an Address or null. */
export function parseAddress(s) {
  s = String(s || '').trim().replace(/[;,]+$/, '');
  if (!s) return null;
  const m = s.match(/^\s*"?([^"<]*?)"?\s*<([^>]+)>\s*$/);
  if (m) {
    const address = m[2].trim();
    return EMAIL_RE.test(address) ? { name: m[1].trim() || address, address } : null;
  }
  return EMAIL_RE.test(s) ? { name: s, address: s } : null;
}

export function fileToBase64(file) {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => {
      const s = String(r.result);
      resolve(s.slice(s.indexOf(',') + 1));
    };
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });
}

/** Converts plain text from HTML (used for local previews). */
export function htmlToText(html) {
  // Inert document: a detached <div> of this document would still load images and fire onerror.
  const doc = new DOMParser().parseFromString(String(html ?? ''), 'text/html');
  return (doc.body && doc.body.textContent) || '';
}

// Touch screens fire mouse events after a tap; hover-only UI must not react to them.
let lastPointer = 'mouse';
document.addEventListener('pointerdown', (e) => { lastPointer = e.pointerType || 'mouse'; }, true);
document.addEventListener('pointermove', (e) => { if (e.pointerType === 'mouse') lastPointer = 'mouse'; }, true);
/** The last input came from a finger or pen, not a mouse. */
export function touchInput() { return lastPointer !== 'mouse'; }
