// Account setup: chooser (Microsoft 365 / Google / IMAP), IMAP wizard, server settings editor.
import { call } from './api.js';
import { h, esc, EMAIL_RE, initials, avatarColor } from './util.js';
import { icon } from './icons.js';
import { modal, toast, reportError, busyClick, confirmDialog } from './dialogs.js';

const SECURITY = [['ssl', 'SSL/TLS'], ['starttls', 'STARTTLS'], ['none', 'Keine (unverschlüsselt)']];
const DEFAULT_PORT = {
  imap: { ssl: 993, starttls: 143, none: 143 },
  smtp: { ssl: 465, starttls: 587, none: 25 },
};
const SOURCE_HINT = {
  ispdb: ['ok', 'Einstellungen aus der Thunderbird-Datenbank'],
  autoconfig: ['ok', 'Automatisch beim Anbieter erkannt'],
  guess: ['warn', 'Geschätzt – bitte prüfen'],
};

export function accountKind(acc) { return (acc && acc.kind) || 'o365'; }
export function isImap(acc) { return accountKind(acc) === 'imap'; }
/** IMAP account signed in with Google (Gmail / Google Workspace). */
export function isGoogle(acc) { return isImap(acc) && acc.auth === 'google'; }
export function kindLabel(acc) { return isGoogle(acc) ? 'Google' : isImap(acc) ? 'IMAP' : 'M365'; }
/** CSS class of the account badge. */
export function kindClass(acc) { return isGoogle(acc) ? 'google' : isImap(acc) ? 'imap' : 'o365'; }

const GOOGLE_HOSTS = ['imap.gmail.com', 'smtp.gmail.com', 'imap.googlemail.com', 'smtp.googlemail.com'];
/** Mailboxes at Google: gmail.com addresses, or servers discovered at Google. */
function looksLikeGoogle(email, found) {
  const domain = (email.split('@')[1] || '').toLowerCase();
  return domain === 'gmail.com' || domain === 'googlemail.com'
    || !!(found && found.imap && GOOGLE_HOSTS.includes(String(found.imap.host || '').toLowerCase()));
}

/** Editor for one server (IMAP or SMTP). Returns { el, get(), set(s) }. */
export function serverEditor(kind, initial = {}) {
  const s = { host: '', port: DEFAULT_PORT[kind].ssl, security: 'ssl', username: '', ...initial };
  let portTouched = false;
  const host = h('input.input', { value: s.host, placeholder: kind === 'imap' ? 'imap.example.com' : 'smtp.example.com', name: kind + 'Host', 'aria-label': 'Server', spellcheck: false });
  const port = h('input.input.srv-port', { type: 'number', min: '1', max: '65535', value: String(s.port), name: kind + 'Port', 'aria-label': 'Port' });
  const sec = h('select.input', { name: kind + 'Security', 'aria-label': 'Sicherheit' },
    ...SECURITY.map(([v, l]) => h('option', { value: v, selected: v === s.security }, l)));
  const user = h('input.input', { value: s.username, name: kind + 'User', 'aria-label': 'Benutzername', spellcheck: false, autocomplete: 'off' });
  port.addEventListener('input', () => { portTouched = true; });
  sec.addEventListener('change', () => {
    const was = Object.values(DEFAULT_PORT[kind]).includes(parseInt(port.value, 10));
    if (!portTouched || was) port.value = String(DEFAULT_PORT[kind][sec.value]);
  });
  const warn = h('div.srv-warn.hidden', { html: icon('warning', 14) + '<span>Ohne Verschlüsselung werden Passwort und Nachrichten im Klartext übertragen.</span>' });
  const paintWarn = () => warn.classList.toggle('hidden', sec.value !== 'none');
  sec.addEventListener('change', paintWarn);
  paintWarn();
  const el = h('div.srv-box', { dataset: { testid: 'srv-' + kind } },
    h('div.srv-head', { html: icon(kind === 'imap' ? 'inbox' : 'send', 16) + `<span>${kind === 'imap' ? 'Posteingang (IMAP)' : 'Postausgang (SMTP)'}</span>` }),
    h('div.srv-grid',
      h('label.srv-l', 'Server'), host,
      h('label.srv-l', 'Sicherheit'), h('div.srv-hostrow', sec, h('span.dim', 'Port'), port),
      h('label.srv-l', 'Benutzername'), user),
    warn);
  return {
    el,
    get: () => ({ host: host.value.trim(), port: parseInt(port.value, 10) || DEFAULT_PORT[kind][sec.value], security: sec.value, username: user.value.trim() }),
    set: (v) => { host.value = v.host || ''; port.value = String(v.port || DEFAULT_PORT[kind][v.security || 'ssl']); sec.value = v.security || 'ssl'; user.value = v.username || ''; paintWarn(); portTouched = false; },
    validate: () => {
      let ok = true;
      for (const [inp, cond] of [[host, !!host.value.trim()], [port, parseInt(port.value, 10) > 0 && parseInt(port.value, 10) < 65536], [user, !!user.value.trim()]]) {
        inp.classList.toggle('invalid', !cond);
        ok = ok && cond;
      }
      return ok;
    },
  };
}

function guessSettings(email) {
  const [local, domain = ''] = email.split('@');
  return {
    imap: { host: 'imap.' + domain, port: 993, security: 'ssl', username: email || local },
    smtp: { host: 'smtp.' + domain, port: 587, security: 'starttls', username: email || local },
    source: 'guess',
  };
}

function resultLine(label, ok, err) {
  return h('div.test-line.' + (ok ? 'ok' : 'fail'), { dataset: { testid: 'test-' + label.toLowerCase() } },
    h('span', { html: icon(ok ? 'check' : 'close', 14) }), h('b', label), h('span.test-msg', ok ? 'Verbindung erfolgreich' : (err || 'Fehlgeschlagen')));
}

/** Connection test UI (button + result area) for a given input getter. */
function testPanel(getInput) {
  const out = h('div.test-out', { dataset: { testid: 'test-out' } });
  const btn = h('button.btn', { type: 'button', dataset: { testid: 'imap-test' }, html: icon('sync', 15) + ' Verbindung testen' });
  btn.addEventListener('click', async () => {
    const input = getInput();
    if (!input) return;
    btn.disabled = true;
    out.innerHTML = '';
    out.appendChild(h('div.test-line', h('span.busy.on.inline-busy'), 'Verbindung wird geprüft…'));
    try {
      const r = await call('test_imap_account', { input }, { silent: true });
      out.innerHTML = '';
      out.append(resultLine('IMAP', r.imapOk, r.imapError), resultLine('SMTP', r.smtpOk, r.smtpError));
    } catch (e) {
      out.innerHTML = '';
      out.appendChild(h('div.test-line.fail', h('span', { html: icon('close', 14) }), h('span.test-msg', (e && e.message) || String(e))));
    } finally { btn.disabled = false; }
  });
  return h('div.test-panel', btn, out);
}

/** "Konto hinzufügen": choose the kind of account. Returns 'o365' | 'google' | 'imap' | null. */
export async function chooseAccountKind() {
  let picked = null;
  const card = (kind, ic, title, text) => {
    const c = h('button.kind-card', { type: 'button', dataset: { testid: 'kind-' + kind } },
      h('span.kind-ic', { html: icon(ic, 26) }),
      h('span.kind-text', h('span.kind-title', title), h('span.kind-desc', text)),
      h('span.kind-go', { html: icon('chevronRight', 16) }));
    return c;
  };
  const o365 = card('o365', 'mail', 'Microsoft 365 / Exchange Online', 'E-Mail, Kalender, Kontakte und Kategorien über Microsoft Graph. Anmeldung im Browser.');
  const google = card('google', 'google', 'Google / Gmail', 'Gmail und Google Workspace. Anmeldung im Browser bei Google, E-Mail über IMAP/SMTP.');
  const imap = card('imap', 'server', 'IMAP / SMTP', 'Andere Anbieter (z. B. GMX, web.de, IONOS, eigener Server). Nur E-Mail, Kategorien lokal.');
  const body = h('div.kind-choice', o365, google, imap);
  return new Promise((resolve) => {
    const p = modal({ title: 'Konto hinzufügen', body, width: 520, buttons: [{ label: 'Abbrechen', value: null, cancel: true }] });
    for (const [el, kind] of [[o365, 'o365'], [google, 'google'], [imap, 'imap']]) {
      el.addEventListener('click', () => {
        picked = kind;
        const x = el.closest('.modal').querySelector('.modal-footer .btn');
        x.click();
      });
    }
    p.then(() => resolve(picked));
  });
}

/**
 * IMAP/SMTP wizard. Returns the created Account, null, or { google: true }
 * when the user switches to "Mit Google anmelden" for a Gmail address.
 */
export async function imapWizard() {
  const name = h('input.input', { name: 'imapName', placeholder: 'Max Mustermann', autocomplete: 'name' });
  const email = h('input.input', { type: 'email', name: 'imapEmail', placeholder: 'name@beispiel.de', autocomplete: 'email', spellcheck: false });
  const pw = h('input.input', { type: 'password', name: 'imapPassword', autocomplete: 'current-password' });
  let found = null;

  // step 1
  for (;;) {
    const step1 = h('div.wizard',
      h('div.wiz-steps', h('span.on', '1 Konto'), h('span', '2 Server')),
      h('p.note', 'Geben Sie Ihre E-Mail-Adresse und Ihr Passwort ein. Die Servereinstellungen werden anschließend automatisch gesucht.'),
      h('div.wiz-grid',
        h('label.srv-l', 'Ihr Name'), name,
        h('label.srv-l', 'E-Mail-Adresse'), email,
        h('label.srv-l', 'Passwort'), pw),
      h('p.field-hint.wiz-hint', 'Das Passwort wird im Schlüsselbund des Betriebssystems gespeichert. Für Gmail, iCloud & Co. ist meist ein App-Passwort nötig.'));
    const r1 = await modal({
      title: 'IMAP-Konto hinzufügen', body: step1, width: 560,
      buttons: [{ label: 'Weiter', value: 'next', primary: true, id: 'imap-next' }, { label: 'Abbrechen', value: null, cancel: true }],
      onBeforeClose: async () => {
        const e = email.value.trim();
        let ok = true;
        if (!EMAIL_RE.test(e)) { email.classList.add('invalid'); ok = false; } else email.classList.remove('invalid');
        if (!pw.value) { pw.classList.add('invalid'); ok = false; } else pw.classList.remove('invalid');
        if (!ok) { toast('Bitte E-Mail-Adresse und Passwort angeben.', 'error'); return false; }
        try {
          found = await call('discover_imap', { email: e }, { silent: true });
          if (!found || !found.imap) found = guessSettings(e);
        } catch (err) {
          console.warn(err);
          found = guessSettings(e);
        }
        return 'next';
      },
    });
    if (r1 !== 'next') return null;
    if (looksLikeGoogle(email.value.trim(), found)) {
      const useGoogle = await confirmDialog('Dieses Postfach liegt bei Google. Empfohlen ist die Anmeldung mit Google – dann ist kein App-Passwort nötig.', { title: 'Gmail-Konto', ok: 'Mit Google anmelden', cancel: 'Mit Passwort fortfahren' });
      if (useGoogle) return { google: true };
    }

    // step 2
    const imapEd = serverEditor('imap', found.imap);
    const smtpEd = serverEditor('smtp', found.smtp);
    const ownSmtpPw = h('input', { type: 'checkbox', name: 'ownSmtpPassword' });
    const smtpPw = h('input.input.hidden', { type: 'password', name: 'smtpPassword', placeholder: 'SMTP-Passwort', autocomplete: 'off' });
    ownSmtpPw.addEventListener('change', () => smtpPw.classList.toggle('hidden', !ownSmtpPw.checked));
    const [kindCls, hintText] = SOURCE_HINT[found.source] || SOURCE_HINT.guess;
    const buildInput = (validate = true) => {
      if (validate) {
        const a = imapEd.validate(), b = smtpEd.validate();
        if (!a || !b) { toast('Bitte alle Servereinstellungen ausfüllen.', 'error'); return null; }
      }
      return {
        email: email.value.trim(), displayName: name.value.trim() || email.value.trim(),
        imap: imapEd.get(), smtp: smtpEd.get(), password: pw.value,
        smtpPassword: ownSmtpPw.checked && smtpPw.value ? smtpPw.value : null,
      };
    };
    const step2 = h('div.wizard',
      h('div.wiz-steps', h('span.done', '1 Konto'), h('span.on', '2 Server')),
      h('div.src-hint.' + kindCls, { dataset: { testid: 'discover-source' }, html: icon(kindCls === 'ok' ? 'check' : 'info', 15) + `<span>${esc(hintText)}</span>` }),
      h('div.srv-pair', imapEd.el, smtpEd.el),
      h('label.checkrow', ownSmtpPw, 'Für SMTP ein anderes Passwort verwenden'), smtpPw,
      testPanel(() => buildInput()));
    const r2 = await modal({
      title: 'IMAP-Konto hinzufügen', body: step2, width: 780,
      buttons: [{ label: 'Konto hinzufügen', value: 'add', primary: true, id: 'imap-add' }, { label: 'Zurück', value: 'back' }, { label: 'Abbrechen', value: null, cancel: true }],
      onBeforeClose: async (v) => {
        if (v !== 'add') return v;
        const input = buildInput();
        if (!input) return false;
        const btn = document.getElementById('imap-add');
        if (btn) { btn.disabled = true; btn.textContent = 'Wird geprüft…'; }
        try {
          const acc = await call('add_imap_account', { input }, { silent: true });
          return { account: acc };
        } catch (e) {
          if (btn) { btn.disabled = false; btn.textContent = 'Konto hinzufügen'; }
          throw e;
        }
      },
    });
    if (r2 === 'back') continue;
    if (r2 && r2.account) { toast(`Konto ${r2.account.email} wurde hinzugefügt.`); return r2.account; }
    return null;
  }
}

/** Detail panel of an account signed in with Google: name, sign in again, remove. */
function googleDetail(acc, box, remove, onSaved) {
  const nameIn = h('input.input', { name: 'accDisplayName', value: acc.displayName || '' });
  const save = h('button.btn.primary', { type: 'button', dataset: { testid: 'acc-save' } }, 'Kontoeinstellungen speichern');
  save.addEventListener('click', busyClick(async () => {
    try {
      const inp = await call('get_imap_account', { accountId: acc.id }, { silent: true });
      const updated = await call('update_imap_account', { accountId: acc.id, input: { ...inp, displayName: nameIn.value.trim() || acc.email } });
      toast('Kontoeinstellungen gespeichert.');
      onSaved && onSaved(updated);
    } catch (e) { reportError(e); }
  }));
  const relogin = h('button.btn', { type: 'button', dataset: { testid: 'acc-google-relogin' }, html: icon('google', 15) + ' Erneut mit Google anmelden' });
  relogin.addEventListener('click', busyClick(async () => {
    const updated = await googleLogin(acc.email);
    if (updated) onSaved && onSaved(updated);
  }));
  box.append(
    h('p.note', 'Google-Konto (Gmail / Google Workspace). Die Anmeldung läuft über Google, E-Mails über IMAP/SMTP – ein Passwort wird nicht gespeichert. Kategorien und Wiedervorlage werden als IMAP-Schlagwörter auf dem Server abgelegt.'),
    h('div.wiz-grid', h('label.srv-l', 'Anzeigename'), nameIn),
    h('div.acc-actions', save, relogin, h('span.spacer'), remove));
  return box;
}

/**
 * Sign-in with Google in the browser (new account, or renewal when
 * `loginHint` names an existing one). Returns the Account or null.
 */
export async function googleLogin(loginHint = null) {
  const waiting = h('div.confirm-body', h('span', { html: icon('google', 32) }), h('div',
    h('div', { style: { fontWeight: '600', marginBottom: '4px' } }, 'Anmeldung im Browser…'),
    h('div', 'Bitte melden Sie sich im geöffneten Browserfenster bei Google an und erlauben Sie den Zugriff auf Gmail. Dieses Fenster schließt sich automatisch.')));
  const overlay = h('div.modal-overlay', h('div.modal', { style: { width: '420px' } }, h('div.modal-title', h('span', 'Mit Google anmelden')), h('div.modal-body', waiting)));
  document.body.appendChild(overlay);
  try {
    const acc = await call('add_google_account', { loginHint }, { silent: true });
    toast(loginHint ? `Konto ${acc.email} ist wieder angemeldet.` : `Konto ${acc.email} wurde hinzugefügt.`);
    return acc;
  } catch (e) { reportError(e); return null; } finally { overlay.remove(); }
}

/**
 * Round account picture, or the initials when none is set. `size`: avatar size
 * class (xs, sm, lg, xl). With `mb`, a shared mailbox keeps its own initials.
 */
export function accountAvatar(acc, size = 'xs', mb = null) {
  const name = mb ? mb.displayName : acc && acc.displayName;
  const addr = mb ? mb.address : acc && acc.email;
  if (acc && acc.avatar && (!mb || mb.kind === 'primary')) {
    return h('span.avatar.img.' + size, { dataset: { testid: 'acc-avatar-img' } }, h('img', { src: acc.avatar, alt: '', draggable: 'false' }));
  }
  return h('span.avatar.' + size, { style: { background: avatarColor(addr) } }, initials(name, addr));
}

const AVATAR_PX = 128;

/** Reads an image file and returns a square, center-cropped 128 px data URL. */
export function avatarFromFile(file) {
  return new Promise((resolve, reject) => {
    if (!/^image\/(png|jpeg|webp)$/.test(file.type)) { reject(new Error('Bitte ein PNG- oder JPG-Bild wählen.')); return; }
    // data: URL – the window's CSP allows data: images, not blob:.
    const reader = new FileReader();
    const img = new Image();
    img.onload = () => {
      const side = Math.min(img.naturalWidth, img.naturalHeight);
      if (!side) { reject(new Error('Das Bild konnte nicht gelesen werden.')); return; }
      const c = document.createElement('canvas');
      c.width = c.height = AVATAR_PX;
      const ctx = c.getContext('2d');
      ctx.imageSmoothingQuality = 'high';
      ctx.drawImage(img, (img.naturalWidth - side) / 2, (img.naturalHeight - side) / 2, side, side, 0, 0, AVATAR_PX, AVATAR_PX);
      // PNG keeps transparency; photos are much smaller as JPEG.
      resolve(file.type === 'image/png' ? c.toDataURL('image/png') : c.toDataURL('image/jpeg', 0.9));
    };
    img.onerror = () => reject(new Error('Das Bild konnte nicht gelesen werden.'));
    reader.onload = () => { img.src = String(reader.result); };
    reader.onerror = () => reject(new Error('Das Bild konnte nicht gelesen werden.'));
    reader.readAsDataURL(file);
  });
}

/** Picture of the account: choose (PNG/JPG, shown round) or remove. */
function avatarEditor(acc, onChanged) {
  const file = h('input', { type: 'file', accept: 'image/png,image/jpeg,image/webp', style: { display: 'none' }, dataset: { testid: 'acc-avatar-file' } });
  const pick = h('button.btn.small', { type: 'button', dataset: { testid: 'acc-avatar-pick' } }, acc.avatar ? 'Bild ändern…' : 'Bild wählen…');
  const remove = acc.avatar ? h('button.btn.small', { type: 'button', dataset: { testid: 'acc-avatar-remove' } }, 'Bild entfernen') : null;
  const set = async (avatar) => {
    await call('set_account_avatar', { accountId: acc.id, avatar });
    acc.avatar = avatar;
    onChanged();
  };
  pick.addEventListener('click', () => file.click());
  file.addEventListener('change', async () => {
    const f = file.files[0];
    file.value = '';
    if (!f) return;
    try { await set(await avatarFromFile(f)); } catch (e) { reportError(e); }
  });
  if (remove) remove.addEventListener('click', busyClick(() => set(null)));
  return h('div.acc-avatar-edit', file, pick, remove);
}

/** Account detail panel for the settings dialog ("Konten" tab). */
export function accountDetail(acc, { onRemoved, onSaved, onAvatar }) {
  const box = h('div.acc-detail', { dataset: { testid: 'acc-detail' } });
  const header = h('div.acc-head',
    accountAvatar(acc, 'xl'),
    h('div.acc-head-text',
      h('div', h('span.kind-badge.' + kindClass(acc), kindLabel(acc))),
      h('div.acc-name', acc.displayName || acc.email), h('div.dim', acc.email),
      avatarEditor(acc, () => onAvatar && onAvatar(acc))));
  box.appendChild(header);
  const remove = h('button.btn.danger', { type: 'button', dataset: { testid: 'acc-remove' }, html: icon('signOut', 15) + ' Konto entfernen' });
  remove.addEventListener('click', () => onRemoved(acc));
  if (isGoogle(acc)) return googleDetail(acc, box, remove, onSaved);
  if (!isImap(acc)) {
    box.append(
      h('p.note', 'Microsoft-365-Konto. Anmeldung, E-Mail, Kalender, Kontakte und Kategorien laufen über Microsoft Graph. Der Anzeigename wird von Microsoft 365 übernommen.'),
      h('div.acc-actions', remove));
    return box;
  }
  const nameIn = h('input.input', { name: 'accDisplayName', value: acc.displayName || '' });
  const pw = h('input.input', { type: 'password', name: 'accPassword', placeholder: 'unverändert lassen', autocomplete: 'off' });
  const smtpPw = h('input.input', { type: 'password', name: 'accSmtpPassword', placeholder: 'wie IMAP / unverändert', autocomplete: 'off' });
  const host = h('div.acc-servers', h('div.dim', 'Einstellungen werden geladen…'));
  let imapEd, smtpEd;
  const save = h('button.btn.primary', { type: 'button', dataset: { testid: 'acc-save' }, disabled: true }, 'Kontoeinstellungen speichern');
  const getInput = (validate = true) => {
    if (!imapEd) return null;
    if (validate && (!imapEd.validate() || !smtpEd.validate())) { toast('Bitte alle Servereinstellungen ausfüllen.', 'error'); return null; }
    return { email: acc.email, displayName: nameIn.value.trim() || acc.email, imap: imapEd.get(), smtp: smtpEd.get(), password: pw.value, smtpPassword: smtpPw.value || null };
  };
  call('get_imap_account', { accountId: acc.id }, { silent: true }).then((inp) => {
    imapEd = serverEditor('imap', inp.imap);
    smtpEd = serverEditor('smtp', inp.smtp);
    host.innerHTML = '';
    host.append(h('div.srv-pair', imapEd.el, smtpEd.el));
    save.disabled = false;
  }).catch((e) => { host.innerHTML = ''; host.appendChild(h('div.dim', 'Einstellungen konnten nicht geladen werden: ' + ((e && e.message) || e))); });
  save.addEventListener('click', async () => {
    const input = getInput();
    if (!input) return;
    save.disabled = true;
    try {
      const updated = await call('update_imap_account', { accountId: acc.id, input });
      pw.value = ''; smtpPw.value = '';
      toast('Kontoeinstellungen gespeichert.');
      onSaved && onSaved(updated);
    } catch (e) { reportError(e); } finally { save.disabled = false; }
  });
  box.append(
    h('div.wiz-grid',
      h('label.srv-l', 'Anzeigename'), nameIn,
      h('label.srv-l', 'Passwort'), pw,
      h('label.srv-l', 'SMTP-Passwort'), smtpPw),
    host,
    testPanel(() => getInput()),
    h('div.acc-actions', save, h('span.spacer'), remove));
  return box;
}
