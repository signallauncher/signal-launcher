// Notification centre (the bell): a history of what happened — crashes, errors, finished installs,
// updates. Toasts feed it (see toast.js); entries may carry a `target` to jump to.
// Kept in localStorage, newest first, at most 100.

const KEY = 'signal.notifications';
const MAX = 100;
const subscribers = new Set();
let items = load();

function load() {
  try {
    const list = JSON.parse(localStorage.getItem(KEY) || '[]');
    return Array.isArray(list) ? list.slice(0, MAX) : [];
  } catch (e) {
    return [];
  }
}

function save() {
  try { localStorage.setItem(KEY, JSON.stringify(items)); } catch (e) {}
  subscribers.forEach((fn) => { try { fn(items.slice()); } catch (e) { console.error(e); } });
}

export const notifications = {
  /** { kind: 'success'|'error'|'info', title, message, target? } */
  add(entry) {
    const title = String(entry.title || '').trim();
    const message = String(entry.message || '').trim();
    if (!title && !message) return null;
    const item = {
      id: Date.now().toString(36) + Math.random().toString(36).slice(2, 6),
      kind: entry.kind || 'info',
      title: title,
      message: message.slice(0, 600),
      target: entry.target || null,
      at: Date.now(),
      read: false
    };
    items = [item].concat(items).slice(0, MAX);
    save();
    return item.id;
  },
  list() {
    return items.slice();
  },
  unread() {
    return items.filter((i) => !i.read).length;
  },
  markAllRead() {
    if (!items.some((i) => !i.read)) return;
    items = items.map((i) => (i.read ? i : Object.assign({}, i, { read: true })));
    save();
  },
  remove(id) {
    items = items.filter((i) => i.id !== id);
    save();
  },
  clear() {
    items = [];
    save();
  },
  /** fn(list) on every change; returns an unsubscribe function. */
  subscribe(fn) {
    subscribers.add(fn);
    return () => subscribers.delete(fn);
  }
};
