// Patch notes reader. The body is HTML from Mojang's feed; it is rebuilt node by node from a
// tag allowlist (no attributes except safe link targets), because a script injected into this
// window could reach the launcher's Rust commands.

const BLOCK = { P: 'p', H1: 'h3', H2: 'h3', H3: 'h4', H4: 'h5', H5: 'h5', H6: 'h5', UL: 'ul', OL: 'ol', LI: 'li', PRE: 'pre', BLOCKQUOTE: 'blockquote' };
const INLINE = { STRONG: 'strong', B: 'strong', EM: 'em', I: 'em', CODE: 'code', BR: 'br' };
const SKIP = new Set(['SCRIPT', 'STYLE', 'IFRAME', 'OBJECT', 'EMBED', 'IMG', 'VIDEO', 'AUDIO', 'SVG', 'FORM', 'INPUT', 'BUTTON', 'TEMPLATE', 'NOSCRIPT']);

import { t, translateDom } from './i18n/index.js';

let ui = null;
let linkOpener = null;
let lastFocus = null;

/** Untrusted HTML → a DocumentFragment of allowed elements and text only. */
export function sanitize(html) {
  const doc = new DOMParser().parseFromString(String(html), 'text/html');
  const out = document.createDocumentFragment();
  const walk = (node, parent) => {
    node.childNodes.forEach((child) => {
      if (child.nodeType === Node.TEXT_NODE) {
        parent.appendChild(document.createTextNode(child.textContent));
        return;
      }
      if (child.nodeType !== Node.ELEMENT_NODE || SKIP.has(child.tagName)) return;
      const tag = BLOCK[child.tagName] || INLINE[child.tagName];
      if (child.tagName === 'A') {
        const href = child.getAttribute('href') || '';
        const link = document.createElement('a');
        link.className = 'nr-link';
        // Never a live href: clicks go through the backend's host allowlist.
        link.href = '#';
        link.dataset.url = /^https:\/\//i.test(href) ? href : '';
        walk(child, link);
        parent.appendChild(link);
        return;
      }
      if (!tag) {
        walk(child, parent); // unknown wrapper (div, span…): keep its text
        return;
      }
      const el = document.createElement(tag);
      if (tag !== 'br') walk(child, el);
      parent.appendChild(el);
    });
  };
  walk(doc.body, out);
  return out;
}

function build() {
  if (ui) return ui;
  const root = document.createElement('div');
  root.className = 'nr-veil';
  root.hidden = true;
  root.innerHTML = `
    <article class="nr" role="dialog" aria-modal="true" aria-labelledby="nr-title">
      <header class="nr-head">
        <span class="nr-meta"></span>
        <h2 id="nr-title" class="nr-title"></h2>
        <button type="button" class="nr-x" aria-label="Закрыть">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"></path></svg>
        </button>
      </header>
      <div class="nr-body" tabindex="0"></div>
      <footer class="nr-foot">Текст — из официальной ленты Mojang (на английском).</footer>
    </article>`;
  translateDom(root);
  document.body.appendChild(root);
  ui = { root, meta: root.querySelector('.nr-meta'), title: root.querySelector('.nr-title'), body: root.querySelector('.nr-body'), close: root.querySelector('.nr-x') };
  ui.close.addEventListener('click', close);
  root.addEventListener('click', (e) => {
    if (e.target === root) close();
    const a = e.target.closest('.nr-link');
    if (a) {
      e.preventDefault();
      if (a.dataset.url && linkOpener) linkOpener(a.dataset.url);
    }
  });
  root.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });
  return ui;
}

function close() {
  if (!ui) return;
  ui.root.hidden = true;
  if (lastFocus && lastFocus.focus) lastFocus.focus();
}

export const newsReader = {
  configure({ openLink }) {
    linkOpener = openLink;
  },

  /** `load()` resolves to the article HTML. */
  open({ title, meta, load }) {
    build();
    lastFocus = document.activeElement;
    ui.title.textContent = title;
    ui.meta.textContent = t(meta || '');
    ui.body.replaceChildren(Object.assign(document.createElement('p'), { className: 'nr-wait', textContent: t('Загружаю патчноуты…') }));
    ui.root.hidden = false;
    ui.close.focus();
    load()
      .then((html) => { ui.body.replaceChildren(sanitize(html)); ui.body.scrollTop = 0; })
      .catch((e) => ui.body.replaceChildren(Object.assign(document.createElement('p'), { className: 'nr-wait', textContent: t('Не удалось загрузить: ' + e) })));
  },

  close
};
