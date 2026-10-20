// Signatures per account: settings panel and helpers for the compose window.
import { call } from './api.js';
import { h, esc } from './util.js';
import { icon } from './icons.js';
import { createEditor } from './editor.js';
import { confirmDialog, toast } from './dialogs.js';

const EMPTY = { signatures: [], defaultId: null, useOnNew: true, useOnReply: true, useOnForward: false };
const cache = new Map(); // accountId -> SignatureSettings

export async function loadSignatures(accountId, { fresh = false } = {}) {
  if (!accountId) return { ...EMPTY };
  if (!fresh && cache.has(accountId)) return cache.get(accountId);
  let s;
  try { s = await call('get_signatures', { accountId }, { silent: true }); } catch (e) { console.warn(e); s = null; }
  s = { ...EMPTY, ...(s || {}) };
  cache.set(accountId, s);
  return s;
}

/** Signature that create_draft would insert for `mode` (null = none). */
export function defaultSignatureFor(settings, mode) {
  if (!settings || !settings.defaultId) return null;
  const use = mode === 'forward' ? settings.useOnForward : (mode === 'reply' || mode === 'replyAll') ? settings.useOnReply : settings.useOnNew;
  if (!use) return null;
  return settings.signatures.find((s) => s.id === settings.defaultId) || null;
}

/**
 * Puts `sig` (or nothing when null) into the editor document: replaces the
 * existing div.signature block, else inserts one (new mail: at the end; reply/forward: on top).
 */
export function applySignature(doc, sig, mode) {
  if (!doc || !doc.body) return;
  let block = doc.body.querySelector('div.signature');
  if (!sig) {
    if (block) block.remove();
    return;
  }
  if (!block) {
    block = doc.createElement('div');
    block.className = 'signature';
    const spacer = doc.createElement('div');
    spacer.innerHTML = '<br>';
    if (mode === 'new' || !mode) {
      doc.body.append(spacer, block);
    } else {
      const first = doc.body.firstChild;
      doc.body.insertBefore(spacer, first);
      doc.body.insertBefore(block, first);
    }
  }
  block.setAttribute('data-signature-id', sig.id);
  block.innerHTML = sig.html || '';
}

/** Id of the signature currently in the editor (or null). */
export function currentSignatureId(doc) {
  const b = doc && doc.body && doc.body.querySelector('div.signature');
  return b ? (b.getAttribute('data-signature-id') || '') : null;
}

// ------------------------------------------------------------------ settings panel
/**
 * Settings tab. accounts: Account[]. Returns { el, save(): Promise<void> } – save persists every
 * account whose signatures were touched.
 */
export function signaturesPanel(accounts, initialAccountId) {
  const work = new Map(); // accountId -> { settings, dirty }
  let accountId = initialAccountId || (accounts[0] && accounts[0].id);
  let selKey = null;
  let keySeq = 0;
  const key = () => 'k' + (++keySeq);

  const accSel = h('select.input', { name: 'sigAccount', 'aria-label': 'Konto', dataset: { testid: 'sig-account' } },
    ...accounts.map((a) => h('option', { value: a.id, selected: a.id === accountId }, `${a.displayName} <${a.email}>`)));
  const listEl = h('div.sig-list', { role: 'listbox', dataset: { testid: 'sig-list' } });
  const nameIn = h('input.input', { name: 'sigName', placeholder: 'Name der Signatur', 'aria-label': 'Name der Signatur' });
  const editor = createEditor({ minHeight: 170, onInput: () => { const s = selected(); if (s) { s.html = editor.getHtml(); markDirty(); } } });
  const edHost = h('div.sig-edit', h('div.sig-namerow', nameIn), h('div.sig-editor-box', editor.toolbar, editor.area));
  const empty = h('div.sig-empty.dim', 'Noch keine Signatur. Legen Sie mit „Neue Signatur“ eine an.');
  const cbNew = h('input', { type: 'checkbox', name: 'useOnNew' });
  const cbReply = h('input', { type: 'checkbox', name: 'useOnReply' });
  const cbFwd = h('input', { type: 'checkbox', name: 'useOnForward' });
  const flags = h('div.sig-flags',
    h('div.sig-flags-title', 'Standardsignatur automatisch einfügen bei:'),
    h('label.checkrow', cbNew, 'Neuen E-Mails'),
    h('label.checkrow', cbReply, 'Antworten (Antworten / Allen antworten)'),
    h('label.checkrow', cbFwd, 'Weiterleitungen'));

  const cur = () => work.get(accountId);
  const markDirty = () => { const w = cur(); if (w) w.dirty = true; };
  const selected = () => { const w = cur(); return w && w.settings.signatures.find((s) => s._k === selKey); };

  for (const [cb, prop] of [[cbNew, 'useOnNew'], [cbReply, 'useOnReply'], [cbFwd, 'useOnForward']]) {
    cb.addEventListener('change', () => { const w = cur(); if (!w) return; w.settings[prop] = cb.checked; w.dirty = true; });
  }
  nameIn.addEventListener('input', () => { const s = selected(); if (s) { s.name = nameIn.value; markDirty(); renderList(); } });

  async function ensure(accId) {
    if (work.has(accId)) return work.get(accId);
    const s = await loadSignatures(accId, { fresh: true });
    const settings = { ...s, signatures: s.signatures.map((x) => ({ ...x, _k: key() })) };
    const w = { settings, dirty: false, defaultKey: null };
    const d = settings.signatures.find((x) => x.id && x.id === settings.defaultId);
    w.defaultKey = d ? d._k : null;
    work.set(accId, w);
    return w;
  }

  function renderList() {
    const w = cur();
    listEl.innerHTML = '';
    if (!w) return;
    for (const s of w.settings.signatures) {
      const isDef = w.defaultKey === s._k;
      const star = h('button.icon-btn.sm.sig-star' + (isDef ? '.on' : ''), { type: 'button', title: isDef ? 'Standardsignatur' : 'Als Standard festlegen', 'aria-label': 'Als Standard', html: icon('star', 15), dataset: { testid: 'sig-default' } });
      star.addEventListener('click', (e) => { e.stopPropagation(); if (isDef) return; w.defaultKey = s._k; w.dirty = true; renderList(); });
      const del = h('button.icon-btn.sm', { type: 'button', title: 'Signatur löschen', 'aria-label': 'Signatur löschen', html: icon('delete', 15), dataset: { testid: 'sig-delete' } });
      del.addEventListener('click', async (e) => {
        e.stopPropagation();
        if (!(await confirmDialog(`Signatur „${s.name || 'Ohne Namen'}“ löschen?`, { danger: true, ok: 'Löschen' }))) return;
        w.settings.signatures = w.settings.signatures.filter((x) => x !== s);
        if (w.defaultKey === s._k) w.defaultKey = null;
        w.dirty = true;
        if (selKey === s._k) selKey = w.settings.signatures[0] ? w.settings.signatures[0]._k : null;
        await showSelected();
      });
      const row = h('div.sig-item' + (s._k === selKey ? '.selected' : ''), { role: 'option', dataset: { testid: 'sig-item' } },
        h('span.sig-ic', { html: icon('signature', 16) }),
        h('span.sig-name', s.name || 'Ohne Namen'),
        h('span.sig-tools', star, del));
      row.addEventListener('click', async () => { if (selKey !== s._k) { selKey = s._k; await showSelected(); } });
      listEl.appendChild(row);
    }
  }

  async function showSelected() {
    renderList();
    const s = selected();
    edHost.classList.toggle('hidden', !s);
    empty.classList.toggle('hidden', !!s);
    if (!s) return;
    nameIn.value = s.name;
    await editor.setHtml(s.html || '');
  }

  async function switchAccount(id) {
    accountId = id;
    const w = await ensure(id);
    cbNew.checked = !!w.settings.useOnNew;
    cbReply.checked = !!w.settings.useOnReply;
    cbFwd.checked = !!w.settings.useOnForward;
    selKey = w.settings.signatures[0] ? w.settings.signatures[0]._k : null;
    await showSelected();
  }
  accSel.addEventListener('change', () => switchAccount(accSel.value).catch(console.warn));

  const addBtn = h('button.btn.small', { type: 'button', dataset: { testid: 'sig-new' }, html: icon('plus', 14) + ' Neue Signatur' });
  addBtn.addEventListener('click', async () => {
    const w = cur();
    if (!w) return;
    const n = w.settings.signatures.length + 1;
    const s = { id: '', name: n === 1 ? 'Standard' : `Signatur ${n}`, html: '', _k: key() };
    w.settings.signatures.push(s);
    if (!w.defaultKey) w.defaultKey = s._k;
    w.dirty = true;
    selKey = s._k;
    await showSelected();
    nameIn.focus();
    nameIn.select();
  });

  const el = h('div.sig-panel',
    accounts.length > 1 ? h('div.sig-accrow', h('span.dim', 'Konto'), accSel) : null,
    h('div.sig-main',
      h('div.sig-side', listEl, addBtn),
      h('div.sig-right', edHost, empty)),
    flags);
  if (accountId) switchAccount(accountId).catch(console.warn);

  async function save() {
    for (const [accId, w] of work) {
      if (!w.dirty) continue;
      const sigs = w.settings.signatures;
      const defIdx = sigs.findIndex((s) => s._k === w.defaultKey);
      const strip = (list) => list.map(({ id, name, html }) => ({ id: id || '', name: (name || '').trim() || 'Ohne Namen', html: html || '' }));
      const base = { useOnNew: w.settings.useOnNew, useOnReply: w.settings.useOnReply, useOnForward: w.settings.useOnForward };
      const def = defIdx >= 0 ? sigs[defIdx] : null;
      let res = await call('save_signatures', { accountId: accId, settings: { ...base, signatures: strip(sigs), defaultId: def && def.id ? def.id : null } });
      if (def && !def.id && res && res.signatures && res.signatures[defIdx]) {
        // New entries get their ids on save – set the default in a second step.
        res = await call('save_signatures', { accountId: accId, settings: { ...res, defaultId: res.signatures[defIdx].id } });
      }
      cache.set(accId, { ...EMPTY, ...(res || {}) });
      w.dirty = false;
    }
  }
  return { el, save };
}

/** Compose toolbar control: "Signatur ▾". */
export function signatureMenuItems({ settings, current, onPick, onEdit }) {
  const items = (settings.signatures || []).map((s) => ({
    label: s.name || 'Ohne Namen', icon: 'signature', checked: current === s.id,
    shortcut: s.id === settings.defaultId ? 'Standard' : '', onClick: () => onPick(s), testid: 'sigpick-' + (s.name || '').replace(/\s+/g, '-'),
  }));
  if (!items.length) items.push({ label: 'Keine Signaturen vorhanden', disabled: true });
  items.push({ separator: true },
    { label: 'Keine Signatur', checked: current === null, onClick: () => onPick(null), testid: 'sigpick-none' },
    { separator: true },
    { label: 'Signaturen bearbeiten…', icon: 'settings', onClick: onEdit, testid: 'sigpick-edit' });
  return items;
}

export function invalidateSignatures(accountId) { if (accountId) cache.delete(accountId); else cache.clear(); }
export { esc, toast };
