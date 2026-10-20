// Tag (category) picker popover: filter, up to 20 categories as a checklist,
// inline "Neue Kategorie…", link to the category manager.
import { call } from './api.js';
import { h, esc, load, store } from './util.js';
import { icon } from './icons.js';
import { reportError, toast } from './dialogs.js';
import { PRESETS, presetHex, getCategories } from './categories.js';
import { loadCategories } from './state.js';

const MAX_VISIBLE = 20;
let current = null;

export function closeTagPicker() {
  if (current) { current.close(); current = null; }
}

function usage() { return load('tagUsage', {}); }
export function countTagUse(names) {
  const u = usage();
  for (const n of names) u[n] = (u[n] || 0) + 1;
  store('tagUsage', u);
}

/** Orders categories: assigned first, then most used, then alphabetical. */
export function orderedCategories(cats, assigned) {
  const u = usage();
  const set = new Set(assigned);
  return [...cats].sort((a, b) =>
    (set.has(b.displayName) - set.has(a.displayName))
    || ((u[b.displayName] || 0) - (u[a.displayName] || 0))
    || a.displayName.localeCompare(b.displayName, 'de', { sensitivity: 'base' }));
}

/**
 * opts: { accountId, assigned: () => string[], onToggle: async (name, on) => void, onManage: () => void }
 */
export function openTagPicker(anchor, opts) {
  closeTagPicker();
  let filter = '';
  let active = 0;
  let creating = false;
  const pop = h('div.tagpop', { role: 'dialog', 'aria-label': 'Kategorien', dataset: { testid: 'tag-popover' } });
  const input = h('input.tagpop-filter', { type: 'text', placeholder: 'Kategorie suchen…', 'aria-label': 'Kategorie suchen', autocomplete: 'off', spellcheck: false });
  const listEl = h('div.tagpop-list', { role: 'listbox', 'aria-multiselectable': 'true' });
  const footer = h('div.tagpop-footer');
  pop.append(h('div.tagpop-head', h('span', { html: icon('search', 15) }), input), listEl, footer);

  const visible = () => {
    const cats = orderedCategories(getCategories(opts.accountId), opts.assigned());
    const f = filter.trim().toLowerCase();
    return (f ? cats.filter((c) => c.displayName.toLowerCase().includes(f)) : cats).slice(0, MAX_VISIBLE);
  };

  async function toggle(name) {
    const on = !opts.assigned().includes(name);
    try {
      await opts.onToggle(name, on);
      if (on) countTagUse([name]);
    } catch (e) { reportError(e); }
    renderList();
  }

  function renderList() {
    const items = visible();
    const assigned = new Set(opts.assigned());
    if (active >= items.length) active = Math.max(0, items.length - 1);
    listEl.innerHTML = '';
    if (!items.length) listEl.appendChild(h('div.tagpop-empty', filter ? `Keine Kategorie „${filter}“.` : 'Noch keine Kategorien vorhanden.'));
    items.forEach((c, i) => {
      const on = assigned.has(c.displayName);
      const row = h('div.tag-opt' + (on ? '.on' : '') + (i === active ? '.active' : ''), {
        role: 'option', 'aria-selected': on ? 'true' : 'false', dataset: { name: c.displayName, testid: 'tag-opt-' + c.displayName }, title: c.displayName,
      },
      h('span.tag-dot', { style: { background: presetHex(c.color) } }),
      h('span.tag-name', c.displayName),
      h('span.tag-check', { html: on ? icon('check', 16) : '' }));
      row.addEventListener('mousedown', (e) => e.preventDefault());
      row.addEventListener('click', () => { active = i; toggle(c.displayName); });
      row.addEventListener('mousemove', () => { if (active !== i) { active = i; paintActive(); } });
      listEl.appendChild(row);
    });
    const total = getCategories(opts.accountId).length;
    if (!filter && total > MAX_VISIBLE) listEl.appendChild(h('div.tagpop-hint', `${total - MAX_VISIBLE} weitere – zum Finden tippen`));
  }
  function paintActive() {
    listEl.querySelectorAll('.tag-opt').forEach((r, i) => r.classList.toggle('active', i === active));
  }

  function renderFooter() {
    footer.innerHTML = '';
    if (!creating) {
      footer.append(
        h('button.tagpop-link', { type: 'button', dataset: { testid: 'tag-new' }, html: icon('plus', 15) + '<span>Neue Kategorie…</span>', onclick: () => { creating = true; renderFooter(); } }),
        h('button.tagpop-link', { type: 'button', dataset: { testid: 'tag-manage' }, html: icon('settings', 15) + '<span>Kategorien verwalten…</span>', onclick: () => { closeTagPicker(); opts.onManage && opts.onManage(); } }),
      );
      return;
    }
    let color = 'preset7';
    const name = h('input.input.tagpop-newname', { type: 'text', value: filter.trim(), placeholder: 'Name der Kategorie', name: 'newTagName' });
    const pal = h('div.tagpop-palette');
    for (const p of PRESETS) {
      const sw = h('button.pcolor' + (p.id === color ? '.on' : ''), { type: 'button', title: p.name, style: { background: p.hex }, dataset: { color: p.id } });
      sw.addEventListener('click', () => { color = p.id; pal.querySelectorAll('.pcolor').forEach((x) => x.classList.toggle('on', x === sw)); });
      pal.appendChild(sw);
    }
    const create = async () => {
      const n = name.value.trim();
      if (!n) { name.focus(); return; }
      if (getCategories(opts.accountId).some((c) => c.displayName.toLowerCase() === n.toLowerCase())) { toast('Eine Kategorie mit diesem Namen existiert bereits.', 'error'); return; }
      try {
        await call('create_category', { accountId: opts.accountId, displayName: n, color });
        await loadCategories(opts.accountId);
        creating = false; filter = ''; input.value = '';
        await opts.onToggle(n, true);
        countTagUse([n]);
        renderFooter(); renderList();
      } catch (e) { reportError(e); }
    };
    name.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); create(); } else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); creating = false; renderFooter(); input.focus(); } });
    footer.append(
      h('div.tagpop-new', name, pal,
        h('div.tagpop-newbtns',
          h('button.btn.small', { type: 'button', onclick: () => { creating = false; renderFooter(); } }, 'Abbrechen'),
          h('button.btn.small.primary', { type: 'button', dataset: { testid: 'tag-create' }, onclick: create }, 'Erstellen & zuweisen'))),
    );
    setTimeout(() => name.focus(), 0);
  }

  input.addEventListener('input', () => { filter = input.value; active = 0; renderList(); });
  input.addEventListener('keydown', (e) => {
    const items = visible();
    if (e.key === 'ArrowDown') { e.preventDefault(); active = Math.min(items.length - 1, active + 1); paintActive(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); active = Math.max(0, active - 1); paintActive(); }
    else if (e.key === 'Enter') { e.preventDefault(); if (items[active]) toggle(items[active].displayName); else if (filter.trim()) { creating = true; renderFooter(); } }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); closeTagPicker(); }
  });

  const onDown = (e) => { if (!pop.contains(e.target) && !anchor.contains(e.target)) closeTagPicker(); };
  document.addEventListener('mousedown', onDown, true);
  const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); closeTagPicker(); } };
  document.addEventListener('keydown', onKey, true);

  renderList();
  renderFooter();
  document.body.appendChild(pop);
  // position: below anchor, right-aligned to it
  const r = anchor.getBoundingClientRect();
  const pr = pop.getBoundingClientRect();
  let left = Math.min(r.right - pr.width, innerWidth - pr.width - 8);
  left = Math.max(8, left);
  let top = r.bottom + 6;
  if (top + pr.height > innerHeight - 8) top = Math.max(8, r.top - pr.height - 6);
  pop.style.left = left + 'px';
  pop.style.top = top + 'px';
  input.focus();

  current = {
    el: pop,
    close() {
      document.removeEventListener('mousedown', onDown, true);
      document.removeEventListener('keydown', onKey, true);
      pop.remove();
      if (opts.onClose) opts.onClose();
    },
  };
  return current;
}

export { esc };
