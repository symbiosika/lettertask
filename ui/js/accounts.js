// Account setup: chooser (Microsoft 365 / IMAP), IMAP wizard, server settings editor.
import { call } from './api.js';
import { h, esc, EMAIL_RE, initials, avatarColor, AVATAR_COLORS } from './util.js';
import { icon } from './icons.js';
import { modal, toast, reportError, busyClick } from './dialogs.js';

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
export function kindLabel(acc) { return isImap(acc) ? 'IMAP' : 'M365'; }

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
      h('label.srv-l', 'Sicherheit'), sec,
      h('label.srv-l', 'Port'), port,
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

/** "Konto hinzufügen": choose the kind of account. Returns 'o365' | 'imap' | null. */
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
  const imap = card('imap', 'server', 'IMAP / SMTP', 'Andere Anbieter (z. B. GMX, web.de, IONOS, eigener Server). Nur E-Mail, Kategorien lokal.');
  const body = h('div.kind-choice', o365, imap);
  return new Promise((resolve) => {
    const p = modal({ title: 'Konto hinzufügen', body, width: 520, buttons: [{ label: 'Abbrechen', value: null, cancel: true }] });
    for (const [el, kind] of [[o365, 'o365'], [imap, 'imap']]) {
      el.addEventListener('click', () => {
        picked = kind;
        const x = el.closest('.modal').querySelector('.modal-footer .btn');
        x.click();
      });
    }
    p.then(() => resolve(picked));
  });
}

/** IMAP/SMTP wizard. Returns the created Account or null. */
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

/** Name of the account in the views: the own name, else the address. */
export function accountLabel(acc) {
  return (acc && (acc.label || acc.email)) || '';
}

/**
 * Round account picture, or the badge (own text/colour) or initials when none
 * is set. `size`: avatar size class (xs, sm, lg, xl). With `mb`, a shared
 * mailbox keeps its own initials.
 */
export function accountAvatar(acc, size = 'xs', mb = null) {
  const name = mb ? mb.displayName : acc && acc.displayName;
  const addr = mb ? mb.address : acc && acc.email;
  const own = acc && (!mb || mb.kind === 'primary');
  if (own && acc.avatar) {
    return h('span.avatar.img.' + size, { dataset: { testid: 'acc-avatar-img' } }, h('img', { src: acc.avatar, alt: '', draggable: 'false' }));
  }
  const badge = (own && acc.badge) || {};
  const background = badge.color || avatarColor(addr);
  if (badge.text) {
    return h('span.avatar.txt.' + size, { style: { background }, dataset: { testid: 'acc-avatar-text' }, title: badgeLines(badge.text).join(' '), html: badgeSvg(badge.text) });
  }
  return h('span.avatar.' + size, { style: { background } }, initials(name, addr));
}

export const BADGE_LINES = 2;
export const BADGE_LINE_MAX = 8;

/** Badge text → at most 2 trimmed, non-empty lines of at most 8 characters. */
export function badgeLines(text) {
  return String(text || '').split('\n').map((l) => l.trim()).filter(Boolean)
    .slice(0, BADGE_LINES).map((l) => [...l].slice(0, BADGE_LINE_MAX).join(''));
}

const MEASURE_PX = 40;
let measureCtx = null;
function textWidth(s) {
  try {
    if (!measureCtx) {
      measureCtx = document.createElement('canvas').getContext('2d');
      measureCtx.font = `600 ${MEASURE_PX}px ${getComputedStyle(document.body).fontFamily}`;
    }
    return measureCtx.measureText(s).width;
  } catch {
    return [...s].length * MEASURE_PX * 0.62;
  }
}

/**
 * Badge text as SVG in a 100×100 box: the font size is chosen so the widest
 * line fits into the circle, and the SVG scales with every avatar size.
 */
export function badgeSvg(text) {
  const lines = badgeLines(text);
  const two = lines.length > 1;
  const maxW = two ? 66 : 76;
  const maxH = two ? 34 : 44;
  const widest = Math.max(1, ...lines.map(textWidth));
  const fs = Math.min(maxH, (MEASURE_PX * maxW) / widest);
  const lh = fs * 1.05;
  const y0 = 50 - (lh * (lines.length - 1)) / 2;
  return `<svg viewBox="0 0 100 100" aria-hidden="true">${lines.map((l, i) =>
    `<text x="50" y="${(y0 + i * lh).toFixed(1)}" font-size="${fs.toFixed(1)}" text-anchor="middle" dominant-baseline="central">${esc(l)}</text>`).join('')}</svg>`;
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

/**
 * Look of the account in the views: own name instead of the address, colour
 * and text of the round badge (used when there is no picture). Saved right
 * away; `onPreview` gets each change while typing, `onChanged` after saving.
 */
function lookEditor(acc, { onPreview, onChanged }) {
  const badge = acc.badge || {};
  let color = badge.color || null;
  const label = h('input.input', { name: 'accLabel', value: acc.label || '', placeholder: acc.email, maxLength: 60, dataset: { testid: 'acc-label' } });
  const text = h('textarea.input.acc-badge-text', { name: 'accBadgeText', rows: 2, placeholder: initials(acc.displayName, acc.email), dataset: { testid: 'acc-badge-text' } });
  text.value = badge.text || '';
  const current = () => ({ label: label.value.trim() || null, badge: { color, text: badgeLines(text.value).join('\n') || null } });
  const swatches = h('div.acc-swatches', { role: 'radiogroup', 'aria-label': 'Farbe' });
  const picker = h('input.acc-color', { type: 'color', title: 'Eigene Farbe', 'aria-label': 'Eigene Farbe', dataset: { testid: 'acc-color' } });
  const renderSwatches = () => {
    swatches.innerHTML = '';
    const sw = (c, title) => h('button.acc-swatch' + (c === color ? '.selected' : '') + (c ? '' : '.auto'), {
      type: 'button', role: 'radio', 'aria-checked': c === color ? 'true' : 'false', title,
      style: { background: c || avatarColor(acc.email) }, dataset: { testid: 'acc-swatch', color: c || '' },
      onclick: () => { color = c; renderSwatches(); save(); },
    }, c ? null : 'A');
    swatches.appendChild(sw(null, 'Automatisch'));
    for (const c of AVATAR_COLORS) swatches.appendChild(sw(c, c));
    picker.value = color || avatarColor(acc.email);
    swatches.appendChild(picker);
  };
  let saved = JSON.stringify(current());
  const save = async () => {
    const look = current();
    onPreview(look);
    const key = JSON.stringify(look);
    if (key === saved) return;
    try {
      await call('set_account_look', { accountId: acc.id, ...look });
      saved = key;
      acc.label = look.label;
      acc.badge = look.badge.color || look.badge.text ? look.badge : null;
      onChanged();
    } catch (e) { reportError(e); }
  };
  picker.addEventListener('input', () => { color = picker.value.toLowerCase(); onPreview(current()); });
  picker.addEventListener('change', () => { renderSwatches(); save(); });
  // At most 2 lines of 8 characters, already while typing.
  text.addEventListener('input', () => {
    const lines = text.value.split('\n').slice(0, BADGE_LINES).map((l) => [...l].slice(0, BADGE_LINE_MAX).join(''));
    const v = lines.join('\n');
    if (v !== text.value) text.value = v;
    onPreview(current());
  });
  text.addEventListener('change', save);
  label.addEventListener('input', () => onPreview(current()));
  label.addEventListener('change', save);
  renderSwatches();
  return h('div.acc-look', { dataset: { testid: 'acc-look' } },
    h('div.wiz-grid',
      h('label.srv-l', 'Name in der Ansicht'), label,
      h('label.srv-l', 'Farbe'), swatches,
      h('label.srv-l', 'Inhalt im Kreis'), h('div', text,
        h('div.note', `Ohne Bild zeigt der Kreis diesen Text – bis zu ${BADGE_LINES} Zeilen mit je ${BADGE_LINE_MAX} Zeichen, die Schrift passt sich an. Leer = Initialen.`))));
}

/** Account detail panel for the settings dialog ("Konten" tab). */
export function accountDetail(acc, { onRemoved, onSaved, onAvatar, onLook }) {
  const box = h('div.acc-detail', { dataset: { testid: 'acc-detail' } });
  let avatar = accountAvatar(acc, 'xl');
  const header = h('div.acc-head',
    avatar,
    h('div.acc-head-text',
      h('div', h('span.kind-badge.' + (isImap(acc) ? 'imap' : 'o365'), kindLabel(acc))),
      h('div.acc-name', acc.displayName || acc.email), h('div.dim', acc.email),
      avatarEditor(acc, () => onAvatar && onAvatar(acc))));
  // Live preview while editing (the picture, if any, stays in front).
  const preview = (look) => {
    const next = accountAvatar({ ...acc, label: look.label, badge: look.badge }, 'xl');
    avatar.replaceWith(next);
    avatar = next;
  };
  box.append(header, lookEditor(acc, { onPreview: preview, onChanged: () => onLook && onLook(acc) }));
  const remove = h('button.btn.danger', { type: 'button', dataset: { testid: 'acc-remove' }, html: icon('signOut', 15) + ' Konto entfernen' });
  remove.addEventListener('click', () => onRemoved(acc));
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
