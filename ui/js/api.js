// Transport to the Rust service layer. See docs/API.md.
const TAURI = window.__TAURI__;
export const isTauri = !!(TAURI && TAURI.core);

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
        if (data.event === 'open-window' && data.payload && !window.opener && location.pathname.match(/(^|\/)(index\.html)?$/)) {
          window.open(`${data.payload.kind}.html?${data.payload.query}`, '_blank', 'width=1000,height=760');
        }
        dispatch(data.event, data.payload);
      } catch (e) { console.warn('bad event', e); }
    };
  } catch (e) { console.warn('no event source', e); }
}

let busyCount = 0;
const busyListeners = new Set();
export function onBusy(cb) { busyListeners.add(cb); }
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

/** Subscribes to a backend event. Returns an unsubscribe function. */
export function on(event, cb) {
  if (!listeners.has(event)) {
    listeners.set(event, new Set());
    if (isTauri) {
      TAURI.event.listen(event, (e) => dispatch(event, e.payload));
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
  if (isTauri) return call('open_window', { kind, params: clean }, { silent: true });
  const q = new URLSearchParams(clean).toString();
  const w = window.open(`${kind}.html?${q}`, '_blank', 'width=1000,height=760');
  return w ? null : null;
}

export async function openUrl(url) {
  if (isTauri) return call('open_url', { url }, { silent: true });
  window.open(url, '_blank', 'noopener');
}

/** Closes the current window (used by compose/message/event windows). */
export async function closeWindow() {
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
