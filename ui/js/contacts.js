// Contacts module: source list (sidebar), contact list, detail card, editor dialog.
import { call, openWindow } from './api.js';
import { h, esc, initials, avatarColor, debounce, load, store, EMAIL_RE } from './util.js';
import { icon } from './icons.js';
import { modal, confirmDialog, reportError, toast, showMenu, busyClick } from './dialogs.js';
import { state, mailboxes } from './state.js';
import { isImap, isGoogle, kindLabel, kindClass, accountLabel } from './accounts.js';
import { makeSplitter } from './mail.js';

const PHONE_KINDS = [['business', 'Geschäftlich'], ['mobile', 'Mobil'], ['home', 'Privat']];
const SOURCE_LABEL = { o365: 'Microsoft 365', google: 'Google', local: 'Lokal', collected: 'Gesammelt' };

const ct = {
  source: load('contactsSource', 'all'), // 'all' | 'collected' | accountId
  query: '',
  items: [],
  total: 0,
  selectedId: null,
  loadToken: 0,
};
const els = {};
let onSelectionChanged = () => {};

export function initContacts({ navHost, contentHost, onSelection }) {
  onSelectionChanged = onSelection || onSelectionChanged;
  els.nav = navHost;
  els.view = h('div.contactsview.hidden#contactsview');
  els.listpane = h('div.ct-listpane');
  els.search = h('input.input.ct-search', { type: 'search', placeholder: 'Kontakte durchsuchen', 'aria-label': 'Kontakte durchsuchen', dataset: { testid: 'contacts-search' }, spellcheck: false });
  els.head = h('div.list-head', h('div.lh-title', h('span.ct-title#ct-title', 'Kontakte'), h('span.lh-count#ct-count'), h('span.spacer')), els.search);
  els.list = h('div.ct-list', { tabindex: '0', role: 'listbox', 'aria-label': 'Kontakte', dataset: { testid: 'contacts-list' } });
  els.listpane.append(els.head, els.list);
  els.split = h('div.splitter');
  els.detail = h('div.ct-detail', { dataset: { testid: 'contact-detail' } });
  els.view.append(els.listpane, els.split, els.detail);
  contentHost.appendChild(els.view);
  els.listpane.style.width = load('ctListWidth', 360) + 'px';
  makeSplitter(els.split, els.listpane, 'x', 'ctListWidth');
  const run = debounce(() => { ct.query = els.search.value.trim(); reload().catch(reportError); }, 200);
  els.search.addEventListener('input', run);
  els.search.addEventListener('keydown', (e) => { if (e.key === 'Escape') { els.search.value = ''; run(); } });
  els.list.addEventListener('keydown', (e) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const i = ct.items.findIndex((c) => c.id === ct.selectedId);
    const n = ct.items[Math.max(0, Math.min(ct.items.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))];
    if (n) select(n.id);
  });
}

export function showContactsView(show) {
  els.view.classList.toggle('hidden', !show);
  if (show) { renderNav(); reload().catch(reportError); }
}

export function isVisible() { return els.view && !els.view.classList.contains('hidden'); }
export function selectedContact() { return ct.items.find((c) => c.id === ct.selectedId) || null; }

// ------------------------------------------------------------------ sidebar
export function renderNav() {
  if (!els.nav) return;
  els.nav.innerHTML = '';
  const node = (key, label, ic, extra) => {
    const sel = ct.source === key;
    const n = h('div.tree-node' + (sel ? '.selected' : ''), { style: { paddingLeft: '8px' }, role: 'treeitem', 'aria-selected': sel ? 'true' : 'false', dataset: { testid: 'ct-src-' + (key.length > 20 ? 'account' : key), source: key }, title: label },
      h('span.twisty'), h('span.fic', { html: icon(ic, 18) }), h('span.fname', label), extra || null);
    n.addEventListener('click', () => { ct.source = key; store('contactsSource', key); ct.selectedId = null; renderNav(); reload().catch(reportError); });
    return n;
  };
  const sec = h('div.tree-section', { role: 'tree' });
  sec.appendChild(node('all', 'Alle Kontakte', 'contacts'));
  for (const a of state.accounts) {
    sec.appendChild(node(a.id, accountLabel(a), isImap(a) ? 'server' : 'people', h('span.kind-badge.sm.' + kindClass(a), kindLabel(a))));
  }
  sec.appendChild(node('collected', 'Gesammelte Adressen', 'clock'));
  els.nav.appendChild(sec);
  els.nav.appendChild(h('p.tree-hint.ct-hint', 'Adressen, an die Sie schreiben, werden automatisch gesammelt und beim Verfassen vorgeschlagen.'));
}

// ------------------------------------------------------------------ data
export async function reload() {
  const token = ++ct.loadToken;
  if (!ct.items.length) {
    els.list.innerHTML = '';
    els.list.appendChild(h('div.loading-state', h('span.busy'), h('div', 'Kontakte werden geladen…')));
  }
  const accountId = ct.source !== 'all' && ct.source !== 'collected' ? ct.source : null;
  const source = ct.source === 'collected' ? 'collected' : null;
  const res = await call('list_contacts', { query: ct.query || null, accountId, source, offset: 0, limit: 2000 }, { silent: true }).catch((e) => { reportError(e); return { items: [], total: 0 }; });
  if (token !== ct.loadToken) return;
  const items = (res && res.items) || [];
  ct.items = items;
  ct.total = res.total ?? items.length;
  if (ct.selectedId && !items.some((c) => c.id === ct.selectedId)) ct.selectedId = null;
  if (!ct.selectedId && items.length) ct.selectedId = items[0].id;
  renderList();
  renderDetail();
}

export async function sync() {
  await Promise.all(state.accounts.filter((a) => !isImap(a) || isGoogle(a)).map((a) => call('sync_contacts', { accountId: a.id }, { silent: true }).catch((e) => console.warn(e))));
  await reload();
}

function select(id) {
  ct.selectedId = id;
  els.list.querySelectorAll('.ct-row').forEach((r) => r.classList.toggle('selected', r.dataset.id === id));
  const row = els.list.querySelector(`.ct-row[data-id="${CSS.escape(id)}"]`);
  if (row) row.scrollIntoView({ block: 'nearest' });
  renderDetail();
}

// ------------------------------------------------------------------ list
function sortKey(c) { return (c.displayName || c.emails[0] || '').trim(); }
function letterOf(c) {
  const ch = sortKey(c).charAt(0).toUpperCase().normalize('NFD').replace(/[̀-ͯ]/g, '');
  return /[A-Z]/.test(ch) ? ch : '#';
}

function renderList() {
  const title = document.getElementById('ct-title');
  const count = document.getElementById('ct-count');
  const acc = state.accounts.find((a) => a.id === ct.source);
  if (title) title.textContent = ct.source === 'all' ? 'Alle Kontakte' : ct.source === 'collected' ? 'Gesammelte Adressen' : acc ? accountLabel(acc) : 'Kontakte';
  if (count) count.textContent = String(ct.total);
  els.list.innerHTML = '';
  if (!ct.items.length) {
    els.list.appendChild(h('div.empty', h('span.empty-ic', { html: icon('contacts', 40) }), ct.query ? 'Keine Kontakte gefunden.' : 'Keine Kontakte vorhanden.'));
    onSelectionChanged();
    return;
  }
  const sorted = [...ct.items].sort((a, b) => sortKey(a).localeCompare(sortKey(b), 'de', { sensitivity: 'base' }));
  let last = null;
  for (const c of sorted) {
    const L = letterOf(c);
    if (L !== last) { els.list.appendChild(h('div.ct-letter', L)); last = L; }
    const name = c.displayName || c.emails[0] || '(ohne Namen)';
    const sub = [c.emails[0], c.company].filter(Boolean).join(' · ');
    const row = h('div.ct-row' + (c.id === ct.selectedId ? '.selected' : ''), { role: 'option', dataset: { id: c.id, testid: 'contact-row' }, title: name },
      h('span.avatar.sm', { style: { background: avatarColor(c.emails[0] || name) } }, initials(name, c.emails[0])),
      h('div.m-main', h('div.m-l1', h('span.m-from', name), c.source === 'collected' ? h('span.ct-src', 'gesammelt') : null), h('div.m-l3', h('span.m-prev', sub))));
    row.addEventListener('click', () => select(c.id));
    row.addEventListener('dblclick', () => editContact(c).catch(reportError));
    row.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      select(c.id);
      showMenu([
        { label: 'E-Mail schreiben', icon: 'mail', disabled: !c.emails.length, onClick: () => writeTo(c, c.emails[0]) },
        { label: 'Bearbeiten…', icon: 'edit', onClick: () => editContact(c) },
        { separator: true },
        { label: 'Löschen', icon: 'delete', disabled: c.readOnly, onClick: () => deleteContact(c) },
      ], e.clientX, e.clientY);
    });
    els.list.appendChild(row);
  }
  onSelectionChanged();
}

// ------------------------------------------------------------------ detail
function renderDetail() {
  els.detail.innerHTML = '';
  const c = selectedContact();
  onSelectionChanged();
  if (!c) { els.detail.appendChild(h('div.empty', h('span.empty-ic', { html: icon('contacts', 44) }), 'Wählen Sie einen Kontakt aus.')); return; }
  const name = c.displayName || c.emails[0] || '(ohne Namen)';
  const acc = state.accounts.find((a) => a.id === c.accountId);
  const role = [c.jobTitle, c.company].filter(Boolean).join(' · ');
  const btn = (label, ic, fn, opts = {}) => h('button.btn.small' + (opts.primary ? '.primary' : ''), { type: 'button', disabled: !!opts.disabled, dataset: { testid: opts.testid || '' }, html: icon(ic, 15) + ' ' + esc(label), onclick: busyClick(fn) });
  const section = (title, ic, rows) => (rows.length ? h('div.ct-sec', h('div.ct-sec-title', { html: icon(ic, 15) + `<span>${esc(title)}</span>` }), ...rows) : null);
  const card = h('div.ct-card',
    h('div.ct-hero',
      h('span.avatar.xl', { style: { background: avatarColor(c.emails[0] || name) } }, initials(name, c.emails[0])),
      h('div.ct-hero-text',
        h('h2.ct-name', { dataset: { testid: 'contact-name' } }, name),
        role ? h('div.ct-role', role) : null,
        h('div.ct-origin.dim', `${SOURCE_LABEL[c.source] || c.source}${acc ? ' · ' + accountLabel(acc) : ''}${c.readOnly ? ' · schreibgeschützt' : ''}`)),
      h('div.ct-actions',
        btn('E-Mail schreiben', 'mail', () => writeTo(c, c.emails[0]), { primary: true, disabled: !c.emails.length, testid: 'contact-write' }),
        btn('Bearbeiten', 'edit', () => editContact(c), { testid: 'contact-edit' }),
        btn('Löschen', 'delete', () => deleteContact(c), { disabled: c.readOnly, testid: 'contact-delete' }))),
    h('div.ct-body',
      section('E-Mail', 'mail', c.emails.map((e) => {
        const a = h('a.ct-mail', { href: '#', dataset: { testid: 'contact-email' } }, e);
        a.addEventListener('click', (ev) => { ev.preventDefault(); writeTo(c, e); });
        return h('div.ct-line', a);
      })),
      section('Telefon', 'phone', (c.phones || []).map((p) => h('div.ct-line', h('span.ct-kind', (PHONE_KINDS.find(([k]) => k === p.kind) || [0, p.kind])[1]), h('span.selectable', p.number)))),
      section('Firma', 'building', [c.company, c.jobTitle].filter(Boolean).map((t) => h('div.ct-line.selectable', t))),
      section('Notizen', 'note', c.notes ? [h('div.ct-notes.selectable', c.notes)] : [])));
  els.detail.appendChild(card);
}

function mailboxForContact(c) {
  const mbs = mailboxes();
  return (mbs.find((m) => m.accountId === c.accountId) || mbs[0] || {}).id;
}

export function writeTo(c, address) {
  if (!address) return;
  const name = c.displayName && c.displayName !== address ? `${c.displayName} <${address}>` : address;
  return openWindow('compose', { mailboxId: mailboxForContact(c), mode: 'new', to: name });
}

// ------------------------------------------------------------------ editor
export async function newContact() {
  const def = state.accounts.find((a) => a.id === ct.source) || state.accounts[0];
  return editContact(null, def && def.id);
}

export async function editContact(c, accountId) {
  const v = c ? { ...c, emails: [...(c.emails || [])], phones: (c.phones || []).map((p) => ({ ...p })) }
    : { displayName: '', givenName: '', surname: '', emails: [''], company: '', jobTitle: '', phones: [], notes: '' };
  if (!v.emails.length) v.emails.push('');
  const inp = (name, value, ph) => h('input.input', { name, value: value || '', placeholder: ph || '', spellcheck: false });
  const given = inp('givenName', v.givenName, 'Vorname');
  const sur = inp('surname', v.surname, 'Nachname');
  const disp = inp('displayName', v.displayName, 'Anzeigename');
  let dispTouched = !!(c && c.displayName && c.displayName !== `${c.givenName || ''} ${c.surname || ''}`.trim());
  const autoName = () => { if (!dispTouched) disp.value = `${given.value} ${sur.value}`.trim(); };
  given.addEventListener('input', autoName);
  sur.addEventListener('input', autoName);
  disp.addEventListener('input', () => { dispTouched = true; });
  const company = inp('company', v.company, 'Firma');
  const job = inp('jobTitle', v.jobTitle, 'Position');
  const notes = h('textarea.input', { name: 'notes', rows: 3 });
  notes.value = v.notes || '';
  const accSel = h('select.input', { name: 'contactAccount' }, ...state.accounts.map((a) => h('option', { value: a.id, selected: a.id === accountId }, `${accountLabel(a)} (${isGoogle(a) ? 'Google' : isImap(a) ? 'lokal' : 'Microsoft 365'})`)));

  const emailsHost = h('div.ct-multi', { dataset: { testid: 'ct-emails' } });
  const renderEmails = () => {
    emailsHost.innerHTML = '';
    v.emails.forEach((e, i) => {
      const x = h('input.input', { name: `email${i}`, type: 'email', value: e, placeholder: 'name@beispiel.de', spellcheck: false });
      x.addEventListener('input', () => { v.emails[i] = x.value; });
      const del = h('button.icon-btn.sm', { type: 'button', title: 'Entfernen', html: icon('close', 13), disabled: v.emails.length === 1, onclick: () => { v.emails.splice(i, 1); renderEmails(); } });
      emailsHost.appendChild(h('div.ct-multirow', x, del));
    });
    emailsHost.appendChild(h('button.linkbtn', { type: 'button', dataset: { testid: 'ct-add-email' }, onclick: () => { v.emails.push(''); renderEmails(); emailsHost.querySelector(`input[name=email${v.emails.length - 1}]`).focus(); } }, '+ Weitere E-Mail-Adresse'));
  };
  const phonesHost = h('div.ct-multi');
  const renderPhones = () => {
    phonesHost.innerHTML = '';
    v.phones.forEach((p, i) => {
      const kind = h('select.input.ct-pkind', { name: `phoneKind${i}` }, ...PHONE_KINDS.map(([k, l]) => h('option', { value: k, selected: k === p.kind }, l)));
      kind.addEventListener('change', () => { p.kind = kind.value; });
      const num = h('input.input', { name: `phone${i}`, value: p.number, placeholder: '+49 …' });
      num.addEventListener('input', () => { p.number = num.value; });
      phonesHost.appendChild(h('div.ct-multirow', kind, num, h('button.icon-btn.sm', { type: 'button', title: 'Entfernen', html: icon('close', 13), onclick: () => { v.phones.splice(i, 1); renderPhones(); } })));
    });
    phonesHost.appendChild(h('button.linkbtn', { type: 'button', onclick: () => { v.phones.push({ kind: v.phones.length ? 'mobile' : 'business', number: '' }); renderPhones(); } }, '+ Telefonnummer'));
  };
  renderEmails();
  renderPhones();
  const row = (label, ...x) => h('div.ct-frow', h('label.srv-l', label), h('div.ct-fval', ...x));
  const body = h('div.ct-form',
    !c && state.accounts.length > 1 ? row('Speichern in', accSel) : null,
    row('Name', h('div.ct-namegrid', given, sur)),
    row('Anzeigen als', disp),
    row('E-Mail', emailsHost),
    row('Firma', h('div.ct-namegrid', company, job)),
    row('Telefon', phonesHost),
    row('Notizen', notes),
    c && c.source === 'collected' ? h('p.field-hint.ct-collected-hint', 'Gesammelte Adresse – beim Speichern wird daraus ein lokaler Kontakt.') : null);
  const res = await modal({
    title: c ? 'Kontakt bearbeiten' : 'Neuer Kontakt', body, width: 560,
    buttons: [{ label: 'Speichern', value: 'ok', primary: true, id: 'contact-save' }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: async () => {
      const emails = v.emails.map((e) => e.trim()).filter(Boolean);
      const bad = emails.filter((e) => !EMAIL_RE.test(e));
      if (bad.length) { toast(`Ungültige E-Mail-Adresse: ${bad.join(', ')}`, 'error'); return false; }
      const input = {
        displayName: disp.value.trim() || `${given.value} ${sur.value}`.trim() || emails[0] || '',
        givenName: given.value.trim(), surname: sur.value.trim(), emails,
        company: company.value.trim(), jobTitle: job.value.trim(),
        phones: v.phones.filter((p) => p.number.trim()).map((p) => ({ kind: p.kind, number: p.number.trim() })),
        notes: notes.value,
      };
      if (!input.displayName) { toast('Bitte einen Namen oder eine E-Mail-Adresse angeben.', 'error'); given.focus(); return false; }
      const saved = c
        ? await call('update_contact', { id: c.id, input })
        : await call('create_contact', { accountId: accSel.value || accountId, input });
      return { saved };
    },
  });
  if (res && res.saved) {
    ct.selectedId = res.saved.id;
    await reload();
    toast(c ? 'Kontakt gespeichert.' : 'Kontakt angelegt.');
  }
}

export async function deleteContact(c) {
  c = c || selectedContact();
  if (!c || c.readOnly) return;
  if (!(await confirmDialog(`Kontakt „${c.displayName || c.emails[0]}“ löschen?${c.source === 'o365' ? ' Er wird auch in Microsoft 365 gelöscht.' : c.source === 'google' ? ' Er wird auch in Google gelöscht.' : ''}`, { danger: true, ok: 'Löschen', title: 'Kontakt löschen' }))) return;
  await call('delete_contact', { id: c.id });
  ct.selectedId = null;
  await reload();
}

export async function onContactsChanged() { if (isVisible()) await reload(); }
export function onAccountsChanged() { if (isVisible()) { renderNav(); reload().catch(reportError); } }
export function focusSearch() { els.search.focus(); }
