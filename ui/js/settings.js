// Dialogs: settings, category manager, auto reply, shared mailbox, welcome / account setup.
import { call } from './api.js';
import { h, esc, toLocalIso } from './util.js';
import { icon } from './icons.js';
import { modal, confirmDialog, reportError, toast, field } from './dialogs.js';
import { PRESETS, presetHex, textOn, getCategories, setCategories } from './categories.js';
import { state, loadCategories } from './state.js';

export const hooks = { onConfigSaved: () => {}, onAccountsChanged: () => {} };

function select(options, value) {
  const s = h('select.input');
  for (const [v, label] of options) s.appendChild(h('option', { value: v, selected: v === value }, label));
  return s;
}

// ---------------------------------------------------------------- settings
export async function openSettings() {
  const c = { ...state.config };
  const clientId = h('input.input', { value: c.clientId || '', placeholder: '00000000-0000-0000-0000-000000000000', name: 'clientId' });
  const tenant = h('input.input', { value: c.tenant || 'organizations', name: 'tenant' });
  const interval = h('input.input', { type: 'number', min: '30', step: '30', value: String(c.syncIntervalSecs ?? 120), name: 'syncIntervalSecs', style: { width: '90px' } });
  const remote = h('input', { type: 'checkbox', checked: !!c.loadRemoteImages, name: 'loadRemoteImages' });
  const pane = select([['right', 'Rechts'], ['bottom', 'Unten'], ['off', 'Aus']], c.readingPane || 'right');
  pane.name = 'readingPane';
  const mrOpts = [['0', 'Sofort beim Anzeigen'], ['2', 'Nach 2 Sekunden'], ['5', 'Nach 5 Sekunden'], ['10', 'Nach 10 Sekunden'], ['30', 'Nach 30 Sekunden'], ['-1', 'Nie automatisch']];
  if (!mrOpts.some(([v]) => v === String(c.markReadDelaySecs ?? 0))) mrOpts.push([String(c.markReadDelaySecs), `Nach ${c.markReadDelaySecs} Sekunden`]);
  const markRead = select(mrOpts, String(c.markReadDelaySecs ?? 0));
  markRead.name = 'markReadDelaySecs';
  const listMode = select([['columns', 'Spaltenansicht (klassisch)'], ['compact', 'Kompakt, zweizeilig (eM Client)']], c.listMode || 'columns');
  listMode.name = 'listMode';
  const sig = h('div.input.selectable', { contentEditable: 'true', style: { height: '110px', overflow: 'auto', fontFamily: 'Calibri, Segoe UI, sans-serif', fontSize: '11pt', padding: '4px 6px' }, dataset: { testid: 'signature' } });
  sig.innerHTML = c.signatureHtml || '';

  const body = h('div.settings',
    h('fieldset', h('legend', 'Microsoft 365 / Azure-App-Registrierung'),
      field('Anwendungs-ID (Client-ID):', clientId),
      field('Mandant (Tenant):', tenant, '„organizations", „common" oder Ihre Mandanten-ID/Domäne'),
      c.demo ? h('div.field-hint', { style: { marginTop: '4px' } }, 'Demo-Modus aktiv – es wird ein eingebauter Testserver verwendet.') : null),
    h('fieldset', h('legend', 'Senden/Empfangen'),
      field('Automatisch alle (Sekunden):', interval)),
    h('fieldset', h('legend', 'Lesen'),
      field('Lesebereich:', pane),
      field('Als gelesen markieren:', markRead),
      field('Nachrichtenliste:', listMode),
      h('label.checkrow', remote, 'Externe Bilder in HTML-Nachrichten automatisch herunterladen')),
    h('fieldset', h('legend', 'Signatur (wird neuen Nachrichten, Antworten und Weiterleitungen hinzugefügt)'), sig),
  );
  const res = await modal({
    title: 'Optionen', body, width: 600,
    buttons: [{ label: 'OK', value: 'ok', primary: true, id: 'settings-ok' }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: async () => {
      const iv = parseInt(interval.value, 10);
      if (!(iv >= 30)) { interval.classList.add('invalid'); toast('Das Synchronisierungsintervall muss mindestens 30 Sekunden betragen.', 'error'); return false; }
      const config = {
        ...state.config,
        clientId: clientId.value.trim(), tenant: tenant.value.trim() || 'organizations', syncIntervalSecs: iv,
        loadRemoteImages: remote.checked, signatureHtml: sig.innerHTML.trim() === '<br>' ? '' : sig.innerHTML,
        readingPane: pane.value, markReadDelaySecs: parseInt(markRead.value, 10), listMode: listMode.value,
      };
      state.config = await call('save_config', { config });
      return 'ok';
    },
  });
  if (res === 'ok') { hooks.onConfigSaved(); toast('Einstellungen gespeichert.'); }
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
export async function addSharedMailbox() {
  if (!state.accounts.length) { toast('Bitte fügen Sie zuerst ein Konto hinzu.'); return; }
  const acc = select(state.accounts.map((a) => [a.id, a.email]), state.accounts[0].id);
  const addr = h('input.input', { type: 'email', placeholder: 'postfach@firma.de', style: { width: '100%' }, name: 'sharedAddress' });
  const res = await modal({
    title: 'Freigegebenes Postfach hinzufügen',
    body: h('div',
      h('p', { style: { margin: '0 0 8px', lineHeight: '1.45' } }, 'Geben Sie die Adresse eines Postfachs ein, für das Sie Vollzugriff besitzen (z. B. ein Team- oder Funktionspostfach).'),
      state.accounts.length > 1 ? field('Über Konto:', acc) : null,
      field('E-Mail-Adresse:', addr)),
    width: 460,
    buttons: [{ label: 'Hinzufügen', value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: async () => {
      const a = addr.value.trim();
      if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(a)) { addr.classList.add('invalid'); addr.focus(); return false; }
      await call('add_shared_mailbox', { accountId: acc.value, address: a });
      return 'ok';
    },
  });
  if (res === 'ok') { toast('Postfach hinzugefügt.'); hooks.onAccountsChanged(); }
}

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

export async function addAccountFlow() {
  const c = state.config || {};
  if (!c.demo && !c.clientId) {
    const ok = await welcomeDialog();
    if (!ok) return;
  } else {
    await loginWithWait();
  }
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

export async function welcomeDialog() {
  const c = state.config || {};
  const clientId = h('input.input', { value: c.clientId || '', placeholder: 'Anwendungs-ID (GUID)', style: { width: '100%' }, name: 'clientId' });
  const tenant = h('input.input', { value: c.tenant || 'organizations', style: { width: '100%' }, name: 'tenant' });
  const body = h('div.welcome',
    h('p', h('b', 'Willkommen bei Outlook Classic.'), ' Diese Anwendung greift über die Microsoft Graph-API auf Ihr Microsoft 365-Postfach und Ihren Kalender zu.'),
    h('p', 'Einmalig wird eine App-Registrierung in Microsoft Entra ID benötigt:'),
    h('ol',
      h('li', 'Entra Admin Center → App-Registrierungen → Neue Registrierung'),
      h('li', { html: 'Plattform <b>Mobile und Desktop</b>, Umleitungs-URI <code>http://localhost</code>' }),
      h('li', { html: 'Delegierte Berechtigungen: <code>Mail.ReadWrite</code>, <code>Mail.Send</code>, <code>Calendars.ReadWrite</code>, <code>MailboxSettings.ReadWrite</code>, <code>People.Read</code>, <code>User.Read</code>, <code>offline_access</code> (für freigegebene Postfächer zusätzlich <code>*.Shared</code>)' }),
      h('li', '„Öffentliche Clientflows zulassen" aktivieren und die Anwendungs-ID hier eintragen.')),
    field('Anwendungs-ID:', clientId),
    field('Mandant:', tenant),
  );
  const res = await modal({
    title: 'Konto einrichten', body, width: 600,
    buttons: [{ label: 'Anmelden…', value: 'ok', primary: true }, { label: 'Später', value: null, cancel: true }],
    onBeforeClose: async () => {
      const id = clientId.value.trim();
      if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) { clientId.classList.add('invalid'); toast('Bitte eine gültige Anwendungs-ID (GUID) eingeben.', 'error'); return false; }
      state.config = await call('save_config', { config: { ...state.config, clientId: id, tenant: tenant.value.trim() || 'organizations' } });
      return 'ok';
    },
  });
  if (res !== 'ok') return false;
  try { await loginWithWait(); return true; } catch (e) { reportError(e); return false; }
}

export async function aboutDialog() {
  await modal({
    title: 'Über Outlook Classic',
    body: h('div.confirm-body', h('span', { html: icon('mail', 40) }), h('div',
      h('div', { style: { fontWeight: '600', fontSize: '14px' } }, 'Outlook Classic'),
      h('div', 'E-Mail- und Kalender-Client für Microsoft 365 (Graph API).'),
      h('div', { style: { marginTop: '6px', color: '#666' } }, 'Rust · Tauri · Microsoft Graph v1.0'),
      h('div', { style: { marginTop: '10px', color: '#666' } }, 'Tastenkürzel: Strg+N Neu · Strg+R Antworten · Strg+Umschalt+R Allen antworten · Strg+F Weiterleiten · Strg+Q gelesen · Strg+U ungelesen · Entf löschen · F9 Senden/Empfangen · Strg+1 E-Mail · Strg+2 Kalender'))),
    width: 480,
  });
}

export { setCategories, textOn };
