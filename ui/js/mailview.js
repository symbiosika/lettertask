// Renders a MessageDetail (reading pane and message window share this).
import { h, esc, fmtDateTime, fmtAddress, displayName, initials, avatarColor, fmtSize } from './util.js';
import { icon } from './icons.js';
import { pillHtml, visibleCats, hiddenCats, isHiddenCategory } from './categories.js';
import { recurrenceOf, recurrenceBadgeHtml, recurrenceMenuItems, recurrenceCategory, isRecurrenceCategory } from './recurrence.js';
import { call, openUrl, openWindow } from './api.js';
import { showMenu, showMenuAt, reportError, toast, busyClick } from './dialogs.js';
import { openTagPicker } from './tagpicker.js';
import { fmtSnoozeTarget } from './snooze.js';

// Mail HTML is untrusted. It is parsed with DOMParser (inert: no scripts, no
// loads), cleaned, and rendered in a sandboxed iframe without allow-scripts and
// with its own CSP. Remote resources stay blocked unless the user allows them.

/** Attributes that make the browser fetch a resource (links excluded). */
const FETCH_ATTRS = ['src', 'srcset', 'poster', 'background', 'data', 'lowsrc', 'dynsrc', 'imagesrcset'];
/** Elements whose href / xlink:href loads a resource instead of being a link. */
const HREF_LOADERS = new Set(['link', 'image', 'use', 'feimage', 'script', 'tref']);
const ABS_URL = /(^|[\s,])\s*([a-z][a-z0-9+.-]*:)?\/\//i;
const LOCAL_URL = /^\s*(data:|cid:|about:|#|$)/i;

function parseMail(html) {
  return new DOMParser().parseFromString(String(html || ''), 'text/html');
}

function remoteUrl(v) { return !LOCAL_URL.test(v) && ABS_URL.test(v); }

/** True if CSS text can load something from outside (url(), @import, image-set strings). */
function remoteCss(css) {
  if (!css) return false;
  if (/@import/i.test(css)) return true;
  if (/["']\s*(https?:)?\/\//i.test(css)) return true;
  const re = /url\(\s*(["']?)([^"')]*)/gi;
  let m;
  while ((m = re.exec(css))) {
    if (!/^\s*(data:|cid:|#)/i.test(m[2])) return true;
  }
  return false;
}

function hasRemoteIn(doc) {
  for (const s of doc.querySelectorAll('style')) if (remoteCss(s.textContent)) return true;
  for (const el of doc.querySelectorAll('*')) {
    for (const a of el.attributes) {
      const name = a.name.toLowerCase();
      if (FETCH_ATTRS.includes(name) && remoteUrl(a.value)) return true;
      if ((name === 'href' || name === 'xlink:href') && HREF_LOADERS.has(el.localName.toLowerCase()) && remoteUrl(a.value)) return true;
      if (name === 'style' && remoteCss(a.value)) return true;
    }
  }
  return false;
}

export function hasRemoteContent(html) { return hasRemoteIn(parseMail(html)); }

/**
 * A fixed inline `height` beats the `height:auto` below, so an image shrunk by
 * `max-width:100%` would be squeezed horizontally. Move pixel sizes into the
 * width/height attributes: they still give the size and aspect ratio, but the
 * stylesheet's `height:auto` can now scale the image proportionally.
 */
function keepAspect(im) {
  const px = (v) => { const m = /^(\d+(?:\.\d+)?)(px)?$/i.exec(String(v || '').trim()); return m ? m[1] : null; };
  const sh = px(im.style.height);
  if (!sh) return;
  const w = px(im.style.width) || px(im.getAttribute('width'));
  if (!w) return;
  im.setAttribute('width', w);
  im.setAttribute('height', sh);
  im.style.removeProperty('width');
  im.style.removeProperty('height');
}

/** Builds the iframe srcdoc for a mail body. */
export function buildSrcdoc(bodyHtml, allowRemote) {
  const img = allowRemote ? "data: cid: https: http:" : "data: cid:";
  const remote = allowRemote ? ' https: http:' : '';
  const csp = `default-src 'none'; img-src ${img}; style-src 'unsafe-inline'${remote}; font-src data:${remote}; media-src data:; form-action 'none'; base-uri 'none'`;
  const doc = parseMail(bodyHtml);
  // Nothing in a mail may run code, embed documents, redirect or change the CSP/base.
  doc.querySelectorAll('script,meta,base,iframe,frame,frameset,object,embed,applet,portal').forEach((n) => n.remove());
  doc.querySelectorAll(allowRemote ? 'link:not([rel~=stylesheet i])' : 'link').forEach((n) => n.remove());
  for (const el of doc.querySelectorAll('*')) {
    for (const a of [...el.attributes]) {
      // event handlers never run (sandbox), target="_self" etc. must not navigate the pane
      if (/^on/i.test(a.name) || a.name.toLowerCase() === 'target') el.removeAttribute(a.name);
    }
  }
  for (const im of doc.querySelectorAll('img')) keepAspect(im);
  const head = doc.head;
  head.insertAdjacentHTML('afterbegin',
    `<meta charset="utf-8">` +
    `<meta http-equiv="Content-Security-Policy" content="${csp}">` +
    `<style>html{background:#fff}body{font-family:"Segoe UI Variable Text","Segoe UI",system-ui,-apple-system,Arial,sans-serif;font-size:14px;line-height:1.5;color:#1f2328;margin:4px 2px 24px;word-wrap:break-word;overflow-wrap:anywhere}` +
    `img{max-width:100%;height:auto}pre{white-space:pre-wrap;font-family:Consolas,"Cascadia Mono",monospace;font-size:13px}blockquote{margin:0 0 0 .8ex;border-left:2px solid #c9d3e3;padding-left:1.2ex;color:#57606a}a{color:#2f6fe4}` +
    `::-webkit-scrollbar{width:10px;height:10px}::-webkit-scrollbar-thumb{background:#d0d4da;border-radius:5px;border:2px solid #fff}</style>`);
  return '<!doctype html>' + doc.documentElement.outerHTML;
}

/** Link target of a clicked element: <a>, <area> or SVG <a> (href / xlink:href). */
function linkOf(target) {
  const a = target && target.closest ? target.closest('a, area') : null;
  if (!a) return null;
  return { el: a, href: (a.getAttribute('href') || a.getAttributeNS('http://www.w3.org/1999/xlink', 'href') || '').trim() };
}

/** Mounts the body iframe and wires link interception. */
export function mountBody(host, bodyHtml, { allowRemote, mailboxId }) {
  host.innerHTML = '';
  // No allow-scripts, no allow-popups/forms/top-navigation: links open through
  // the click handler below (http(s) in the browser, mailto in compose). WebKitGTK
  // delivers no events from this frame, so there a click navigates the frame and
  // the app's navigation guard (src-tauri) hands the URL to the browser instead.
  const frame = h('iframe.mail-frame', { sandbox: 'allow-same-origin', title: 'Nachrichtentext', referrerpolicy: 'no-referrer' });
  frame.srcdoc = buildSrcdoc(bodyHtml, allowRemote);
  frame.addEventListener('load', () => {
    let doc;
    try { doc = frame.contentDocument; } catch { return; }
    if (!doc) return;
    doc.addEventListener('click', (e) => {
      const link = linkOf(e.target);
      if (!link) return;
      e.preventDefault();
      const { href } = link;
      if (/^mailto:/i.test(href)) {
        let to = href.slice(7).split('?')[0];
        try { to = decodeURIComponent(to); } catch { /* keep raw */ }
        openWindow('compose', { mailboxId, mode: 'new', to }).catch(reportError);
      } else if (/^https?:\/\//i.test(href)) {
        openUrl(href).catch(reportError);
      } else if (href.startsWith('#')) {
        const t = doc.getElementById(href.slice(1)) || doc.getElementsByName(href.slice(1))[0];
        if (t) t.scrollIntoView();
      }
    });
    // Middle click / other buttons never follow a link.
    doc.addEventListener('auxclick', (e) => { if (linkOf(e.target)) e.preventDefault(); });
    // Forward keyboard shortcuts typed while the iframe has focus.
    doc.addEventListener('keydown', (e) => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: e.key, code: e.code, ctrlKey: e.ctrlKey, shiftKey: e.shiftKey, altKey: e.altKey, metaKey: e.metaKey, bubbles: true }));
    });
  });
  host.appendChild(frame);
  return frame;
}

function addrList(list) {
  return (list || []).map((a) => `<span class="addr" title="${esc(a.address)}">${esc(displayName(a))}</span>`).join(', ');
}

function fileIcon(a) {
  if (/^image\//.test(a.contentType)) return 'image';
  return 'file';
}
function fileExt(name) {
  const m = /\.([a-z0-9]{1,5})$/i.exec(name || '');
  return m ? m[1].toUpperCase() : '';
}

const FLAG_TEXT = { flagged: 'Zur Nachverfolgung gekennzeichnet', complete: 'Als erledigt markiert' };

/**
 * Renders a message into container.
 * opts: { accountId, config, onChanged(), onResponded(response) → Promise (meeting request answered),
 *         setCategories(cats) → Promise, onAction(name), moreItems: [] , onManageCategories() }
 */
export function renderMessage(container, msg, opts) {
  const { accountId, config } = opts;
  container.innerHTML = '';
  let allowRemote = !!(config && config.loadRemoteImages);
  const remote = hasRemoteContent(msg.bodyHtml);
  // Kanban lane markers ("[K_…]") stay hidden but must survive tag edits.
  let lanes = hiddenCats(msg.categories);
  let cats = visibleCats(msg.categories);
  let rule = recurrenceOf(msg);

  const card = h('article.rp-card');
  const from = msg.from || { name: '(Unbekannt)', address: '' };

  // ---- title row: subject + tags (top right)
  const tags = h('div.rp-tags', { dataset: { testid: 'rp-tags' } });
  const renderTags = () => {
    tags.innerHTML = cats.map((c) => pillHtml(accountId, c, !!opts.setCategories)).join('');
    if (opts.setCategories) {
      const add = h('button.tag-add', { type: 'button', title: 'Kategorien zuweisen (T)', dataset: { testid: 'tag-add' }, html: icon('tag', 15) + `<span>${cats.length ? '' : 'Tag'}</span>` + icon('plus', 12) });
      add.addEventListener('click', () => openPicker(add));
      tags.appendChild(add);
    }
  };
  const applyCats = async (next) => {
    cats = next;
    renderTags();
    await opts.setCategories([...next, ...lanes]);
  };
  const openPicker = (anchor) => openTagPicker(anchor || tags.querySelector('.tag-add'), {
    accountId,
    assigned: () => cats,
    onToggle: (name, on) => applyCats(on ? [...cats, name] : cats.filter((c) => c !== name)),
    onManage: opts.onManageCategories,
  });
  tags.addEventListener('click', (e) => {
    const x = e.target.closest('.tp-x');
    if (!x) return;
    const name = x.closest('.tag-pill').dataset.name;
    applyCats(cats.filter((c) => c !== name)).catch(reportError);
  });
  renderTags();
  container._openTagPicker = () => openPicker();

  // recurring task badge (click: change / remove)
  const rec = h('span.rp-rec', { dataset: { testid: 'rp-rec' } });
  const renderRec = () => {
    rec.innerHTML = '';
    if (!rule) return;
    const b = h('button.rec-btn', { type: 'button', title: 'Wiederholung ändern', html: recurrenceBadgeHtml(rule) });
    if (opts.setRecurrence) b.addEventListener('click', () => showMenuAt(b, recurrenceMenuItems(rule, (k) => applyRule(k))));
    rec.appendChild(b);
  };
  const applyRule = async (k) => {
    rule = k;
    lanes = [...lanes.filter((c) => !isRecurrenceCategory(c)), ...(k ? [recurrenceCategory(k)] : [])];
    renderRec();
    try { await opts.setRecurrence(k); } catch (e) { reportError(e); }
  };
  renderRec();
  container._setRecurrence = opts.setRecurrence ? applyRule : null;

  const titleRow = h('div.rp-titlerow',
    h('h1.rp-subject', { title: msg.subject || '' }, msg.subject || '(Kein Betreff)'),
    rec, tags);

  // ---- sender row
  const act = (name, ic, title, testid) => h('button.icon-btn', { type: 'button', title, 'aria-label': title, dataset: { testid }, html: icon(ic, 18), onclick: (e) => opts.onAction && opts.onAction(name, e.currentTarget) });
  const actions = opts.onAction ? h('div.rp-actions',
    act('reply', 'reply', 'Antworten (Strg+R)', 'rp-reply'),
    act('replyAll', 'replyAll', 'Allen antworten (Strg+Umschalt+R)', 'rp-replyall'),
    act('forward', 'forward', 'Weiterleiten (Strg+F)', 'rp-forward'),
    h('span.rp-actsep'),
    act('more', 'more', 'Weitere Aktionen', 'rp-more')) : null;

  const recip = h('div.rp-recips');
  const toList = msg.to || [];
  const ccList = msg.cc || [];
  const LIMIT = 4;
  const lines = [];
  if (toList.length) lines.push(['An', toList]);
  if (ccList.length) lines.push(['Cc', ccList]);
  if ((msg.bcc || []).length) lines.push(['Bcc', msg.bcc]);
  const renderRecips = (expanded) => {
    recip.innerHTML = '';
    for (const [lbl, list] of lines) {
      const shown = expanded ? list : list.slice(0, LIMIT);
      const line = h('div.rp-line', h('span.rp-lbl', lbl + ':'), h('span.rp-val', { html: addrList(shown) }));
      if (!expanded && list.length > LIMIT) {
        line.appendChild(h('button.linkbtn', { type: 'button', onclick: () => renderRecips(true) }, `+${list.length - LIMIT} weitere`));
      }
      recip.appendChild(line);
    }
  };
  renderRecips(false);

  const fromRow = h('div.rp-fromrow',
    h('div.avatar.lg', { style: { background: avatarColor(from.address || from.name) } }, initials(from.name, from.address)),
    h('div.rp-fromblock',
      h('div.rp-from', h('span.rp-fromname', { title: from.address || '' }, from.name || from.address), from.address && from.name && from.name !== from.address ? h('span.rp-fromaddr', from.address) : null),
      recip),
    h('div.rp-meta',
      h('div.rp-date', fmtDateTime(new Date(msg.receivedAt || msg.sentAt))),
      actions));

  card.append(titleRow, fromRow);

  // ---- info bars
  const bars = h('div.rp-bars');
  if (msg.dueAt && opts.snoozed) {
    bars.appendChild(h('div.infobar.snooze', { html: icon('snooze', 16) + ' Auf Wiedervorlage bis ' + esc(fmtSnoozeTarget(new Date(msg.dueAt))) }));
  } else if (msg.flag && msg.flag !== 'notFlagged') {
    bars.appendChild(h('div.infobar.flag', { html: icon('flag', 16, msg.flag === 'flagged' ? 'flagged' : '') + ' ' + esc(FLAG_TEXT[msg.flag]) + (msg.flag === 'flagged' && msg.dueAt ? ' – fällig ' + esc(fmtSnoozeTarget(new Date(msg.dueAt))) : '') }));
  }
  if (msg.importance === 'high') bars.appendChild(h('div.infobar.important', { html: icon('important', 16) + ' Diese Nachricht wurde mit hoher Wichtigkeit gesendet.' }));
  if (msg.isMeetingRequest && msg.eventId) {
    const bar = h('div.infobar.meeting', h('span.ib-text', { html: icon('calendar', 16) + ' Besprechungsanfrage' }), h('span.spacer'));
    const DONE = { accept: 'Zugesagt', tentativelyAccept: 'Mit Vorbehalt zugesagt', decline: 'Abgelehnt' };
    const mk = (label, ic, response) => h('button.btn.small', {
      onclick: busyClick(async () => {
        // Answered once is answered: the buttons do not come back.
        if (bar.dataset.answered) return;
        await call('respond_event', { accountId, eventId: msg.eventId, response, comment: '', sendResponse: true });
        bar.dataset.answered = response;
        for (const b of bar.querySelectorAll('.btn')) b.remove();
        bar.appendChild(h('span.ib-done', { dataset: { testid: 'meeting-answered' } }, DONE[response]));
        if (opts.onResponded) await opts.onResponded(response);
        else { toast('Antwort gesendet.'); opts.onChanged && opts.onChanged(); }
      }, bar), html: icon(ic, 15) + ' ' + esc(label),
    });
    bar.append(mk('Annehmen', 'accept', 'accept'), mk('Mit Vorbehalt', 'tentative', 'tentativelyAccept'), mk('Ablehnen', 'decline', 'decline'));
    bars.appendChild(bar);
  }
  if (opts.delegation && opts.onDelegationDone) {
    const d = opts.delegation;
    const bar = h('div.infobar.delegation', { dataset: { testid: 'delegation-bar' } },
      h('span.ib-text', { html: icon('people', 16) + `<span>Antwort auf die an <b>${esc(d.name)}</b> delegierte Aufgabe „${esc(d.subject)}“</span>` }),
      h('span.spacer'));
    bar.appendChild(h('button.btn.small.success', {
      type: 'button', dataset: { testid: 'delegation-done' },
      title: 'Delegierte Aufgabe und diese Antwort archivieren',
      onclick: busyClick(() => opts.onDelegationDone(), bar),
      html: icon('check', 15) + ' Aufgabe erledigt &amp; archivieren',
    }));
    bars.appendChild(bar);
  }
  let remoteBar = null;
  if (remote && !allowRemote) {
    remoteBar = h('div.infobar.remote', { role: 'button', title: 'Bilder herunterladen' },
      h('span.ib-text', { html: icon('image', 16) + ' Externe Bilder wurden zum Schutz Ihrer Privatsphäre blockiert.' }),
      h('span.spacer'),
      h('span.linkbtn', 'Bilder herunterladen'));
    remoteBar.addEventListener('click', () => { allowRemote = true; remoteBar.remove(); mountBody(bodyHost, msg.bodyHtml, { allowRemote, mailboxId: msg.mailboxId }); });
    bars.appendChild(remoteBar);
  }
  if (bars.childNodes.length) card.appendChild(bars);

  if (msg.attachments && msg.attachments.length) {
    const strip = h('div.rp-attachments');
    for (const a of msg.attachments) {
      const ext = fileExt(a.name);
      const el = h('button.attachment', { type: 'button', title: `${a.name} (${fmtSize(a.size)}) – Klicken zum Öffnen, Rechtsklick zum Speichern` },
        h('span.att-ic', { html: icon(fileIcon(a), 18) }),
        h('span.att-text', h('span.att-name', a.name), h('span.att-size', [ext, fmtSize(a.size)].filter(Boolean).join(' · '))));
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
    card.appendChild(strip);
  }

  const bodyHost = h('div.rp-body');
  card.appendChild(bodyHost);
  container.appendChild(card);
  mountBody(bodyHost, msg.bodyHtml, { allowRemote, mailboxId: msg.mailboxId });
}

/** Standard "⋯" menu for a message. */
export function showMoreMenu(anchor, items) {
  showMenuAt(anchor, items);
}

export function printMessage(container) {
  const f = (container || document).querySelector('.mail-frame');
  if (f && f.contentWindow) f.contentWindow.print(); else window.print();
}

export { fmtAddress };
