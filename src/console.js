// Developer console: a bottom sheet with the game's live output, one session per installation.
// Rendered outside the design template on purpose — thousands of lines would make every
// re-render of the launcher slow. Lines arrive in batches from Rust (`game-log`).

import { t, translateDom, plural } from './i18n/index.js';

const CAPACITY = 5000;
const LEVELS = { info: 'Инфо', warn: 'Предупреждение', error: 'Ошибка', fatal: 'Критическая ошибка', trace: 'Стек вызовов', debug: 'Отладка' };
const FILTERS = [['all', 'Все'], ['warn', 'Предупреждения'], ['error', 'Ошибки']];

const sessions = new Map(); // instanceId → { id, name, lines, skipped, state, exit }
let current = null;
let filter = 'all';
let query = '';
let follow = true;
let unseen = 0;
let ui = null;
let snapshotLoader = null;
let folderOpener = null;

const el = (tag, cls, text) => {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text != null) node.textContent = text;
  return node;
};
const icon = (paths, size = 16) =>
  '<svg width="' + size + '" height="' + size + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + paths + '</svg>';

function passes(line) {
  if (filter === 'warn' && !['warn', 'error', 'fatal'].includes(line.level)) return false;
  if (filter === 'error' && !['error', 'fatal', 'trace'].includes(line.level)) return false;
  return !query || line.text.toLowerCase().includes(query);
}

function lineNode(line) {
  const row = el('div', 'gc-l gc-' + line.level);
  row.dataset.n = line.n;
  row.title = t(LEVELS[line.level] || '');
  row.appendChild(el('span', 'gc-n', String(line.n)));
  const text = el('span', 'gc-t');
  if (query) {
    // Highlight every match; text nodes only, so log contents are never parsed as HTML.
    const low = line.text.toLowerCase();
    let at = 0;
    for (let i = low.indexOf(query); i >= 0; i = low.indexOf(query, at)) {
      text.appendChild(document.createTextNode(line.text.slice(at, i)));
      text.appendChild(el('mark', null, line.text.slice(i, i + query.length)));
      at = i + query.length;
    }
    text.appendChild(document.createTextNode(line.text.slice(at)));
  } else {
    text.textContent = line.text;
  }
  row.appendChild(text);
  return row;
}

function build() {
  if (ui) return ui;
  const root = el('section', 'gc');
  root.setAttribute('role', 'dialog');
  root.setAttribute('aria-label', 'Консоль игры');
  root.hidden = true;
  root.innerHTML = `
    <div class="gc-grip" title="Потяните, чтобы изменить высоту" aria-hidden="true"></div>
    <header class="gc-head">
      <span class="gc-ico">${icon('<rect x="3" y="4" width="18" height="16" rx="2"></rect><path d="M7 9l3 3-3 3M12 15h5"></path>', 18)}</span>
      <span class="gc-title"><b class="gc-name">Консоль</b><span class="gc-status"></span></span>
      <select class="gc-session field" aria-label="Сессия" hidden></select>
      <label class="gc-search"><span class="sr">Поиск по логу</span>
        ${icon('<circle cx="11" cy="11" r="6.5"></circle><path d="M20 20l-4-4"></path>', 15)}
        <input type="search" placeholder="Поиск по логу" spellcheck="false">
      </label>
      <div class="gc-filters" role="radiogroup" aria-label="Уровень"></div>
      <div class="gc-tools">
        <button type="button" class="gc-btn" data-act="copy">${icon('<rect x="9" y="9" width="11" height="11" rx="2"></rect><path d="M5 15V5a2 2 0 0 1 2-2h10"></path>')}<span>Копировать</span></button>
        <button type="button" class="gc-btn" data-act="clear">${icon('<path d="M4 7h16M10 11v6M14 11v6"></path><path d="M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"></path>')}<span>Очистить</span></button>
        <button type="button" class="gc-btn" data-act="folder">${icon('<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"></path>')}<span>Папка логов</span></button>
        <button type="button" class="gc-btn gc-x" data-act="close" aria-label="Закрыть консоль">${icon('<path d="M6 6l12 12M18 6L6 18"></path>')}</button>
      </div>
    </header>
    <div class="gc-crash" role="alert" hidden>
      <span class="gc-crash-ico">${icon('<circle cx="12" cy="12" r="9"></circle><path d="M12 7.5v5.5M12 16.5h.01"></path>', 20)}</span>
      <div class="gc-crash-body">
        <b class="gc-crash-title"></b>
        <span class="gc-crash-hint"></span>
        <code class="gc-crash-detail" hidden></code>
      </div>
      <div class="gc-crash-actions">
        <button type="button" class="gc-btn gc-btn-hot" data-act="reason">Показать причину краша</button>
        <button type="button" class="gc-btn" data-act="report" hidden>Отчёт о сбое</button>
        <button type="button" class="gc-btn" data-act="log">Полный лог</button>
      </div>
    </div>
    <div class="gc-body" tabindex="0" aria-label="Вывод игры">
      <div class="gc-lines" role="log" aria-live="off"></div>
      <div class="gc-empty"></div>
    </div>
    <button type="button" class="gc-follow" hidden>${icon('<path d="M12 5v14M6 13l6 6 6-6"></path>', 14)}<span></span></button>
    <footer class="gc-foot"></footer>`;
  translateDom(root);
  document.body.appendChild(root);

  const q = (s) => root.querySelector(s);
  ui = {
    root,
    name: q('.gc-name'),
    status: q('.gc-status'),
    session: q('.gc-session'),
    search: q('.gc-search input'),
    filters: q('.gc-filters'),
    crash: q('.gc-crash'),
    crashTitle: q('.gc-crash-title'),
    crashHint: q('.gc-crash-hint'),
    crashDetail: q('.gc-crash-detail'),
    reportBtn: q('[data-act="report"]'),
    body: q('.gc-body'),
    lines: q('.gc-lines'),
    empty: q('.gc-empty'),
    followBtn: q('.gc-follow'),
    foot: q('.gc-foot')
  };

  FILTERS.forEach(([key, label]) => {
    const b = el('button', 'gc-chip', t(label));
    b.type = 'button';
    b.dataset.filter = key;
    b.setAttribute('role', 'radio');
    ui.filters.appendChild(b);
  });
  ui.filters.addEventListener('click', (e) => {
    const b = e.target.closest('[data-filter]');
    if (!b) return;
    filter = b.dataset.filter;
    renderAll();
  });
  let searchTimer = 0;
  ui.search.addEventListener('input', () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => { query = ui.search.value.trim().toLowerCase(); renderAll(); }, 120);
  });
  ui.session.addEventListener('change', () => show(ui.session.value));
  root.addEventListener('click', (e) => {
    const b = e.target.closest('[data-act]');
    if (!b) return;
    const s = sessions.get(current);
    const act = b.dataset.act;
    if (act === 'close') close();
    if (act === 'copy' && s) copyVisible(s);
    if (act === 'clear' && s) { s.lines = []; s.skipped = 0; renderAll(); }
    if (act === 'folder' && s && folderOpener) folderOpener('%MINECRAFT%\\instances\\' + s.id + '\\logs');
    if (act === 'reason' && s) revealReason(s);
    if (act === 'report' && s && s.exit && s.exit.crashReport && folderOpener) folderOpener(s.exit.crashReport);
    if (act === 'log' && s && folderOpener) folderOpener((s.exit && s.exit.log) || '%MINECRAFT%\\instances\\' + s.id + '\\signal-launch.log');
  });
  ui.body.addEventListener('scroll', () => {
    const atBottom = ui.body.scrollTop + ui.body.clientHeight >= ui.body.scrollHeight - 24;
    if (atBottom !== follow) {
      follow = atBottom;
      if (follow) unseen = 0;
      updateFollow();
    }
  });
  ui.followBtn.addEventListener('click', () => { follow = true; unseen = 0; scrollToEnd(); updateFollow(); });
  root.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });

  // Resize by dragging the top edge.
  const grip = q('.gc-grip');
  grip.addEventListener('pointerdown', (e) => {
    const startY = e.clientY;
    const startH = root.getBoundingClientRect().height;
    grip.setPointerCapture(e.pointerId);
    const move = (ev) => {
      const h = Math.min(window.innerHeight * 0.88, Math.max(180, startH + (startY - ev.clientY)));
      root.style.height = h + 'px';
    };
    const up = () => { grip.removeEventListener('pointermove', move); grip.removeEventListener('pointerup', up); };
    grip.addEventListener('pointermove', move);
    grip.addEventListener('pointerup', up);
  });
  return ui;
}

function scrollToEnd() {
  ui.body.scrollTop = ui.body.scrollHeight;
}

function updateFollow() {
  ui.followBtn.hidden = follow || unseen === 0;
  ui.followBtn.querySelector('span').textContent = t(unseen + ' ' + plural(unseen, 'новая строка', 'новые строки', 'новых строк'));
}


function renderHead() {
  const s = sessions.get(current);
  ui.name.textContent = s ? s.name : t('Консоль');
  const states = {
    starting: ['Подготовка запуска…', 'is-run'],
    running: ['Игра идёт', 'is-run'],
    exited: ['Игра закрыта', ''],
    crashed: ['Аварийное завершение', 'is-crash'],
    failed: ['Запуск не удался', 'is-crash']
  };
  const st = s ? states[s.state] || states.exited : ['Нет запущенных сессий', ''];
  let text = st[0];
  if (s && s.exit && s.exit.code != null && s.state === 'crashed') text += ' · код ' + s.exit.code;
  ui.status.textContent = t(text);
  ui.status.className = 'gc-status ' + st[1];
  ui.root.classList.toggle('is-crashed', !!s && (s.state === 'crashed' || s.state === 'failed'));

  ui.session.hidden = sessions.size < 2;
  ui.session.replaceChildren(...[...sessions.values()].map((x) => {
    const o = el('option', null, x.name);
    o.value = x.id;
    o.selected = x.id === current;
    return o;
  }));
  ui.filters.querySelectorAll('[data-filter]').forEach((b) => {
    const on = b.dataset.filter === filter;
    b.classList.toggle('is-on', on);
    b.setAttribute('aria-checked', String(on));
  });

  const reason = s && s.exit && s.exit.reason;
  ui.crash.hidden = !reason && !(s && s.state === 'failed');
  if (s && s.state === 'failed') {
    ui.crashTitle.textContent = t('Игра не запустилась');
    ui.crashHint.textContent = t(s.failure || '');
    ui.crashDetail.hidden = true;
    ui.reportBtn.hidden = true;
  } else if (reason) {
    ui.crashTitle.textContent = t(reason.title);
    ui.crashHint.textContent = t(reason.hint);
    ui.crashDetail.hidden = !reason.detail;
    ui.crashDetail.textContent = reason.detail || '';
    ui.reportBtn.hidden = !s.exit.crashReport;
  }
  ui.root.querySelector('[data-act="reason"]').hidden = !(reason && reason.line);
}

function renderFoot() {
  const s = sessions.get(current);
  if (!s) { ui.foot.textContent = ''; return; }
  const shown = ui.lines.childElementCount;
  const parts = [s.lines.length + ' ' + plural(s.lines.length, 'строка', 'строки', 'строк') + ' в памяти (последние ' + CAPACITY + ')'];
  if (filter !== 'all' || query) parts.push('показано ' + shown);
  if (s.skipped) parts.push('пропущено при спаме: ' + s.skipped + ' — они есть в полном логе');
  ui.foot.textContent = parts.map(t).join(' · ');
}

function renderAll() {
  if (!ui) return;
  const s = sessions.get(current);
  const frag = document.createDocumentFragment();
  if (s) s.lines.forEach((l) => { if (passes(l)) frag.appendChild(lineNode(l)); });
  ui.lines.replaceChildren(frag);
  ui.empty.hidden = ui.lines.childElementCount > 0;
  ui.empty.textContent = t(!s
    ? 'Здесь появится вывод игры. Включите «Консоль разработчика» в настройках, чтобы она открывалась при запуске.'
    : (s.lines.length ? 'Нет строк, подходящих под фильтр.' : (s.state === 'starting' ? 'Скачиваю файлы и запускаю Java…' : 'Пока пусто.')));
  renderHead();
  renderFoot();
  follow = true;
  unseen = 0;
  updateFollow();
  scrollToEnd();
}

function copyVisible(s) {
  const text = s.lines.filter(passes).map((l) => l.text).join('\n');
  try {
    navigator.clipboard.writeText(text);
    window.SignalBridge && window.SignalBridge.toast(s.lines.filter(passes).length + ' строк скопировано.', { kind: 'success', title: 'Лог в буфере обмена', timeout: 2500, notify: false });
  } catch (e) {}
}

function revealReason(s) {
  const n = s.exit && s.exit.reason && s.exit.reason.line;
  if (!n) return;
  if (!s.lines.some((l) => l.n === n && passes(l))) {
    filter = 'all';
    query = '';
    ui.search.value = '';
    renderAll();
  }
  const row = ui.lines.querySelector('[data-n="' + n + '"]');
  if (!row) return;
  follow = false;
  ui.lines.querySelectorAll('.is-hit').forEach((r) => r.classList.remove('is-hit'));
  row.classList.add('is-hit');
  row.scrollIntoView({ block: 'center', behavior: 'smooth' });
  updateFollow();
}

function show(id) {
  current = id;
  renderAll();
}

function open(id) {
  build();
  if (id && sessions.has(id)) current = id;
  if (!current && sessions.size) current = [...sessions.keys()].pop();
  ui.root.hidden = false;
  requestAnimationFrame(() => ui.root.classList.add('is-open'));
  const s = sessions.get(current);
  // Opened after a reload mid-game: fetch what Rust has kept.
  if (s && !s.lines.length && snapshotLoader) {
    snapshotLoader(s.id).then((lines) => { if (lines && lines.length && !s.lines.length) { s.lines = lines; renderAll(); } }).catch(() => {});
  }
  renderAll();
}

function close() {
  if (!ui) return;
  ui.root.classList.remove('is-open');
  setTimeout(() => { if (!ui.root.classList.contains('is-open')) ui.root.hidden = true; }, 180);
}

export const gameConsole = {
  /** Wire-up from main.js: fetch kept lines, open a folder or file. */
  configure({ loadSnapshot, openFolder }) {
    snapshotLoader = loadSnapshot;
    folderOpener = openFolder;
  },

  /** A new run of `id`: fresh session; opens the panel if asked (setting «Консоль разработчика»). */
  begin(id, name, { open: openNow } = {}) {
    sessions.delete(id);
    sessions.set(id, { id, name, lines: [], skipped: 0, state: 'starting', exit: null });
    if (openNow || (ui && !ui.root.hidden)) open(id);
  },

  /** `game-log` batch from Rust. */
  push(batch) {
    let s = sessions.get(batch.instanceId);
    if (!s) {
      s = { id: batch.instanceId, name: batch.instanceId, lines: [], skipped: 0, state: 'running', exit: null };
      sessions.set(s.id, s);
    }
    const started = s.state === 'starting';
    if (started) s.state = 'running';
    s.lines.push(...batch.lines);
    s.skipped += batch.skipped || 0;
    const overflow = s.lines.length - CAPACITY;
    if (overflow > 0) s.lines.splice(0, overflow);
    if (!ui || ui.root.hidden || current !== s.id) return;

    const frag = document.createDocumentFragment();
    batch.lines.forEach((l) => { if (passes(l)) frag.appendChild(lineNode(l)); });
    const added = frag.childElementCount;
    ui.lines.appendChild(frag);
    const extra = ui.lines.childElementCount - CAPACITY;
    if (extra > 0) {
      // One range delete instead of hundreds of single removals.
      const range = document.createRange();
      range.setStartBefore(ui.lines.firstChild);
      range.setEndAfter(ui.lines.children[extra - 1]);
      range.deleteContents();
    }
    ui.empty.hidden = ui.lines.childElementCount > 0;
    if (follow) scrollToEnd(); else { unseen += added; updateFollow(); }
    // The header only changes with the session state; rebuilding it per batch costs more than the lines.
    if (started) renderHead();
    renderFoot();
  },

  /** The launch itself failed (download, Java, installer). */
  fail(id, message) {
    const s = sessions.get(id);
    if (!s) return;
    s.state = 'failed';
    s.failure = message;
    if (ui && !ui.root.hidden && current === id) renderAll();
  },

  /** `game-exit` from Rust. A crash keeps (or opens) the panel with the likely cause on top. */
  end(exit) {
    const s = sessions.get(exit.instanceId);
    if (!s) return;
    s.state = exit.crashed ? 'crashed' : 'exited';
    s.exit = exit;
    if (exit.crashed) open(exit.instanceId);
    else if (ui && !ui.root.hidden && current === s.id) { renderHead(); renderFoot(); }
  },

  open,
  close,
  toggle(id) {
    if (ui && !ui.root.hidden && (!id || id === current || !sessions.has(id))) close();
    else open(id);
  },
  revealReason(id) {
    open(id);
    const s = sessions.get(id);
    if (s) revealReason(s);
  }
};
