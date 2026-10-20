// Rich-text editor in an isolated iframe (designMode), used by compose and event windows.
// The iframe keeps styles of quoted/forwarded HTML mails away from the window's own UI.
import { h } from './util.js';
import { icon } from './icons.js';
import { promptDialog, showMenuAt } from './dialogs.js';

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

export function createEditor({ html = '', onInput, minHeight = 200, lead = [] } = {}) {
  const frame = h('iframe.editor-frame', { sandbox: 'allow-same-origin', title: 'Nachrichtentext', style: { minHeight: minHeight + 'px' } });
  let doc = null;
  let pending = html;
  let resolveReady;
  const ready = new Promise((resolve) => { resolveReady = resolve; });
  frame.addEventListener('load', () => {
      doc = frame.contentDocument;
      doc.designMode = 'on';
      try { doc.execCommand('styleWithCSS', false, true); doc.execCommand('defaultParagraphSeparator', false, 'p'); } catch { /* ignore */ }
      doc.addEventListener('input', () => onInput && onInput());
      doc.addEventListener('keyup', updateState);
      doc.addEventListener('mouseup', updateState);
      doc.addEventListener('paste', (e) => {
        const raw = e.clipboardData && e.clipboardData.getData('text/html');
        if (!raw) return;
        e.preventDefault();
        doc.execCommand('insertHTML', false, splitHtml(raw).body);
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
    frame.srcdoc = `<!doctype html><html><head><meta charset="utf-8"><meta data-oc="1" http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data: cid: https: http:; style-src 'unsafe-inline'"><style data-oc="1">${BASE_CSS}</style>${styles}</head><body>${body}</body></html>`;
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
    ]);
  });
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
    btn('link', 'Link einfügen', async () => {
      const sel = doc && doc.getSelection();
      const range = sel && sel.rangeCount ? sel.getRangeAt(0).cloneRange() : null;
      const url = await promptDialog('Adresse:', { title: 'Link einfügen', value: 'https://' });
      if (!url || !/^(https?:|mailto:)/i.test(url)) return;
      frame.contentWindow.focus();
      if (range) { sel.removeAllRanges(); sel.addRange(range); }
      if (!range || range.collapsed) exec('insertHTML', `<a href="${url.replace(/"/g, '&quot;')}">${url.replace(/</g, '&lt;')}</a>`);
      else exec('createLink', url);
    }),
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
      const styles = [...doc.head.querySelectorAll('style:not([data-oc])')].map((s) => s.outerHTML).join('');
      return styles ? `<html><head>${styles}</head><body>${doc.body.innerHTML}</body></html>` : doc.body.innerHTML;
    },
    getText: () => (doc ? doc.body.innerText : ''),
    /** The editable document (null until loaded). */
    doc: () => doc,
    /** Notifies listeners after programmatic changes. */
    changed: () => onInput && onInput(),
    setHtml: async (x) => { pending = x; const p = new Promise((r) => frame.addEventListener('load', r, { once: true })); setContent(x); await p; },
    focus: async () => {
      await ready;
      frame.contentWindow.focus();
      const r = doc.createRange();
      r.setStart(doc.body, 0); r.collapse(true);
      const s = doc.getSelection(); s.removeAllRanges(); s.addRange(r);
    },
  };
}
