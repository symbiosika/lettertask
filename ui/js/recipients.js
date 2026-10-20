// Token input for recipients with autocomplete via suggest_recipients.
import { call } from './api.js';
import { h, esc, debounce, parseAddress, displayName, initials, avatarColor } from './util.js';
import { icon } from './icons.js';

let openPopup = null;
function closePopup() { if (openPopup) { openPopup.remove(); openPopup = null; } }

/**
 * Creates a recipient field.
 * opts: { mailboxId: () => string, initial: Address[], placeholder, onChange, name }
 * returns { el, get(), set(list), commit(), invalid() }
 */
export function recipientField(opts = {}) {
  let list = [...(opts.initial || [])];
  const input = h('input.rcpt-input', { type: 'text', autocomplete: 'off', spellcheck: false, placeholder: opts.placeholder || '', 'aria-label': opts.name || 'Empfänger', dataset: { field: opts.name || '' } });
  const box = h('div.rcpt-box.input', { dataset: { field: opts.name || '' } });
  let suggestions = [], active = -1;

  function render() {
    box.querySelectorAll('.rcpt-token').forEach((t) => t.remove());
    for (const [i, a] of list.entries()) {
      const tok = h('span.rcpt-token' + (a.invalid ? '.invalid' : ''), { title: a.address, tabindex: '-1' },
        h('span', displayName(a)),
        h('button.rcpt-x', { type: 'button', title: 'Entfernen', html: icon('close', 9), onclick: (e) => { e.stopPropagation(); list.splice(i, 1); render(); changed(); } }));
      tok.addEventListener('dblclick', () => { list.splice(i, 1); render(); input.value = a.name && a.name !== a.address ? `${a.name} <${a.address}>` : a.address; input.focus(); changed(); });
      box.insertBefore(tok, input);
    }
  }
  function changed() { opts.onChange && opts.onChange(list); }

  function addRaw(text) {
    let added = false;
    for (const part of text.split(/[;,]\s*(?=(?:[^"]*"[^"]*")*[^"]*$)/)) {
      const t = part.trim();
      if (!t) continue;
      const a = parseAddress(t);
      if (a) list.push(a);
      else list.push({ name: t, address: t, invalid: true });
      added = true;
    }
    if (added) { render(); changed(); }
  }

  function commit() {
    if (input.value.trim()) { addRaw(input.value); input.value = ''; }
    closePopup();
  }

  function showSuggestions() {
    closePopup();
    if (!suggestions.length) return;
    const r = box.getBoundingClientRect();
    const pop = h('div.rcpt-pop', { style: { left: r.left + 'px', top: r.bottom + 4 + 'px', width: Math.max(320, Math.min(420, r.width)) + 'px' }, role: 'listbox' });
    suggestions.forEach((s, i) => {
      const meta = [s.address, s.company].filter(Boolean).map(esc).join(' · ');
      const row = h('div.rcpt-sugg' + (i === active ? '.active' : ''), { role: 'option', dataset: { testid: 'rcpt-sugg' }, html: `<span class="avatar xs" style="background:${avatarColor(s.address)}">${esc(initials(s.name, s.address))}</span><span class="rs-text"><span class="sn">${esc(s.name || s.address)}</span><span class="sa">${meta}</span></span>` });
      row.addEventListener('mousedown', (e) => { e.preventDefault(); pick(s); });
      row.addEventListener('mouseenter', () => { active = i; pop.querySelectorAll('.rcpt-sugg').forEach((r, j) => r.classList.toggle('active', j === i)); });
      pop.appendChild(row);
    });
    document.body.appendChild(pop);
    openPopup = pop;
  }

  function pick(s) {
    list.push({ name: s.name || s.address, address: s.address });
    input.value = '';
    suggestions = []; active = -1;
    closePopup(); render(); changed();
    input.focus();
  }

  const query = debounce(async () => {
    const q = input.value.trim();
    if (q.length < 2) { suggestions = []; closePopup(); return; }
    try {
      const res = await call('suggest_recipients', { mailboxId: opts.mailboxId(), query: q }, { silent: true });
      if (input.value.trim() !== q) return;
      const have = new Set(list.map((a) => a.address.toLowerCase()));
      suggestions = (res || []).filter((s) => !have.has(String(s.address).toLowerCase())).slice(0, 10);
      active = suggestions.length ? 0 : -1;
      if (document.activeElement === input) showSuggestions();
    } catch { /* ignore */ }
  }, 200);

  input.addEventListener('input', () => {
    if (/[;,]$/.test(input.value)) { commit(); return; }
    query();
  });
  input.addEventListener('keydown', (e) => {
    if (openPopup && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      e.preventDefault();
      active = (active + (e.key === 'ArrowDown' ? 1 : -1) + suggestions.length) % suggestions.length;
      showSuggestions();
    } else if (e.key === 'Enter' || e.key === 'Tab') {
      if (openPopup && active >= 0) { e.preventDefault(); pick(suggestions[active]); }
      else if (input.value.trim()) { if (e.key === 'Enter') e.preventDefault(); commit(); }
    } else if (e.key === 'Escape' && openPopup) {
      e.stopPropagation(); closePopup();
    } else if (e.key === 'Backspace' && !input.value && list.length) {
      list.pop(); render(); changed();
    }
  });
  input.addEventListener('blur', () => setTimeout(() => { if (document.activeElement !== input) commit(); }, 150));
  input.addEventListener('paste', (e) => {
    const t = (e.clipboardData || window.clipboardData).getData('text');
    if (/[;,\n]/.test(t) || /<[^>]+@[^>]+>/.test(t)) { e.preventDefault(); addRaw(t.replace(/\r?\n/g, ';')); }
  });
  box.addEventListener('click', () => input.focus());
  box.appendChild(input);
  render();

  return {
    el: box,
    input,
    get: () => { commit(); return list.filter((a) => !a.invalid).map(({ name, address }) => ({ name, address })); },
    set: (l) => { list = [...(l || [])]; render(); },
    invalid: () => { commit(); return list.filter((a) => a.invalid).map((a) => a.address); },
    focus: () => input.focus(),
  };
}
