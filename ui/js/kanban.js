// Kanban board for the current mail folder. Lanes are "[K_<name>]" categories
// (docs/API.md); a mail without a configured lane sits in "Ohne Lane".
import { h, esc, fmtListDate, displayName, initials, avatarColor } from './util.js';
import { icon } from './icons.js';
import { chipHtml, visibleCats, laneName, isLaneCategory } from './categories.js';
import { fmtDueShort, fmtSnoozeTarget } from './snooze.js';
import { recurrenceOf, recurrenceBadgeHtml } from './recurrence.js';

export const NO_LANE = '';

/** Configured lane of a message ('' = none). Unknown/removed lanes count as none. */
export function laneOf(m, lanes) {
  for (const c of m.categories || []) {
    const n = laneName(c);
    if (n !== null && lanes.includes(n)) return n;
  }
  return NO_LANE;
}

/**
 * ctx: {
 *   items(): MessageSummary[], lanes(): string[], accountId(), snoozed(): bool,
 *   isSelected(id), cursor(), hideNoLane(), paneMode(): 'bottom'|'right', title(), count(),
 *   onSelect(id, event|null), onOpen(id), onSetLane(ids, lane|null), onContext(id, x, y),
 *   onDelete(permanent), dragPayload(ids), onToggleView(), onToggleNoLane(), onTogglePane(), onEditLanes()
 * }
 */
export function createBoard(host, ctx) {
  host.classList.add('board');
  host.tabIndex = 0;
  host.setAttribute('role', 'grid');
  host.setAttribute('aria-label', 'Kanban-Board');
  const head = h('div.kb-head');
  const cols = h('div.kb-cols');
  host.append(head, cols);
  let columns = []; // [{ lane, ids }]

  function columnDefs() {
    const lanes = ctx.lanes();
    const defs = lanes.map((l) => ({ lane: l, label: l }));
    if (!ctx.hideNoLane() || !lanes.length) defs.unshift({ lane: NO_LANE, label: 'Ohne Lane' });
    return defs;
  }

  function renderHead() {
    head.innerHTML = '';
    const seg = viewToggle('kanban', ctx.onToggleView);
    const pane = ctx.paneMode();
    const noLaneBtn = h('button.icon-btn.sm' + (ctx.hideNoLane() ? '.on' : ''), {
      type: 'button', dataset: { testid: 'kb-toggle-nolane' },
      title: ctx.hideNoLane() ? 'Spalte „Ohne Lane“ einblenden' : 'Spalte „Ohne Lane“ ausblenden',
      html: icon(ctx.hideNoLane() ? 'eyeOff' : 'eye', 16), onclick: () => ctx.onToggleNoLane(),
    });
    const paneBtn = h('button.icon-btn.sm', {
      type: 'button', dataset: { testid: 'kb-toggle-pane' },
      title: pane === 'bottom' ? 'Lesebereich rechts anzeigen' : 'Lesebereich unten anzeigen',
      html: icon(pane === 'bottom' ? 'paneRight' : 'paneBottom', 16), onclick: () => ctx.onTogglePane(),
    });
    const edit = h('button.icon-btn.sm', { type: 'button', title: 'Lanes bearbeiten', dataset: { testid: 'kb-edit-lanes' }, html: icon('settings', 16), onclick: () => ctx.onEditLanes() });
    head.append(
      h('span.folder-name', ctx.title()),
      h('span.lh-count', ctx.count() ? String(ctx.count()) : ''),
      h('span.spacer'),
      seg, h('span.kb-tools', noLaneBtn, paneBtn, edit));
  }

  function card(m, accountId, snoozed) {
    const sel = ctx.isSelected(m.id);
    const el = h('div.kb-card' + (m.isRead ? '' : '.unread') + (sel ? '.selected' : '') + (ctx.cursor() === m.id ? '.cursor' : ''), {
      draggable: 'true', dataset: { id: m.id, testid: 'kb-card' }, role: 'gridcell', 'aria-selected': sel ? 'true' : 'false', title: m.subject || '',
    });
    const from = m.from || {};
    const time = snoozed && m.dueAt
      ? `<span class="m-due" title="Auf Wiedervorlage bis ${esc(fmtSnoozeTarget(new Date(m.dueAt)))}">${icon('snooze', 12)} bis ${esc(fmtDueShort(m.dueAt))}</span>`
      : esc(fmtListDate(m.receivedAt));
    const tags = visibleCats(m.categories).map((c) => chipHtml(accountId, c)).join('');
    const icons = recurrenceBadgeHtml(recurrenceOf(m), { short: true }) + (m.importance === 'high' ? icon('important', 13, 'imp') : '')
      + (m.hasAttachments ? icon('attach', 13) : '')
      + (m.flag === 'flagged' ? icon('flag', 13, 'flagged') : m.flag === 'complete' ? icon('check', 13, 'done') : '');
    el.innerHTML =
      `<div class="kb-l1"><span class="avatar xs" style="background:${avatarColor(from.address || from.name)}">${esc(initials(from.name, from.address))}</span>` +
      `<span class="kb-from">${esc(displayName(m.from) || '(Unbekannt)')}</span><span class="kb-time">${time}</span></div>` +
      `<div class="kb-subj">${esc(m.subject || '(Kein Betreff)')}</div>` +
      (tags || icons ? `<div class="kb-l3"><span class="kb-tags">${tags}</span><span class="kb-icons">${icons}</span></div>` : '');
    return el;
  }

  function render() {
    renderHead();
    const lanes = ctx.lanes();
    const items = ctx.items();
    const accountId = ctx.accountId();
    const snoozed = ctx.snoozed();
    const byLane = new Map(columnDefs().map((d) => [d.lane, []]));
    for (const m of items) {
      const l = laneOf(m, lanes);
      if (byLane.has(l)) byLane.get(l).push(m);
    }
    const scrolls = new Map([...cols.querySelectorAll('.kb-col')].map((c) => [c.dataset.lane, c.querySelector('.kb-body').scrollTop]));
    cols.innerHTML = '';
    columns = [];
    columnDefs().forEach((d, idx) => {
      const ms = byLane.get(d.lane) || [];
      columns.push({ lane: d.lane, ids: ms.map((m) => m.id) });
      const body = h('div.kb-body');
      for (const m of ms) body.appendChild(card(m, accountId, snoozed));
      if (!ms.length) body.appendChild(h('div.kb-empty', d.lane === NO_LANE ? 'Alles einsortiert' : 'Hierher ziehen'));
      const num = d.lane === NO_LANE ? '' : String(lanes.indexOf(d.lane) + 1);
      const col = h('section.kb-col' + (d.lane === NO_LANE ? '.nolane' : ''), { dataset: { lane: d.lane, testid: 'kb-col-' + (d.lane || 'none') } },
        h('header.kb-colhead', { title: num && Number(num) <= 9 ? `Taste ${num}: ausgewählte Mails in diese Lane` : '' },
          h('span.kb-colname', d.label), h('span.kb-colcount', String(ms.length)),
          num && Number(num) <= 9 ? h('kbd.kb-key', num) : null),
        body);
      setupDrop(col, d.lane);
      cols.appendChild(col);
      if (scrolls.has(d.lane)) body.scrollTop = scrolls.get(d.lane);
      void idx;
    });
    if (!lanes.length) {
      cols.appendChild(h('div.kb-nolanes', { dataset: { testid: 'kb-empty-lanes' } },
        h('div.empty-ic', { html: icon('kanban', 40) }),
        h('div', 'Noch keine Lanes – in den Einstellungen anlegen.'),
        h('button.btn.primary', { type: 'button', dataset: { testid: 'kb-setup' }, onclick: () => ctx.onEditLanes() }, 'Lanes einrichten…')));
    }
  }

  function setupDrop(col, lane) {
    col.addEventListener('dragover', (e) => {
      if (!e.dataTransfer.types.includes('application/x-oc-messages')) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      col.classList.add('drop');
    });
    col.addEventListener('dragleave', (e) => { if (!col.contains(e.relatedTarget)) col.classList.remove('drop'); });
    col.addEventListener('drop', (e) => {
      col.classList.remove('drop');
      const raw = e.dataTransfer.getData('application/x-oc-messages');
      if (!raw) return;
      e.preventDefault();
      const data = JSON.parse(raw);
      const ids = (data.ids || []).filter((id) => {
        const c = columns.find((x) => x.ids.includes(id));
        return !c || c.lane !== lane;
      });
      if (ids.length) ctx.onSetLane(ids, lane || null);
    });
  }

  function paintSelection() {
    for (const c of cols.querySelectorAll('.kb-card')) {
      const sel = ctx.isSelected(c.dataset.id);
      c.classList.toggle('selected', sel);
      c.setAttribute('aria-selected', sel ? 'true' : 'false');
      c.classList.toggle('cursor', c.dataset.id === ctx.cursor());
    }
  }

  function position(id) {
    for (let ci = 0; ci < columns.length; ci++) {
      const ri = columns[ci].ids.indexOf(id);
      if (ri >= 0) return { ci, ri };
    }
    return null;
  }

  function focusCard(id) {
    const el = cols.querySelector(`.kb-card[data-id="${CSS.escape(id)}"]`);
    if (el) el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }

  const cardId = (t) => { const c = t && t.closest && t.closest('.kb-card'); return c ? c.dataset.id : null; };
  cols.addEventListener('mousedown', (e) => {
    const id = cardId(e.target);
    if (!id || e.button !== 0) return;
    ctx.onSelect(id, e);
    host.focus({ preventScroll: true });
  });
  cols.addEventListener('dblclick', (e) => { const id = cardId(e.target); if (id) ctx.onOpen(id); });
  cols.addEventListener('contextmenu', (e) => {
    const id = cardId(e.target);
    if (!id) return;
    e.preventDefault();
    if (!ctx.isSelected(id)) ctx.onSelect(id, null);
    ctx.onContext(id, e.clientX, e.clientY);
  });
  cols.addEventListener('dragstart', (e) => {
    const id = cardId(e.target);
    if (!id) return;
    if (!ctx.isSelected(id)) ctx.onSelect(id, null);
    e.dataTransfer.setData('application/x-oc-messages', JSON.stringify(ctx.dragPayload()));
    e.dataTransfer.effectAllowed = 'copyMove';
    e.target.classList.add('dragging');
  });
  cols.addEventListener('dragend', (e) => { const c = e.target.closest && e.target.closest('.kb-card'); if (c) c.classList.remove('dragging'); });

  host.addEventListener('keydown', (e) => {
    if (e.target !== host) return;
    const cur = ctx.cursor();
    let pos = cur ? position(cur) : null;
    const nonEmpty = columns.map((c, i) => (c.ids.length ? i : -1)).filter((i) => i >= 0);
    if (!pos && ['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight'].includes(e.key)) {
      e.preventDefault();
      if (nonEmpty.length) { const id = columns[nonEmpty[0]].ids[0]; ctx.onSelect(id, null); focusCard(id); }
      return;
    }
    if (!pos) return;
    if (e.shiftKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
      e.preventDefault();
      const target = pos.ci + (e.key === 'ArrowLeft' ? -1 : 1);
      if (target < 0 || target >= columns.length) return;
      ctx.onSetLane([cur], columns[target].lane || null);
      return;
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const ids = columns[pos.ci].ids;
      const ri = Math.max(0, Math.min(ids.length - 1, pos.ri + (e.key === 'ArrowDown' ? 1 : -1)));
      ctx.onSelect(ids[ri], null); focusCard(ids[ri]);
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault();
      const dir = e.key === 'ArrowLeft' ? -1 : 1;
      let ci = pos.ci + dir;
      while (ci >= 0 && ci < columns.length && !columns[ci].ids.length) ci += dir;
      if (ci < 0 || ci >= columns.length) return;
      const ids = columns[ci].ids;
      const id = ids[Math.min(pos.ri, ids.length - 1)];
      ctx.onSelect(id, null); focusCard(id);
    } else if (e.key === 'Enter') {
      e.preventDefault(); ctx.onOpen(cur);
    } else if (e.key === 'Delete') {
      e.preventDefault(); ctx.onDelete(e.shiftKey);
    }
  });

  return {
    render,
    paintSelection,
    /** Card ids in visual order (column by column). */
    order: () => columns.flatMap((c) => c.ids),
    focus: () => host.focus({ preventScroll: true }),
    reveal: focusCard,
  };
}

/** "Liste | Kanban" switch used in the list header and the board header. */
export function viewToggle(current, onToggle, { compact = false } = {}) {
  const seg = h('div.segmented.view-toggle' + (compact ? '.compact' : ''), { role: 'radiogroup', 'aria-label': 'Ansicht', title: 'Ansicht wechseln (Strg+Umschalt+K)' });
  for (const [v, label, ic] of [['list', 'Liste', 'listView'], ['kanban', 'Kanban', 'kanban']]) {
    const b = h('button.seg' + (current === v ? '.on' : ''), { type: 'button', role: 'radio', 'aria-checked': String(current === v), 'aria-label': label, title: `${label} (Strg+Umschalt+K)`, dataset: { testid: 'view-' + v }, html: icon(ic, 14) + (compact ? '' : `<span>${label}</span>`) });
    b.addEventListener('click', () => { if (current !== v) onToggle(v); });
    seg.appendChild(b);
  }
  return seg;
}

export { isLaneCategory };
