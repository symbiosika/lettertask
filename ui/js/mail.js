// Mail module of the main window: folder tree, message list, reading pane, mail actions.
import { call, openWindow } from './api.js';
import { h, esc, $, load, store, debounce, dateGroup, fmtListDate, displayName, initials, avatarColor } from './util.js';
import { icon } from './icons.js';
import { isImap, kindLabel } from './accounts.js';
import { showMenu, showMenuAt, reportError, toast, confirmDialog, promptDialog, modal } from './dialogs.js';
import { chipHtml, getCategories, presetHex, swatchHtml, visibleCats, laneCats, isLaneCategory, laneCategory, laneName } from './categories.js';
import { renderMessage, printMessage } from './mailview.js';
import { openTagPicker, countTagUse } from './tagpicker.js';
import { fmtDueShort, fmtSnoozeTarget } from './snooze.js';
import { state, mailboxes, mailboxById, accountOfMailbox, foldersOf, folderById, wellKnown, loadFolders, lanesOf, loadLanes } from './state.js';
import { createBoard, viewToggle, laneOf } from './kanban.js';
import { openCategoryManager } from './settings.js';

const PAGE = 200;
export const WK_ICON = { inbox: 'inbox', snoozed: 'snooze', drafts: 'drafts', sentitems: 'sent', deleteditems: 'trash', archive: 'archive', junkemail: 'junk', outbox: 'outbox' };
const WK_RANK = { inbox: 0, snoozed: 1, drafts: 2, sentitems: 3, deleteditems: 4, archive: 5, junkemail: 6, outbox: 7 };
const TOTAL_COUNT_FOLDERS = new Set(['drafts', 'outbox', 'junkemail']);
const TO_FOLDERS = new Set(['sentitems', 'drafts', 'outbox']);
const SORT_LABELS = { received: 'Datum', from: 'Von', subject: 'Betreff', importance: 'Wichtigkeit', flag: 'Kennzeichnung', attachments: 'Anlage', categories: 'Kategorie' };
const FIELD_LABELS = { all: '', from: 'Von', to: 'An', subject: 'Betreff', body: 'Text' };

const els = {};
const list = {
  items: [], total: 0, unread: 0, canLoadOlder: false, loading: false,
  sortBy: load('sortBy', 'received'), sortDesc: load('sortDesc', true),
  filter: { unreadOnly: false, flaggedOnly: false, category: null },
  text: '', search: null, // search: {text, items, opts} when server search results are shown
  selection: new Set(), cursor: null, anchor: null,
};
let readToken = 0;
let markReadTimer = null;
let currentDetail = null;
let onCountsChanged = () => {};
let onSelectionChanged = () => {};
let view = load('mailView', 'list'); // 'list' | 'kanban'
let board = null;

/** Kanban is shown for the folder unless server search results are displayed. */
export function kanbanActive() { return view === 'kanban' && !list.search; }
export function getView() { return view; }
export function setView(v) {
  if (v !== 'list' && v !== 'kanban') return;
  view = v;
  store('mailView', v);
  applyReadingPaneLayout();
  reloadList({ keepCount: false }).catch(reportError);
  if (kanbanActive()) board.focus(); else els.list.focus();
}
export function toggleView() { setView(view === 'kanban' ? 'list' : 'kanban'); }
function kanbanPane() { return (state.config && state.config.kanbanReadingPane) || load('kanbanPane', 'bottom'); }
function currentAccount() { return accountOfMailbox(state.current.mailboxId); }
export function currentLanes() { return lanesOf(currentAccount()); }

// ============================================================ setup
export function initMail({ navHost, contentHost, onCounts, onSelection }) {
  onCountsChanged = onCounts || onCountsChanged;
  onSelectionChanged = onSelection || onSelectionChanged;
  els.nav = navHost;
  els.content = contentHost;
  els.mailview = h('div.mailview#mailview');
  els.listpane = h('div.listpane#listpane');
  els.head = h('div.list-head',
    h('div.lh-title', h('span.folder-name'), h('span.lh-count'), h('span.spacer'), h('div.lh-sort')),
    h('div.lh-tools'));
  els.searchbar = h('div.search-bar.hidden');
  els.list = h('div.msglist#msglist', { tabindex: '0', role: 'listbox', 'aria-label': 'Nachrichten', 'aria-multiselectable': 'true' });
  els.listpane.append(els.head, els.searchbar, els.list);
  els.split = h('div.splitter');
  els.read = h('div.readpane#readpane');
  els.board = h('div#board.hidden', { dataset: { testid: 'kanban-board' } });
  els.mailview.append(els.board, els.listpane, els.split, els.read);
  contentHost.appendChild(els.mailview);
  board = createBoard(els.board, {
    items: () => list.items,
    lanes: () => currentLanes(),
    accountId: () => currentAccount(),
    snoozed: () => { const f = currentFolder(); return !!(f && f.wellKnown === 'snoozed'); },
    isSelected: (id) => list.selection.has(id),
    cursor: () => list.cursor,
    hideNoLane: () => !!load('kanbanHideNoLane', false),
    paneMode: () => kanbanPane(),
    title: () => { const f = currentFolder(); return f ? f.displayName : ''; },
    count: () => list.total,
    onSelect: (id, e) => {
      if (e && (e.ctrlKey || e.metaKey)) { if (list.selection.has(id)) list.selection.delete(id); else list.selection.add(id); }
      else if (e && e.shiftKey && list.anchor) selectRange(list.anchor, id);
      else { list.selection = new Set([id]); list.anchor = id; }
      list.cursor = id;
      paintSelection();
      selectionChanged();
    },
    onOpen: (id) => openMessage(findItem(id)),
    onSetLane: (ids, lane) => setLaneFor(ids, lane).catch(reportError),
    onContext: (id, x, y) => messageContextMenu(x, y),
    onDelete: (permanent) => deleteSelected(permanent).catch(reportError),
    dragPayload: () => ({ mailboxId: state.current.mailboxId, folderId: state.current.folderId, ids: [...list.selection] }),
    onToggleView: (v) => setView(v),
    onToggleNoLane: () => { store('kanbanHideNoLane', !load('kanbanHideNoLane', false)); renderList(); },
    onTogglePane: () => setKanbanPane(kanbanPane() === 'bottom' ? 'right' : 'bottom').catch(reportError),
    onEditLanes: () => hooks.openLaneSettings && hooks.openLaneSettings(currentAccount()),
  });

  applyReadingPaneLayout();
  setupListEvents();
  renderListHead();
}

async function setKanbanPane(mode) {
  store('kanbanPane', mode);
  if (state.config) {
    try { state.config = await call('save_config', { config: { ...state.config, kanbanReadingPane: mode } }, { silent: true }); } catch (e) { console.warn(e); }
    if (state.config && !state.config.kanbanReadingPane) state.config.kanbanReadingPane = mode;
  }
  applyReadingPaneLayout();
  renderList();
}

export function applyReadingPaneLayout() {
  const kb = kanbanActive();
  els.board.classList.toggle('hidden', !kb);
  els.listpane.classList.toggle('hidden', kb);
  els.mailview.classList.toggle('kanban', kb);
  if (kb) {
    const pane = kanbanPane();
    els.mailview.classList.toggle('bottom', pane === 'bottom');
    els.mailview.classList.remove('off');
    els.split.className = 'splitter' + (pane === 'bottom' ? ' h' : '');
    els.read.classList.remove('hidden');
    if (pane === 'bottom') { els.board.style.width = ''; els.board.style.height = load('boardHeight', 380) + 'px'; }
    else { els.board.style.height = ''; els.board.style.width = load('boardWidth', 760) + 'px'; }
    makeSplitter(els.split, els.board, pane === 'bottom' ? 'y' : 'x', pane === 'bottom' ? 'boardHeight' : 'boardWidth');
    return;
  }
  els.board.style.width = ''; els.board.style.height = '';
  const mode = (state.config && state.config.readingPane) || 'right';
  els.mailview.classList.toggle('bottom', mode === 'bottom');
  els.mailview.classList.toggle('off', mode === 'off');
  els.split.className = 'splitter' + (mode === 'bottom' ? ' h' : '');
  els.split.classList.toggle('hidden', mode === 'off');
  els.read.classList.toggle('hidden', mode === 'off');
  if (mode === 'bottom') { els.listpane.style.width = ''; els.listpane.style.height = load('listHeight', 320) + 'px'; }
  else if (mode === 'right') { els.listpane.style.height = ''; els.listpane.style.width = load('listWidth2', 400) + 'px'; }
  else { els.listpane.style.width = ''; els.listpane.style.height = ''; }
  makeSplitter(els.split, els.listpane, mode === 'bottom' ? 'y' : 'x', mode === 'bottom' ? 'listHeight' : 'listWidth2');
}

export function makeSplitter(split, target, axis, key) {
  split.onmousedown = (e) => {
    e.preventDefault();
    const start = axis === 'x' ? e.clientX : e.clientY;
    const startSize = axis === 'x' ? target.offsetWidth : target.offsetHeight;
    const frames = document.querySelectorAll('iframe');
    frames.forEach((f) => (f.style.pointerEvents = 'none'));
    document.body.classList.add('resizing');
    const move = (ev) => {
      const d = (axis === 'x' ? ev.clientX : ev.clientY) - start;
      const size = Math.max(axis === 'x' ? 280 : 120, startSize + d);
      target.style[axis === 'x' ? 'width' : 'height'] = size + 'px';
    };
    const up = () => {
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', up);
      frames.forEach((f) => (f.style.pointerEvents = ''));
      document.body.classList.remove('resizing');
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

function primaryMailbox() { return mailboxes().find((m) => m.kind === 'primary') || mailboxes()[0] || null; }

function favorites() {
  const fav = load('favorites2', null);
  if (fav) return fav;
  const mb = primaryMailbox();
  if (!mb) return [];
  return ['inbox', 'snoozed'].map((w) => {
    const f = wellKnown(mb.id, w);
    return f ? { mailboxId: mb.id, folderId: f.id } : null;
  }).filter(Boolean);
}
function setFavorites(f) { store('favorites2', f); renderFolderTree(); }

function folderCountHtml(f) {
  if (TOTAL_COUNT_FOLDERS.has(f.wellKnown)) return f.totalCount > 0 ? `<span class="count total">${f.totalCount}</span>` : '';
  if (f.wellKnown === 'snoozed') return f.totalCount > 0 ? `<span class="count total">${f.totalCount}</span>` : '';
  return f.unreadCount > 0 ? `<span class="count">${f.unreadCount}</span>` : '';
}

function sortFolders(fs) {
  return [...fs].sort((a, b) => ((WK_RANK[a.wellKnown] ?? 100) - (WK_RANK[b.wellKnown] ?? 100))
    || a.displayName.localeCompare(b.displayName, 'de', { sensitivity: 'base' }));
}

function folderNode(f, depth, opts = {}) {
  const hasChildren = !opts.flat && foldersOf(f.mailboxId).some((c) => c.parentId === f.id);
  const key = `${f.mailboxId}|${f.id}`;
  const open = hasChildren && isExpanded(key);
  const sel = state.mode === 'mail' && state.current.mailboxId === f.mailboxId && state.current.folderId === f.id && !(opts.flat && !opts.favorite);
  const unread = f.unreadCount > 0 && !TOTAL_COUNT_FOLDERS.has(f.wellKnown) && f.wellKnown !== 'snoozed';
  const node = h('div.tree-node' + (sel ? '.selected' : '') + (unread ? '.has-unread' : ''), {
    style: { paddingLeft: 8 + depth * 16 + 'px' }, dataset: { mailboxId: f.mailboxId, folderId: f.id, wk: f.wellKnown || '' }, title: f.displayName,
    role: 'treeitem', 'aria-selected': sel ? 'true' : 'false',
  });
  const tw = h('span.twisty', { html: hasChildren ? icon(open ? 'chevronDown' : 'chevronRight', 14) : '' });
  if (hasChildren) tw.addEventListener('click', (e) => { e.stopPropagation(); toggleExpand(key); });
  node.append(tw, h('span.fic', { html: icon(WK_ICON[f.wellKnown] || 'folder', 18) }), h('span.fname', f.displayName), h('span.fcount', { html: folderCountHtml(f) }));
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
    for (const c of sortFolders(foldersOf(f.mailboxId).filter((c) => c.parentId === f.id))) frag.push(...folderNode(c, depth + 1));
  }
  return frag;
}

function sectionHeader(key, label, leading, extra = {}) {
  const open = isExpanded(key);
  const el = h('div.tree-root', { ...extra, role: 'button', 'aria-expanded': open ? 'true' : 'false' },
    h('span.twisty', { html: icon(open ? 'chevronDown' : 'chevronRight', 14) }),
    leading || null,
    h('span.tr-label', label));
  el.addEventListener('click', () => toggleExpand(key));
  return el;
}

export function renderFolderTree() {
  if (!els.nav) return;
  const scroll = els.nav.scrollTop;
  els.nav.innerHTML = '';
  // favorites
  const favKey = 'root|favorites';
  const favs = favorites().map((f) => folderById(f.mailboxId, f.folderId)).filter(Boolean);
  const favSec = h('div.tree-section.favorites', { role: 'tree', dataset: { section: 'favorites' } });
  favSec.appendChild(sectionHeader(favKey, 'Favoriten', null));
  if (isExpanded(favKey)) {
    for (const f of favs) {
      const [n] = folderNode(f, 0, { flat: true, favorite: true });
      n.dataset.favorite = '1';
      favSec.appendChild(n);
    }
    if (!favs.length) favSec.appendChild(h('div.tree-hint', 'Ordner per Rechtsklick hinzufügen'));
  }
  els.nav.appendChild(favSec);

  for (const mb of mailboxes()) {
    const key = 'root|' + mb.id;
    const sec = h('div.tree-section', { role: 'tree', dataset: { mailboxRoot: mb.id } });
    const av = h('span.avatar.xs', { style: { background: avatarColor(mb.address) } }, initials(mb.displayName, mb.address));
    const root = sectionHeader(key, mb.address || mb.displayName, av, { title: mb.address });
    root.classList.add('account');
    const acc = state.accounts.find((x) => x.id === mb.accountId);
    if (acc) root.appendChild(h('span.kind-badge.sm.' + (isImap(acc) ? 'imap' : 'o365'), { title: isImap(acc) ? 'IMAP/SMTP-Konto' : 'Microsoft-365-Konto' }, kindLabel(acc)));
    root.addEventListener('contextmenu', (e) => { e.preventDefault(); mailboxContextMenu(mb, e.clientX, e.clientY); });
    sec.appendChild(root);
    if (isExpanded(key)) {
      const fs = foldersOf(mb.id);
      const ids = new Set(fs.map((f) => f.id));
      for (const f of sortFolders(fs.filter((f) => !f.parentId || !ids.has(f.parentId)))) sec.append(...folderNode(f, 0));
      if (!fs.length) sec.appendChild(h('div.tree-hint', 'Wird geladen…'));
    }
    els.nav.appendChild(sec);
  }
  els.nav.scrollTop = scroll;
}

function mailboxContextMenu(mb, x, y) {
  showMenu([
    { label: 'Ordner synchronisieren', icon: 'sync', onClick: async () => { await call('sync_folders', { mailboxId: mb.id }); await refreshFolders(mb.id); } },
    { label: 'Neuer Ordner…', icon: 'folder', onClick: () => newFolder(mb.id, null) },
  ], x, y);
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
  renderListHead();
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
  renderListHead();
  onSelectionChanged();
  call('set_active_folder', { mailboxId, folderId }, { silent: true }).catch(() => {});
  await reloadList();
  // pull fresh data in the background
  call('sync_folder', { mailboxId, folderId }, { silent: true })
    .then(() => (state.current.folderId === folderId ? reloadList() : null))
    .catch(() => {});
}

export function currentFolder() { return folderById(state.current.mailboxId, state.current.folderId); }
export function inSnoozedFolder() { const f = currentFolder(); return !!(f && f.wellKnown === 'snoozed') && !list.search; }

// ============================================================ list header
function renderListHead() {
  if (!els.head) return;
  const f = currentFolder();
  const title = els.head.querySelector('.folder-name');
  const count = els.head.querySelector('.lh-count');
  title.textContent = list.search ? 'Suchergebnisse' : (f ? f.displayName : '');
  const n = list.search ? list.search.items.length : list.total;
  count.textContent = n ? String(n) : '';
  count.title = list.search ? '' : `${list.total} Elemente, ${list.unread} ungelesen`;
  const mb = mailboxById(state.current.mailboxId);
  document.title = `${f ? f.displayName : 'Mail'}${mb ? ' – ' + mb.address : ''}`;

  const tools = els.head.querySelector('.lh-tools');
  tools.innerHTML = '';
  const chip = (label, on, fn, testid) => h('button.chip' + (on ? '.on' : ''), { type: 'button', onclick: fn, dataset: { testid } }, label);
  const fl = list.filter;
  if (!list.search) {
    tools.append(
      chip('Alle', !fl.unreadOnly && !fl.flaggedOnly && !fl.category, () => setFilter({ unreadOnly: false, flaggedOnly: false, category: null }), 'filter-all'),
      chip('Ungelesen', fl.unreadOnly, () => setFilter({ ...fl, unreadOnly: !fl.unreadOnly }), 'filter-unread'),
      chip('Gekennzeichnet', fl.flaggedOnly, () => setFilter({ ...fl, flaggedOnly: !fl.flaggedOnly }), 'filter-flagged'),
    );
    const catBtn = h('button.chip' + (fl.category ? '.on' : ''), { type: 'button', dataset: { testid: 'filter-category' }, title: 'Nach Kategorie filtern' });
    catBtn.innerHTML = fl.category
      ? `${swatchHtml(presetHex((getCategories(accountOfMailbox(state.current.mailboxId)).find((c) => c.displayName === fl.category) || {}).color), 9)} ${esc(fl.category)} ${icon('chevronDown', 12)}`
      : `${icon('tag', 14)} ${icon('chevronDown', 12)}`;
    catBtn.addEventListener('click', () => {
      const accountId = accountOfMailbox(state.current.mailboxId);
      showMenuAt(catBtn, [
        { label: 'Alle Kategorien', checked: !fl.category, onClick: () => setFilter({ ...fl, category: null }) },
        { separator: true },
        ...getCategories(accountId).map((c) => ({ label: c.displayName, iconHtml: swatchHtml(presetHex(c.color), 10), checked: fl.category === c.displayName, onClick: () => setFilter({ ...fl, category: c.displayName }) })),
      ]);
    });
    tools.appendChild(catBtn);
  }
  const sortHost = els.head.querySelector('.lh-sort');
  sortHost.innerHTML = '';
  if (!list.search) sortHost.appendChild(viewToggle('list', (v) => setView(v)));
  const sortBtn = h('button.chip.sort', { type: 'button', title: 'Sortieren', dataset: { testid: 'sort-menu' }, html: `${icon('sort', 14)}<span>${esc(SORT_LABELS[list.sortBy] || 'Datum')}</span>` });
  sortBtn.addEventListener('click', () => showMenuAt(sortBtn, [
    { header: 'Sortieren nach' },
    ...['received', 'from', 'subject', 'importance', 'flag', 'attachments'].map((k) => ({ label: SORT_LABELS[k], checked: list.sortBy === k, onClick: () => sortBy(k, true) })),
    { separator: true },
    { label: 'Absteigend', checked: list.sortDesc, onClick: () => setSortDir(true) },
    { label: 'Aufsteigend', checked: !list.sortDesc, onClick: () => setSortDir(false) },
  ]));
  const dirBtn = h('button.icon-btn.sm', { type: 'button', title: list.sortDesc ? 'Absteigend' : 'Aufsteigend', html: icon(list.sortDesc ? 'down' : 'up', 15), onclick: () => setSortDir(!list.sortDesc) });
  sortHost.append(sortBtn, dirBtn);

  // search bar
  if (list.search) {
    const o = list.search.opts || {};
    const scope = o.scope === 'all' ? 'alle Ordner' : (f ? f.displayName : 'aktueller Ordner');
    els.searchbar.innerHTML = '';
    els.searchbar.append(
      h('span', { html: icon('search', 15) }),
      h('span.sb-text', `Suchergebnisse für „${list.search.text}“`),
      h('span.sb-meta', [scope, FIELD_LABELS[o.field] ? 'nur ' + FIELD_LABELS[o.field] : '', o.hasAttachments ? 'mit Anhang' : ''].filter(Boolean).join(' · ')),
      h('button.icon-btn.sm', { type: 'button', title: 'Suche beenden (Esc)', dataset: { testid: 'search-close' }, html: icon('close', 14), onclick: () => clearSearch() }),
    );
    els.searchbar.classList.remove('hidden');
  } else if (list.text) {
    els.searchbar.innerHTML = '';
    els.searchbar.append(h('span', { html: icon('filter', 15) }), h('span.sb-text', `Gefiltert: „${list.text}“`),
      h('button.icon-btn.sm', { type: 'button', title: 'Filter entfernen', html: icon('close', 14), onclick: () => clearSearch() }));
    els.searchbar.classList.remove('hidden');
  } else {
    els.searchbar.classList.add('hidden');
  }
}

function setFilter(f) {
  list.filter = f;
  reloadList({ keepCount: false }).catch(reportError);
}

// ============================================================ list
export async function reloadList({ keepCount = true } = {}) {
  const { mailboxId, folderId } = state.current;
  if (!mailboxId || !folderId) { renderList(); return; }
  if (list.search) { renderList(); return; }
  const kb = kanbanActive();
  const limit = kb ? 1000 : keepCount ? Math.max(PAGE, list.items.length) : PAGE;
  const accountId = accountOfMailbox(mailboxId);
  if (kb && !state.lanes.has(accountId)) await loadLanes(accountId);
  const res = await call('list_messages', {
    mailboxId, folderId, sortBy: kb ? 'received' : list.sortBy, sortDesc: kb ? true : list.sortDesc,
    unreadOnly: kb ? false : list.filter.unreadOnly, flaggedOnly: kb ? false : list.filter.flaggedOnly, category: kb ? null : list.filter.category,
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
  if (list.loading || list.search || kanbanActive()) return;
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

function visibleItems() { return list.search ? list.search.items : list.items; }

function renderList() {
  if (kanbanActive()) { board.render(); renderListHead(); return; }
  const scroll = els.list.scrollTop;
  const compact = state.config && state.config.listMode === 'compact';
  const f = currentFolder();
  const toMode = f && TO_FOLDERS.has(f.wellKnown) && !list.search;
  const snoozedView = f && f.wellKnown === 'snoozed' && !list.search;
  const items = visibleItems();
  const accountId = accountOfMailbox(state.current.mailboxId);
  const separators = !!(state.config ? state.config.showDateSeparators ?? true : true) && list.sortBy === 'received' && !list.search && !snoozedView;

  const frag = document.createDocumentFragment();
  let lastGroup = null;
  for (const m of items) {
    if (separators) {
      const g = dateGroup(m.receivedAt);
      if (g.key !== lastGroup) {
        lastGroup = g.key;
        frag.appendChild(h('div.m-sep', { dataset: { group: g.key } }, g.label));
      }
    }
    frag.appendChild(msgRow(m, accountId, toMode, snoozedView));
  }
  els.list.innerHTML = '';
  els.list.classList.toggle('compact', !!compact);
  els.list.appendChild(frag);
  if (!items.length) {
    els.list.appendChild(h('div.empty', h('div.empty-ic', { html: icon(list.search ? 'search' : 'inbox', 40) }),
      h('div', list.search ? 'Keine Ergebnisse gefunden.' : 'Keine Elemente in dieser Ansicht.')));
  } else if (!list.search && (list.items.length < list.total || list.canLoadOlder || list.loading)) {
    els.list.appendChild(h('div.load-more', list.loading ? 'Weitere Elemente werden geladen…' : (list.canLoadOlder && list.items.length >= list.total ? 'Weitere Elemente auf dem Server – nach unten scrollen, um sie zu laden.' : '')));
  }
  els.list.scrollTop = scroll;
  renderListHead();
}

function msgRow(m, accountId, toMode, snoozedView) {
  const who = toMode ? (m.to || []).map(displayName).join(', ') : displayName(m.from);
  const avatarSrc = toMode ? (m.to && m.to[0]) || {} : (m.from || {});
  const sel = list.selection.has(m.id);
  const cls = 'div.msg' + (m.isRead ? '' : '.unread') + (sel ? '.selected' : '') + (list.cursor === m.id ? '.cursor' : '') + (m.flag === 'flagged' ? '.flagged' : '');
  const el = h(cls, { draggable: 'true', dataset: { id: m.id }, role: 'option', 'aria-selected': sel ? 'true' : 'false' });
  const flagIc = snoozedView ? '' : m.flag === 'flagged' ? icon('flag', 15, 'flagged') : m.flag === 'complete' ? icon('check', 15, 'done') : icon('flag', 15, 'ghost');
  const time = snoozedView && m.dueAt
    ? `<span class="m-due" title="Auf Wiedervorlage bis ${esc(fmtSnoozeTarget(new Date(m.dueAt)))}">${icon('snooze', 13)} bis ${esc(fmtDueShort(m.dueAt))}</span>`
    : esc(fmtListDate(m.receivedAt));
  const cats = visibleCats(m.categories).map((c) => chipHtml(accountId, c)).join('');
  el.innerHTML =
    `<span class="m-dot readcell" title="${m.isRead ? 'Als ungelesen markieren' : 'Als gelesen markieren'}"></span>` +
    `<span class="avatar sm" style="background:${avatarColor(avatarSrc.address || avatarSrc.name)}">${esc(initials(avatarSrc.name, avatarSrc.address))}</span>` +
    `<div class="m-main">` +
      `<div class="m-l1"><span class="m-from" title="${esc(who)}">${m.isDraft ? '<span class="draft-tag">Entwurf</span>' : ''}${esc(who || '(Unbekannt)')}</span><span class="m-time">${time}</span></div>` +
      `<div class="m-l2"><span class="m-subj" title="${esc(m.subject || '')}">${esc(m.subject || '(Kein Betreff)')}</span>` +
        `<span class="m-icons">${m.importance === 'high' ? icon('important', 14, 'imp') : ''}${m.hasAttachments ? icon('attach', 14) : ''}<span class="flagcell" title="Kennzeichnung umschalten">${flagIc}</span></span></div>` +
      `<div class="m-l3"><span class="m-prev">${esc(m.preview || '')}</span>${cats ? `<span class="m-cats">${cats}</span>` : ''}</div>` +
    `</div>` +
    `<div class="m-quick">` +
      `<button type="button" class="qa" data-qa="archive" title="Archivieren">${icon('archive', 16)}</button>` +
      `<button type="button" class="qa" data-qa="delete" title="Löschen">${icon('delete', 16)}</button>` +
      `<button type="button" class="qa" data-qa="read" title="${m.isRead ? 'Als ungelesen markieren' : 'Als gelesen markieren'}">${icon(m.isRead ? 'mail' : 'mailOpen', 16)}</button>` +
    `</div>`;
  return el;
}

function sortBy(key, fromMenu) {
  if (list.sortBy === key && !fromMenu) list.sortDesc = !list.sortDesc;
  else if (list.sortBy !== key) { list.sortBy = key; list.sortDesc = key === 'received' || key === 'importance' || key === 'flag' || key === 'attachments'; }
  store('sortBy', list.sortBy); store('sortDesc', list.sortDesc);
  if (list.search) { list.search.items.sort(localComparator()); renderList(); }
  else reloadList({ keepCount: false }).catch(reportError);
}
function setSortDir(desc) {
  list.sortDesc = desc;
  store('sortDesc', desc);
  if (list.search) { list.search.items.sort(localComparator()); renderList(); }
  else reloadList({ keepCount: false }).catch(reportError);
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

// ---------- search ----------
export const setLocalFilter = debounce((text) => {
  if (list.search) return;
  list.text = text.trim();
  reloadList({ keepCount: false }).catch(reportError);
}, 250);

/** opts: { scope: 'folder'|'all', field: 'all'|'from'|'to'|'subject'|'body', hasAttachments } */
export async function serverSearch(text, opts = {}) {
  text = text.trim();
  if (!text) { clearSearch(); return; }
  const { mailboxId, folderId } = state.current;
  if (!mailboxId) return;
  const args = { mailboxId, text, folderId: opts.scope === 'all' ? null : folderId };
  if (opts.field && opts.field !== 'all') args.field = opts.field;
  if (opts.hasAttachments) args.hasAttachments = true;
  const items = await call('search_messages', args);
  list.search = { text, items: items || [], opts };
  list.text = '';
  applyReadingPaneLayout();
  list.selection.clear(); list.cursor = null;
  showReading(null);
  renderList();
  onSelectionChanged();
}

export function isSearching() { return !!list.search || !!list.text; }

export function clearSearch() {
  const had = list.search || list.text;
  list.search = null;
  list.text = '';
  const sb = $('#search-input'); if (sb) sb.value = '';
  if (had) applyReadingPaneLayout();
  if (had) reloadList({ keepCount: false }).catch(reportError);
  else renderListHead();
}

// ---------- events & selection ----------
function rowId(el) { const r = el && el.closest && el.closest('.msg'); return r ? r.dataset.id : null; }
function findItem(id) { return visibleItems().find((m) => m.id === id) || null; }
function visibleOrder() { return kanbanActive() ? board.order() : Array.from(els.list.querySelectorAll('.msg')).map((r) => r.dataset.id); }

function setupListEvents() {
  const L = els.list;
  L.addEventListener('mousedown', (e) => {
    const id = rowId(e.target);
    if (!id || e.button !== 0) return;
    if (e.target.closest('.flagcell') || e.target.closest('.readcell') || e.target.closest('.qa')) return;
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
    const qa = e.target.closest('.qa');
    if (qa) {
      e.stopPropagation();
      if (qa.dataset.qa === 'archive') archiveIds([id]).catch(reportError);
      else if (qa.dataset.qa === 'delete') deleteIds([id], false).catch(reportError);
      else if (qa.dataset.qa === 'read') applyRead([id], !m.isRead).catch(reportError);
      return;
    }
    if (e.target.closest('.flagcell')) {
      const next = m.flag === 'flagged' ? 'complete' : m.flag === 'complete' ? 'notFlagged' : 'flagged';
      applyFlag([id], next).catch(reportError);
    } else if (e.target.closest('.readcell')) {
      applyRead([id], !m.isRead).catch(reportError);
    } else if (!e.ctrlKey && !e.shiftKey && list.selection.size > 1) {
      list.selection = new Set([id]); list.anchor = id; paintSelection(); selectionChanged();
    }
  });
  L.addEventListener('dblclick', (e) => { const id = rowId(e.target); if (id && !e.target.closest('.flagcell,.readcell,.qa')) openMessage(findItem(id)); });
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
    if (L.scrollTop + L.clientHeight > L.scrollHeight - 200) loadMore().catch(reportError);
  });
  L.addEventListener('keydown', (e) => {
    const order = visibleOrder();
    if (!order.length) return;
    let idx = order.indexOf(list.cursor);
    if (['ArrowDown', 'ArrowUp', 'Home', 'End', 'PageDown', 'PageUp'].includes(e.key)) {
      e.preventDefault();
      const page = Math.max(1, Math.floor(L.clientHeight / 64) - 1);
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
      const r = L.querySelector(`.msg[data-id="${CSS.escape(id)}"]`);
      if (r) r.scrollIntoView({ block: 'nearest' });
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
  if (kanbanActive()) { board.paintSelection(); return; }
  for (const r of els.list.querySelectorAll('.msg')) {
    const sel = list.selection.has(r.dataset.id);
    r.classList.toggle('selected', sel);
    r.setAttribute('aria-selected', sel ? 'true' : 'false');
    r.classList.toggle('cursor', r.dataset.id === list.cursor);
  }
}

const selectionChanged = debounce(() => {
  onCountsChanged();
  onSelectionChanged();
  if (list.selection.size === 1) showReading(findItem([...list.selection][0]));
  else if (list.selection.size > 1) showReading(null, `${list.selection.size} Elemente ausgewählt`);
  else showReading(null);
}, 50);

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
  const mode = kanbanActive() ? kanbanPane() : (state.config && state.config.readingPane) || 'right';
  if (!m) {
    els.read.innerHTML = '';
    els.read.appendChild(h('div.empty.reading', h('div.empty-ic', { html: icon(text ? 'mail' : 'mailOpen', 48) }), h('div', text || 'Wählen Sie eine Nachricht aus, um sie zu lesen.')));
    return;
  }
  if (mode === 'off') return;
  try {
    const detail = await call('get_message', { mailboxId: m.mailboxId || state.current.mailboxId, messageId: m.id }, { silent: true });
    if (token !== readToken) return;
    currentDetail = detail;
    const accountId = accountOfMailbox(detail.mailboxId || state.current.mailboxId);
    renderMessage(els.read, detail, {
      accountId,
      config: state.config,
      snoozed: inSnoozedFolder(),
      onChanged: () => {},
      setCategories: (cats) => setCategoriesFor([{ m: findItem(m.id) || m, cats }]),
      onManageCategories: () => openCategoryManager(accountId),
      onAction: (name, anchor) => {
        if (name === 'more') showMenuAt(anchor, moreItems(true));
        else respond(name);
      },
    });
    scheduleMarkRead(m);
  } catch (e) {
    if (token !== readToken) return;
    els.read.innerHTML = '';
    els.read.appendChild(h('div.empty.reading', 'Die Nachricht konnte nicht geladen werden: ' + e.message));
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

/** Opens the tag picker: reading pane for one message, else anchored at `anchor`. */
export function openTagsForSelection(anchor) {
  const s = getSelection();
  if (!s.ids.length) { toast('Bitte wählen Sie zuerst eine Nachricht aus.'); return; }
  if (s.ids.length === 1 && els.read._openTagPicker && currentDetail && currentDetail.id === s.ids[0]) { els.read._openTagPicker(); return; }
  const accountId = accountOfMailbox(s.mailboxId);
  const common = () => {
    const items = getSelection().items;
    return getCategories(accountId).map((c) => c.displayName).filter((n) => items.length && items.every((m) => (m.categories || []).includes(n)));
  };
  const a = anchor || els.list.querySelector('.msg.selected') || els.head;
  openTagPicker(a, {
    accountId,
    assigned: common,
    onToggle: (name, on) => setCategoriesFor(getSelection().items.map((m) => {
      const cur = m.categories || [];
      return { m, cats: on ? (cur.includes(name) ? cur : [...cur, name]) : cur.filter((c) => c !== name) };
    })),
    onManage: () => openCategoryManager(accountId),
  });
}

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
export async function toggleFlagSelected() {
  const s = requireSelection();
  if (!s) return;
  const allFlagged = s.items.every((m) => m.flag === 'flagged');
  await applyFlag(s.ids, allFlagged ? 'notFlagged' : 'flagged');
}

async function removeFromList(ids) {
  const order = visibleOrder();
  const set = new Set(ids);
  const lastIdx = Math.max(...ids.map((id) => order.indexOf(id)));
  const next = order.slice(lastIdx + 1).find((id) => !set.has(id)) || order.slice(0, Math.max(0, lastIdx)).reverse().find((id) => !set.has(id)) || null;
  const removedUnread = list.items.filter((m) => set.has(m.id) && !m.isRead).length;
  list.items = list.items.filter((m) => !set.has(m.id));
  if (list.search) list.search.items = list.search.items.filter((m) => !set.has(m.id));
  list.total = Math.max(0, list.total - ids.length);
  list.unread = Math.max(0, list.unread - removedUnread);
  list.selection = new Set(next ? [next] : []);
  list.cursor = next; list.anchor = next;
  renderList();
  selectionChanged();
}

export async function deleteSelected(permanent = false) {
  const s = requireSelection();
  if (s) await deleteIds(s.ids, permanent);
}

async function deleteIds(ids, permanent) {
  const mailboxId = state.current.mailboxId;
  const f = currentFolder();
  if (permanent || (f && f.wellKnown === 'deleteditems' && !list.search)) {
    const ok = await confirmDialog(ids.length === 1 ? 'Das ausgewählte Element wird endgültig gelöscht. Fortfahren?' : `${ids.length} Elemente werden endgültig gelöscht. Fortfahren?`, { danger: true, ok: 'Endgültig löschen', cancel: 'Abbrechen', title: 'Endgültig löschen' });
    if (!ok) return;
    permanent = true;
  }
  await removeFromList(ids);
  await call('delete_messages', { mailboxId, ids, permanent });
  await refreshFolders(mailboxId);
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
  if (s) await archiveIds(s.ids);
}

async function archiveIds(ids) {
  const mailboxId = state.current.mailboxId;
  const archive = wellKnown(mailboxId, 'archive');
  if (archive && archive.id === state.current.folderId && !list.search) { toast('Die Nachricht befindet sich bereits im Archiv.'); return; }
  await removeFromList(ids);
  try {
    await call('archive_messages', { mailboxId, ids });
  } catch (e) {
    if (!/Unbekannter Befehl/.test(e.message) || !archive) throw e;
    await call('move_messages', { mailboxId, ids, destFolderId: archive.id });
  }
  toast(ids.length === 1 ? 'Nachricht archiviert.' : `${ids.length} Nachrichten archiviert.`);
  await refreshFolders(mailboxId);
}

/** Auf Wiedervorlage bis `until` (Date). */
export async function snoozeSelected(until) {
  const s = requireSelection();
  if (!s) return;
  if (!(until instanceof Date) || isNaN(until) || until <= new Date()) { toast('Der Zeitpunkt muss in der Zukunft liegen.', 'error'); return; }
  await removeFromList(s.ids);
  await call('snooze_messages', { mailboxId: s.mailboxId, ids: s.ids, until: until.toISOString() });
  toast(`Auf Wiedervorlage bis ${fmtSnoozeTarget(until)}.`);
  await refreshFolders(s.mailboxId);
}

export async function unsnoozeSelected() {
  const s = requireSelection();
  if (!s) return;
  await removeFromList(s.ids);
  await call('unsnooze_messages', { mailboxId: s.mailboxId, ids: s.ids });
  toast(s.ids.length === 1 ? 'Nachricht ist wieder im Posteingang.' : `${s.ids.length} Nachrichten sind wieder im Posteingang.`);
  await refreshFolders(s.mailboxId);
}

/** Menu items listing all folders of the current mailbox (indented). */
export function folderPickerItems(onPick, excludeId) {
  const mbId = state.current.mailboxId;
  const fs = foldersOf(mbId);
  const ids = new Set(fs.map((f) => f.id));
  const out = [];
  const walk = (parent, depth) => {
    for (const f of sortFolders(fs.filter((x) => (parent === null ? !x.parentId || !ids.has(x.parentId) : x.parentId === parent)))) {
      if (f.wellKnown === 'snoozed') continue;
      out.push({ label: '   '.repeat(depth) + f.displayName, icon: WK_ICON[f.wellKnown] || 'folder', disabled: f.id === excludeId, onClick: () => onPick(f), testid: 'folder-pick-' + f.displayName });
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
    ...cats.slice(0, 20).map((c) => ({
      label: c.displayName, iconHtml: swatchHtml(presetHex(c.color), 10),
      checked: s.items.length > 0 && s.items.every((m) => (m.categories || []).includes(c.displayName)),
      testid: 'cat-' + c.displayName,
      onClick: () => toggleCategory(c.displayName),
    })),
    cats.length ? { separator: true } : null,
    { label: 'Alle Kategorien entfernen', disabled: !s.items.some((m) => visibleCats(m.categories).length), onClick: () => setCategoriesFor(s.items.map((m) => ({ m, cats: laneCats(m.categories) }))) },
    { label: 'Kategorien verwalten…', icon: 'settings', onClick: () => openCategoryManager(accountId) },
  ].filter(Boolean);
}

async function toggleCategory(name) {
  const s = requireSelection();
  if (!s) return;
  const all = s.items.every((m) => (m.categories || []).includes(name));
  if (!all) countTagUse([name]);
  await setCategoriesFor(s.items.map((m) => {
    const cur = m.categories || [];
    return { m, cats: all ? cur.filter((c) => c !== name) : cur.includes(name) ? cur : [...cur, name] };
  }));
}

async function setCategoriesFor(changes) {
  const groups = new Map();
  for (const { m, cats } of changes) {
    m.categories = cats;
    localUpdate([m.id], (x) => { x.categories = cats; });
    if (currentDetail && currentDetail.id === m.id) currentDetail.categories = cats;
    const k = JSON.stringify(cats);
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k).push(m.id);
  }
  renderList();
  for (const [k, ids] of groups) await call('set_categories', { mailboxId: state.current.mailboxId, ids, categories: JSON.parse(k) }, { silent: true });
}

export function flagMenuItems() {
  return [
    { label: 'Kennzeichnen', icon: 'flag', onClick: () => setFlagSelected('flagged') },
    { label: 'Als erledigt markieren', icon: 'check', onClick: () => setFlagSelected('complete') },
    { label: 'Kennzeichnung entfernen', onClick: () => setFlagSelected('notFlagged') },
  ];
}

export async function showSource() {
  const s = requireSelection();
  if (!s) return;
  const src = await call('get_message_source', { mailboxId: s.mailboxId, messageId: s.ids[0] });
  const ta = h('textarea.input.selectable.source', { readOnly: true });
  ta.value = src || '';
  await modal({ title: 'Nachrichtenquelle – ' + (s.items[0].subject || ''), body: ta, width: 860, buttons: [{ label: 'Schließen', value: true, primary: true }] });
}

function moreItems(fromReadingPane) {
  const s = getSelection();
  const single = s.items.length === 1;
  const anyUnread = s.items.some((m) => !m.isRead);
  return [
    anyUnread
      ? { label: 'Als gelesen markieren', icon: 'mailOpen', shortcut: 'Strg+Q', onClick: () => markSelected(true) }
      : { label: 'Als ungelesen markieren', icon: 'mail', shortcut: 'Strg+U', onClick: () => markSelected(false) },
    { label: 'Kennzeichnung', icon: 'flag', submenu: flagMenuItems },
    { label: 'Verschieben', icon: 'move', submenu: () => moveMenuItems('move') },
    { separator: true },
    { label: 'In neuem Fenster öffnen', icon: 'open', disabled: !single, onClick: () => openMessage(s.items[0]) },
    fromReadingPane ? { label: 'Drucken', icon: 'print', onClick: () => printMessage(els.read) } : null,
    { label: 'Quelltext anzeigen', icon: 'source', disabled: !single, onClick: () => showSource() },
  ].filter(Boolean);
}

export function snoozeMenuItems(buildOptions) {
  return buildOptions();
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
      : { label: 'Als ungelesen markieren', icon: 'mail', shortcut: 'Strg+U', onClick: () => markSelected(false) },
    { label: 'Kategorisieren', icon: 'tag', shortcut: 'T', submenu: categoryMenuItems },
    { label: 'Kennzeichnung', icon: 'flag', submenu: flagMenuItems },
    { label: 'Auf Wiedervorlage', icon: 'snooze', shortcut: 'S', submenu: () => (hooks.snoozeItems ? hooks.snoozeItems() : []), testid: 'ctx-snooze' },
    { label: 'Kanban-Lane', icon: 'kanban', submenu: laneMenuItems, testid: 'ctx-lane' },
    { separator: true },
    { label: 'Archivieren', icon: 'archive', shortcut: 'E', onClick: () => archiveSelected(), testid: 'ctx-archive' },
    { label: 'Verschieben', icon: 'move', submenu: () => moveMenuItems('move'), testid: 'ctx-move' },
    { label: 'Kopieren', icon: 'copy', submenu: () => moveMenuItems('copy') },
    { label: 'Löschen', icon: 'delete', shortcut: 'Entf', onClick: () => deleteSelected(false), testid: 'ctx-delete' },
    { separator: true },
    { label: 'Quelltext anzeigen', icon: 'source', disabled: !single, onClick: () => showSource() },
  ], x, y);
}

/** Set by the shell (snooze menu items are built there). */
export const hooks = { snoozeItems: null, openLaneSettings: null };

/** Called by the shell when backend events arrive. */
export async function onMailChanged(p) {
  const tasks = [];
  if (p && p.mailboxId) tasks.push(refreshFolders(p.mailboxId));
  if (p && p.mailboxId === state.current.mailboxId && (!p.folderId || p.folderId === state.current.folderId)) tasks.push(reloadList());
  await Promise.all(tasks);
}

export function focusList() { if (kanbanActive()) board.focus(); else els.list.focus(); }

// ============================================================ Kanban lanes
/** Moves messages into `lane` (null = no lane); keeps all other categories. Undo via toast. */
export async function setLaneFor(ids, lane) {
  const { mailboxId } = state.current;
  const lanes = currentLanes();
  if (lane && !lanes.includes(lane)) { toast(`Die Lane „${lane}“ existiert nicht.`, 'error'); return; }
  const before = new Map();
  for (const id of ids) { const m = findItem(id); if (m) before.set(id, laneOf(m, lanes) || null); }
  const apply = (id, l) => localUpdate([id], (m) => {
    m.categories = [...visibleCats(m.categories), ...laneCats(m.categories).filter((c) => !lanes.includes(laneName(c))), ...(l ? [laneCategory(l)] : [])];
  });
  for (const id of ids) apply(id, lane);
  renderList();
  if (currentDetail && ids.includes(currentDetail.id)) currentDetail.categories = (findItem(currentDetail.id) || currentDetail).categories;
  try {
    await call('set_lane', { mailboxId, ids, lane: lane || null }, { silent: true });
  } catch (e) {
    for (const [id, l] of before) apply(id, l);
    renderList();
    throw e;
  }
  const label = lane ? `„${lane}“` : '„Ohne Lane“';
  toast(ids.length === 1 ? `Nach ${label} verschoben.` : `${ids.length} Nachrichten nach ${label} verschoben.`, 'info', 5000, {
    action: { label: 'Rückgängig', onClick: async () => {
      const groups = new Map();
      for (const [id, l] of before) { const k = l || ''; if (!groups.has(k)) groups.set(k, []); groups.get(k).push(id); }
      for (const [id, l] of before) apply(id, l);
      renderList();
      for (const [k, gids] of groups) await call('set_lane', { mailboxId, ids: gids, lane: k || null }, { silent: true });
    } },
  });
}

/** Keyboard 1–9: lane N of the current account. */
export async function setLaneByIndex(n) {
  const s = getSelection();
  if (!s.ids.length) return;
  const lanes = currentLanes();
  if (!lanes.length) { toast('Noch keine Kanban-Lanes – in den Einstellungen unter „Kanban“ anlegen.'); return; }
  const lane = lanes[n - 1];
  if (!lane) return;
  await setLaneFor(s.ids, lane);
}

export function laneMenuItems() {
  const s = getSelection();
  const lanes = currentLanes();
  return [
    ...lanes.map((l, i) => ({
      label: l, shortcut: i < 9 ? String(i + 1) : '', checked: s.items.length > 0 && s.items.every((m) => laneOf(m, lanes) === l),
      onClick: () => setLaneFor(s.ids, l), testid: 'lane-' + l,
    })),
    lanes.length ? { separator: true } : null,
    { label: 'Ohne Lane', disabled: !s.items.some((m) => laneOf(m, lanes)), onClick: () => setLaneFor(s.ids, null) },
    { label: 'Lanes bearbeiten…', icon: 'settings', onClick: () => hooks.openLaneSettings && hooks.openLaneSettings(currentAccount()) },
  ].filter(Boolean);
}

export async function onLanesChanged(accountId) {
  await loadLanes(accountId);
  if (accountId === currentAccount()) renderList();
}
