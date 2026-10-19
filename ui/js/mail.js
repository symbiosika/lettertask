// Mail module of the main window: folder tree, message list, reading pane, mail actions.
import { call, openWindow } from './api.js';
import { h, esc, $, load, store, debounce, dateGroup, fmtListDate, displayName } from './util.js';
import { icon } from './icons.js';
import { showMenu, showMenuAt, reportError, toast, confirmDialog, promptDialog, modal } from './dialogs.js';
import { chipHtml, getCategories, presetHex, swatchHtml } from './categories.js';
import { renderMessage } from './mailview.js';
import { state, mailboxes, mailboxById, accountOfMailbox, foldersOf, folderById, wellKnown, loadFolders } from './state.js';
import { openCategoryManager } from './settings.js';

const PAGE = 200;
const WK_ICON = { inbox: 'inbox', drafts: 'drafts', sentitems: 'sent', deleteditems: 'trash', archive: 'archive', junkemail: 'junk', outbox: 'outbox' };
const TOTAL_COUNT_FOLDERS = new Set(['drafts', 'outbox', 'junkemail']);
const TO_FOLDERS = new Set(['sentitems', 'drafts', 'outbox']);

const els = {};
const list = {
  items: [], total: 0, unread: 0, canLoadOlder: false, loading: false,
  sortBy: load('sortBy', 'received'), sortDesc: load('sortDesc', true),
  filter: { unreadOnly: false, flaggedOnly: false, category: null },
  text: '', search: null, // search: {text, items} when server search results are shown
  selection: new Set(), cursor: null, anchor: null,
  collapsed: new Set(),
};
let readToken = 0;
let markReadTimer = null;
let currentDetail = null;
let onCountsChanged = () => {};

// ============================================================ setup
export function initMail({ navHost, contentHost, onCounts }) {
  onCountsChanged = onCounts || onCountsChanged;
  els.nav = navHost;
  els.content = contentHost;
  els.mailview = h('div.mailview#mailview');
  els.listpane = h('div.listpane#listpane');
  els.title = h('div.pane-title', h('span.folder-name'), h('span.sub'));
  els.filterbar = h('div.filterbar');
  els.list = h('div.msglist#msglist', { tabindex: '0' });
  els.listpane.append(els.title, els.filterbar, els.list);
  els.split = h('div.splitter');
  els.read = h('div.readpane#readpane');
  els.mailview.append(els.listpane, els.split, els.read);
  contentHost.appendChild(els.mailview);

  applyReadingPaneLayout();
  setupListEvents();
  renderFilterBar();
}

export function applyReadingPaneLayout() {
  const mode = (state.config && state.config.readingPane) || 'right';
  els.mailview.classList.toggle('bottom', mode === 'bottom');
  els.mailview.classList.toggle('off', mode === 'off');
  els.split.className = 'splitter' + (mode === 'bottom' ? ' h' : '');
  els.split.classList.toggle('hidden', mode === 'off');
  els.read.classList.toggle('hidden', mode === 'off');
  if (mode === 'bottom') { els.listpane.style.width = ''; els.listpane.style.height = load('listHeight', 320) + 'px'; }
  else if (mode === 'right') { els.listpane.style.height = ''; els.listpane.style.width = load('listWidth', 640) + 'px'; }
  else { els.listpane.style.width = ''; els.listpane.style.height = ''; }
  makeSplitter(els.split, els.listpane, mode === 'bottom' ? 'y' : 'x', mode === 'bottom' ? 'listHeight' : 'listWidth');
}

export function makeSplitter(split, target, axis, key) {
  split.onmousedown = (e) => {
    e.preventDefault();
    const start = axis === 'x' ? e.clientX : e.clientY;
    const startSize = axis === 'x' ? target.offsetWidth : target.offsetHeight;
    const frames = document.querySelectorAll('iframe');
    frames.forEach((f) => (f.style.pointerEvents = 'none'));
    const move = (ev) => {
      const d = (axis === 'x' ? ev.clientX : ev.clientY) - start;
      const size = Math.max(120, startSize + d);
      target.style[axis === 'x' ? 'width' : 'height'] = size + 'px';
    };
    const up = () => {
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', up);
      frames.forEach((f) => (f.style.pointerEvents = ''));
      store(key, axis === 'x' ? target.offsetWidth : target.offsetHeight);
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', up);
  };
}

export function showMailView(show) {
  els.mailview.classList.toggle('hidden', !show);
}

// ============================================================ folder tree
function expanded() { return new Set(load('expanded', [])); }
function setExpanded(key, on) {
  const s = expanded();
  if (on) s.add(key); else s.delete(key);
  store('expanded', [...s]);
}
function isExpanded(key) {
  if (key.startsWith('root|')) return !load('collapsedRoots', []).includes(key);
  return expanded().has(key);
}
function toggleExpand(key) {
  if (key.startsWith('root|')) {
    const c = new Set(load('collapsedRoots', []));
    if (c.has(key)) c.delete(key); else c.add(key);
    store('collapsedRoots', [...c]);
  } else {
    setExpanded(key, !isExpanded(key));
  }
  renderFolderTree();
}

function favorites() {
  const fav = load('favorites', null);
  if (fav) return fav;
  const mb = mailboxes().find((m) => m.kind === 'primary') || mailboxes()[0];
  if (!mb) return [];
  return ['inbox', 'sentitems', 'deleteditems'].map((w) => {
    const f = wellKnown(mb.id, w);
    return f ? { mailboxId: mb.id, folderId: f.id } : null;
  }).filter(Boolean);
}
function setFavorites(f) { store('favorites', f); renderFolderTree(); }

function folderCountHtml(f) {
  if (TOTAL_COUNT_FOLDERS.has(f.wellKnown)) return f.totalCount > 0 ? `<span class="count total">[${f.totalCount}]</span>` : '';
  return f.unreadCount > 0 ? `<span class="count">(${f.unreadCount})</span>` : '';
}

function folderNode(f, depth, opts = {}) {
  const hasChildren = !opts.flat && foldersOf(f.mailboxId).some((c) => c.parentId === f.id);
  const key = `${f.mailboxId}|${f.id}`;
  const open = hasChildren && isExpanded(key);
  const sel = state.mode === 'mail' && state.current.mailboxId === f.mailboxId && state.current.folderId === f.id;
  const unread = f.unreadCount > 0 && !TOTAL_COUNT_FOLDERS.has(f.wellKnown);
  const node = h('div.tree-node' + (sel ? '.selected' : '') + (unread ? '.has-unread' : ''), {
    style: { paddingLeft: 6 + depth * 14 + 'px' }, dataset: { mailboxId: f.mailboxId, folderId: f.id }, title: f.displayName,
    role: 'treeitem', 'aria-selected': sel ? 'true' : 'false',
  });
  const tw = h('span.twisty', { html: hasChildren ? icon(open ? 'triDown' : 'triRight', 10) : '' });
  if (hasChildren) tw.addEventListener('click', (e) => { e.stopPropagation(); toggleExpand(key); });
  node.append(tw, h('span', { html: icon(WK_ICON[f.wellKnown] || 'folder', 16) }), h('span.fname', f.displayName), h('span', { html: folderCountHtml(f) }));
  node.addEventListener('click', () => selectFolder(f.mailboxId, f.id));
  node.addEventListener('contextmenu', (e) => { e.preventDefault(); folderContextMenu(f, e.clientX, e.clientY); });
  // drop target
  node.addEventListener('dragover', (e) => {
    if (!e.dataTransfer.types.includes('application/x-oc-messages')) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = e.ctrlKey ? 'copy' : 'move';
    node.classList.add('drop-target');
  });
  node.addEventListener('dragleave', () => node.classList.remove('drop-target'));
  node.addEventListener('drop', (e) => {
    node.classList.remove('drop-target');
    const raw = e.dataTransfer.getData('application/x-oc-messages');
    if (!raw) return;
    e.preventDefault();
    const data = JSON.parse(raw);
    if (data.mailboxId !== f.mailboxId) { toast('Verschieben zwischen Postfächern wird nicht unterstützt.', 'error'); return; }
    if (data.folderId === f.id) return;
    moveOrCopy(data.ids, f.id, e.ctrlKey ? 'copy' : 'move').catch(reportError);
  });
  const frag = [node];
  if (open) {
    for (const c of foldersOf(f.mailboxId).filter((c) => c.parentId === f.id)) frag.push(...folderNode(c, depth + 1));
  }
  return frag;
}

export function renderFolderTree() {
  if (!els.nav) return;
  const scroll = els.nav.scrollTop;
  els.nav.innerHTML = '';
  // favorites
  const favKey = 'root|favorites';
  const favOpen = isExpanded(favKey);
  const favs = favorites().map((f) => folderById(f.mailboxId, f.folderId)).filter(Boolean);
  const favSec = h('div.tree-section', { role: 'tree' });
  const favRoot = h('div.tree-root', h('span.twisty', { html: icon(favOpen ? 'triDown' : 'triRight', 10) }), h('span', { html: icon('star', 14) }), h('span', { style: { marginLeft: '5px' } }, 'Favoriten'));
  favRoot.addEventListener('click', () => toggleExpand(favKey));
  favSec.appendChild(favRoot);
  if (favOpen) {
    for (const f of favs) {
      const [n] = folderNode(f, 1, { flat: true });
      n.dataset.favorite = '1';
      favSec.appendChild(n);
    }
  }
  els.nav.appendChild(favSec);

  for (const mb of mailboxes()) {
    const key = 'root|' + mb.id;
    const open = isExpanded(key);
    const sec = h('div.tree-section', { role: 'tree', dataset: { mailboxRoot: mb.id } });
    const root = h('div.tree-root', { title: mb.address },
      h('span.twisty', { html: icon(open ? 'triDown' : 'triRight', 10) }),
      h('span', { html: icon(mb.kind === 'shared' ? 'shared' : 'mailbox', 14) }),
      h('span', { style: { marginLeft: '5px', overflow: 'hidden', textOverflow: 'ellipsis' } }, mb.address || mb.displayName));
    root.addEventListener('click', () => toggleExpand(key));
    root.addEventListener('contextmenu', (e) => { e.preventDefault(); mailboxContextMenu(mb, e.clientX, e.clientY); });
    sec.appendChild(root);
    if (open) {
      const fs = foldersOf(mb.id);
      const ids = new Set(fs.map((f) => f.id));
      for (const f of fs.filter((f) => !f.parentId || !ids.has(f.parentId))) sec.append(...folderNode(f, 1));
      if (!fs.length) sec.appendChild(h('div.tree-node', { style: { paddingLeft: '34px', color: '#888' } }, 'Wird geladen…'));
    }
    els.nav.appendChild(sec);
  }
  els.nav.scrollTop = scroll;
}

function mailboxContextMenu(mb, x, y) {
  showMenu([
    { label: 'Ordner synchronisieren', icon: 'sync', onClick: async () => { await call('sync_folders', { mailboxId: mb.id }); await refreshFolders(mb.id); } },
    { label: 'Neuer Ordner…', icon: 'folder', onClick: () => newFolder(mb.id, null) },
    { separator: true },
    mb.kind === 'shared' ? { label: 'Postfach entfernen', icon: 'delete', onClick: async () => {
      if (await confirmDialog(`Das freigegebene Postfach „${mb.address}" aus der Ansicht entfernen?`)) {
        await call('remove_mailbox', { mailboxId: mb.id });
      }
    } } : null,
  ].filter(Boolean), x, y);
}

function folderContextMenu(f, x, y) {
  const favs = favorites();
  const isFav = favs.some((v) => v.mailboxId === f.mailboxId && v.folderId === f.id);
  const canEmpty = f.wellKnown === 'deleteditems' || f.wellKnown === 'junkemail';
  showMenu([
    { label: 'Neuer Ordner…', icon: 'folder', onClick: () => newFolder(f.mailboxId, f.id) },
    { label: 'Ordner umbenennen…', disabled: !!f.wellKnown, onClick: async () => {
      const name = await promptDialog('Neuer Name:', { title: 'Ordner umbenennen', value: f.displayName });
      if (name && name !== f.displayName) { await call('rename_folder', { mailboxId: f.mailboxId, folderId: f.id, name }); await refreshFolders(f.mailboxId); }
    } },
    { label: 'Ordner löschen', icon: 'delete', disabled: !!f.wellKnown, onClick: async () => {
      if (await confirmDialog(`Möchten Sie den Ordner „${f.displayName}" und seinen gesamten Inhalt in den Ordner „Gelöschte Elemente" verschieben?`, { danger: true, ok: 'Ja', cancel: 'Nein' })) {
        await call('delete_folder', { mailboxId: f.mailboxId, folderId: f.id });
        if (state.current.folderId === f.id) { const inbox = wellKnown(f.mailboxId, 'inbox'); if (inbox) await selectFolder(f.mailboxId, inbox.id); }
        await refreshFolders(f.mailboxId);
      }
    } },
    { separator: true },
    { label: 'Alle als gelesen markieren', icon: 'mailOpen', disabled: !f.unreadCount, onClick: async () => {
      await call('mark_folder_read', { mailboxId: f.mailboxId, folderId: f.id });
      await refreshFolders(f.mailboxId);
      if (state.current.folderId === f.id) await reloadList();
    } },
    canEmpty ? { label: 'Ordner leeren', icon: 'trash', onClick: async () => {
      if (await confirmDialog(`Alle Elemente im Ordner „${f.displayName}" werden endgültig gelöscht. Fortfahren?`, { danger: true, ok: 'Ja', cancel: 'Nein', title: 'Ordner leeren' })) {
        await call('empty_folder', { mailboxId: f.mailboxId, folderId: f.id });
        await refreshFolders(f.mailboxId);
        if (state.current.folderId === f.id) await reloadList();
      }
    } } : null,
    { label: 'Ordner synchronisieren', icon: 'sync', onClick: async () => {
      await call('sync_folder', { mailboxId: f.mailboxId, folderId: f.id });
      await refreshFolders(f.mailboxId);
      if (state.current.folderId === f.id) await reloadList();
    } },
    { separator: true },
    isFav
      ? { label: 'Aus Favoriten entfernen', icon: 'star', onClick: () => setFavorites(favs.filter((v) => !(v.mailboxId === f.mailboxId && v.folderId === f.id))) }
      : { label: 'Zu Favoriten hinzufügen', icon: 'star', onClick: () => setFavorites([...favs, { mailboxId: f.mailboxId, folderId: f.id }]) },
  ].filter(Boolean), x, y);
}

async function newFolder(mailboxId, parentId) {
  const name = await promptDialog('Name des neuen Ordners:', { title: 'Neuen Ordner erstellen' });
  if (!name) return;
  await call('create_folder', { mailboxId, parentId, name });
  if (parentId) setExpanded(`${mailboxId}|${parentId}`, true);
  await refreshFolders(mailboxId);
}

export async function refreshFolders(mailboxId) {
  await loadFolders(mailboxId);
  renderFolderTree();
  updateTitle();
  onCountsChanged();
}

export async function selectFolder(mailboxId, folderId) {
  const changed = state.current.mailboxId !== mailboxId || state.current.folderId !== folderId;
  state.current = { mailboxId, folderId };
  store('lastFolder', state.current);
  if (changed) {
    list.selection.clear(); list.cursor = null; list.anchor = null; list.search = null; list.items = [];
    list.filter = { unreadOnly: false, flaggedOnly: false, category: null };
    list.text = '';
    const sb = $('#search-input'); if (sb) sb.value = '';
    els.list.scrollTop = 0;
    showReading(null);
  }
  renderFolderTree();
  renderFilterBar();
  updateTitle();
  call('set_active_folder', { mailboxId, folderId }, { silent: true }).catch(() => {});
  await reloadList();
  // pull fresh data in the background
  call('sync_folder', { mailboxId, folderId }, { silent: true })
    .then(() => (state.current.folderId === folderId ? reloadList() : null))
    .catch(() => {});
}

function updateTitle() {
  const f = folderById(state.current.mailboxId, state.current.folderId);
  const mb = mailboxById(state.current.mailboxId);
  els.title.querySelector('.folder-name').textContent = list.search ? 'Suchergebnisse' : (f ? f.displayName : '');
  els.title.querySelector('.sub').textContent = mb ? '– ' + mb.address : '';
  document.title = `${f ? f.displayName : 'Outlook'} – ${mb ? mb.address : ''} – Outlook Classic`;
}

// ============================================================ list
function currentFolder() { return folderById(state.current.mailboxId, state.current.folderId); }

export async function reloadList({ keepCount = true } = {}) {
  const { mailboxId, folderId } = state.current;
  if (!mailboxId || !folderId) { renderList(); return; }
  if (list.search) { renderList(); return; }
  const limit = keepCount ? Math.max(PAGE, list.items.length) : PAGE;
  const res = await call('list_messages', {
    mailboxId, folderId, sortBy: list.sortBy, sortDesc: list.sortDesc,
    unreadOnly: list.filter.unreadOnly, flaggedOnly: list.filter.flaggedOnly, category: list.filter.category,
    text: list.text || null, offset: 0, limit,
  }, { silent: true });
  if (state.current.folderId !== folderId) return;
  list.items = res.items || [];
  list.total = res.total || 0;
  list.unread = res.unread || 0;
  list.canLoadOlder = !!res.canLoadOlder;
  const ids = new Set(list.items.map((m) => m.id));
  for (const id of [...list.selection]) if (!ids.has(id)) list.selection.delete(id);
  if (list.cursor && !ids.has(list.cursor)) list.cursor = null;
  renderList();
  onCountsChanged();
}

async function loadMore() {
  if (list.loading || list.search) return;
  const { mailboxId, folderId } = state.current;
  if (list.items.length < list.total) {
    list.loading = true;
    try {
      const res = await call('list_messages', {
        mailboxId, folderId, sortBy: list.sortBy, sortDesc: list.sortDesc,
        unreadOnly: list.filter.unreadOnly, flaggedOnly: list.filter.flaggedOnly, category: list.filter.category,
        text: list.text || null, offset: list.items.length, limit: PAGE,
      }, { silent: true });
      if (state.current.folderId !== folderId) return;
      const have = new Set(list.items.map((m) => m.id));
      list.items.push(...(res.items || []).filter((m) => !have.has(m.id)));
      list.total = res.total; list.canLoadOlder = !!res.canLoadOlder;
      renderList();
    } finally { list.loading = false; }
  } else if (list.canLoadOlder) {
    list.loading = true;
    renderList();
    try {
      const n = await call('load_older', { mailboxId, folderId }, { silent: true });
      if (!n) list.canLoadOlder = false;
    } finally { list.loading = false; }
    await reloadList();
  }
}

const COLUMNS = [
  { key: 'importance', label: icon('important', 12), title: 'Wichtigkeit', w: 20, icon: true, sort: 'importance' },
  { key: 'read', label: icon('mail', 13), title: 'Gelesen/Ungelesen', w: 24, icon: true },
  { key: 'attachments', label: icon('attach', 13), title: 'Anlage', w: 22, icon: true, sort: 'attachments' },
  { key: 'from', label: 'Von', w: 140, sort: 'from' },
  { key: 'subject', label: 'Betreff', w: null, sort: 'subject' },
  { key: 'received', label: 'Erhalten', w: 112, sort: 'received' },
  { key: 'categories', label: 'Kategorien', w: 96, sort: 'categories' },
  { key: 'flag', label: icon('flag', 13), title: 'Kennzeichnungsstatus', w: 24, icon: true, sort: 'flag' },
];

function visibleItems() { return list.search ? list.search.items : list.items; }

function renderList() {
  const scroll = els.list.scrollTop;
  const compact = state.config && state.config.listMode === 'compact';
  const f = currentFolder();
  const toMode = f && TO_FOLDERS.has(f.wellKnown);
  const widths = load('colWidths', {});
  const items = visibleItems();
  const accountId = accountOfMailbox(state.current.mailboxId);

  const table = h('table.msgtable' + (compact ? '.compact' : ''), { role: 'grid', 'aria-label': 'Nachrichtenliste' });
  const colgroup = h('colgroup');
  for (const c of compact ? [{ key: 'all' }] : COLUMNS) {
    const w = c.key === 'all' ? null : (widths[c.key] ?? c.w);
    colgroup.appendChild(h('col', { style: w ? { width: w + 'px' } : {} }));
  }
  table.appendChild(colgroup);
  if (!compact) {
    const tr = h('tr');
    for (const c of COLUMNS) {
      const label = c.key === 'from' && toMode ? 'An' : c.key === 'received' && toMode && f.wellKnown !== 'drafts' ? 'Gesendet' : c.label;
      const sorted = c.sort && list.sortBy === c.sort;
      const th = h('th' + (c.icon ? '.c-icon' : '') + (sorted ? '.sorted' : ''), { title: c.title || '', dataset: { col: c.key } });
      th.innerHTML = c.icon ? label : esc(label);
      if (sorted && !c.icon) th.insertAdjacentHTML('beforeend', `<span class="sort-ind">${list.sortDesc ? '▼' : '▲'}</span>`);
      if (c.sort) th.addEventListener('click', (e) => { if (e.target.classList.contains('resizer')) return; sortBy(c.sort); });
      if (c.w) {
        const rz = h('span.resizer');
        rz.addEventListener('mousedown', (e) => startColResize(e, c.key, th));
        rz.addEventListener('click', (e) => e.stopPropagation());
        th.appendChild(rz);
      }
      tr.appendChild(th);
    }
    table.appendChild(h('thead', tr));
  }

  const tbody = h('tbody');
  const ncols = compact ? 1 : COLUMNS.length;
  const groupByDate = list.sortBy === 'received' && !list.search;
  let lastGroup = null;
  let groupRows = null;
  const groups = groupByDate ? countGroups(items) : null;
  for (const m of items) {
    if (groupByDate) {
      const g = dateGroup(m.receivedAt);
      if (g.key !== lastGroup) {
        lastGroup = g.key;
        const collapsed = list.collapsed.has(g.key);
        const gtr = h('tr.group', { dataset: { group: g.key } }, h('td', { colSpan: ncols },
          h('span.twisty', { html: icon(collapsed ? 'triRight' : 'triDown', 10) }), g.label, h('span.gcount', `(${groups.get(g.key)})`)));
        gtr.addEventListener('click', () => { if (list.collapsed.has(g.key)) list.collapsed.delete(g.key); else list.collapsed.add(g.key); renderList(); });
        tbody.appendChild(gtr);
        groupRows = collapsed;
      }
      if (groupRows) continue;
    }
    tbody.appendChild(compact ? compactRow(m, accountId, toMode) : columnRow(m, accountId, toMode));
  }
  table.appendChild(tbody);
  els.list.innerHTML = '';
  els.list.appendChild(table);
  if (!items.length) {
    els.list.appendChild(h('div.empty', list.search ? 'Es wurden keine Ergebnisse gefunden.' : 'Keine Elemente in dieser Ansicht.'));
  } else if (!list.search && (list.items.length < list.total || list.canLoadOlder || list.loading)) {
    els.list.appendChild(h('div.load-more', list.loading ? 'Weitere Elemente werden geladen…' : (list.canLoadOlder && list.items.length >= list.total ? 'Weitere Elemente auf dem Server – nach unten scrollen, um sie zu laden.' : '')));
  }
  els.list.scrollTop = scroll;
  renderFilterBar();
}

function countGroups(items) {
  const m = new Map();
  for (const it of items) { const k = dateGroup(it.receivedAt).key; m.set(k, (m.get(k) || 0) + 1); }
  return m;
}

function rowBase(m) {
  const cls = 'tr.msg' + (m.isRead ? '' : '.unread') + (list.selection.has(m.id) ? '.selected' : '') + (list.cursor === m.id ? '.cursor' : '');
  const tr = h(cls, { draggable: 'true', dataset: { id: m.id }, role: 'row', 'aria-selected': list.selection.has(m.id) ? 'true' : 'false' });
  return tr;
}

function flagIcon(m) {
  if (m.flag === 'flagged') return icon('flag', 14);
  if (m.flag === 'complete') return icon('check', 14);
  return icon('flagOutline', 14, 'flagOutline');
}

function columnRow(m, accountId, toMode) {
  const tr = rowBase(m);
  const who = toMode ? (m.to || []).map(displayName).join('; ') : displayName(m.from);
  tr.innerHTML =
    `<td class="c-icon">${m.importance === 'high' ? icon('important', 12) : m.importance === 'low' ? icon('lowImportance', 12) : ''}</td>` +
    `<td class="c-icon readcell" title="${m.isRead ? 'Als ungelesen markieren' : 'Als gelesen markieren'}">${icon(m.isDraft ? 'drafts' : m.isRead ? 'mailOpen' : 'mailClosed', 15)}</td>` +
    `<td class="c-icon">${m.hasAttachments ? icon('attach', 13) : ''}</td>` +
    `<td class="c-from" title="${esc(who)}">${m.isDraft && !toMode ? '<span style="color:#c8262b">[Entwurf] </span>' : ''}${esc(who)}</td>` +
    `<td class="c-subject" title="${esc(m.subject)}">${esc(m.subject || '(Kein Betreff)')}</td>` +
    `<td class="c-date">${esc(fmtListDate(m.receivedAt))}</td>` +
    `<td class="c-cats">${(m.categories || []).map((c) => chipHtml(accountId, c)).join('')}</td>` +
    `<td class="c-icon flagcell" title="Kennzeichnung">${flagIcon(m)}</td>`;
  return tr;
}

function compactRow(m, accountId, toMode) {
  const tr = rowBase(m);
  const who = toMode ? (m.to || []).map(displayName).join('; ') : displayName(m.from);
  tr.innerHTML = `<td><div class="cmp">
    <div class="cmp-from">${esc(who || '(Unbekannt)')}</div>
    <div class="cmp-date">${esc(fmtListDate(m.receivedAt))}</div>
    <div class="cmp-subj">${m.importance === 'high' ? icon('important', 11) : ''}${esc(m.subject || '(Kein Betreff)')}</div>
    <div class="cmp-icons">${(m.categories || []).map((c) => chipHtml(accountId, c)).join('')}${m.hasAttachments ? icon('attach', 12) : ''}<span class="flagcell">${flagIcon(m)}</span></div>
    <div class="cmp-prev">${esc(m.preview || '')}</div></div></td>`;
  return tr;
}

function startColResize(e, key, th) {
  e.preventDefault(); e.stopPropagation();
  const startX = e.clientX;
  const startW = th.offsetWidth;
  const idx = COLUMNS.findIndex((c) => c.key === key);
  const col = els.list.querySelectorAll('col')[idx];
  const move = (ev) => { col.style.width = Math.max(20, startW + ev.clientX - startX) + 'px'; };
  const up = () => {
    document.removeEventListener('mousemove', move); document.removeEventListener('mouseup', up);
    const w = load('colWidths', {}); w[key] = parseInt(col.style.width, 10); store('colWidths', w);
  };
  document.addEventListener('mousemove', move); document.addEventListener('mouseup', up);
}

function sortBy(key) {
  if (list.sortBy === key) list.sortDesc = !list.sortDesc;
  else { list.sortBy = key; list.sortDesc = key === 'received' || key === 'importance' || key === 'flag' || key === 'attachments'; }
  store('sortBy', list.sortBy); store('sortDesc', list.sortDesc);
  if (list.search) {
    list.search.items.sort(localComparator());
    renderList();
  } else reloadList({ keepCount: false }).catch(reportError);
}

function localComparator() {
  const dir = list.sortDesc ? -1 : 1;
  const key = {
    received: (m) => m.receivedAt, from: (m) => displayName(m.from).toLowerCase(), subject: (m) => (m.subject || '').toLowerCase(),
    importance: (m) => ({ low: 0, normal: 1, high: 2 }[m.importance] || 1), flag: (m) => ({ notFlagged: 0, complete: 1, flagged: 2 }[m.flag] || 0),
    attachments: (m) => (m.hasAttachments ? 1 : 0), categories: (m) => (m.categories || []).join(','),
  }[list.sortBy] || ((m) => m.receivedAt);
  return (a, b) => (key(a) < key(b) ? -dir : key(a) > key(b) ? dir : 0);
}

// ---------- filter bar ----------
function renderFilterBar() {
  const fb = els.filterbar;
  if (!fb) return;
  fb.innerHTML = '';
  if (list.search) {
    fb.append(h('span', { html: icon('search', 13) }),
      h('span', `Suchergebnisse für „${list.search.text}" (${list.search.items.length})`),
      h('span.search-info', h('span.link', { onclick: () => clearSearch() }, 'Suche schließen')));
    return;
  }
  const mk = (label, on, fn, testid) => h('button.fbtn' + (on ? '.on' : ''), { type: 'button', onclick: fn, dataset: { testid } }, label);
  const f = list.filter;
  fb.append(
    mk('Alle', !f.unreadOnly && !f.flaggedOnly && !f.category, () => setFilter({ unreadOnly: false, flaggedOnly: false, category: null }), 'filter-all'),
    mk('Ungelesen', f.unreadOnly, () => setFilter({ ...f, unreadOnly: !f.unreadOnly }), 'filter-unread'),
    mk('Gekennzeichnet', f.flaggedOnly, () => setFilter({ ...f, flaggedOnly: !f.flaggedOnly }), 'filter-flagged'),
  );
  const catBtn = h('button.fbtn' + (f.category ? '.on' : ''), { type: 'button', dataset: { testid: 'filter-category' } });
  catBtn.innerHTML = f.category ? `${icon('tag', 12)} ${esc(f.category)} ▾` : 'Kategorie ▾';
  catBtn.addEventListener('click', () => {
    const accountId = accountOfMailbox(state.current.mailboxId);
    showMenuAt(catBtn, [
      { label: 'Alle Kategorien', checked: !f.category, onClick: () => setFilter({ ...f, category: null }) },
      { separator: true },
      ...getCategories(accountId).map((c) => ({ label: c.displayName, iconHtml: swatchHtml(presetHex(c.color)), checked: f.category === c.displayName, onClick: () => setFilter({ ...f, category: c.displayName }) })),
    ]);
  });
  fb.appendChild(catBtn);
  if (list.text) fb.appendChild(h('span.search-info', `Gefiltert: „${list.text}"`, h('span.link', { onclick: () => { list.text = ''; const sb = $('#search-input'); if (sb) sb.value = ''; reloadList({ keepCount: false }).catch(reportError); } }, 'Filter entfernen')));
}

function setFilter(f) {
  list.filter = f;
  reloadList({ keepCount: false }).catch(reportError);
}

// ---------- search ----------
export const setLocalFilter = debounce((text) => {
  if (list.search) return;
  list.text = text.trim();
  reloadList({ keepCount: false }).catch(reportError);
}, 250);

export async function serverSearch(text) {
  text = text.trim();
  if (!text) { clearSearch(); return; }
  const { mailboxId, folderId } = state.current;
  if (!mailboxId) return;
  const items = await call('search_messages', { mailboxId, text, folderId });
  list.search = { text, items: items || [] };
  list.selection.clear(); list.cursor = null;
  showReading(null);
  updateTitle();
  renderList();
}

export function clearSearch() {
  list.search = null;
  list.text = '';
  const sb = $('#search-input'); if (sb) sb.value = '';
  updateTitle();
  reloadList({ keepCount: false }).catch(reportError);
}

// ---------- events & selection ----------
function rowId(el) { const tr = el && el.closest && el.closest('tr.msg'); return tr ? tr.dataset.id : null; }
function findItem(id) { return visibleItems().find((m) => m.id === id) || null; }
function visibleOrder() { return Array.from(els.list.querySelectorAll('tr.msg')).map((tr) => tr.dataset.id); }

function setupListEvents() {
  const L = els.list;
  L.addEventListener('mousedown', (e) => {
    const id = rowId(e.target);
    if (!id || e.button !== 0) return;
    if (e.target.closest('.flagcell') || e.target.closest('.readcell')) return;
    if (e.ctrlKey || e.metaKey) {
      if (list.selection.has(id)) list.selection.delete(id); else list.selection.add(id);
      list.anchor = id;
    } else if (e.shiftKey && list.anchor) {
      selectRange(list.anchor, id);
    } else if (!list.selection.has(id) || list.selection.size === 1) {
      list.selection = new Set([id]); list.anchor = id;
    }
    list.cursor = id;
    paintSelection();
    selectionChanged();
  });
  L.addEventListener('click', (e) => {
    const id = rowId(e.target);
    if (!id) return;
    const m = findItem(id);
    if (e.target.closest('.flagcell')) {
      const next = { notFlagged: 'flagged', flagged: 'complete', complete: 'notFlagged' }[m.flag] || 'flagged';
      applyFlag([id], next).catch(reportError);
    } else if (e.target.closest('.readcell')) {
      applyRead([id], !m.isRead).catch(reportError);
    } else if (!e.ctrlKey && !e.shiftKey && list.selection.size > 1) {
      list.selection = new Set([id]); list.anchor = id; paintSelection(); selectionChanged();
    }
  });
  L.addEventListener('dblclick', (e) => { const id = rowId(e.target); if (id && !e.target.closest('.flagcell,.readcell')) openMessage(findItem(id)); });
  L.addEventListener('contextmenu', (e) => {
    const id = rowId(e.target);
    if (!id) return;
    e.preventDefault();
    if (!list.selection.has(id)) { list.selection = new Set([id]); list.cursor = id; list.anchor = id; paintSelection(); selectionChanged(); }
    messageContextMenu(e.clientX, e.clientY);
  });
  L.addEventListener('dragstart', (e) => {
    const id = rowId(e.target);
    if (!id) return;
    if (!list.selection.has(id)) { list.selection = new Set([id]); paintSelection(); selectionChanged(); }
    e.dataTransfer.setData('application/x-oc-messages', JSON.stringify({ mailboxId: state.current.mailboxId, folderId: state.current.folderId, ids: [...list.selection] }));
    e.dataTransfer.effectAllowed = 'copyMove';
  });
  L.addEventListener('scroll', () => {
    if (L.scrollTop + L.clientHeight > L.scrollHeight - 150) loadMore().catch(reportError);
  });
  L.addEventListener('keydown', (e) => {
    const order = visibleOrder();
    if (!order.length) return;
    let idx = order.indexOf(list.cursor);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Home' || e.key === 'End' || e.key === 'PageDown' || e.key === 'PageUp') {
      e.preventDefault();
      const page = Math.max(1, Math.floor(L.clientHeight / 23) - 1);
      if (e.key === 'ArrowDown') idx = Math.min(order.length - 1, idx + 1);
      else if (e.key === 'ArrowUp') idx = Math.max(0, idx < 0 ? 0 : idx - 1);
      else if (e.key === 'Home') idx = 0;
      else if (e.key === 'End') idx = order.length - 1;
      else if (e.key === 'PageDown') idx = Math.min(order.length - 1, idx + page);
      else idx = Math.max(0, idx - page);
      const id = order[idx];
      if (e.shiftKey && list.anchor) selectRange(list.anchor, id);
      else { list.selection = new Set([id]); list.anchor = id; }
      list.cursor = id;
      paintSelection();
      const tr = L.querySelector(`tr.msg[data-id="${CSS.escape(id)}"]`);
      if (tr) tr.scrollIntoView({ block: 'nearest' });
      selectionChanged();
    } else if (e.key === 'a' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      list.selection = new Set(order);
      paintSelection(); selectionChanged();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (list.cursor) openMessage(findItem(list.cursor));
    } else if (e.key === 'Delete') {
      e.preventDefault();
      deleteSelected(e.shiftKey).catch(reportError);
    }
  });
}

function selectRange(a, b) {
  const order = visibleOrder();
  let i = order.indexOf(a), j = order.indexOf(b);
  if (i < 0) i = j;
  if (i > j) [i, j] = [j, i];
  list.selection = new Set(order.slice(i, j + 1));
}

function paintSelection() {
  for (const tr of els.list.querySelectorAll('tr.msg')) {
    const sel = list.selection.has(tr.dataset.id);
    tr.classList.toggle('selected', sel);
    tr.setAttribute('aria-selected', sel ? 'true' : 'false');
    tr.classList.toggle('cursor', tr.dataset.id === list.cursor);
  }
}

const selectionChanged = debounce(() => {
  onCountsChanged();
  if (list.selection.size === 1) showReading(findItem([...list.selection][0]));
  else if (list.selection.size > 1) showReading(null, `${list.selection.size} Elemente ausgewählt.`);
  else showReading(null);
}, 60);

export function getSelection() {
  const items = visibleItems().filter((m) => list.selection.has(m.id));
  return { mailboxId: state.current.mailboxId, folderId: state.current.folderId, ids: items.map((m) => m.id), items };
}
export function listStats() { return { total: list.search ? list.search.items.length : list.total, unread: list.unread }; }

// ============================================================ reading pane
async function showReading(m, text) {
  clearTimeout(markReadTimer);
  const token = ++readToken;
  currentDetail = null;
  const mode = (state.config && state.config.readingPane) || 'right';
  if (!m) {
    els.read.innerHTML = '';
    els.read.appendChild(h('div.empty', text || 'Wählen Sie ein Element aus, um es zu lesen.'));
    return;
  }
  if (mode === 'off') return;
  try {
    const detail = await call('get_message', { mailboxId: m.mailboxId || state.current.mailboxId, messageId: m.id }, { silent: true });
    if (token !== readToken) return;
    currentDetail = detail;
    renderMessage(els.read, detail, {
      accountId: accountOfMailbox(detail.mailboxId || state.current.mailboxId),
      config: state.config,
      onChanged: () => {},
    });
    scheduleMarkRead(m);
  } catch (e) {
    if (token !== readToken) return;
    els.read.innerHTML = '';
    els.read.appendChild(h('div.empty', 'Die Nachricht konnte nicht geladen werden: ' + e.message));
  }
}

function scheduleMarkRead(m) {
  if (m.isRead) return;
  const delay = state.config ? state.config.markReadDelaySecs : 0;
  if (delay < 0) return;
  markReadTimer = setTimeout(() => {
    if (list.selection.size === 1 && list.selection.has(m.id)) applyRead([m.id], true).catch(reportError);
  }, delay * 1000);
}

export function currentMessage() { return currentDetail; }

// ============================================================ actions
function requireSelection() {
  const s = getSelection();
  if (!s.ids.length) { toast('Bitte wählen Sie zuerst ein Element aus.'); return null; }
  return s;
}

export function openMessage(m) {
  if (!m) return;
  if (m.isDraft) openWindow('compose', { mailboxId: m.mailboxId || state.current.mailboxId, draftId: m.id }).catch(reportError);
  else openWindow('message', { mailboxId: m.mailboxId || state.current.mailboxId, messageId: m.id, folderId: state.current.folderId }).catch(reportError);
}

export function newMail() {
  const mb = state.current.mailboxId || (mailboxes()[0] && mailboxes()[0].id);
  if (!mb) { toast('Bitte fügen Sie zuerst ein Konto hinzu.'); return; }
  openWindow('compose', { mailboxId: mb, mode: 'new' }).catch(reportError);
}

export function respond(mode) {
  const s = requireSelection();
  if (!s) return;
  const m = s.items[0];
  openWindow('compose', { mailboxId: m.mailboxId || s.mailboxId, mode, sourceMessageId: m.id }).catch(reportError);
}

function localUpdate(ids, fn) {
  const set = new Set(ids);
  for (const m of list.items) if (set.has(m.id)) fn(m);
  if (list.search) for (const m of list.search.items) if (set.has(m.id)) fn(m);
}

export async function applyRead(ids, isRead) {
  const { mailboxId, folderId } = state.current;
  let delta = 0;
  localUpdate(ids, (m) => { if (m.isRead !== isRead) { delta += isRead ? -1 : 1; m.isRead = isRead; } });
  const f = folderById(mailboxId, folderId);
  if (f && !list.search) { f.unreadCount = Math.max(0, f.unreadCount + delta); list.unread = Math.max(0, list.unread + delta); }
  renderList(); renderFolderTree(); onCountsChanged();
  await call('set_read', { mailboxId, ids, isRead }, { silent: true });
}

export async function applyFlag(ids, flag) {
  localUpdate(ids, (m) => { m.flag = flag; });
  renderList();
  await call('set_flag', { mailboxId: state.current.mailboxId, ids, flag }, { silent: true });
  if (currentDetail && ids.includes(currentDetail.id)) refreshReading();
}

function refreshReading() {
  const s = getSelection();
  if (s.items.length === 1) showReading(s.items[0]);
}

export async function toggleRead() {
  const s = requireSelection();
  if (!s) return;
  const anyUnread = s.items.some((m) => !m.isRead);
  await applyRead(s.ids, anyUnread);
}
export async function markSelected(isRead) {
  const s = requireSelection();
  if (s) await applyRead(s.ids, isRead);
}

export async function setFlagSelected(flag) {
  const s = requireSelection();
  if (s) await applyFlag(s.ids, flag);
}

async function removeFromList(ids) {
  const order = visibleOrder();
  const set = new Set(ids);
  const lastIdx = Math.max(...ids.map((id) => order.indexOf(id)));
  let next = order.slice(lastIdx + 1).find((id) => !set.has(id)) || order.slice(0, lastIdx).reverse().find((id) => !set.has(id)) || null;
  list.items = list.items.filter((m) => !set.has(m.id));
  if (list.search) list.search.items = list.search.items.filter((m) => !set.has(m.id));
  list.total = Math.max(0, list.total - ids.length);
  list.selection = new Set(next ? [next] : []);
  list.cursor = next; list.anchor = next;
  renderList();
  selectionChanged();
}

export async function deleteSelected(permanent = false) {
  const s = requireSelection();
  if (!s) return;
  const f = currentFolder();
  if (permanent || (f && f.wellKnown === 'deleteditems')) {
    const ok = await confirmDialog(s.ids.length === 1 ? 'Das ausgewählte Element wird endgültig gelöscht. Fortfahren?' : `${s.ids.length} Elemente werden endgültig gelöscht. Fortfahren?`, { danger: true, ok: 'Ja', cancel: 'Nein', title: 'Endgültig löschen' });
    if (!ok) return;
    permanent = true;
  }
  await removeFromList(s.ids);
  await call('delete_messages', { mailboxId: s.mailboxId, ids: s.ids, permanent });
  await refreshFolders(s.mailboxId);
}

export async function moveOrCopy(ids, destFolderId, kind = 'move') {
  const { mailboxId } = state.current;
  if (kind === 'move') await removeFromList(ids);
  await call(kind === 'move' ? 'move_messages' : 'copy_messages', { mailboxId, ids, destFolderId });
  const dest = folderById(mailboxId, destFolderId);
  toast(`${ids.length} ${ids.length === 1 ? 'Element' : 'Elemente'} ${kind === 'move' ? 'verschoben' : 'kopiert'} nach „${dest ? dest.displayName : ''}".`);
  await refreshFolders(mailboxId);
}

export async function archiveSelected() {
  const s = requireSelection();
  if (!s) return;
  const archive = wellKnown(s.mailboxId, 'archive');
  if (!archive) { toast('Für dieses Postfach ist kein Archivordner vorhanden.', 'error'); return; }
  if (archive.id === s.folderId) return;
  await moveOrCopy(s.ids, archive.id, 'move');
}

/** Menu items listing all folders of the current mailbox (indented). */
export function folderPickerItems(onPick, excludeId) {
  const mbId = state.current.mailboxId;
  const fs = foldersOf(mbId);
  const ids = new Set(fs.map((f) => f.id));
  const out = [];
  const walk = (parent, depth) => {
    for (const f of fs.filter((x) => (parent === null ? !x.parentId || !ids.has(x.parentId) : x.parentId === parent))) {
      out.push({ label: ' '.repeat(depth) + f.displayName, icon: WK_ICON[f.wellKnown] || 'folder', disabled: f.id === excludeId, onClick: () => onPick(f), testid: 'folder-pick-' + f.displayName });
      walk(f.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

export function moveMenuItems(kind = 'move') {
  return folderPickerItems((f) => {
    const s = requireSelection();
    if (s) return moveOrCopy(s.ids, f.id, kind);
  }, state.current.folderId);
}

export function categoryMenuItems() {
  const accountId = accountOfMailbox(state.current.mailboxId);
  const s = getSelection();
  const cats = getCategories(accountId);
  return [
    ...cats.map((c) => ({
      label: c.displayName, iconHtml: swatchHtml(presetHex(c.color), 14),
      checked: s.items.length > 0 && s.items.every((m) => (m.categories || []).includes(c.displayName)),
      testid: 'cat-' + c.displayName,
      onClick: () => toggleCategory(c.displayName),
    })),
    cats.length ? { separator: true } : null,
    { label: 'Alle Kategorien löschen', disabled: !s.items.some((m) => (m.categories || []).length), onClick: () => setCategoriesFor(s.items.map((m) => ({ m, cats: [] }))) },
    { label: 'Alle Kategorien…', icon: 'tag', onClick: () => openCategoryManager(accountId) },
  ].filter(Boolean);
}

async function toggleCategory(name) {
  const s = requireSelection();
  if (!s) return;
  const all = s.items.every((m) => (m.categories || []).includes(name));
  await setCategoriesFor(s.items.map((m) => {
    const cur = m.categories || [];
    return { m, cats: all ? cur.filter((c) => c !== name) : cur.includes(name) ? cur : [...cur, name] };
  }));
}

async function setCategoriesFor(changes) {
  const groups = new Map();
  for (const { m, cats } of changes) {
    m.categories = cats;
    const k = JSON.stringify(cats);
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k).push(m.id);
  }
  renderList();
  for (const [k, ids] of groups) await call('set_categories', { mailboxId: state.current.mailboxId, ids, categories: JSON.parse(k) }, { silent: true });
  refreshReading();
}

export function flagMenuItems() {
  return [
    { label: 'Zur Nachverfolgung kennzeichnen', icon: 'flag', onClick: () => setFlagSelected('flagged') },
    { label: 'Als erledigt markieren', icon: 'check', onClick: () => setFlagSelected('complete') },
    { label: 'Kennzeichnung löschen', icon: 'flagOutline', onClick: () => setFlagSelected('notFlagged') },
  ];
}

export async function showSource() {
  const s = requireSelection();
  if (!s) return;
  const src = await call('get_message_source', { mailboxId: s.mailboxId, messageId: s.ids[0] });
  const ta = h('textarea.input.selectable', { readOnly: true, style: { width: '100%', height: '60vh', fontFamily: 'Consolas, monospace', fontSize: '11px', whiteSpace: 'pre' } });
  ta.value = src || '';
  await modal({ title: 'Nachrichtenquelle – ' + (s.items[0].subject || ''), body: ta, width: 860, buttons: [{ label: 'Schließen', value: true, primary: true }] });
}

function messageContextMenu(x, y) {
  const s = getSelection();
  const single = s.items.length === 1;
  const anyUnread = s.items.some((m) => !m.isRead);
  showMenu([
    { label: 'Öffnen', icon: 'open', disabled: !single, onClick: () => openMessage(s.items[0]) },
    { separator: true },
    { label: 'Antworten', icon: 'reply', disabled: !single, shortcut: 'Strg+R', onClick: () => respond('reply') },
    { label: 'Allen antworten', icon: 'replyAll', disabled: !single, shortcut: 'Strg+Umschalt+R', onClick: () => respond('replyAll') },
    { label: 'Weiterleiten', icon: 'forward', disabled: !single, shortcut: 'Strg+F', onClick: () => respond('forward') },
    { separator: true },
    anyUnread
      ? { label: 'Als gelesen markieren', icon: 'mailOpen', shortcut: 'Strg+Q', onClick: () => markSelected(true) }
      : { label: 'Als ungelesen markieren', icon: 'mailClosed', shortcut: 'Strg+U', onClick: () => markSelected(false) },
    { label: 'Kategorisieren', icon: 'tag', submenu: categoryMenuItems },
    { label: 'Zur Nachverfolgung', icon: 'flag', submenu: flagMenuItems },
    { separator: true },
    { label: 'Verschieben', icon: 'move', submenu: () => moveMenuItems('move'), testid: 'ctx-move' },
    { label: 'Kopieren', icon: 'copy', submenu: () => moveMenuItems('copy') },
    { label: 'Archivieren', icon: 'archive', onClick: () => archiveSelected() },
    { label: 'Löschen', icon: 'delete', shortcut: 'Entf', onClick: () => deleteSelected(false), testid: 'ctx-delete' },
    { separator: true },
    { label: 'Quelltext anzeigen', icon: 'source', disabled: !single, onClick: () => showSource() },
  ], x, y);
}

/** Called by the shell when backend events arrive. */
export async function onMailChanged(p) {
  const tasks = [];
  if (p && p.mailboxId) tasks.push(refreshFolders(p.mailboxId));
  if (p && p.mailboxId === state.current.mailboxId && (!p.folderId || p.folderId === state.current.folderId)) tasks.push(reloadList());
  await Promise.all(tasks);
}

export function focusList() { els.list.focus(); }
