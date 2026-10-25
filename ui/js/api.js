// Transport to the Rust service layer. See docs/API.md.

// On the phone the compose/message/event/task "windows" are full-screen frames
// inside the main page (see mobile.js). They always use the main page's
// bridge: Android injects Tauri into frames too, but delivers the answers to
// the main page only, so a frame's own calls would never return.
export const framed = window.parent !== window;
function parentTauri() { try { return framed ? window.parent.__TAURI__ : null; } catch { return null; } }
const TAURI = parentTauri() || window.__TAURI__;
export const isTauri = !!(TAURI && TAURI.core);

/** Phone layout: Android/iOS app, a phone browser, or `?mobile=1` (tests). */
export const isMobile = (() => {
  try {
    if (framed && window.parent.document.documentElement.classList.contains('mobile')) return true;
  } catch { /* other origin */ }
  return /Android|iPhone|iPod/i.test(navigator.userAgent) || new URLSearchParams(location.search).has('mobile');
})();
if (isMobile) document.documentElement.classList.add('mobile');

// A page shown as a full-screen layer on the phone gets a close button top left
// (the window has no title bar). The page may object to closing (unsaved changes).
if (isMobile && framed) {
  document.documentElement.classList.add('framed');
  const addClose = () => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'm-frame-close';
    b.title = 'Schließen';
    b.setAttribute('aria-label', 'Schließen');
    b.dataset.testid = 'm-frame-close';
    b.innerHTML = '<svg width="22" height="22" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M5.5 5.5l9 9M14.5 5.5l-9 9"/></svg>';
    b.addEventListener('click', async () => {
      const ask = window.__ltBeforeClose;
      if (ask && !(await ask())) return;
      closeWindow();
    });
    document.body.appendChild(b);
  };
  if (document.body) addClose(); else document.addEventListener('DOMContentLoaded', addClose);
}

/** The main page's shell (layers, sheets) when running on the phone. */
export function shell() {
  try { return isMobile ? window.top.__lt || null : null; } catch { return null; }
}

/** The app window of this page – none for a frame, its "window" is the main page's. */
export function currentWindow() {
  if (framed || !isTauri || !TAURI.window || !TAURI.window.getCurrentWindow) return null;
  try { return TAURI.window.getCurrentWindow(); } catch { return null; }
}

// The WebView's own context menu ("Neu laden", "Untersuchen", …) is not part of
// the app. It stays only where it helps: text fields and selected text (copy/paste).
export function suppressNativeMenu(target) {
  if (!isTauri) return;
  target.addEventListener('contextmenu', (e) => {
    if (e.defaultPrevented) return;
    const t = e.target && e.target.closest ? e.target : null;
    if (t && t.closest('input, textarea, [contenteditable=""], [contenteditable="true"]')) return;
    const doc = t ? t.ownerDocument : document;
    if (doc.designMode === 'on') return;
    const sel = (doc.defaultView || window).getSelection();
    if (sel && !sel.isCollapsed && String(sel).trim()) return;
    e.preventDefault();
  });
}
suppressNativeMenu(window);

const listeners = new Map(); // event -> Set(cb)
let eventSource = null;
// Browser mode: connect early so backend-initiated windows (open-window) are received.
if (!isTauri) setTimeout(() => ensureEventSource(), 0);

function dispatch(event, payload) {
  const set = listeners.get(event);
  if (set) for (const cb of set) { try { cb(payload); } catch (e) { console.error(e); } }
}

function ensureEventSource() {
  if (isTauri || eventSource) return;
  try {
    eventSource = new EventSource('/api/events');
    eventSource.onmessage = (m) => {
      try {
        const data = JSON.parse(m.data);
        // Backend-initiated windows (e.g. mailto: links) – only the main window opens them.
        if (data.event === 'open-window' && data.payload && !window.opener && !framed && !isMobile && location.pathname.match(/(^|\/)(index\.html)?$/)) {
          window.open(`${data.payload.kind}.html?${data.payload.query}`, '_blank', 'width=1000,height=760');
        }
        dispatch(data.event, data.payload);
      } catch (e) { console.warn('bad event', e); }
    };
  } catch (e) { console.warn('no event source', e); }
}

let busyCount = 0;
const busyListeners = new Set();
/** Subscribes to the "a visible backend call is running" state. Returns an unsubscribe function. */
export function onBusy(cb) { busyListeners.add(cb); return () => busyListeners.delete(cb); }
export function isBusy() { return busyCount > 0; }
function setBusy(delta) {
  busyCount = Math.max(0, busyCount + delta);
  for (const cb of busyListeners) cb(busyCount > 0);
}

/** Calls a backend command. Rejects with an Error whose message is human readable. */
export async function call(cmd, args = {}, { silent = false } = {}) {
  if (!silent) setBusy(1);
  try {
    if (isTauri) {
      try {
        return await TAURI.core.invoke('call', { cmd, args });
      } catch (e) {
        throw new Error(typeof e === 'string' ? e : (e && e.message) || String(e));
      }
    }
    let res;
    try {
      res = await fetch('/api/call/' + encodeURIComponent(cmd), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(args ?? {}),
      });
    } catch (e) {
      throw new Error('Keine Verbindung zum Dienst: ' + e.message);
    }
    const text = await res.text();
    let data = null;
    if (text) { try { data = JSON.parse(text); } catch { data = text; } }
    if (!res.ok) throw new Error((data && data.error) || `Fehler ${res.status}`);
    return data;
  } finally {
    if (!silent) setBusy(-1);
  }
}

const unlistens = [];
// A frame's listeners live in the main page's bridge: drop them with the frame.
if (framed) window.addEventListener('pagehide', () => { for (const u of unlistens.splice(0)) Promise.resolve(u).then((f) => f && f()).catch(() => {}); });

/** Subscribes to a backend event. Returns an unsubscribe function. */
export function on(event, cb) {
  if (!listeners.has(event)) {
    listeners.set(event, new Set());
    if (isTauri) {
      unlistens.push(TAURI.event.listen(event, (e) => dispatch(event, e.payload)));
    }
  }
  listeners.get(event).add(cb);
  ensureEventSource();
  return () => listeners.get(event).delete(cb);
}

/** Opens a separate window (compose/message/event). */
export async function openWindow(kind, params = {}) {
  const clean = {};
  for (const [k, v] of Object.entries(params)) if (v !== null && v !== undefined) clean[k] = String(v);
  const sh = shell();
  if (sh) { sh.openSheet(kind, new URLSearchParams(clean).toString()); return null; }
  if (isTauri) return call('open_window', { kind, params: clean }, { silent: true });
  const q = new URLSearchParams(clean).toString();
  const w = window.open(`${kind}.html?${q}`, '_blank', 'width=1000,height=760');
  return w ? null : null;
}

export async function openUrl(url) {
  if (isTauri) return call('open_url', { url }, { silent: true });
  // Same rule as the backend: only web links leave the app.
  if (!/^https?:\/\//i.test(String(url || '').trim())) throw new Error('Nur http(s)-Links können geöffnet werden.');
  window.open(String(url).trim(), '_blank', 'noopener');
}

/** Closes the current window (used by compose/message/event windows). */
export async function closeWindow() {
  const sh = shell();
  if (framed && sh) { sh.closeSheet(window); return; }
  try {
    if (isTauri && TAURI.window && TAURI.window.getCurrentWindow) {
      await TAURI.window.getCurrentWindow().close();
      return;
    }
  } catch (e) { console.warn(e); }
  window.close();
}

export function params() {
  return Object.fromEntries(new URLSearchParams(location.search));
}
