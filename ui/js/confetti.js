// Small, calm confetti burst for finished work (archive, "Erledigt", empty inbox).
// No dependencies; skipped with prefers-reduced-motion or when disabled in the settings.
import { load, h } from './util.js';
import { icon } from './icons.js';

export function confettiEnabled() { return load('confetti', true) !== false; }

function reducedMotion() {
  try { return window.matchMedia('(prefers-reduced-motion: reduce)').matches; } catch { return false; }
}

function colors() {
  const accent = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() || '#2f6fe4';
  return [accent, accent, '#9db7e8', '#f2c94c', '#8fd3b6'];
}

/** Point (viewport coords) from an element, a DOMRect or {x, y}; falls back to the screen centre. */
function originOf(src) {
  if (!src) return { x: innerWidth / 2, y: innerHeight / 2 };
  if (typeof src.x === 'number' && typeof src.width !== 'number') return src;
  const r = src.getBoundingClientRect ? src.getBoundingClientRect() : src;
  if (!r || (!r.width && !r.height)) return { x: innerWidth / 2, y: innerHeight / 2 };
  return { x: r.left + Math.min(r.width / 2, 60), y: r.top + r.height / 2 };
}

/** Burst from `src` (element / rect / point). opts: { count, power, duration } */
export function burst(src, { count = 40, power = 1, duration = 900 } = {}) {
  if (!confettiEnabled() || reducedMotion()) return false;
  const { x, y } = originOf(src);
  const canvas = h('canvas.confetti', { 'aria-hidden': 'true', dataset: { testid: 'confetti' } });
  const dpr = window.devicePixelRatio || 1;
  canvas.width = innerWidth * dpr;
  canvas.height = innerHeight * dpr;
  Object.assign(canvas.style, { position: 'fixed', inset: '0', width: '100vw', height: '100vh', pointerEvents: 'none', zIndex: '4000' });
  document.body.appendChild(canvas);
  const ctx = canvas.getContext('2d');
  if (!ctx) { canvas.remove(); return false; }
  ctx.scale(dpr, dpr);
  const pal = colors();
  const parts = Array.from({ length: count }, () => {
    const a = -Math.PI / 2 + (Math.random() - 0.5) * Math.PI * 1.1;
    const v = (2.2 + Math.random() * 3.2) * power;
    return {
      x, y, vx: Math.cos(a) * v, vy: Math.sin(a) * v, rot: Math.random() * Math.PI, vr: (Math.random() - 0.5) * 0.3,
      w: 3 + Math.random() * 3, h: 5 + Math.random() * 4, c: pal[Math.floor(Math.random() * pal.length)],
    };
  });
  const start = performance.now();
  const frame = (now) => {
    const t = now - start;
    ctx.clearRect(0, 0, innerWidth, innerHeight);
    const fade = Math.max(0, 1 - t / duration);
    for (const p of parts) {
      p.vy += 0.12; p.vx *= 0.985; p.vy *= 0.985;
      p.x += p.vx; p.y += p.vy; p.rot += p.vr;
      ctx.save();
      ctx.globalAlpha = fade;
      ctx.translate(p.x, p.y);
      ctx.rotate(p.rot);
      ctx.fillStyle = p.c;
      ctx.fillRect(-p.w / 2, -p.h / 2, p.w, p.h);
      ctx.restore();
    }
    if (t < duration) requestAnimationFrame(frame); else canvas.remove();
  };
  requestAnimationFrame(frame);
  return true;
}

/** Larger but still calm celebration plus a quiet centred note (empty inbox). */
export function celebrateEmptyInbox(host) {
  if (!confettiEnabled()) return;
  const r = host && host.getBoundingClientRect ? host.getBoundingClientRect() : null;
  const pt = r ? { x: r.left + r.width / 2, y: r.top + r.height * 0.4 } : null;
  burst(pt, { count: 80, power: 1.35, duration: 1300 });
  const note = h('div.inbox-zero-note', { role: 'status', dataset: { testid: 'inbox-zero' } },
    h('span.iz-ic', { html: icon('party', 22) }), h('span', 'Posteingang leer'));
  if (pt) Object.assign(note.style, { left: pt.x + 'px', top: pt.y + 'px' });
  document.body.appendChild(note);
  setTimeout(() => note.classList.add('out'), 1800);
  setTimeout(() => note.remove(), 2300);
}
