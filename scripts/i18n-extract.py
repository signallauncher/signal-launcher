"""Collects every Russian UI string for the translation catalog (src/i18n/en.js).

Sources:
  * src/launcher.html — text nodes (normalised exactly like dc-runtime's cleanText) and the
    translatable attributes (aria-label, placeholder, title, alt);
  * src/*.js — string literals, template literals and `'a' + x + 'b'` chains (the chain becomes a
    pattern "a{1}b"; the runtime matches it and translates the captured parts too);
  * src-tauri/src/*.rs — string literals, format placeholders become {n}.

Usage:  python scripts/i18n-extract.py            → prints keys missing in src/i18n/en.js
        python scripts/i18n-extract.py --json OUT → writes the full catalog
"""

import html.parser
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CYR = re.compile('[\u0400-\u04FF]')
ATTRS = {'aria-label', 'placeholder', 'title', 'alt'}
# Not UI text: the system prompt sent to the AI model.
IGNORE_PREFIXES = ('Ты — сборщик модпаков Minecraft',)


def clean_text(text):
    """Mirror of dc-runtime.js cleanText."""
    if '\n' not in text:
        return text
    lines = re.split(r'\r?\n', text)
    out = []
    for i, line in enumerate(lines):
        l = line.replace('\t', ' ')
        if i > 0:
            l = l.lstrip()
        if i < len(lines) - 1:
            l = l.rstrip()
        if l:
            out.append(l)
    return ' '.join(out)


class TemplateParser(html.parser.HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.keys = set()
        self.skip = 0

    def handle_starttag(self, tag, attrs):
        if tag in ('style', 'script', 'helmet'):
            self.skip += 1
        for name, value in attrs:
            if name in ATTRS and value and CYR.search(value):
                self.keys.add(value)

    def handle_startendtag(self, tag, attrs):
        for name, value in attrs:
            if name in ATTRS and value and CYR.search(value):
                self.keys.add(value)

    def handle_endtag(self, tag):
        if tag in ('style', 'script', 'helmet') and self.skip:
            self.skip -= 1

    def handle_data(self, data):
        if self.skip:
            return
        t = clean_text(data)
        if t.strip() and CYR.search(t):
            self.keys.add(t)


# ----------------------------------------------------------------------------- JS tokenizer

REGEX_PREV = set('(,=:[!&|?{};+-*%<>~^') | {'return', 'typeof', 'case', 'do', 'else', 'in', 'of', '=>'}


def unescape_js(s):
    return re.sub(r"\\(u\{[0-9a-fA-F]+\}|u[0-9a-fA-F]{4}|x[0-9a-fA-F]{2}|.)", lambda m: {
        'n': '\n', 't': '\t', 'r': '\r', '\\': '\\', "'": "'", '"': '"', '`': '`'
    }.get(m.group(1), chr(int(m.group(1)[1:].strip('{}'), 16)) if m.group(1)[0] in 'ux' else m.group(1)), s)


def tokenize(src):
    toks = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c.isspace():
            i += 1
            continue
        if src.startswith('//', i):
            j = src.find('\n', i)
            i = n if j < 0 else j
            continue
        if src.startswith('/*', i):
            j = src.find('*/', i + 2)
            i = n if j < 0 else j + 2
            continue
        if c in '\'"':
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == '\\' else 1
            toks.append(('str', unescape_js(src[i + 1:j])))
            i = j + 1
            continue
        if c == '`':
            parts, buf, j = [], '', i + 1
            while j < n and src[j] != '`':
                if src[j] == '\\':
                    buf += src[j:j + 2]
                    j += 2
                elif src.startswith('${', j):
                    parts.append(('s', unescape_js(buf)))
                    buf = ''
                    depth, k = 1, j + 2
                    while k < n and depth:
                        if src[k] == '{':
                            depth += 1
                        elif src[k] == '}':
                            depth -= 1
                        elif src[k] in '\'"`':
                            q = src[k]
                            k += 1
                            while k < n and src[k] != q:
                                k += 2 if src[k] == '\\' else 1
                        k += 1
                    parts.append(('e', src[j + 2:k - 1]))
                    j = k
                else:
                    buf += src[j]
                    j += 1
            parts.append(('s', unescape_js(buf)))
            toks.append(('tmpl', parts))
            i = j + 1
            continue
        if c == '/':
            prev = toks[-1] if toks else None
            if prev is None or (prev[0] == 'p' and prev[1] in REGEX_PREV) or (prev[0] == 'id' and prev[1] in REGEX_PREV):
                j, in_class = i + 1, False
                while j < n:
                    if src[j] == '\\':
                        j += 2
                        continue
                    if src[j] == '[':
                        in_class = True
                    elif src[j] == ']':
                        in_class = False
                    elif src[j] == '/' and not in_class:
                        break
                    j += 1
                j += 1
                while j < n and src[j].isalpha():
                    j += 1
                toks.append(('re', src[i:j]))
                i = j
                continue
        m = re.match(r'[A-Za-z_$][\w$]*|\d[\d._]*', src[i:])
        if m:
            toks.append(('id', m.group(0)))
            i += len(m.group(0))
            continue
        m = re.match(r'=>|\?\.|\.\.\.|===|!==|==|!=|<=|>=|&&|\|\||\?\?|\+\+|--|\+=|-=|[-+*/%=<>!&|^~?:;,.()\[\]{}]', src[i:])
        tok = m.group(0) if m else c
        toks.append(('p', tok))
        i += len(tok)
    return toks


OPEN = {'(': ')', '[': ']', '{': '}'}
CLOSE = {v: k for k, v in OPEN.items()}


def operand_back(toks, j):
    """Start index of the operand that ends at j (member/call chains, brackets)."""
    k = j
    while k >= 0:
        t = toks[k]
        if t[0] == 'p' and t[1] in CLOSE:
            depth, m = 0, k
            while m >= 0:
                if toks[m][0] == 'p' and toks[m][1] in CLOSE:
                    depth += 1
                elif toks[m][0] == 'p' and toks[m][1] in OPEN:
                    depth -= 1
                    if depth == 0:
                        break
                m -= 1
            k = m - 1
            if k >= 0 and (toks[k][0] in ('id', 'str') or (toks[k][0] == 'p' and toks[k][1] in CLOSE)):
                continue
            return k + 1
        if t[0] in ('id', 'str', 'tmpl', 're'):
            if k - 1 >= 0 and toks[k - 1][0] == 'p' and toks[k - 1][1] in ('.', '?.'):
                k -= 2
                continue
            return k
        return k + 1
    return 0


def operand_fwd(toks, i):
    """End index (inclusive) of the operand that starts at i."""
    k = i
    n = len(toks)
    while k < n:
        t = toks[k]
        if t[0] == 'p' and t[1] in OPEN:
            depth, m = 0, k
            while m < n:
                if toks[m][0] == 'p' and toks[m][1] in OPEN:
                    depth += 1
                elif toks[m][0] == 'p' and toks[m][1] in CLOSE:
                    depth -= 1
                    if depth == 0:
                        break
                m += 1
            k = m
        nxt = toks[k + 1] if k + 1 < n else None
        if nxt and nxt[0] == 'p' and nxt[1] in ('.', '?.', '(', '['):
            k += 1 if nxt[1] in ('(', '[') else 2
            continue
        return k
    return n - 1


def js_keys(src):
    toks = tokenize(src)
    keys = set()
    for t in toks:
        if t[0] == 'str' and CYR.search(t[1]):
            keys.add(t[1])
        if t[0] == 'tmpl' and any(CYR.search(p[1]) for p in t[1] if p[0] == 's'):
            static = ' '.join(p[1] for p in t[1] if p[0] == 's')
            if '<' in static and '>' in static:
                # Markup built in JS (console.js, news-reader.js): translated node by node in the DOM.
                tp = TemplateParser()
                tp.feed(static)
                keys.update(k.strip() for k in tp.keys if k.strip())
                continue
            out, n = '', 0
            for kind, v in t[1]:
                if kind == 's':
                    out += v
                else:
                    n += 1
                    out += '{' + str(n) + '}'
            keys.add(out)
    # '+' chains
    seen_chains = set()
    for idx, t in enumerate(toks):
        if t[0] != 'str' or not CYR.search(t[1]):
            continue
        start = idx
        while start - 2 >= 0 and toks[start - 1] == ('p', '+'):
            b = operand_back(toks, start - 2)
            start = b
        end = idx
        while end + 2 < len(toks) and toks[end + 1] == ('p', '+'):
            end = operand_fwd(toks, end + 2)
        if start == idx and end == idx:
            continue
        if (start, end) in seen_chains:
            continue
        seen_chains.add((start, end))
        # split into operands on top-level '+'
        ops, cur, depth = [], [], 0
        for tk in toks[start:end + 1]:
            if tk[0] == 'p' and tk[1] in OPEN:
                depth += 1
            elif tk[0] == 'p' and tk[1] in CLOSE:
                depth -= 1
            if depth == 0 and tk == ('p', '+'):
                ops.append(cur)
                cur = []
            else:
                cur.append(tk)
        ops.append(cur)
        out, n = '', 0
        for op in ops:
            if len(op) == 1 and op[0][0] == 'str':
                out += op[0][1]
            else:
                n += 1
                out += '{' + str(n) + '}'
        if n and CYR.search(re.sub(r'\{\d+\}', '', out)):
            keys.add(out)
    return keys


def rust_strings(src):
    """String literals of Rust source: skips comments, char literals and lifetimes, reads raw strings."""
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith('//', i):
            j = src.find('\n', i)
            i = n if j < 0 else j
        elif src.startswith('/*', i):
            j = src.find('*/', i + 2)
            i = n if j < 0 else j + 2
        elif c == 'r' and (src.startswith('r"', i) or src.startswith('r#', i)) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == '_')):
            j = i + 1
            hashes = 0
            while j < n and src[j] == '#':
                hashes += 1
                j += 1
            if j < n and src[j] == '"':
                end = src.find('"' + '#' * hashes, j + 1)
                out.append(src[j + 1:end])
                i = end + 1 + hashes
            else:
                i += 1
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == '\\' else 1
            out.append(src[i + 1:j])
            i = j + 1
        elif c == "'":
            # char literal ('x', '\n', '\u{..}') or a lifetime ('a, '_, 'static)
            if src.startswith("'\\", i):
                j = src.find("'", i + 2)
                i = j + 1
            elif i + 2 < n and src[i + 2] == "'":
                i += 3
            else:
                i += 1
        else:
            i += 1
    return out


def rust_keys(src):
    keys = set()
    for s in rust_strings(src):
        if not CYR.search(s):
            continue
        s = re.sub(r'\\\s*\n\s*', '', s)  # line continuations
        s = s.replace('\\n', '\n').replace('\\"', '"').replace('\\\\', '\\')
        n = 0

        def ph(mm):
            nonlocal n
            if mm.group(0) in ('{{', '}}'):
                return mm.group(0)[0]
            n += 1
            return '{' + str(n) + '}'
        s = re.sub(r'\{\{|\}\}|\{[^{}]*\}', ph, s)
        keys.add(s)
    return keys


def collect():
    keys = {}
    tp = TemplateParser()
    tp.feed((ROOT / 'src' / 'launcher.html').read_text(encoding='utf-8'))
    for k in tp.keys:
        keys.setdefault(k, set()).add('html')
    for f in sorted((ROOT / 'src').glob('*.js')):
        if f.name in ('dc-runtime.js',):
            continue
        for k in js_keys(f.read_text(encoding='utf-8')):
            keys.setdefault(k, set()).add(f.name)
    for f in sorted((ROOT / 'src-tauri' / 'src').glob('*.rs')):
        text = f.read_text(encoding='utf-8')
        text = re.split(r'#\[cfg\(test\)\]\s*mod tests', text)[0]  # test strings are not UI
        for k in rust_keys(text):
            keys.setdefault(k, set()).add(f.name)
    return keys


def known_keys():
    path = ROOT / 'src' / 'i18n' / 'en.js'
    if not path.exists():
        return set()
    text = path.read_text(encoding='utf-8')
    body = text[text.index('export default ') + len('export default '):text.rindex('}') + 1]
    return set(json.loads(body)['exact'].keys()) | {p[0] for p in json.loads(body)['patterns']}


if __name__ == '__main__':
    sys.stdout.reconfigure(encoding='utf-8')
    catalog = {k: v for k, v in collect().items() if not k.startswith(IGNORE_PREFIXES)}
    if len(sys.argv) > 1 and sys.argv[1] == '--prune':
        # Drop translations of strings the code no longer has (a stale pattern can catch new text).
        path = ROOT / 'src' / 'i18n' / 'en.js'
        text = path.read_text(encoding='utf-8')
        head, body = text.split('export default ', 1)
        data = json.loads(body.rstrip().rstrip(';'))
        before = len(data['exact']) + len(data['patterns'])
        data['exact'] = {k: v for k, v in data['exact'].items() if k in catalog}
        data['patterns'] = [p for p in data['patterns'] if p[0] in catalog]
        path.write_text(head + 'export default ' + json.dumps(data, ensure_ascii=False, indent=1) + ';\n', encoding='utf-8')
        print('removed', before - len(data['exact']) - len(data['patterns']), 'stale entries')
    elif len(sys.argv) > 2 and sys.argv[1] == '--json':
        out = [{'key': k, 'from': sorted(v), 'pattern': bool(re.search(r'\{\d+\}', k))} for k, v in sorted(catalog.items())]
        Path(sys.argv[2]).write_text(json.dumps(out, ensure_ascii=False, indent=1), encoding='utf-8')
        print(len(out), 'keys,', sum(1 for o in out if o['pattern']), 'patterns')
    else:
        missing = sorted(set(catalog) - known_keys())
        for k in missing:
            print(json.dumps(k, ensure_ascii=False), '←', ', '.join(sorted(catalog[k])))
        print(len(missing), 'missing of', len(catalog))
