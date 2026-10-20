// Dialogs: settings, category manager, auto reply, welcome / account setup.
import { call } from './api.js';
import { h, esc, toLocalIso } from './util.js';
import { icon } from './icons.js';
import { modal, confirmDialog, reportError, toast, field } from './dialogs.js';
import { PRESETS, presetHex, textOn, getCategories, setCategories } from './categories.js';
import { state, loadCategories, loadLanes, lanesOf } from './state.js';
import { LANE_NAME_RE } from './categories.js';
import { DEFAULT_SNOOZE, parseRule, formatRule, computeSnooze, fmtSnoozeTarget, snoozeOptions, BASE_LABELS } from './snooze.js';
import { signaturesPanel } from './signatures.js';
import { chooseAccountKind, imapWizard, accountDetail, isImap, kindLabel } from './accounts.js';

export const hooks = { onConfigSaved: () => {}, onAccountsChanged: () => {} };

function select(options, value) {
  const s = h('select.input');
  for (const [v, label] of options) s.appendChild(h('option', { value: v, selected: v === value }, label));
  return s;
}

// ---------------------------------------------------------------- settings
const TABS = [
  ['general', 'Allgemein', 'settings'],
  ['reading', 'Lesen', 'mailOpen'],
  ['signatures', 'Signaturen', 'signature'],
  ['snooze', 'Wiedervorlage', 'snooze'],
  ['kanban', 'Kanban', 'kanban'],
  ['accounts', 'Konten', 'user'],
];
const TAB_ALIASES = { compose: 'signatures', account: 'accounts' };

function snoozeEditor(initial) {
  let rows = initial.map((o) => ({ label: o.label, ...(parseRule(o.rule) || parseRule('tomorrow@08:00')) }));
  const host = h('div.snooze-editor', { dataset: { testid: 'snooze-editor' } });
  const render = () => {
    host.innerHTML = '';
    rows.forEach((r, i) => {
      const label = h('input.input.sn-label', { value: r.label, placeholder: 'Bezeichnung', name: `snoozeLabel${i}`, 'aria-label': 'Bezeichnung' });
      label.addEventListener('input', () => { r.label = label.value; });
      const base = select(BASE_LABELS, r.base);
      base.classList.add('sn-base');
      base.name = `snoozeBase${i}`;
      const days = h('input.input.sn-days', { type: 'number', min: '1', max: '365', value: String(r.days || 1), name: `snoozeDays${i}`, 'aria-label': 'Tage' });
      const daysWrap = h('span.sn-dayswrap' + (r.base === 'days' ? '' : '.hidden'), days, h('span.dim', 'Tage'));
      base.addEventListener('change', () => { r.base = base.value; daysWrap.classList.toggle('hidden', r.base !== 'days'); preview(); });
      days.addEventListener('input', () => { r.days = Math.max(1, parseInt(days.value, 10) || 1); preview(); });
      const time = h('input.input.sn-time', { type: 'time', value: `${String(r.hour).padStart(2, '0')}:${String(r.minute).padStart(2, '0')}`, name: `snoozeTime${i}`, 'aria-label': 'Uhrzeit' });
      time.addEventListener('change', () => { const [hh, mm] = (time.value || '08:00').split(':').map(Number); r.hour = hh || 0; r.minute = mm || 0; preview(); });
      const pv = h('span.sn-preview.dim');
      const preview = () => { const d = computeSnooze(formatRule(r)); pv.textContent = d ? '→ ' + fmtSnoozeTarget(d) : ''; };
      preview();
      const mv = (d) => { const j = i + d; if (j < 0 || j >= rows.length) return; [rows[i], rows[j]] = [rows[j], rows[i]]; render(); };
      host.appendChild(h('div.sn-row',
        h('div.sn-top', label, pv, h('span.spacer'),
          h('button.icon-btn.sm', { type: 'button', title: 'Nach oben', html: icon('up', 14), disabled: i === 0, onclick: () => mv(-1) }),
          h('button.icon-btn.sm', { type: 'button', title: 'Nach unten', html: icon('down', 14), disabled: i === rows.length - 1, onclick: () => mv(1) }),
          h('button.icon-btn.sm', { type: 'button', title: 'Entfernen', html: icon('close', 14), onclick: () => { rows.splice(i, 1); render(); } })),
        h('div.sn-rule', base, daysWrap, h('span.dim', 'um'), time)));
    });
    host.appendChild(h('div.sn-actions',
      h('button.btn.small', { type: 'button', dataset: { testid: 'snooze-add' }, html: icon('plus', 14) + ' Option hinzufügen', onclick: () => { rows.push({ label: 'In 3 Tagen', base: 'days', days: 3, hour: 8, minute: 0 }); render(); } }),
      h('button.btn.small', { type: 'button', onclick: () => { rows = DEFAULT_SNOOZE.map((o) => ({ label: o.label, ...parseRule(o.rule) })); render(); } }, 'Standard wiederherstellen')));
  };
  render();
  return { el: host, get: () => rows.filter((r) => r.label.trim()).map((r) => ({ label: r.label.trim(), rule: formatRule(r) })) };
}

/** Kanban lanes per account: ordered list with add / rename / delete / reorder. */
function lanesPanel(initialAccountId) {
  const accounts = state.accounts;
  let accountId = initialAccountId && accounts.some((a) => a.id === initialAccountId) ? initialAccountId : accounts[0].id;
  const edits = new Map(); // accountId -> string[] (only accounts the user touched)
  const original = new Map();
  const host = h('div.lane-editor', { dataset: { testid: 'lane-editor' } });
  const accSel = select(accounts.map((a) => [a.id, `${a.displayName || a.email} <${a.email}>`]), accountId);
  accSel.name = 'laneAccount';
  accSel.addEventListener('change', () => { accountId = accSel.value; ensure().then(render); });
  const listEl = h('div.lane-list', { role: 'list' });
  const input = h('input.input.lane-new', { type: 'text', placeholder: 'Neue Lane, z. B. „ToDo“', name: 'newLane', maxLength: 30, dataset: { testid: 'lane-input', noSubmit: '1' } });
  const hint = h('div.field-hint.lane-hint', 'Nur Buchstaben, Ziffern, Leerzeichen, - und _ (max. 30 Zeichen).');
  const lanes = () => edits.get(accountId) || [];

  async function ensure() {
    if (edits.has(accountId)) return;
    const l = await loadLanes(accountId);
    original.set(accountId, [...l]);
    edits.set(accountId, [...l]);
  }
  const validate = (name, except = -1) => {
    const n = name.trim();
    if (!LANE_NAME_RE.test(n)) return 'Ungültiger Name. Nur Buchstaben, Ziffern, Leerzeichen, - und _ (1–30 Zeichen).';
    if (lanes().some((x, i) => i !== except && x.toLowerCase() === n.toLowerCase())) return `Die Lane „${n}“ gibt es bereits.`;
    return null;
  };
  const add = () => {
    const err = validate(input.value);
    if (err) { input.classList.add('invalid'); toast(err, 'error'); return; }
    input.classList.remove('invalid');
    lanes().push(input.value.trim());
    input.value = '';
    render();
    input.focus();
  };
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); add(); } });
  input.addEventListener('input', () => input.classList.remove('invalid'));

  function render() {
    listEl.innerHTML = '';
    const l = lanes();
    l.forEach((name, i) => {
      const nameEl = h('span.lane-name', name);
      const mv = (d) => { const j = i + d; if (j < 0 || j >= l.length) return; [l[i], l[j]] = [l[j], l[i]]; render(); };
      const rename = async () => {
        const nn = await promptDialogLocal(name);
        if (nn === null || nn.trim() === name) return;
        const err = validate(nn, i);
        if (err) { toast(err, 'error'); return; }
        const ok = await confirmDialog(`Lane „${name}“ in „${nn.trim()}“ umbenennen? Karten der bisherigen Lane verlieren dabei ihre Lane-Zuordnung (sie landen in „Ohne Lane“).`, { title: 'Lane umbenennen', ok: 'Umbenennen' });
        if (ok) { l[i] = nn.trim(); render(); }
      };
      const del = async () => {
        const ok = await confirmDialog(`Lane „${name}“ löschen? Karten dieser Lane landen in „Ohne Lane“.`, { title: 'Lane löschen', ok: 'Löschen', danger: true });
        if (ok) { l.splice(i, 1); render(); }
      };
      listEl.appendChild(h('div.lane-item', { role: 'listitem', dataset: { testid: 'lane-item', name } },
        h('kbd.kb-key', i < 9 ? String(i + 1) : '·'), nameEl, h('span.spacer'),
        h('button.icon-btn.sm', { type: 'button', title: 'Nach oben', html: icon('up', 14), disabled: i === 0, onclick: () => mv(-1), dataset: { testid: 'lane-up' } }),
        h('button.icon-btn.sm', { type: 'button', title: 'Nach unten', html: icon('down', 14), disabled: i === l.length - 1, onclick: () => mv(1), dataset: { testid: 'lane-down' } }),
        h('button.icon-btn.sm', { type: 'button', title: 'Umbenennen', html: icon('edit', 14), onclick: () => rename().catch(reportError), dataset: { testid: 'lane-rename' } }),
        h('button.icon-btn.sm', { type: 'button', title: 'Löschen', html: icon('close', 14), onclick: () => del().catch(reportError), dataset: { testid: 'lane-delete' } })));
    });
    if (!l.length) listEl.appendChild(h('div.dim.lane-none', 'Noch keine Lanes. Legen Sie z. B. „ToDo“, „In Arbeit“ und „Warten“ an.'));
  }
  async function promptDialogLocal(value) {
    const inp = h('input.input', { value, maxLength: 30, style: { width: '100%' }, name: 'laneRename' });
    const r = await modal({ title: 'Lane umbenennen', body: h('div', h('label.lbl', 'Neuer Name:'), inp), width: 340,
      buttons: [{ label: 'OK', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }] });
    return r === 'ok' ? inp.value : null;
  }

  ensure().then(render).catch(reportError);
  host.append(...[
    accounts.length > 1 ? field('Konto', accSel) : null,
    listEl,
    h('div.lane-add', input, h('button.btn.small', { type: 'button', dataset: { testid: 'lane-add' }, html: icon('plus', 14) + ' Hinzufügen', onclick: add })),
    hint,
  ].filter(Boolean));
  return {
    el: host,
    async save() {
      for (const [acc, l] of edits) {
        const before = original.get(acc) || [];
        if (JSON.stringify(before) === JSON.stringify(l)) continue;
        const saved = await call('save_lanes', { accountId: acc, lanes: l });
        state.lanes.set(acc, saved || l);
        original.set(acc, [...(saved || l)]);
      }
    },
  };
}

export async function openSettings(initialTab = 'general', { accountId = null } = {}) {
  initialTab = TAB_ALIASES[initialTab] || initialTab;
  const c = { ...state.config };
  const clientId = h('input.input', { value: c.clientId || '', placeholder: '00000000-0000-0000-0000-000000000000', name: 'clientId' });
  const tenant = h('input.input', { value: c.tenant || 'organizations', name: 'tenant' });
  const interval = h('input.input', { type: 'number', min: '30', step: '30', value: String(c.syncIntervalSecs ?? 120), name: 'syncIntervalSecs', style: { width: '100px' } });
  const remote = h('input', { type: 'checkbox', checked: !!c.loadRemoteImages, name: 'loadRemoteImages' });
  const seps = h('input', { type: 'checkbox', checked: c.showDateSeparators !== false, name: 'showDateSeparators' });
  const pane = select([['right', 'Rechts'], ['bottom', 'Unten'], ['off', 'Aus']], c.readingPane || 'right');
  pane.name = 'readingPane';
  const mrOpts = [['0', 'Sofort beim Anzeigen'], ['2', 'Nach 2 Sekunden'], ['5', 'Nach 5 Sekunden'], ['10', 'Nach 10 Sekunden'], ['30', 'Nach 30 Sekunden'], ['-1', 'Nie automatisch']];
  if (!mrOpts.some(([v]) => v === String(c.markReadDelaySecs ?? 0))) mrOpts.push([String(c.markReadDelaySecs), `Nach ${c.markReadDelaySecs} Sekunden`]);
  const markRead = select(mrOpts, String(c.markReadDelaySecs ?? 0));
  markRead.name = 'markReadDelaySecs';
  const listMode = select([['comfortable', 'Komfortabel – mit Vorschautext'], ['compact', 'Kompakt – zwei Zeilen']], c.listMode === 'compact' ? 'compact' : 'comfortable');
  listMode.name = 'listMode';
  const snooze = snoozeEditor(snoozeOptions(c));
  const sigs = state.accounts.length ? signaturesPanel(state.accounts, accountId) : null;
  const lanes = state.accounts.length ? lanesPanel(accountId) : null;
  const kbPane = select([['bottom', 'Unten (unter dem Board)'], ['right', 'Rechts']], c.kanbanReadingPane || 'bottom');
  kbPane.name = 'kanbanReadingPane';
  const accounts = accountsPanel({ clientId, tenant, builtin: !!c.builtinClientId, demo: !!c.demo, initialAccountId: accountId });

  const panels = {
    general: h('div.set-panel',
      h('h3', 'Allgemein'),
      field('Automatisch synchronisieren alle', h('span.inline', interval, h('span.dim', 'Sekunden'))),
      c.demo ? h('p.note', 'Demo-Modus aktiv – es wird ein eingebauter Testserver mit Beispieldaten verwendet.') : null),
    reading: h('div.set-panel',
      h('h3', 'Lesen'),
      field('Lesebereich', pane),
      field('Als gelesen markieren', markRead),
      field('Nachrichtenliste', listMode),
      h('label.checkrow', seps, 'Datumstrenner (Heute, Gestern, …) in der Liste anzeigen'),
      h('label.checkrow', remote, 'Externe Bilder in Nachrichten automatisch laden')),
    signatures: h('div.set-panel',
      h('h3', 'Signaturen'),
      h('p.note', 'Jedes Konto hat eigene Signaturen. Die Standardsignatur (Stern) wird automatisch eingefügt; im Verfassen-Fenster kann sie über „Signatur“ gewechselt werden.'),
      sigs ? sigs.el : h('p.dim', 'Fügen Sie zuerst ein Konto hinzu.')),
    snooze: h('div.set-panel',
      h('h3', 'Auf Wiedervorlage'),
      h('p.note', 'Diese Zeitpunkte erscheinen im Menü „Auf Wiedervorlage“. Die Nachricht verschwindet bis dahin im Ordner „Wiedervorlage“ und kommt dann ungelesen in den Posteingang zurück.'),
      snooze.el),
    kanban: h('div.set-panel',
      h('h3', 'Kanban'),
      h('p.note', 'Lanes sind die Spalten der Kanban-Ansicht (Mail › „Kanban“). Technisch sind es Kategorien „[K_Name]“ – sie werden in der normalen Ansicht ausgeblendet, funktionieren aber auch in anderen Mailprogrammen. Tasten 1–9 verschieben die ausgewählte Mail in die jeweilige Lane.'),
      lanes ? lanes.el : h('p.dim', 'Fügen Sie zuerst ein Konto hinzu.'),
      field('Lesebereich im Kanban', kbPane)),
    accounts: h('div.set-panel', h('h3', 'Konten'), accounts.el),
  };
  const nav = h('div.set-nav', { role: 'tablist' });
  const content = h('div.set-content');
  const show = (key) => {
    nav.querySelectorAll('.set-tab').forEach((t) => t.classList.toggle('on', t.dataset.tab === key));
    content.innerHTML = '';
    content.appendChild(panels[key]);
  };
  for (const [key, label, ic] of TABS) {
    const t = h('button.set-tab', { type: 'button', role: 'tab', dataset: { tab: key, testid: 'settings-tab-' + key }, html: icon(ic, 16) + `<span>${esc(label)}</span>` });
    t.addEventListener('click', () => show(key));
    nav.appendChild(t);
  }
  show(panels[initialTab] ? initialTab : 'general');

  const res = await modal({
    title: 'Einstellungen', body: h('div.settings', nav, content), width: 760, className: 'settings-modal',
    buttons: [{ label: 'Speichern', value: 'ok', primary: true, id: 'settings-ok' }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: async () => {
      const iv = parseInt(interval.value, 10);
      if (!(iv >= 30)) { show('general'); interval.classList.add('invalid'); toast('Das Synchronisierungsintervall muss mindestens 30 Sekunden betragen.', 'error'); return false; }
      const config = {
        ...state.config,
        clientId: clientId.value.trim(), tenant: tenant.value.trim() || 'organizations', syncIntervalSecs: iv,
        loadRemoteImages: remote.checked,
        readingPane: pane.value, markReadDelaySecs: parseInt(markRead.value, 10), listMode: listMode.value,
        showDateSeparators: seps.checked, snoozeOptions: snooze.get(),
        kanbanReadingPane: kbPane.value,
      };
      delete config.signatureHtml;
      state.config = await call('save_config', { config });
      if (sigs) await sigs.save();
      if (lanes) { try { await lanes.save(); } catch (e) { show('kanban'); throw e; } }
      try { localStorage.setItem('oc.kanbanPane', JSON.stringify(kbPane.value)); } catch { /* ignore */ }
      return 'ok';
    },
  });
  if (res === 'ok') { hooks.onConfigSaved(); toast('Einstellungen gespeichert.'); }
}

// ---------------------------------------------------------------- accounts tab
function accountsPanel({ clientId, tenant, builtin, demo, initialAccountId }) {
  let selected = initialAccountId || (state.accounts[0] && state.accounts[0].id);
  const listEl = h('div.acc-list', { role: 'listbox', dataset: { testid: 'acc-list' } });
  const detailHost = h('div.acc-detail-host');
  const render = () => {
    listEl.innerHTML = '';
    if (!state.accounts.some((a) => a.id === selected)) selected = state.accounts[0] && state.accounts[0].id;
    for (const a of state.accounts) {
      const row = h('div.acc-item' + (a.id === selected ? '.selected' : ''), { role: 'option', dataset: { testid: 'acc-item', id: a.id } },
        h('span.avatar.xs', { style: { background: '#5b8def' } }, (a.displayName || a.email || '?').slice(0, 1).toUpperCase()),
        h('span.acc-text', h('span.acc-n', a.displayName || a.email), h('span.acc-e', a.email)),
        h('span.kind-badge.' + (isImap(a) ? 'imap' : 'o365'), kindLabel(a)));
      row.addEventListener('click', () => { selected = a.id; render(); });
      listEl.appendChild(row);
    }
    if (!state.accounts.length) listEl.appendChild(h('div.dim.acc-none', 'Noch kein Konto eingerichtet.'));
    detailHost.innerHTML = '';
    const acc = state.accounts.find((a) => a.id === selected);
    if (acc) {
      detailHost.appendChild(accountDetail(acc, {
        onRemoved: async (x) => { if (await removeAccount(x)) render(); },
        onSaved: async () => { hooks.onAccountsChanged(); await reloadAccounts(); render(); },
      }));
    }
  };
  const reloadAccounts = async () => { state.accounts = (await call('list_accounts', {}, { silent: true })) || []; };
  const add = h('button.btn.small', { type: 'button', dataset: { testid: 'acc-add' }, html: icon('plus', 14) + ' Konto hinzufügen…' });
  add.addEventListener('click', async () => {
    const acc = await addAccountFlow();
    if (acc) { await reloadAccounts(); selected = acc.id; render(); }
  });
  const adv = h('details.welcome-adv.acc-adv',
    h('summary', 'Microsoft-365-Anmeldung (App-Registrierung)'),
    builtin ? h('p.note', 'Diese Version enthält bereits eine App-Registrierung. Eigene Werte sind nur für Sonderfälle nötig.') : null,
    demo ? h('p.note', 'Demo-Modus: Die Anmeldung läuft gegen den eingebauten Testserver.') : null,
    field('Anwendungs-ID (Client-ID)', clientId),
    field('Mandant (Tenant)', tenant, '„organizations“, „common“ oder Ihre Mandanten-ID/Domäne'));
  render();
  return { el: h('div.acc-panel', h('div.acc-cols', h('div.acc-side', listEl, add), detailHost), adv) };
}

async function removeAccount(acc) {
  const ok = await confirmDialog(`Konto „${acc.email}“ entfernen? Lokale Daten (Zwischenspeicher, Anmeldung, Signaturen) werden gelöscht. Auf dem Server wird nichts gelöscht.`, { title: 'Konto entfernen', danger: true, ok: 'Entfernen' });
  if (!ok) return false;
  await call('remove_account', { accountId: acc.id });
  state.accounts = state.accounts.filter((a) => a.id !== acc.id);
  hooks.onAccountsChanged();
  return true;
}

// ---------------------------------------------------------------- categories
function palettePicker(value, onPick) {
  const pal = h('div.palette');
  for (const p of PRESETS) {
    const sw = h('div.pcolor' + (p.id === value ? '.on' : ''), { title: p.name, style: { background: p.hex }, dataset: { color: p.id } });
    sw.addEventListener('click', () => {
      pal.querySelectorAll('.pcolor').forEach((x) => x.classList.remove('on'));
      sw.classList.add('on');
      onPick(p.id);
    });
    pal.appendChild(sw);
  }
  return pal;
}

async function categoryEditor({ title, name = '', color = 'preset7', nameEditable = true }) {
  let picked = color;
  const input = h('input.input', { value: name, readOnly: !nameEditable, style: { width: '100%' }, name: 'categoryName' });
  const body = h('div',
    h('label.lbl', 'Name:'), input,
    h('label.lbl', { style: { marginTop: '10px' } }, 'Farbe:'),
    palettePicker(color, (c) => (picked = c)),
    nameEditable ? null : h('div.field-hint', { style: { marginTop: '8px' } }, 'Microsoft 365 erlaubt das Umbenennen von Kategorien nicht.'),
  );
  return modal({
    title, body, width: 300,
    buttons: [{ label: 'OK', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: () => {
      const n = input.value.trim();
      if (!n) { input.classList.add('invalid'); input.focus(); return false; }
      return { name: n, color: picked };
    },
  });
}

export async function openCategoryManager(accountId) {
  accountId = accountId || (state.accounts[0] && state.accounts[0].id);
  if (!accountId) { toast('Kein Konto vorhanden.'); return; }
  let selected = null;
  const listEl = h('div.catlist', { role: 'listbox' });
  const render = () => {
    listEl.innerHTML = '';
    const cats = getCategories(accountId);
    if (!cats.length) listEl.appendChild(h('div.empty', 'Keine Kategorien vorhanden.'));
    for (const c of cats) {
      const row = h('div.catrow' + (selected === c.id ? '.selected' : ''), { role: 'option', dataset: { id: c.id } },
        h('span.cat-swatch', { style: { background: presetHex(c.color), width: '28px', height: '14px' } }), h('span.catname', c.displayName));
      row.addEventListener('click', () => { selected = c.id; render(); });
      row.addEventListener('dblclick', () => editColor(c));
      listEl.appendChild(row);
    }
  };
  const refresh = async () => { await loadCategories(accountId); render(); };
  const editColor = async (c) => {
    const r = await categoryEditor({ title: 'Farbe ändern', name: c.displayName, color: c.color, nameEditable: false });
    if (!r) return;
    await call('update_category', { accountId, id: c.id, color: r.color });
    await refresh();
  };
  const acc = state.accounts.find((a) => a.id === accountId);
  const body = h('div',
    acc && state.accounts.length > 1 ? h('div', { style: { marginBottom: '6px', color: '#555' } }, 'Konto: ' + acc.email) : null,
    h('div', { style: { display: 'flex', gap: '10px' } },
      h('div', { style: { flex: '1' } }, listEl),
      h('div', { style: { display: 'flex', flexDirection: 'column', gap: '6px' } },
        h('button.btn', { type: 'button', dataset: { testid: 'cat-new' }, onclick: async () => {
          try {
            const r = await categoryEditor({ title: 'Neue Kategorie hinzufügen' });
            if (!r) return;
            if (getCategories(accountId).some((c) => c.displayName.toLowerCase() === r.name.toLowerCase())) { toast('Eine Kategorie mit diesem Namen existiert bereits.', 'error'); return; }
            const created = await call('create_category', { accountId, displayName: r.name, color: r.color });
            selected = created && created.id;
            await refresh();
          } catch (e) { reportError(e); }
        } }, 'Neu…'),
        h('button.btn', { type: 'button', onclick: () => { const c = getCategories(accountId).find((x) => x.id === selected); if (c) editColor(c).catch(reportError); } }, 'Farbe…'),
        h('button.btn', { type: 'button', onclick: async () => {
          const c = getCategories(accountId).find((x) => x.id === selected);
          if (!c) return;
          if (!(await confirmDialog(`Möchten Sie die Kategorie „${c.displayName}" wirklich löschen? Elemente behalten den Kategorienamen, er wird jedoch ohne Farbe angezeigt.`, { danger: true, ok: 'Ja', cancel: 'Nein' }))) return;
          try { await call('delete_category', { accountId, id: c.id }); selected = null; await refresh(); } catch (e) { reportError(e); }
        } }, 'Löschen'),
      ),
    ),
  );
  render();
  refresh().catch(reportError);
  await modal({ title: 'Farbkategorien', body, width: 460, buttons: [{ label: 'Schließen', value: true, primary: true }] });
}

// ---------------------------------------------------------------- auto reply
export async function openAutoReply(accountId) {
  accountId = accountId || (state.accounts[0] && state.accounts[0].id);
  if (!accountId) return;
  const ar = await call('get_auto_reply', { accountId });
  const mode = h('div',
    h('label.checkrow', h('input', { type: 'radio', name: 'ar', value: 'disabled', checked: ar.status === 'disabled' }), 'Keine automatischen Antworten senden'),
    h('label.checkrow', h('input', { type: 'radio', name: 'ar', value: 'on', checked: ar.status !== 'disabled' }), 'Automatische Antworten senden'),
  );
  const toLocalInput = (iso) => (iso ? toLocalIso(new Date(iso)).slice(0, 16) : '');
  const sched = h('input', { type: 'checkbox', checked: ar.status === 'scheduled' });
  const start = h('input.input', { type: 'datetime-local', value: toLocalInput(ar.scheduledStart) || toLocalIso(new Date()).slice(0, 16) });
  const end = h('input.input', { type: 'datetime-local', value: toLocalInput(ar.scheduledEnd) || toLocalIso(new Date(Date.now() + 7 * 86400000)).slice(0, 16) });
  const internal = h('textarea.input', { rows: 5, style: { width: '100%' } }); internal.value = stripHtml(ar.internalReplyMessage);
  const external = h('textarea.input', { rows: 5, style: { width: '100%' } }); external.value = stripHtml(ar.externalReplyMessage);
  const audience = select([['none', 'Keine externen Antworten'], ['contactsOnly', 'Nur meine Kontakte'], ['all', 'Alle externen Absender']], ar.externalAudience || 'all');
  const body = h('div', mode,
    h('label.checkrow', sched, 'Nur in diesem Zeitraum senden:'),
    h('div', { style: { display: 'flex', gap: '8px', alignItems: 'center', marginLeft: '22px' } }, 'Beginn', start, 'Ende', end),
    h('fieldset', h('legend', 'Innerhalb meiner Organisation'), internal),
    h('fieldset', h('legend', 'Außerhalb meiner Organisation'), field('Senden an:', audience), external),
  );
  const res = await modal({
    title: 'Automatische Antworten (Abwesenheitsnotiz)', body, width: 580,
    buttons: [{ label: 'OK', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: async () => {
      const on = body.querySelector('input[name=ar]:checked').value === 'on';
      const autoReply = {
        status: !on ? 'disabled' : sched.checked ? 'scheduled' : 'alwaysEnabled',
        internalReplyMessage: textToHtml(internal.value), externalReplyMessage: textToHtml(external.value),
        scheduledStart: sched.checked && start.value ? new Date(start.value).toISOString() : null,
        scheduledEnd: sched.checked && end.value ? new Date(end.value).toISOString() : null,
        externalAudience: audience.value,
      };
      if (autoReply.status === 'scheduled' && autoReply.scheduledEnd <= autoReply.scheduledStart) { toast('Das Ende muss nach dem Beginn liegen.', 'error'); return false; }
      await call('set_auto_reply', { accountId, autoReply });
      return 'ok';
    },
  });
  if (res === 'ok') toast('Automatische Antworten gespeichert.');
}

function stripHtml(s) {
  if (!s) return '';
  if (!/<[a-z]/i.test(s)) return s;
  const d = document.createElement('div');
  d.innerHTML = s.replace(/<br\s*\/?>/gi, '\n').replace(/<\/p>/gi, '\n');
  return d.textContent.trim();
}
function textToHtml(t) { return t ? '<p>' + esc(t).replace(/\n/g, '<br>') + '</p>' : ''; }

// ---------------------------------------------------------------- accounts
export async function removeAccountDialog() {
  if (!state.accounts.length) return;
  const acc = select(state.accounts.map((a) => [a.id, `${a.displayName} <${a.email}>`]), state.accounts[0].id);
  const res = await modal({
    title: 'Konto entfernen', body: h('div', field('Konto:', acc), h('p', { style: { color: '#a33' } }, 'Lokale Daten (Zwischenspeicher, Anmeldung) dieses Kontos werden entfernt. Auf dem Server wird nichts gelöscht.')),
    width: 480, buttons: [{ label: 'Entfernen', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
  });
  if (res !== 'ok') return;
  await call('remove_account', { accountId: acc.value });
  hooks.onAccountsChanged();
}

/** "Konto hinzufügen": choose Microsoft 365 or IMAP. Returns the new Account or null. */
export async function addAccountFlow() {
  const kind = await chooseAccountKind();
  if (kind === 'imap') {
    const acc = await imapWizard();
    if (acc) hooks.onAccountsChanged();
    return acc;
  }
  if (kind !== 'o365') return null;
  const c = state.config || {};
  if (!c.demo && !c.clientId && !c.builtinClientId) return (await welcomeDialog({ o365Only: true })) || null;
  try { return await loginWithWait(); } catch (e) { reportError(e); return null; }
}

async function loginWithWait() {
  const waiting = h('div.confirm-body', h('span', { html: icon('user', 32) }), h('div',
    h('div', { style: { fontWeight: '600', marginBottom: '4px' } }, 'Anmeldung im Browser…'),
    h('div', 'Bitte melden Sie sich im geöffneten Browserfenster bei Microsoft 365 an. Dieses Fenster schließt sich automatisch.')));
  const overlay = h('div.modal-overlay', h('div.modal', { style: { width: '420px' } }, h('div.modal-title', h('span', 'Konto hinzufügen')), h('div.modal-body', waiting)));
  document.body.appendChild(overlay);
  try {
    const acc = await call('add_account', {});
    toast(`Konto ${acc.email} wurde hinzugefügt.`);
    hooks.onAccountsChanged();
    return acc;
  } finally { overlay.remove(); }
}

export async function welcomeDialog({ o365Only = false } = {}) {
  const c = state.config || {};
  const clientId = h('input.input', { value: c.clientId || '', placeholder: 'Anwendungs-ID (GUID)', style: { width: '100%' }, name: 'clientId' });
  const tenant = h('input.input', { value: c.tenant || 'organizations', style: { width: '100%' }, name: 'tenant' });
  const builtin = !!c.builtinClientId;
  const adv = h('details.welcome-adv' + (builtin ? '' : '.open'), { open: !builtin },
    h('summary', builtin ? 'Erweitert: eigene App-Registrierung verwenden' : 'App-Registrierung'),
    builtin ? null : h('ol',
      h('li', 'Entra Admin Center → App-Registrierungen → Neue Registrierung'),
      h('li', { html: 'Plattform <b>Mobile und Desktop</b>, Umleitungs-URI <code>http://localhost</code>' }),
      h('li', { html: 'Delegierte Berechtigungen: <code>Mail.ReadWrite</code>, <code>Mail.Send</code>, <code>Calendars.ReadWrite</code>, <code>MailboxSettings.ReadWrite</code>, <code>People.Read</code>, <code>User.Read</code>, <code>offline_access</code>' }),
      h('li', '„Öffentliche Clientflows zulassen“ aktivieren und die Anwendungs-ID hier eintragen.')),
    field('Anwendungs-ID', clientId),
    field('Mandant', tenant));
  const body = h('div.welcome',
    h('div.welcome-hero', h('span.welcome-ic', { html: icon('mail', 36) }),
      h('div', h('h2', 'Willkommen'), h('p', 'Melden Sie sich mit Ihrem Microsoft 365-Konto an. E-Mails, Kalender, Kontakte und Kategorien werden über Microsoft Graph synchronisiert. Andere Anbieter lassen sich per IMAP/SMTP einbinden.'))),
    adv);
  const res = await modal({
    title: 'Konto einrichten', body, width: 560,
    buttons: [{ label: 'Mit Microsoft 365 anmelden', value: 'ok', primary: true, id: 'welcome-login' },
      ...(o365Only ? [] : [{ label: 'IMAP-Konto…', value: 'imap', id: 'welcome-imap' }]),
      { label: 'Später', value: null, cancel: true }],
    onBeforeClose: async (v) => {
      if (v === 'imap') return 'imap';
      const id = clientId.value.trim();
      const custom = id && id !== (c.clientId || '');
      if (!builtin || custom || adv.open) {
        if (!builtin && !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) { adv.open = true; clientId.classList.add('invalid'); toast('Bitte eine gültige Anwendungs-ID (GUID) eingeben.', 'error'); return false; }
        if (id || tenant.value.trim() !== (c.tenant || 'organizations')) {
          state.config = await call('save_config', { config: { ...state.config, clientId: id, tenant: tenant.value.trim() || 'organizations' } });
        }
      }
      return 'ok';
    },
  });
  if (res === 'imap') {
    const acc = await imapWizard();
    if (acc) hooks.onAccountsChanged();
    return acc || false;
  }
  if (res !== 'ok') return false;
  try { return await loginWithWait(); } catch (e) { reportError(e); return false; }
}

export async function aboutDialog() {
  await modal({
    title: 'Über',
    body: h('div.confirm-body', h('span', { html: icon('mail', 40) }), h('div',
      h('div', { style: { fontWeight: '600', fontSize: '15px' } }, 'Mail, Kalender & Kontakte'),
      h('div', 'Fokussierter E-Mail-Client für Microsoft 365 (Graph API) und IMAP/SMTP.'),
      h('div', { style: { marginTop: '6px', color: '#666' } }, 'Rust · Tauri · Microsoft Graph v1.0 · IMAP/SMTP'),
      h('div', { style: { marginTop: '10px', color: '#666' } }, 'Tastenkürzel: Strg+N Neu · Strg+Umschalt+N Aufgabe · E Archivieren · S Wiedervorlage · 1–9 Kanban-Lane · Strg+Umschalt+K Liste/Kanban · Strg+R Antworten · Strg+Umschalt+R Allen antworten · Strg+F Weiterleiten · Strg+Q gelesen · Strg+U ungelesen · T Kategorien · Entf löschen · Strg+E Suchen · F9 Senden/Empfangen · Strg+1 Mail · Strg+2 Kalender · Strg+3 Kontakte'))),
    width: 480,
  });
}

export { setCategories, textOn };
