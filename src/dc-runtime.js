// Minimal renderer for the design's template format (the markup exported from the
// Signal Launcher canvas). It understands:
//   {{ path }}                       dotted lookups into renderVals() / loop variables
//   <sc-if value="{{ cond }}">       conditional blocks
//   <sc-for list="{{ items }}" as="item">   loops ($index is available inside)
//   onClick="{{ handler }}", ref="{{ fn }}"  events and refs
//   <helmet>                         styles moved into <head>
// and renders it with React, so the component logic from the design runs unchanged.
import React from 'react';
import { createRoot } from 'react-dom/client';
import { t } from './i18n/index.js';

const h = React.createElement;
const SVG_NS = 'http://www.w3.org/2000/svg';
const WHOLE_HOLE = /^\s*\{\{\s*([^}]+?)\s*\}\}\s*$/;
const ANY_HOLE = /\{\{\s*([^}]+?)\s*\}\}/g;

// HTML parsing lower-cases attribute names, so map them back to React's prop names.
const EVENT_NAMES = [
  'onClick', 'onDoubleClick', 'onChange', 'onInput', 'onSubmit', 'onKeyDown', 'onKeyUp',
  'onFocus', 'onBlur', 'onMouseEnter', 'onMouseLeave', 'onMouseOver', 'onMouseOverCapture',
  'onClickCapture', 'onPointerDown', 'onPointerMove', 'onPointerUp', 'onPointerCancel',
  'onDragOver', 'onDragLeave', 'onDrop', 'onWheel', 'onContextMenu'
];
const PROP_NAMES = {
  class: 'className', for: 'htmlFor', tabindex: 'tabIndex', spellcheck: 'spellCheck',
  autoplay: 'autoPlay', playsinline: 'playsInline', maxlength: 'maxLength', minlength: 'minLength',
  autocomplete: 'autoComplete', readonly: 'readOnly', crossorigin: 'crossOrigin', srcset: 'srcSet',
  colspan: 'colSpan', rowspan: 'rowSpan', enctype: 'encType', inputmode: 'inputMode'
};
EVENT_NAMES.forEach((name) => { PROP_NAMES[name.toLowerCase()] = name; });

const camel = (s) => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
// Attributes people read (or hear): translated like text. Everything else (class, style, value…)
// is left alone.
const READABLE = new Set(['aria-label', 'placeholder', 'title', 'alt', 'aria-valuetext']);
const tr = (v) => (typeof v === 'string' ? t(v) : v);

function parseLiteral(expr) {
  if (expr === 'true') return { ok: true, value: true };
  if (expr === 'false') return { ok: true, value: false };
  if (expr === 'null') return { ok: true, value: null };
  if (expr === 'undefined') return { ok: true, value: undefined };
  if (/^-?\d+(\.\d+)?$/.test(expr)) return { ok: true, value: Number(expr) };
  if (/^(['"]).*\1$/.test(expr)) return { ok: true, value: expr.slice(1, -1) };
  return { ok: false };
}

function lookup(expr, scope) {
  const lit = parseLiteral(expr);
  if (lit.ok) return lit.value;
  const parts = expr.split('.');
  let cur = Object.prototype.hasOwnProperty.call(scope.locals, parts[0]) ? scope.locals[parts[0]] : scope.vals[parts[0]];
  for (let i = 1; i < parts.length && cur != null; i++) cur = cur[parts[i]];
  return cur;
}

// `readable`: the static text is translated once here, and every value filled in at render time
// goes through the translator too (the logic writes Russian; i18n/index.js does the rest).
function compileValue(raw, readable) {
  if (readable) raw = t(raw);
  const whole = raw.match(WHOLE_HOLE);
  if (whole) {
    const expr = whole[1];
    return readable ? (scope) => tr(lookup(expr, scope)) : (scope) => lookup(expr, scope);
  }
  if (!raw.includes('{{')) return () => raw;
  const pieces = [];
  let last = 0;
  raw.replace(ANY_HOLE, (m, expr, idx) => {
    pieces.push(raw.slice(last, idx));
    pieces.push({ expr });
    last = idx + m.length;
    return m;
  });
  pieces.push(raw.slice(last));
  return (scope) => pieces.map((p) => {
    if (typeof p === 'string') return p;
    const v = lookup(p.expr, scope);
    return v == null ? '' : (readable ? t(String(v)) : String(v));
  }).join('');
}

// JSX-like whitespace handling: drop indentation-only text, collapse line breaks.
function cleanText(text) {
  if (!text.includes('\n')) return text;
  const lines = text.split(/\r?\n/);
  const out = [];
  lines.forEach((line, i) => {
    let l = line.replace(/\t/g, ' ');
    if (i > 0) l = l.replace(/^\s+/, '');
    if (i < lines.length - 1) l = l.replace(/\s+$/, '');
    if (l) out.push(l);
  });
  return out.join(' ');
}

function parseStyle(text) {
  const style = {};
  let depth = 0;
  let start = 0;
  const decls = [];
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === '(') depth++;
    else if (c === ')') depth--;
    else if (c === ';' && depth === 0) { decls.push(text.slice(start, i)); start = i + 1; }
  }
  decls.push(text.slice(start));
  decls.forEach((d) => {
    const k = d.indexOf(':');
    if (k < 0) return;
    const prop = d.slice(0, k).trim();
    const value = d.slice(k + 1).trim();
    if (!prop) return;
    if (prop.startsWith('--')) style[prop] = value;
    else if (prop.startsWith('-webkit-')) style['Webkit' + camel(prop.slice(8)).replace(/^./, (c) => c.toUpperCase())] = value;
    else style[camel(prop)] = value;
  });
  return style;
}

function compileNode(node) {
  if (node.nodeType === 3) {
    const text = cleanText(node.nodeValue);
    if (!text) return null;
    const get = compileValue(text, true);
    return { kind: 'text', get };
  }
  if (node.nodeType !== 1) return null;
  const tag = node.tagName.toLowerCase();
  const children = () => Array.from(node.childNodes).map(compileNode).filter(Boolean);
  if (tag === 'helmet') return null;
  if (tag === 'sc-if') {
    return { kind: 'if', cond: compileValue(node.getAttribute('value') || ''), children: children() };
  }
  if (tag === 'sc-for') {
    return { kind: 'for', list: compileValue(node.getAttribute('list') || ''), as: node.getAttribute('as') || 'item', children: children() };
  }
  const isSvg = node.namespaceURI === SVG_NS;
  const attrs = [];
  Array.from(node.attributes).forEach((a) => {
    const raw = a.name;
    if (raw.startsWith('hint-') || raw === 'xmlns') return;
    let name;
    if (raw.startsWith('data-') || raw.startsWith('aria-')) name = raw;
    else if (PROP_NAMES[raw]) name = PROP_NAMES[raw];
    else if (isSvg && raw.includes('-')) name = camel(raw);
    else name = raw;
    attrs.push({ name, get: compileValue(a.value, READABLE.has(raw)) });
  });
  return { kind: 'el', tag: isSvg ? node.tagName : tag, attrs, children: children() };
}

function renderList(nodes, scope, keyPrefix) {
  const out = [];
  nodes.forEach((n, i) => {
    const r = renderNode(n, scope, keyPrefix + i);
    if (Array.isArray(r)) out.push(...r);
    else if (r != null) out.push(r);
  });
  return out;
}

function renderNode(n, scope, key) {
  if (n.kind === 'text') {
    const v = n.get(scope);
    return v == null || v === false ? null : String(v);
  }
  if (n.kind === 'if') {
    return n.cond(scope) ? h(React.Fragment, { key }, ...renderList(n.children, scope, key + '.')) : null;
  }
  if (n.kind === 'for') {
    const list = n.list(scope) || [];
    return list.map((item, index) => {
      const locals = Object.assign({}, scope.locals, { [n.as]: item, $index: index });
      return h(React.Fragment, { key: key + ':' + index }, ...renderList(n.children, { vals: scope.vals, locals }, key + ':' + index + '.'));
    });
  }
  const props = { key };
  n.attrs.forEach((a) => {
    let v = a.get(scope);
    if (a.name === 'style') { if (typeof v === 'string') v = parseStyle(v); }
    else if (a.name === 'ref') { if (typeof v !== 'function') return; }
    else if (a.name === 'value' && v == null) v = '';
    props[a.name] = v;
  });
  return h(n.tag, props, ...renderList(n.children, scope, key + '.'));
}

/**
 * Mount a design component.
 * @param {object} opts
 * @param {string} opts.template  full .dc.html markup (or just the <x-dc> part)
 * @param {(DCLogic: typeof React.Component) => typeof React.Component} opts.defineLogic
 * @param {HTMLElement} opts.container
 * @param {object} [opts.props]
 */
export function mountDesign({ template, defineLogic, container, props = {} }) {
  const doc = new DOMParser().parseFromString(template, 'text/html');
  const root = doc.querySelector('x-dc') || doc.body;
  const helmet = root.querySelector('helmet');
  if (helmet) {
    Array.from(helmet.children).forEach((el) => {
      if (el.tagName.toLowerCase() === 'style') {
        const style = document.createElement('style');
        style.textContent = el.textContent;
        document.head.appendChild(style);
      }
    });
  }
  const ast = Array.from(root.childNodes).map(compileNode).filter(Boolean);

  class DCLogic extends React.Component {
    render() {
      return h(React.Fragment, null, ...renderList(ast, { vals: this.renderVals(), locals: {} }, 'r'));
    }
  }

  const Component = defineLogic(DCLogic);
  createRoot(container).render(h(Component, props));
}
