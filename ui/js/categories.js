// Outlook master-category color presets (classic desktop palette) and helpers.
import { esc } from './util.js';

export const PRESETS = [
  { id: 'preset0', name: 'Rot', hex: '#e7a1a2' },
  { id: 'preset1', name: 'Orange', hex: '#f9ba89' },
  { id: 'preset2', name: 'Braun', hex: '#f7dd8f' },
  { id: 'preset3', name: 'Gelb', hex: '#fcfa90' },
  { id: 'preset4', name: 'Grün', hex: '#78d168' },
  { id: 'preset5', name: 'Blaugrün', hex: '#9fdcc9' },
  { id: 'preset6', name: 'Olivgrün', hex: '#c6d2b0' },
  { id: 'preset7', name: 'Blau', hex: '#9db7e8' },
  { id: 'preset8', name: 'Lila', hex: '#b5a1e2' },
  { id: 'preset9', name: 'Cranberry', hex: '#daaec2' },
  { id: 'preset10', name: 'Stahl', hex: '#dad9dc' },
  { id: 'preset11', name: 'Dunkler Stahl', hex: '#6b7994' },
  { id: 'preset12', name: 'Grau', hex: '#bfbfbf' },
  { id: 'preset13', name: 'Dunkelgrau', hex: '#6f6f6f' },
  { id: 'preset14', name: 'Schwarz', hex: '#4f4f4f' },
  { id: 'preset15', name: 'Dunkelrot', hex: '#c11a25' },
  { id: 'preset16', name: 'Dunkelorange', hex: '#e2620d' },
  { id: 'preset17', name: 'Dunkelbraun', hex: '#c79930' },
  { id: 'preset18', name: 'Dunkelgelb', hex: '#b9b300' },
  { id: 'preset19', name: 'Dunkelgrün', hex: '#368f2b' },
  { id: 'preset20', name: 'Dunkles Blaugrün', hex: '#329b7a' },
  { id: 'preset21', name: 'Dunkles Olivgrün', hex: '#778b45' },
  { id: 'preset22', name: 'Dunkelblau', hex: '#2858a5' },
  { id: 'preset23', name: 'Dunkellila', hex: '#5c3fa3' },
  { id: 'preset24', name: 'Dunkles Cranberry', hex: '#93446b' },
];
const NONE = '#e0e0e0';
const BY_ID = Object.fromEntries(PRESETS.map((p) => [p.id, p]));

export function presetHex(color) {
  return (BY_ID[color] && BY_ID[color].hex) || NONE;
}

/** Text color with sufficient contrast on the given background hex. */
export function textOn(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex || '');
  if (!m) return '#222';
  const n = parseInt(m[1], 16);
  const r = n >> 16, g = (n >> 8) & 255, b = n & 255;
  const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return lum > 0.6 ? '#222' : '#fff';
}

// ---- Hidden marker categories: Kanban lanes "[K_<name>]", follow-ups "[F_<hash>]",
// recurrence "[#monthly]". They never show up as tags but must survive tag edits.
export const LANE_PREFIX = '[K_';
export const LANE_NAME_RE = /^[A-Za-z0-9ÄÖÜäöüß _-]{1,30}$/;
const HIDDEN_PREFIXES = ['[K_', '[F_', '[#'];
export function isHiddenCategory(name) {
  return typeof name === 'string' && name.endsWith(']') && HIDDEN_PREFIXES.some((p) => name.startsWith(p));
}
export function isLaneCategory(name) { return typeof name === 'string' && name.startsWith(LANE_PREFIX) && name.endsWith(']'); }
export function laneCategory(lane) { return `${LANE_PREFIX}${lane}]`; }
export function laneName(category) { return isLaneCategory(category) ? category.slice(LANE_PREFIX.length, -1) : null; }
/** Categories without hidden markers (what the user sees as tags). */
export function visibleCats(list) { return (list || []).filter((c) => !isHiddenCategory(c)); }
/** Hidden markers of a category list (lanes, follow-ups, recurrence). */
export function hiddenCats(list) { return (list || []).filter(isHiddenCategory); }

// Category lists per account (filled by the app shell). Hidden markers are kept out.
const byAccount = new Map();
export function setCategories(accountId, list) { byAccount.set(accountId, (list || []).filter((c) => !isHiddenCategory(c.displayName))); }
export function getCategories(accountId) { return byAccount.get(accountId) || []; }

export function categoryHex(accountId, name) {
  const c = getCategories(accountId).find((x) => x.displayName === name);
  return c ? presetHex(c.color) : NONE;
}

export function chipHtml(accountId, name) {
  const bg = categoryHex(accountId, name);
  return `<span class="cat-chip" style="background:${bg};color:${textOn(bg)}" title="${esc(name)}">${esc(name)}</span>`;
}

/** Larger tag pill (reading pane). */
export function pillHtml(accountId, name, removable = false) {
  const bg = categoryHex(accountId, name);
  return `<span class="tag-pill" style="background:${bg};color:${textOn(bg)}" data-name="${esc(name)}" title="${esc(name)}"><span class="tp-name">${esc(name)}</span>${removable ? `<button type="button" class="tp-x" title="Kategorie entfernen" aria-label="${esc(name)} entfernen">×</button>` : ''}</span>`;
}

export function swatchHtml(hex, size = 12) {
  return `<span class="cat-swatch" style="background:${hex};width:${size}px;height:${size}px"></span>`;
}
