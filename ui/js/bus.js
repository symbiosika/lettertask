// Tiny cross-window message bus (task window → main window toasts).
// BroadcastChannel where available, localStorage "storage" events as fallback.
const NAME = 'oc-bus';
const KEY = 'oc-bus-msg';
let channel = null;
try { channel = new BroadcastChannel(NAME); } catch { channel = null; }

export function broadcast(msg) {
  const m = { ...msg, _id: Math.random().toString(36).slice(2), _t: Date.now() };
  if (channel) { try { channel.postMessage(m); } catch { /* ignore */ } }
  try { localStorage.setItem(KEY, JSON.stringify(m)); } catch { /* ignore */ }
}

export function onBroadcast(cb) {
  const seen = new Set();
  const handle = (m) => {
    if (!m || !m._id || seen.has(m._id)) return;
    seen.add(m._id);
    cb(m);
  };
  if (channel) channel.addEventListener('message', (e) => handle(e.data));
  window.addEventListener('storage', (e) => {
    if (e.key !== KEY || !e.newValue) return;
    try { handle(JSON.parse(e.newValue)); } catch { /* ignore */ }
  });
}
