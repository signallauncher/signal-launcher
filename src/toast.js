// Toast notifications in the top-right corner (container: #toasts in index.html).
// Finished successes and errors also go to the notification centre (notify.js).
import { notifications } from './notify.js';
import { t } from './i18n/index.js';
const ICONS = {
  info: '<circle cx="12" cy="12" r="9"></circle><path d="M12 11v5M12 8h.01"></path>',
  success: '<path d="M5 12.5l4.5 4.5L19 7.5"></path>',
  error: '<circle cx="12" cy="12" r="9"></circle><path d="M12 7.5v5.5M12 16.5h.01"></path>',
  loading: '<path d="M21 12a9 9 0 1 1-6.2-8.56"></path>'
};
const MAX_VISIBLE = 4;

function container() {
  let root = document.getElementById('toasts');
  if (!root) {
    root = document.createElement('div');
    root.id = 'toasts';
    document.body.appendChild(root);
  }
  root.className = 'toasts';
  root.setAttribute('aria-live', 'polite');
  return root;
}

/**
 * Shows a toast. Returns a handle to update or close it (handy for "loading → done").
 * @param {string} message
 * @param {{ kind?: 'info'|'success'|'error'|'loading', title?: string, timeout?: number,
 *   actions?: { label: string, onClick: () => void }[],
 *   notify?: boolean, target?: object }} [opts]
 *   `notify`: true — keep an info toast in the history too; false — never (copied, nothing new…).
 *   `target`: where the history entry leads (see launcher.logic.js → openTarget).
 */
export function toast(message, opts = {}) {
  const root = container();
  const el = document.createElement('div');
  const icon = document.createElement('span');
  const body = document.createElement('div');
  const titleEl = document.createElement('div');
  const textEl = document.createElement('div');
  const actionsEl = document.createElement('div');
  const close = document.createElement('button');

  icon.className = 'toast-ico';
  body.className = 'toast-body';
  titleEl.className = 'toast-title';
  textEl.className = 'toast-text';
  actionsEl.className = 'toast-actions';
  close.className = 'toast-close';
  close.type = 'button';
  close.setAttribute('aria-label', t('Закрыть уведомление'));
  close.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"></path></svg>';
  body.append(titleEl, textEl, actionsEl);
  el.append(icon, body, close);

  let timer = null;
  let logged = false;
  const remove = () => {
    clearTimeout(timer);
    el.classList.remove('show');
    el.classList.add('hide');
    setTimeout(() => el.remove(), 220);
  };
  const apply = ({ kind = 'info', title = '', message: msg = message, timeout = 4200, actions = [], notify, target } = {}) => {
    // A loading toast is logged once, in its final state.
    if (!logged && kind !== 'loading' && notify !== false && (notify === true || kind === 'success' || kind === 'error')) {
      logged = true;
      notifications.add({ kind: kind === 'error' ? 'error' : (kind === 'success' ? 'success' : 'info'), title, message: msg, target });
    }
    el.className = 'toast toast-' + kind + (el.classList.contains('show') ? ' show' : '');
    el.setAttribute('role', kind === 'error' ? 'alert' : 'status');
    icon.innerHTML = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + (ICONS[kind] || ICONS.info) + '</svg>';
    titleEl.textContent = t(title);
    titleEl.hidden = !title;
    textEl.textContent = t(msg);
    actionsEl.replaceChildren(...actions.map((a) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'toast-action';
      b.textContent = t(a.label);
      b.addEventListener('click', () => { a.onClick(); remove(); });
      return b;
    }));
    actionsEl.hidden = !actions.length;
    clearTimeout(timer);
    if (timeout > 0) timer = setTimeout(remove, timeout);
  };

  close.addEventListener('click', remove);
  apply(Object.assign({ message }, opts));
  root.appendChild(el);
  while (root.children.length > MAX_VISIBLE) root.firstElementChild.remove();
  requestAnimationFrame(() => el.classList.add('show'));

  return {
    update: (next) => apply(Object.assign({ message }, next)),
    close: remove
  };
}
