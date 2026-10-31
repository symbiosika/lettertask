// Rich-text editor in an isolated iframe (designMode), used by compose and event windows.
// The iframe keeps styles of quoted/forwarded HTML mails away from the window's own UI.
import { h } from './util.js';
import { icon } from './icons.js';
import { promptDialog, showMenuAt } from './dialogs.js';
import { installNativeImageMenu } from './imagemenu.js';

installNativeImageMenu();

const COLORS = ['#000000', '#7f7f7f', '#c00000', '#ff0000', '#ffc000', '#00b050', '#0070c0', '#002060', '#7030a0'];
// html needs a height so the body's min-height fills the editor: clicks
// anywhere in an empty editor land in the text and it is never 0 px tall.
const BASE_CSS = 'html{height:100%}body{font-family:"Segoe UI",Calibri,system-ui,Arial,sans-serif;font-size:11pt;line-height:1.45;color:#1f2328;margin:14px 20px;max-width:820px;word-wrap:break-word;min-height:calc(100% - 28px);box-sizing:border-box}p{margin:0}img{max-width:100%}blockquote{margin:0 0 0 .8ex;border-left:2px solid #b5c4df;padding-left:1ex}';

function splitHtml(html) {
  const doc = new DOMParser().parseFromString(String(html || ''), 'text/html');
  doc.querySelectorAll('script,base,meta,link,iframe,object,embed,title').forEach((n) => n.remove());
  for (const el of doc.querySelectorAll('*')) {
    for (const a of [...el.attributes]) {
      if (/^on/i.test(a.name) || (/^(href|src)$/i.test(a.name) && /^\s*javascript:/i.test(a.value))) el.removeAttribute(a.name);
    }
  }
  const styles = [...doc.querySelectorAll('style')].map((s) => s.outerHTML).join('');
  doc.querySelectorAll('style').forEach((s) => s.remove());
  return { styles, body: doc.body ? doc.body.innerHTML : '' };
}

const URL_RE = /\b(?:https?:\/\/|www\.)[^\s<>"]+/gi;

/** Turns bare web addresses in the text into links (text inside <a> stays as is). */
function linkify(root) {
  const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (n.parentElement && n.parentElement.closest('a,style,script') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  const nodes = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) if (/https?:\/\/|www\./i.test(n.data)) nodes.push(n);
  for (const node of nodes) {
    const text = node.data;
    const frag = root.ownerDocument.createDocumentFragment();
    let last = 0;
    for (const m of text.matchAll(URL_RE)) {
      // Trailing punctuation usually ends the sentence, not the address.
      const url = m[0].replace(/[.,;:!?)\]}'»“”]+$/, '');
      if (!url || /^www\.$/i.test(url)) continue;
      frag.append(text.slice(last, m.index));
      const a = root.ownerDocument.createElement('a');
      a.href = /^www\./i.test(url) ? 'https://' + url : url;
      a.textContent = url;
      frag.append(a);
      last = m.index + url.length;
    }
    if (!last) continue;
    frag.append(text.slice(last));
    node.replaceWith(frag);
  }
}

function fileToDataUrl(file) {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result);
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });
}

/**
 * WebKit hands out pasted images as blob: URLs, which only live as long as the
 * document. Replace them with data URLs so the HTML can be stored and sent.
 */
async function embedBlobImages(doc) {
  const imgs = [...doc.querySelectorAll('img')].filter((i) => /^blob:/i.test(i.getAttribute('src') || ''));
  await Promise.all(imgs.map(async (img) => {
    try {
      // Polled: WebKit delivers no load events from the sandboxed frame.
      for (let i = 0; !img.complete && i < 100; i++) await new Promise((r) => setTimeout(r, 50));
      if (!img.naturalWidth) return;
      const c = doc.createElement('canvas');
      c.width = img.naturalWidth;
      c.height = img.naturalHeight;
      c.getContext('2d').drawImage(img, 0, 0);
      img.setAttribute('src', c.toDataURL('image/png'));
    } catch (e) { console.warn('Bild konnte nicht übernommen werden', e); }
  }));
}

/**
 * Pictures in the text can be resized: a click on one shows a frame with a
 * corner handle (drag, aspect ratio kept) and quick sizes. The frame lives
 * in the outer window, so it never ends up in the mail's HTML.
 */
function bindImageResize(frame, doc, changed) {
  let img = null;
  const box = h('div.img-resize', { dataset: { testid: 'img-resize' } });
  const handle = h('div.img-resize-handle', { dataset: { testid: 'img-resize-handle' } });
  const label = h('span.img-resize-size');
  const sizes = h('div.img-resize-bar',
    ...[['25 %', 0.25], ['50 %', 0.5], ['100 %', 1]].map(([l, f]) => {
      const b = h('button', { type: 'button', dataset: { testid: 'img-size-' + Math.round(f * 100) } }, l);
      b.addEventListener('mousedown', (e) => e.preventDefault());
      b.addEventListener('click', () => setWidth(Math.round((img.naturalWidth || img.width) * f)));
      return b;
    }), label);
  box.append(handle, sizes);
  const place = () => {
    if (!img || !img.isConnected) { hide(); return; }
    const f = frame.getBoundingClientRect();
    const r = img.getBoundingClientRect();
    Object.assign(box.style, { left: f.left + r.left + 'px', top: f.top + r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
    label.textContent = `${Math.round(r.width)} × ${Math.round(r.height)}`;
  };
  function show(el) {
    img = el;
    if (!box.isConnected) document.body.appendChild(box);
    place();
  }
  function hide() { img = null; box.remove(); }
  function setWidth(w) {
    if (!img) return;
    const max = doc.body.clientWidth || w;
    w = Math.max(16, Math.min(Math.round(w), Math.max(max, 16)));
    // Width attribute for mail programs that ignore CSS, height follows.
    img.setAttribute('width', String(w));
    img.removeAttribute('height');
    img.style.width = w + 'px';
    img.style.height = 'auto';
    changed();
    place();
  }
  doc.addEventListener('mousedown', (e) => {
    const t = e.target && e.target.closest ? e.target.closest('img') : null;
    if (t) show(t); else hide();
  });
  doc.addEventListener('keydown', () => hide());
  doc.addEventListener('scroll', place);
  window.addEventListener('resize', place);
  handle.addEventListener('pointerdown', (e) => {
    if (!img) return;
    e.preventDefault();
    // Captured: the drag keeps working when the pointer crosses the editor frame.
    handle.setPointerCapture(e.pointerId);
    const startX = e.clientX, startW = img.getBoundingClientRect().width;
    const move = (m) => setWidth(startW + m.clientX - startX);
    const up = () => { handle.removeEventListener('pointermove', move); handle.removeEventListener('pointerup', up); handle.removeEventListener('pointercancel', up); };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  });
  handle.addEventListener('mousedown', (e) => e.preventDefault());
  // Clicks in the outer window (toolbar excepted) end the resize mode.
  document.addEventListener('mousedown', (e) => { if (img && !box.contains(e.target)) hide(); });
}

/** Local paths when `text` consists only of file:// addresses (one per line
 *  or separated by blanks), else []. */
export function fileUriPaths(text) {
  const parts = String(text || '').split(/[\r\n]+|\s+(?=file:)/).map((x) => x.trim()).filter((x) => x && !x.startsWith('#'));
  if (!parts.length || !parts.every((x) => /^file:\/\/[^\s]+$/i.test(x))) return [];
  return parts.map((u) => {
    let p = u.replace(/^file:\/\/(localhost)?/i, '');
    try { p = decodeURIComponent(p); } catch { /* keep as is */ }
    // Windows: file:///C:/… → C:/…
    if (/^\/[A-Za-z]:[\\/]/.test(p)) p = p.slice(1);
    return p;
  });
}

/** Removes text that a paste inserted when it is nothing but file addresses;
 *  returns their paths. */
function takePastedFileUris(records) {
  const out = [];
  for (const r of records) {
    if (r.type === 'characterData') {
      const now = r.target.data, old = r.oldValue || '';
      if (now.length <= old.length) continue;
      let a = 0;
      while (a < old.length && old[a] === now[a]) a++;
      let b = 0;
      while (b < old.length - a && old[old.length - 1 - b] === now[now.length - 1 - b]) b++;
      const inserted = now.slice(a, now.length - b);
      const paths = inserted.length > 7 ? fileUriPaths(inserted) : [];
      if (paths.length) { r.target.data = old; out.push(...paths); }
    } else if (r.type === 'childList') {
      for (const n of r.addedNodes) {
        if (!n.isConnected) continue;
        const text = n.nodeType === 3 ? n.data : n.nodeType === 1 && !n.querySelector('img,table') ? n.textContent : '';
        const paths = text && text.length > 7 ? fileUriPaths(text) : [];
        if (paths.length) { n.remove(); out.push(...paths); }
      }
    }
  }
  return out;
}

export function createEditor({ html = '', onInput, minHeight = 200, lead = [], onDropFiles, onPasteFiles, onPastePaths } = {}) {
  const frame = h('iframe.editor-frame', { sandbox: 'allow-same-origin', title: 'Nachrichtentext', style: { minHeight: minHeight + 'px' } });
  let doc = null;
  let pending = html;
  let resolveReady;
  const ready = new Promise((resolve) => { resolveReady = resolve; });
  frame.addEventListener('load', () => {
      doc = frame.contentDocument;
      doc.designMode = 'on';
      try { doc.execCommand('styleWithCSS', false, true); doc.execCommand('defaultParagraphSeparator', false, 'p'); } catch { /* ignore */ }
      // Changes are watched with a MutationObserver, not input events: WebKit
      // (WebKitGTK on Linux) dispatches no events at all from a frame sandboxed
      // without allow-scripts, so typing and pasting would go unnoticed.
      const d = doc;
      new MutationObserver((records) => {
        if (d !== doc) return;
        // WebKitGTK fires no paste event here: pasted file addresses are
        // recognised in the inserted text, removed and attached as files.
        if (onPastePaths) {
          const paths = takePastedFileUris(records);
          if (paths.length) onPastePaths(paths);
        }
        onInput && onInput();
        // A native paste (e.g. an image in WebKit) may bring blob: images.
        if (d.querySelector('img[src^="blob:" i]')) embedBlobImages(d).catch(console.warn);
      }).observe(d.documentElement, { subtree: true, childList: true, characterData: true, characterDataOldValue: true, attributes: true });
      if (onDropFiles) {
        // Files dropped onto the text become attachments (the drop lands in this frame, not the window).
        const hasFiles = (e) => e.dataTransfer && [...e.dataTransfer.types].includes('Files');
        const isMailAtt = (e) => e.dataTransfer && [...e.dataTransfer.types].includes('application/x-lt-attachment');
        doc.addEventListener('dragover', (e) => { if (hasFiles(e) || isMailAtt(e)) e.preventDefault(); });
        doc.addEventListener('drop', (e) => {
          // An attachment of a mail: handled by the window (compose.js).
          if (isMailAtt(e)) {
            e.preventDefault();
            window.document.dispatchEvent(new DragEvent('drop', { dataTransfer: e.dataTransfer, bubbles: true }));
            return;
          }
          if (!hasFiles(e) || !e.dataTransfer.files.length) return;
          e.preventDefault();
          window.document.body.classList.remove('dropping');
          onDropFiles([...e.dataTransfer.files]);
        });
      }
      bindImageResize(frame, doc, () => { onInput && onInput(); });
      doc.addEventListener('keyup', updateState);
      doc.addEventListener('mouseup', updateState);
      doc.addEventListener('paste', (e) => {
        const cd = e.clipboardData;
        // Copied files (not pictures) become attachments.
        const other = cd ? [...cd.files].filter((f) => !/^image\//.test(f.type)) : [];
        if (other.length && onPasteFiles) { e.preventDefault(); onPasteFiles(other); return; }
        // Only file:// addresses (files copied in the file manager): attach them.
        const uris = cd && !cd.files.length ? fileUriPaths(cd.getData('text/uri-list') || cd.getData('text/plain')) : [];
        if (uris.length && onPastePaths) { e.preventDefault(); onPastePaths(uris); return; }
        const raw = cd && cd.getData('text/html');
        const files = cd ? [...cd.files].filter((f) => /^image\//.test(f.type)) : [];
        if (!raw && !files.length) return;
        e.preventDefault();
        const target = doc;
        (async () => {
          let html = raw ? splitHtml(raw).body : '';
          // Image only (screenshot, copied picture): embed it as data URL.
          if (!html) html = (await Promise.all(files.map(fileToDataUrl))).map((u) => `<img src="${u}">`).join('');
          if (target !== doc) return;
          doc.execCommand('insertHTML', false, html);
        })().catch(console.warn);
      });
      doc.addEventListener('keydown', (e) => {
        // forward window shortcuts (Ctrl+Enter send, Ctrl+S save, Esc)
        if ((e.ctrlKey || e.metaKey) && (e.key === 'Enter' || e.key.toLowerCase() === 's')) {
          e.preventDefault();
          window.dispatchEvent(new KeyboardEvent('keydown', { key: e.key, ctrlKey: true, bubbles: true }));
        }
      });
      resolveReady();
  });
  setContent(pending);

  function setContent(x) {
    const { styles, body } = splitHtml(x);
    frame.srcdoc = `<!doctype html><html><head><meta charset="utf-8"><meta data-oc="1" http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data: cid: blob: https: http:; style-src 'unsafe-inline'"><style data-oc="1">${BASE_CSS}</style>${styles}</head><body>${body}</body></html>`;
  }

  const exec = (cmd, val = null) => { if (!doc) return; frame.contentWindow.focus(); doc.execCommand(cmd, false, val); onInput && onInput(); updateState(); };
  const btn = (ic, title, fn, cmd) => {
    const b = h('button.ed-btn', { type: 'button', title, 'aria-label': title, html: icon(ic, 18), dataset: { cmd: cmd || '' } });
    b.addEventListener('mousedown', (e) => e.preventDefault());
    b.addEventListener('click', fn);
    return b;
  };
  const FONTS = ['Segoe UI', 'Calibri', 'Arial', 'Verdana', 'Georgia', 'Times New Roman', 'Consolas'];
  const size = h('select.ed-select', { title: 'Schriftgrad', 'aria-label': 'Schriftgrad' },
    ...[['1', '8'], ['2', '10'], ['3', '11'], ['4', '14'], ['5', '18'], ['6', '24'], ['7', '36']].map(([v, l]) => h('option', { value: v, selected: v === '3' }, l)));
  size.addEventListener('change', () => exec('fontSize', size.value));
  const colorBtn = btn('textColor', 'Schriftfarbe', () => {
    const pal = h('div.ed-colors');
    for (const c of COLORS) {
      const sw = h('button.ed-color', { type: 'button', style: { background: c }, title: c });
      sw.addEventListener('mousedown', (e) => e.preventDefault());
      sw.addEventListener('click', () => { exec('foreColor', c); pal.remove(); });
      pal.appendChild(sw);
    }
    const r = colorBtn.getBoundingClientRect();
    Object.assign(pal.style, { left: r.left + 'px', top: r.bottom + 4 + 'px' });
    document.body.appendChild(pal);
    setTimeout(() => document.addEventListener('mousedown', function off(e) { if (!pal.contains(e.target)) { pal.remove(); document.removeEventListener('mousedown', off); } }), 0);
  });
  const moreBtn = btn('more', 'Weitere Formatierungen', () => {
    showMenuAt(moreBtn, [
      { label: 'Schriftart', submenu: () => FONTS.map((f) => ({ label: f, onClick: () => exec('fontName', f) })) },
      { label: 'Zitat', icon: 'quote', onClick: () => exec('formatBlock', 'blockquote') },
      { label: 'Normaler Text', onClick: () => exec('formatBlock', 'p') },
      { label: 'Horizontale Linie', onClick: () => exec('insertHorizontalRule') },
      { label: 'Link entfernen', icon: 'link', onClick: removeLink },
    ]);
  });
  /** Link around the selection: <a> it lies in, else null. */
  const linkAt = (range) => {
    const n = range && range.commonAncestorContainer;
    const el = n && (n.nodeType === 1 ? n : n.parentElement);
    return el && el.closest ? el.closest('a[href]') : null;
  };
  /** "www.x.de" → https://…, "a@b.de" → mailto:…; anything else non-http(s)/mailto is refused. */
  const normalizeUrl = (v) => {
    v = (v || '').trim();
    if (!v || /^(https?:\/\/|mailto:)?$/i.test(v)) return null;
    if (/^(https?:\/\/|mailto:)/i.test(v)) return v;
    if (/^[^\s/@:]+@[^\s/@]+\.[^\s/@]+$/.test(v)) return 'mailto:' + v;
    if (/^[a-z][a-z0-9+.-]*:/i.test(v) && !/^[^:/]+:\d/.test(v)) return null;
    return 'https://' + v.replace(/^\/+/, '');
  };
  // Done on the DOM rather than with execCommand('createLink'), which WebKit
  // ignores in the sandboxed frame once the selection was lost to the dialog.
  async function editLink() {
    if (!doc) return;
    const d = doc;
    const sel = d.getSelection();
    const range = sel && sel.rangeCount ? sel.getRangeAt(0).cloneRange() : null;
    const existing = linkAt(range);
    const input = await promptDialog('Adresse:', { title: existing ? 'Link bearbeiten' : 'Link einfügen', value: existing ? existing.getAttribute('href') : 'https://' });
    if (input == null || d !== doc) return;
    const url = normalizeUrl(input);
    if (!url) return;
    frame.contentWindow.focus();
    if (existing) {
      if (existing.textContent === existing.getAttribute('href')) existing.textContent = url;
      existing.setAttribute('href', url);
    } else {
      const r = range || (() => { const x = d.createRange(); x.selectNodeContents(d.body); x.collapse(false); return x; })();
      const a = d.createElement('a');
      a.setAttribute('href', url);
      if (r.collapsed) a.textContent = url.replace(/^mailto:/i, '');
      else {
        // Links nested in the selection are replaced by the new one.
        const frag = r.extractContents();
        frag.querySelectorAll('a').forEach((x) => x.replaceWith(...x.childNodes));
        a.appendChild(frag);
      }
      r.insertNode(a);
      const after = d.createRange();
      after.setStartAfter(a); after.collapse(true);
      sel.removeAllRanges(); sel.addRange(after);
    }
    onInput && onInput();
    updateState();
  }
  function removeLink() {
    const sel = doc && doc.getSelection();
    const a = linkAt(sel && sel.rangeCount ? sel.getRangeAt(0) : null);
    if (!a) return;
    a.replaceWith(...a.childNodes);
    onInput && onInput();
  }
  const sep = () => h('span.ed-sep');

  const toolbar = h('div.ed-toolbar',
    ...lead, lead.length ? sep() : null,
    btn('bold', 'Fett (Strg+B)', () => exec('bold'), 'bold'),
    btn('italic', 'Kursiv (Strg+I)', () => exec('italic'), 'italic'),
    btn('underline', 'Unterstrichen (Strg+U)', () => exec('underline'), 'underline'),
    colorBtn, size, sep(),
    btn('bullets', 'Aufzählungszeichen', () => exec('insertUnorderedList')),
    btn('numbers', 'Nummerierung', () => exec('insertOrderedList')),
    btn('outdent', 'Einzug verkleinern', () => exec('outdent')),
    btn('indent', 'Einzug vergrößern', () => exec('indent')), sep(),
    btn('link', 'Link einfügen/bearbeiten', () => editLink()),
    btn('clearFormat', 'Formatierung löschen', () => exec('removeFormat')),
    moreBtn,
  );

  function updateState() {
    if (!doc) return;
    for (const cmd of ['bold', 'italic', 'underline']) {
      const b = toolbar.querySelector(`[data-cmd="${cmd}"]`);
      try { b.classList.toggle('on', doc.queryCommandState(cmd)); } catch { /* ignore */ }
    }
  }

  return {
    toolbar,
    area: frame,
    ready,
    getHtml: () => {
      if (!doc) return pending || '';
      // Links are made on read, not while typing: rewriting text under the caret would move it.
      const body = doc.body.cloneNode(true);
      linkify(body);
      const styles = [...doc.head.querySelectorAll('style:not([data-oc])')].map((s) => s.outerHTML).join('');
      return styles ? `<html><head>${styles}</head><body>${body.innerHTML}</body></html>` : body.innerHTML;
    },
    getText: () => (doc ? doc.body.innerText : ''),
    /** The editable document (null until loaded). */
    doc: () => doc,
    /** Notifies listeners after programmatic changes. */
    changed: () => onInput && onInput(),
    setHtml: async (x) => { pending = x; doc = null; const p = new Promise((r) => frame.addEventListener('load', r, { once: true })); setContent(x); await p; },
    focus: async () => {
      await ready;
      frame.contentWindow.focus();
      const r = doc.createRange();
      r.setStart(doc.body, 0); r.collapse(true);
      const s = doc.getSelection(); s.removeAllRanges(); s.addRange(r);
    },
  };
}
