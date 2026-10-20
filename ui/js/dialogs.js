// Modal dialogs, menus, toasts – no native alert()/confirm().
import { h, esc } from './util.js';
import { icon } from './icons.js';
import { onBusy, isBusy } from './api.js';

// ---------------- modal ----------------
/**
 * Opens a modal dialog.
 * buttons: [{label, value, primary, cancel}] – clicking resolves with value (or the result
 * of onBeforeClose(value) if provided; return false there to keep the dialog open).
 */
export function modal({ title, body, buttons = [{ label: 'OK', value: true, primary: true }], width = 420, onBeforeClose, className = '' }) {
  return new Promise((resolve) => {
    const overlay = h('div.modal-overlay');
    const footer = h('div.modal-footer');
    const dlg = h('div.modal' + (className ? '.' + className : ''), { role: 'dialog', 'aria-modal': 'true', 'aria-label': title, style: { width: width + 'px' } },
      h('div.modal-title', h('span', title), h('button.modal-x', { title: 'Schließen', html: icon('close', 12), onclick: () => close(undefined, true) })),
      h('div.modal-body', body),
      footer,
    );
    let done = false;
    let closing = false;
    async function close(value, isCancel = false, btn = null) {
      if (done) return;
      if (onBeforeClose && !isCancel) {
        // onBeforeClose may save to the server: one run at a time, spinner on the button.
        if (closing) return;
        closing = true;
        const stop = whileWorking(btn || footer.querySelector('.primary') || footer);
        try {
          const r = await onBeforeClose(value);
          if (r === false) return;
          if (r !== undefined && r !== true) value = r;
        } catch (e) {
          toast(e.message || String(e), 'error');
          return;
        } finally { closing = false; stop(); }
      }
      done = true;
      overlay.remove();
      document.removeEventListener('keydown', onKey, true);
      resolve(isCancel ? cancelValue : value);
    }
    let cancelValue;
    for (const b of buttons) {
      if (b.cancel) cancelValue = b.value;
      const btn = h('button.btn' + (b.primary ? '.primary' : ''), { type: 'button', onclick: () => close(b.value, !!b.cancel, btn) }, b.label);
      if (b.id) btn.id = b.id;
      footer.appendChild(btn);
    }
    function onKey(e) {
      // Only the topmost dialog reacts (nested dialogs, e.g. a confirm inside the settings).
      const overlays = document.querySelectorAll('.modal-overlay');
      if (overlays.length && overlays[overlays.length - 1] !== overlay) return;
      if (e.key === 'Enter' && e.target && e.target.dataset && e.target.dataset.noSubmit) return;
      if (e.key === 'Escape') { e.stopPropagation(); close(cancelValue, true); }
      else if (e.key === 'Enter' && !(e.target && (e.target.tagName === 'TEXTAREA' || e.target.isContentEditable))) {
        const p = buttons.find((b) => b.primary);
        if (p) { e.preventDefault(); e.stopPropagation(); close(p.value); }
      }
    }
    document.addEventListener('keydown', onKey, true);
    overlay.appendChild(dlg);
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) dlg.classList.add('shake'); setTimeout(() => dlg.classList.remove('shake'), 300); });
    document.body.appendChild(overlay);
    const first = dlg.querySelector('input:not([type=hidden]),select,textarea,[contenteditable]');
    (first || footer.querySelector('.primary') || dlg).focus?.();
  });
}

export function confirmDialog(message, { title = 'Bestätigen', ok = 'OK', cancel = 'Abbrechen', danger = false } = {}) {
  return modal({
    title,
    body: h('div.confirm-body', h('span.confirm-icon', { html: icon(danger ? 'warning' : 'info', 32) }), h('div', message)),
    buttons: [{ label: ok, value: true, primary: true }, { label: cancel, value: false, cancel: true }],
  });
}

export function promptDialog(message, { title = 'Eingabe', value = '', ok = 'OK', placeholder = '' } = {}) {
  const input = h('input.input', { type: 'text', value, placeholder, style: { width: '100%' } });
  return modal({
    title,
    body: h('div', h('label.lbl', message), input),
    buttons: [{ label: ok, value: 'ok', primary: true }, { label: 'Abbrechen', value: null, cancel: true }],
    onBeforeClose: () => {
      const v = input.value.trim();
      if (!v) { input.focus(); return false; }
      return v;
    },
  }).then((r) => (r === null || r === undefined ? null : r));
}

// ---------------- toast ----------------
let toastHost;
/** opts: { action: { label, onClick }, title } – optional button (e.g. "Rückgängig") and tooltip. */
export function toast(message, kind = 'info', ms = 4500, opts = {}) {
  if (!toastHost) { toastHost = h('div.toast-host'); document.body.appendChild(toastHost); }
  const actionBtn = opts.action ? h('button.toast-action', { type: 'button', dataset: { testid: 'toast-action' } }, opts.action.label) : null;
  const t = h('div.toast.' + kind, { role: kind === 'error' ? 'alert' : 'status', title: opts.title || null },
    h('span', { html: icon(kind === 'error' ? 'warning' : 'info', 16) }), h('span.toast-msg', message),
    actionBtn,
    h('button.toast-x', { html: icon('close', 10), onclick: () => t.remove() }));
  if (actionBtn) actionBtn.addEventListener('click', () => { t.remove(); Promise.resolve().then(opts.action.onClick).catch(reportError); });
  toastHost.appendChild(t);
  setTimeout(() => t.classList.add('out'), ms);
  setTimeout(() => t.remove(), ms + 400);
}

export function reportError(e) {
  console.error(e);
  toast((e && e.message) || String(e), 'error', 7000);
}

// Unexpected errors (a bug, a failed call nobody awaited) are shown, not only
// logged to a console the user never sees.
window.addEventListener('error', (ev) => {
  if (!ev.error && !ev.message) return;
  // Harmless browser notice, not an error of the app.
  if (/ResizeObserver loop/.test(ev.message || '')) return;
  reportError(ev.error || new Error(ev.message));
});
window.addEventListener('unhandledrejection', (ev) => reportError(ev.reason instanceof Error ? ev.reason : new Error(String(ev.reason))));

// ---------------- menus ----------------
let openMenus = [];
function closeMenus(from = 0) {
  for (const m of openMenus.splice(from)) m.remove();
}
document.addEventListener('mousedown', (e) => {
  if (!openMenus.length) return;
  if (openMenus.some((m) => m.contains(e.target))) return;
  closeMenus();
}, true);
document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && openMenus.length) { closeMenus(); e.stopPropagation(); } }, true);
window.addEventListener('blur', () => closeMenus());
window.addEventListener('resize', () => closeMenus());

/**
 * items: [{label, icon, iconHtml, onClick, disabled, checked, submenu: items | () => items, separator, shortcut, header}]
 */
export function showMenu(items, x, y, level = 0) {
  closeMenus(level);
  const menu = h('div.menu', { role: 'menu' });
  for (const it of items) {
    if (!it) continue;
    if (it.separator) { menu.appendChild(h('div.menu-sep')); continue; }
    if (it.header) { menu.appendChild(h('div.menu-header', it.header)); continue; }
    const row = h('div.menu-item' + (it.disabled ? '.disabled' : ''), { role: 'menuitem', tabindex: '-1' },
      h('span.menu-icon', { html: it.checked ? icon('check', 14) : (it.iconHtml || (it.icon ? icon(it.icon, 16) : '')) }),
      h('span.menu-label', it.label),
      h('span.menu-shortcut', it.shortcut || ''),
      h('span.menu-arrow', { html: it.submenu ? icon('triRight', 10) : '' }),
    );
    if (it.testid) row.dataset.testid = it.testid;
    if (it.submenu) {
      let timer;
      const open = () => {
        const r = row.getBoundingClientRect();
        const sub = typeof it.submenu === 'function' ? it.submenu() : it.submenu;
        showMenu(sub, r.right - 2, r.top - 3, level + 1);
      };
      row.addEventListener('mouseenter', () => { clearTimeout(timer); timer = setTimeout(open, 150); });
      row.addEventListener('mouseleave', () => clearTimeout(timer));
      row.addEventListener('click', (e) => { e.stopPropagation(); open(); });
    } else {
      row.addEventListener('mouseenter', () => closeMenus(level + 1));
      row.addEventListener('click', (e) => {
        e.stopPropagation();
        if (it.disabled) return;
        closeMenus();
        if (it.onClick) Promise.resolve().then(() => it.onClick()).catch(reportError);
      });
    }
    menu.appendChild(row);
  }
  document.body.appendChild(menu);
  const r = menu.getBoundingClientRect();
  let left = x, top = y;
  if (left + r.width > innerWidth - 4) {
    const parent = level ? openMenus[level - 1] : null;
    left = parent ? parent.getBoundingClientRect().left - r.width + 2 : innerWidth - r.width - 4;
  }
  if (left < 4) left = 4;
  if (top + r.height > innerHeight - 4) top = Math.max(4, innerHeight - r.height - 4);
  menu.style.left = left + 'px';
  menu.style.top = top + 'px';
  openMenus[level] = menu;
  return menu;
}

export function showMenuAt(anchor, items) {
  const r = anchor.getBoundingClientRect();
  return showMenu(items, r.left, r.bottom + 1);
}

export function closeAllMenus() { closeMenus(); }

// ---------------- busy indicator ----------------
export function busyIndicator(el) {
  let t;
  return (busy) => {
    clearTimeout(t);
    if (busy) t = setTimeout(() => el.classList.add('on'), 250);
    else el.classList.remove('on');
  };
}

/** Shows/hides the spinner on a button (it takes the place of the leading icon). */
export function setPending(b, on) {
  b.classList.toggle('pending', !!on);
  if (on) b.setAttribute('aria-busy', 'true'); else b.removeAttribute('aria-busy');
  let spin = b.querySelector(':scope > .btn-spin');
  if (on && !spin) { spin = h('span.btn-spin', { 'aria-hidden': 'true' }); b.prepend(spin); }
  else if (!on && spin) spin.remove();
}

/**
 * Spinner on the button while its action runs AND a backend call is in flight –
 * an action that only waits for a dialog or menu shows none. Returns stop().
 */
export function whileWorking(b, delay = 200) {
  let t = null;
  const update = (busy) => {
    clearTimeout(t);
    const on = b.classList.contains('pending');
    // Short gaps between two calls of one action don't make the spinner flicker.
    if (busy && !on) t = setTimeout(() => setPending(b, true), delay);
    else if (!busy && on) t = setTimeout(() => setPending(b, false), 150);
  };
  const off = onBusy(update);
  update(isBusy());
  return () => { off(); clearTimeout(t); setPending(b, false); };
}

/**
 * Floating "… wird geladen" pill for waits without a button to spin on (a dialog
 * that needs server data first, a window preparing its content). Shown only if the
 * wait takes longer than `delay`. Returns stop().
 */
export function loadingPill(text, delay = 300) {
  const pill = h('div.view-loading.on.fixed', { role: 'status', dataset: { testid: 'loading-pill' } }, h('span.busy.on'), text);
  const t = setTimeout(() => document.body.appendChild(pill), delay);
  return () => { clearTimeout(t); pill.remove(); };
}

/**
 * Click handler for a button that runs an async action: further clicks are ignored
 * while it runs (also on the other buttons of `group`, e.g. Annehmen/Ablehnen), and
 * the button shows a spinner while the backend works.
 */
export function busyClick(fn, group = null) {
  return (e) => {
    const b = e.currentTarget;
    const lock = group || b;
    if (lock.dataset.running) return;
    lock.dataset.running = '1';
    const stop = whileWorking(b);
    Promise.resolve().then(() => fn(e)).catch(reportError).finally(() => { delete lock.dataset.running; stop(); });
  };
}

export function field(label, input, hint) {
  return h('label.field', h('span.field-label', label), input, hint ? h('span.field-hint', hint) : null);
}

export { esc };
