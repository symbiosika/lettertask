// Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y in the Linux app. WebKitGTK binds no keys to
// undo (a browser does that itself), so the window catches them natively and
// sends "edit-undo" { label, redo } (src-tauri). The rich-text editor undoes
// its own steps (editor.js); text fields keep a short history of their own:
// WebKit's native history is shared by the whole page, so there it would first
// replay the editor's typing before reaching the field.
import { isTauri, on, currentWindow } from './api.js';

const PAUSE_MS = 1000;
const MAX = 100;

// Editors by their frame (the frame is the window's focused element while one is edited).
const editors = new Map();
export function registerEditor(frame, ed) { editors.set(frame, ed); }

const isField = (el) => el && (el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && /^(text|search|email|url|tel|password|number|)$/i.test(el.type || '')));
const fields = new WeakMap(); // element → { stack, idx, last, dir }
let restoring = false;

const snap = (el) => {
  let a = null, b = null;
  try { a = el.selectionStart; b = el.selectionEnd; } catch { /* number fields */ }
  return { value: el.value, a, b };
};
function historyOf(el) {
  let h = fields.get(el);
  if (!h) { h = { stack: [snap(el)], idx: 0, last: 0, dir: 0 }; fields.set(el, h); }
  return h;
}
function record(el) {
  const h = historyOf(el);
  const cur = snap(el);
  const top = h.stack[h.idx];
  if (cur.value === top.value) return;
  const now = Date.now();
  const d = Math.sign(cur.value.length - top.value.length);
  if (h.idx > 0 && h.last && now - h.last < PAUSE_MS && (d === h.dir || !d || !h.dir)) {
    h.stack[h.idx] = cur;
    if (d) h.dir = d;
  } else {
    h.stack.length = h.idx + 1;
    h.stack.push(cur);
    if (h.stack.length > MAX) h.stack.shift();
    h.idx = h.stack.length - 1;
    h.dir = d;
  }
  h.last = now;
}
function step(el, redo) {
  const h = historyOf(el);
  // Changed by the app without an input event (e.g. a recipient taken over): that is the latest state.
  if (el.value !== h.stack[h.idx].value) { h.last = 0; record(el); }
  const to = h.idx + (redo ? 1 : -1);
  if (to < 0 || to >= h.stack.length) return;
  const leaving = h.stack[h.idx];
  h.idx = to;
  const s = h.stack[to];
  restoring = true;
  try {
    el.value = s.value;
    const pos = redo ? s : leaving;
    if (pos.a != null) {
      try { el.setSelectionRange(Math.min(pos.a, s.value.length), Math.min(pos.b, s.value.length)); } catch { /* number fields */ }
    }
    el.dispatchEvent(new Event('input', { bubbles: true }));
  } finally { restoring = false; }
  h.last = 0;
}

let installed = false;
export function installUndoKeys() {
  if (installed || !isTauri) return;
  installed = true;
  const w = currentWindow();
  if (!w) return;
  // The field's state before the first change, then every change.
  document.addEventListener('focusin', (e) => { if (isField(e.target)) historyOf(e.target); }, true);
  document.addEventListener('input', (e) => { if (!restoring && isField(e.target)) record(e.target); }, true);
  on('edit-undo', (p) => {
    if (!p || p.label !== w.label) return;
    const el = document.activeElement;
    const ed = editors.get(el);
    if (ed) { if (p.redo) ed.redo(); else ed.undo(); return; }
    if (isField(el) && !el.readOnly) step(el, !!p.redo);
  });
}

installUndoKeys();
