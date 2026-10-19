// Renders a MessageDetail (reading pane and message window share this).
import { h, esc, fmtDateTime, fmtAddress, displayName, initials, avatarColor, fmtSize } from './util.js';
import { icon } from './icons.js';
import { chipHtml } from './categories.js';
import { call, openUrl, openWindow } from './api.js';
import { showMenu, reportError, toast } from './dialogs.js';

const REMOTE_RE = /(<img\b[^>]*\bsrc\s*=\s*["']?\s*(https?:)?\/\/)|(url\(\s*["']?\s*(https?:)?\/\/)|(<image\b[^>]*href\s*=\s*["']?\s*https?:)/i;

export function hasRemoteContent(html) { return REMOTE_RE.test(html || ''); }

/** Builds the iframe srcdoc for a mail body. */
export function buildSrcdoc(bodyHtml, allowRemote) {
  const img = allowRemote ? "data: cid: https: http:" : "data: cid:";
  const csp = `default-src 'none'; img-src ${img}; style-src 'unsafe-inline'${allowRemote ? ' https: http:' : ''}; font-src data:${allowRemote ? ' https: http:' : ''}; media-src data:`;
  const clean = String(bodyHtml || '')
    .replace(/<script\b[\s\S]*?<\/script\s*>/gi, '')
    .replace(/<meta\b[^>]*http-equiv\s*=\s*["']?(refresh|content-security-policy)[^>]*>/gi, '')
    .replace(/<base\b[^>]*>/gi, '');
  return `<!doctype html><html><head><meta charset="utf-8">` +
    `<meta http-equiv="Content-Security-Policy" content="${csp}">` +
    `<base target="_blank">` +
    `<style>html{background:#fff}body{font-family:Calibri,"Segoe UI",Arial,sans-serif;font-size:11pt;color:#1f1f1f;margin:10px 14px;word-wrap:break-word;overflow-wrap:anywhere}` +
    `img{max-width:100%;height:auto}pre{white-space:pre-wrap;font-family:Consolas,monospace;font-size:10pt}blockquote{margin:0 0 0 .8ex;border-left:2px solid #b5c4df;padding-left:1ex;color:#444}a{color:#0563c1}</style>` +
    `</head><body>${clean}</body></html>`;
}

/** Mounts the body iframe and wires link interception. */
export function mountBody(host, bodyHtml, { allowRemote, mailboxId }) {
  host.innerHTML = '';
  const frame = h('iframe.mail-frame', { sandbox: 'allow-same-origin allow-popups allow-popups-to-escape-sandbox', title: 'Nachrichtentext', referrerpolicy: 'no-referrer' });
  frame.srcdoc = buildSrcdoc(bodyHtml, allowRemote);
  frame.addEventListener('load', () => {
    let doc;
    try { doc = frame.contentDocument; } catch { return; }
    if (!doc) return;
    doc.addEventListener('click', (e) => {
      const a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
      if (!a) return;
      e.preventDefault();
      const href = a.getAttribute('href') || '';
      if (/^mailto:/i.test(href)) {
        const to = decodeURIComponent(href.slice(7).split('?')[0]);
        openWindow('compose', { mailboxId, mode: 'new', to }).catch(reportError);
      } else if (/^(https?:|ftp:)/i.test(href)) {
        openUrl(href).catch(reportError);
      } else if (href.startsWith('#')) {
        const t = doc.getElementById(href.slice(1)) || doc.getElementsByName(href.slice(1))[0];
        if (t) t.scrollIntoView();
      }
    });
    // Forward keyboard shortcuts typed while the iframe has focus.
    doc.addEventListener('keydown', (e) => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: e.key, code: e.code, ctrlKey: e.ctrlKey, shiftKey: e.shiftKey, altKey: e.altKey, metaKey: e.metaKey, bubbles: true }));
    });
  });
  host.appendChild(frame);
  return frame;
}

function addrList(list) {
  return (list || []).map((a) => `<span class="addr" title="${esc(a.address)}">${esc(displayName(a))}</span>`).join('; ');
}

const FLAG_TEXT = { flagged: 'Zur Nachverfolgung.', complete: 'Gekennzeichnet: Erledigt.' };

/**
 * Renders a message into container.
 * opts: { accountId, config, onChanged() }
 */
export function renderMessage(container, msg, opts) {
  const { accountId, config } = opts;
  container.innerHTML = '';
  let allowRemote = !!(config && config.loadRemoteImages);
  const remote = hasRemoteContent(msg.bodyHtml);

  const from = msg.from || { name: '(Unbekannt)', address: '' };
  const header = h('div.rp-header',
    h('div.rp-subject', { title: msg.subject || '' }, msg.subject || '(Kein Betreff)'),
    h('div.rp-fromline',
      h('div.avatar', { style: { background: avatarColor(from.address || from.name) } }, initials(from.name, from.address)),
      h('div.rp-fromblock',
        h('div.rp-from', h('span.rp-fromname', from.name || from.address), from.address && from.name !== from.address ? h('span.rp-fromaddr', `<${from.address}>`) : null),
        h('div.rp-date', fmtDateTime(new Date(msg.receivedAt || msg.sentAt))),
      ),
    ),
    msg.to && msg.to.length ? h('div.rp-line', h('span.rp-lbl', 'An:'), h('span.rp-val', { html: addrList(msg.to) })) : null,
    msg.cc && msg.cc.length ? h('div.rp-line', h('span.rp-lbl', 'Cc:'), h('span.rp-val', { html: addrList(msg.cc) })) : null,
    msg.bcc && msg.bcc.length ? h('div.rp-line', h('span.rp-lbl', 'Bcc:'), h('span.rp-val', { html: addrList(msg.bcc) })) : null,
  );
  container.appendChild(header);

  // info bars
  const bars = h('div.rp-bars');
  if (msg.categories && msg.categories.length) {
    bars.appendChild(h('div.rp-catbar', { html: msg.categories.map((c) => chipHtml(accountId, c)).join(' ') }));
  }
  if (msg.flag && msg.flag !== 'notFlagged') {
    bars.appendChild(h('div.infobar.flag', { html: icon(msg.flag === 'complete' ? 'check' : 'flag', 14) + ' ' + esc(FLAG_TEXT[msg.flag]) }));
  }
  if (msg.importance === 'high') bars.appendChild(h('div.infobar', { html: icon('important', 14) + ' Diese Nachricht wurde mit Wichtigkeit &bdquo;Hoch&ldquo; gesendet.' }));
  if (msg.isMeetingRequest && msg.eventId) {
    const mk = (label, ic, response) => h('button.btn.small', {
      onclick: async () => {
        try {
          await call('respond_event', { accountId, eventId: msg.eventId, response, comment: '', sendResponse: true });
          toast('Antwort gesendet.');
          opts.onChanged && opts.onChanged();
        } catch (e) { reportError(e); }
      }, html: icon(ic, 14) + ' ' + esc(label),
    });
    bars.appendChild(h('div.infobar.meeting', h('span', { html: icon('calendar', 14) + ' Besprechungsanfrage' }),
      h('span.spacer'), mk('Annehmen', 'accept', 'accept'), mk('Mit Vorbehalt', 'tentative', 'tentativelyAccept'), mk('Ablehnen', 'decline', 'decline')));
  }
  let remoteBar = null;
  if (remote && !allowRemote) {
    remoteBar = h('div.infobar.remote', h('span', { html: icon('info', 14) + ' Klicken Sie hier, um Bilder herunterzuladen. Um den Datenschutz zu erhöhen, wurde der automatische Download von Bildern in dieser Nachricht verhindert.' }));
    remoteBar.addEventListener('click', () => { allowRemote = true; remoteBar.remove(); mountBody(bodyHost, msg.bodyHtml, { allowRemote, mailboxId: msg.mailboxId }); });
    remoteBar.title = 'Bilder herunterladen';
    remoteBar.setAttribute('role', 'button');
    bars.appendChild(remoteBar);
  }
  if (bars.childNodes.length) container.appendChild(bars);

  if (msg.attachments && msg.attachments.length) {
    const strip = h('div.rp-attachments');
    for (const a of msg.attachments) {
      const ic = /^image\//.test(a.contentType) ? 'image' : 'file';
      const el = h('button.attachment', { title: `${a.name} (${fmtSize(a.size)})`, html: icon(ic, 16) + `<span class="att-name">${esc(a.name)}</span><span class="att-size">(${esc(fmtSize(a.size))})</span>` });
      const dl = (action) => call('download_attachment', { mailboxId: msg.mailboxId, messageId: msg.id, attachmentId: a.id, action })
        .then((p) => { if (p && action === 'save') toast('Gespeichert: ' + p); })
        .catch(reportError);
      el.addEventListener('click', () => dl('open'));
      el.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        showMenu([
          { label: 'Öffnen', icon: 'open', onClick: () => dl('open') },
          { label: 'Speichern unter…', icon: 'save', onClick: () => dl('save') },
        ], e.clientX, e.clientY);
      });
      strip.appendChild(el);
    }
    container.appendChild(strip);
  }

  const bodyHost = h('div.rp-body');
  container.appendChild(bodyHost);
  mountBody(bodyHost, msg.bodyHtml, { allowRemote, mailboxId: msg.mailboxId });
}

export { fmtAddress };
