// Interface language. Russian is the source language of the code: every UI string is written in
// Russian and translated on its way to the screen (dc-runtime, toasts, console, reader), so the
// logic never has to know which language is on.
//
// Lookup order for a string: exact match → a pattern from en.js ("Скачано {1} из {2} МБ", with the
// captured parts translated again) → a number + a word ("14 модов") → piece by piece
// (" · ", line breaks, sentence ends).

import en from './en.js';

const KEY = 'signal.lang';
const SENTENCE_BREAK = /[.!?…]\s|\n| · /;
const CYRILLIC = /[Ѐ-ӿ]/;

export const LANGS = [['ru', 'Русский'], ['en', 'English']];

export const lang = (() => {
  try {
    return localStorage.getItem(KEY) === 'en' ? 'en' : 'ru';
  } catch (e) {
    return 'ru';
  }
})();

/** For dates and numbers. */
export const locale = lang === 'en' ? 'en-US' : 'ru-RU';

/** Switches the language; the window reloads so every part of the UI picks it up. */
export function setLang(next) {
  if (next === lang) return;
  try { localStorage.setItem(KEY, next === 'en' ? 'en' : 'ru'); } catch (e) {}
  window.location.reload();
}

// Keys are written the way the code glues them (' · вышла {1}', 'Установленные '); lookups use the
// trimmed text, so every key is also registered without edge spaces and a leading ' · '.
const variants = (ru, to) => {
  const out = [[ru, to]];
  const bare = (s) => s.replace(/^[\s·,]+/, '').replace(/[\s,:]+$/, '');
  if (bare(ru) && bare(ru) !== ru) out.push([bare(ru), bare(to)]);
  return out;
};
const exact = new Map();
Object.entries(en.exact).forEach(([ru, to]) => variants(ru, to).forEach(([k, v]) => { if (!exact.has(k)) exact.set(k, v); }));
const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
// Most specific first: the more fixed text a pattern has, the earlier it is tried.
const patterns = en.patterns
  .flatMap(([ru, to]) => variants(ru, to))
  .filter(([ru], i, all) => all.findIndex(([r]) => r === ru) === i)
  .map(([ru, to]) => {
    const order = [];
    const source = ru.split(/(\{\d+\})/).map((part) => {
      const m = part.match(/^\{(\d+)\}$/);
      if (!m) return escapeRe(part);
      order.push(m[1]);
      return '([\\s\\S]*?)';
    }).join('');
    const fixed = ru.replace(/\{\d+\}/g, '');
    return { re: new RegExp('^' + source + '$'), order, to, weight: fixed.length, spansSentences: SENTENCE_BREAK.test(fixed) };
  })
  .sort((a, b) => b.weight - a.weight);

const cache = new Map();

/** Russian UI text → the current language (anything without Cyrillic passes through). */
export function t(text) {
  if (lang === 'ru' || typeof text !== 'string' || !CYRILLIC.test(text)) return text;
  const hit = cache.get(text);
  if (hit !== undefined) return hit;
  const out = translate(text);
  if (cache.size > 5000) cache.clear();
  cache.set(text, out);
  return out;
}

function translate(text) {
  const [, lead, core, tail] = text.match(/^(\s*)([\s\S]*?)(\s*)$/);
  const r = whole(core) ?? peeled(core) ?? pieces(core);
  return r == null ? text : lead + r + tail;
}

// A known phrase inside wrappers: «…», (…), a leading «· » or a trailing period.
function peeled(core) {
  const q = core.match(/^«([\s\S]+)»$/);
  if (q) return '“' + t(q[1]) + '”';
  const m = core.match(/^([(·—\s]*)([\s\S]*?)([).,:;!?…\s]*)$/);
  if (!m || (!m[1] && !m[3]) || !m[2]) return null;
  const r = whole(m[2]);
  return r == null ? null : m[1] + r + m[3];
}

function whole(core) {
  if (exact.has(core)) return exact.get(core);
  for (const p of patterns) {
    const m = p.re.exec(core);
    if (!m) continue;
    // «Скачано {1}» must not swallow three more sentences: those are translated one by one.
    if (!p.spansSentences && m.slice(1).some((part) => SENTENCE_BREAK.test(part))) continue;
    const parts = {};
    p.order.forEach((n, i) => { parts[n] = t(m[i + 1]); });
    return p.to.replace(/\{(\d+)\}/g, (_, n) => parts[n] ?? '');
  }
  // «14 модов», «3 файла»
  const num = core.match(/^([\d\s.,]+)\s+([Ѐ-ӿ]+)$/);
  if (num && exact.has(num[2])) return num[1] + ' ' + exact.get(num[2]);
  return null;
}

// Messages glued from independent parts: first sentences, lines and « · » parts; a part that is
// still unknown is then split at « — », and after that at «, » and «: ».
const LEVELS = [/( · |\n|(?<=[.!?…])\s+)/, /( — )/, /(, |: )/];

function pieces(core, level = 0) {
  const sep = LEVELS[level];
  if (!sep) return null;
  const parts = core.split(sep);
  if (parts.length < 2) return pieces(core, level + 1);
  let changed = false;
  const out = parts.map((p, i) => {
    if (i % 2 === 1 || !CYRILLIC.test(p)) return p;
    const r = whole(p) ?? peeled(p) ?? pieces(p, level + 1);
    if (r == null) return p;
    changed = true;
    return r;
  });
  return changed ? out.join('') : null;
}

/** Text nodes and the readable attributes of a DOM subtree (markup built in JS). */
export function translateDom(root) {
  if (lang === 'ru' || !root) return root;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (CYRILLIC.test(n.nodeValue)) n.nodeValue = t(n.nodeValue);
  }
  const els = root.querySelectorAll ? [root, ...root.querySelectorAll('[aria-label],[placeholder],[title],[alt]')] : [];
  els.forEach((el) => {
    ['aria-label', 'placeholder', 'title', 'alt'].forEach((a) => {
      const v = el.getAttribute && el.getAttribute(a);
      if (v && CYRILLIC.test(v)) el.setAttribute(a, t(v));
    });
  });
  return root;
}

/** Russian plural forms → the right word in the current language. */
export function plural(n, one, few, many) {
  if (lang === 'en') return t(Math.abs(n) === 1 ? one : many);
  const a = n % 10;
  const b = n % 100;
  if (a === 1 && b !== 11) return one;
  if (a >= 2 && a <= 4 && (b < 12 || b > 14)) return few;
  return many;
}

/** 2.7 → «2,7» in Russian, «2.7» in English. */
export function decimal(x, digits = 1) {
  const s = Number(x).toFixed(digits);
  return lang === 'ru' ? s.replace('.', ',') : s;
}
