// Signal Launcher component logic. Exported from the design canvas.
import { locale as LOCALE, decimal, plural as pluralize, lang as LANG, LANGS, setLang } from './i18n/index.js';
import { SkinViewer } from './skin-viewer.js';
import CATALOG from './catalog.items.json';

// Wardrobe slots in display order with their tab labels (the server's catalog builder uses the same SLOTS); a tab shows up once
// the catalog has something for it.
const COS_SLOTS = [
  ['hat', 'Шляпы'], ['head', 'Голова'], ['ears', 'Уши'], ['face', 'Лицо'], ['back', 'Спина'], ['wings', 'Крылья'],
  ['cape', 'Плащи'], ['pet', 'Питомец'], ['shoulders', 'Плечи'], ['top', 'Верх'], ['arms', 'Руки'], ['pants', 'Штаны'],
  ['skirt', 'Юбки'], ['shoes', 'Обувь'], ['full_body', 'Костюмы'], ['accessory', 'Аксессуары'], ['effect', 'Эффекты'],
  ['aura', 'Ауры']
];
const emptyLook = () => Object.fromEntries(COS_SLOTS.map(([k]) => [k, null]));
// Bone for a catalog item without `attach` (the mod's WornModels uses the same defaults).
const SLOT_ATTACH = { hat: 'head', head: 'head', ears: 'head', face: 'head', pet: 'shoulder_left', shoulders: 'shoulder_left' };
const RARITY = { common: 'Обычный', rare: 'Редкий', epic: 'Эпический', legendary: 'Легендарный' };
// Card glow for the texture capes; launcher-only styling, not part of the catalog.
const CAPE_GLOW = {
  signal: '0 0 26px rgba(91,208,122,0.45)', ender: '0 0 28px rgba(192,76,255,0.55)', obsidian: '0 0 30px rgba(157,107,255,0.8)',
  midnight: '0 0 22px rgba(120,150,255,0.45)', ember: '0 0 24px rgba(255,120,60,0.5)', frost: '0 0 22px rgba(143,211,255,0.5)'
};
// The classic Steve: the skin of every account without one of its own.
const DEFAULT_SKIN = '/assets/img/default-skin.png';
const NICK = /^[A-Za-z0-9_]{3,16}$/;
// Names whose skin is looked up online. «Player» is the default offline account's name, and the real Mojang
// account with that name is somebody else's: it keeps the default Steve skin with no network request.
const lookupName = (name) => NICK.test(name || '') && name.toLowerCase() !== 'player';
// Nickname skin lookups (Mojang, else Ely.by) are reused this long, so typing and switching accounts or screens
// do not ask again; an offline account looks its nickname up again once a day (players change skins).
const NICK_SKIN_TTL = 30 * 60 * 1000;
const ACCOUNT_SKIN_TTL = 24 * 60 * 60 * 1000;
// 8x8 face (hat layer blended over it) of a skin image, as '#rrggbb' like the backend's (offline_skin.rs).
function faceOf(url) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const c = document.createElement('canvas');
      c.width = 64; c.height = 32;
      const g = c.getContext('2d');
      g.drawImage(img, 0, 0);
      const px = g.getImageData(0, 0, 64, 32).data;
      const hex = (v) => Math.round(v).toString(16).padStart(2, '0');
      const out = [];
      for (let y = 8; y < 16; y++) {
        for (let x = 8; x < 16; x++) {
          const i = (y * 64 + x) * 4, h = i + 32 * 4, a = px[h + 3] / 255;
          out.push('#' + [0, 1, 2].map((k) => hex(px[h + k] * a + px[i + k] * (1 - a))).join(''));
        }
      }
      resolve(out);
    };
    img.onerror = reject;
    img.src = url;
  });
}
// 3D stage: skin px -> CSS px, and the soles this far above the stage's bottom edge (room for the shadow).
const STAGE_SCALE = 11;
const STAGE_FEET_GAP = 40;
// A point in the middle of the stone path of the default background (bg.jpg), as fractions of the image.
const BG_PATH = [0.42, 0.82];
// Settings → Интерфейс. Blocks of the main screen ([data-ui] in launcher.html): name, can be hidden, can be
// dragged. Sound, notifications, the version picker and the console button live inside a draggable block.
const UI_PARTS = [
  ['sidebar', 'Боковое меню', { drag: true }],
  ['title', 'Заголовок страницы', { hide: true, drag: true }],
  ['sysIcons', 'Системные кнопки', { drag: true }],
  ['friends', 'Друзья', { hide: true }],
  ['sound', 'Звук', { hide: true }],
  ['notifications', 'Уведомления', { hide: true }],
  ['account', 'Аккаунт', { hide: true, drag: true }],
  ['stage', 'Персонаж', { hide: true, drag: true }],
  ['launch', 'Панель запуска', { drag: true }],
  ['version', 'Выбор версии', { hide: true }],
  ['console', 'Консоль игры', { hide: true }]
];
const UI_PART = Object.fromEntries(UI_PARTS.map(([k, label, o]) => [k, Object.assign({ label: label }, o)]));
// Visibility switches in the settings (badges are not a block of their own).
const UI_HIDE = [['title', 'Заголовок страницы'], ['friends', 'Кнопка друзей'], ['sound', 'Кнопка звука'], ['notifications', 'Уведомления'],
  ['account', 'Панель аккаунта'], ['badges', 'Бейджи в меню'], ['version', 'Выбор версии'], ['console', 'Кнопка консоли'],
  ['stage', 'Персонаж']];
const UI_BASE = { preset: 'classic', sidebar: 'left', icons: false, glass: false, launch: 'full', mono: false, stage: 'center', hide: {}, pos: {} };
const UI_PRESETS = {
  classic: ['Классический', 'Меню слева, персонаж по центру, панель запуска внизу', {}],
  minimal: ['Минимализм', 'Меню иконками, запуск в углу, только фон и скин', {
    icons: true, glass: true, launch: 'float', hide: { sound: true, notifications: true, title: true, badges: true, console: true }
  }],
  compact: ['Компактный', 'Вкладки сверху, персонаж слева — для окон 1024×600', { sidebar: 'top', stage: 'left', launch: 'float', mono: true }]
};
const UI_GRID = 8;
const uiPreset = (key) => Object.assign({}, UI_BASE, UI_PRESETS[key][2], { preset: key, hide: Object.assign({}, UI_PRESETS[key][2].hide), pos: {} });
// A saved layout with only known values, so an old or hand-edited one cannot break the screen.
function uiNormalize(u) {
  if (!u || typeof u !== 'object') return uiPreset('classic');
  const pick = (v, list, d) => (list.includes(v) ? v : d);
  const pos = {};
  Object.keys(u.pos || {}).forEach((k) => {
    const p = u.pos[k];
    if (UI_PART[k] && UI_PART[k].drag && p && isFinite(p.x) && isFinite(p.y)) pos[k] = { x: Math.max(0, Math.min(95, +p.x)), y: Math.max(0, Math.min(95, +p.y)) };
  });
  const hide = {};
  UI_HIDE.forEach(([k]) => { if (u.hide && u.hide[k]) hide[k] = true; });
  return {
    preset: pick(u.preset, ['classic', 'minimal', 'compact', 'custom'], 'custom'),
    sidebar: pick(u.sidebar, ['left', 'right', 'top'], 'left'),
    icons: !!u.icons, glass: !!u.glass, mono: !!u.mono,
    launch: pick(u.launch, ['full', 'center', 'right', 'float'], 'full'),
    stage: pick(u.stage, ['left', 'center', 'right'], 'center'),
    hide: hide, pos: pos
  };
}
// Shortest repeat of one interface sound, ms (default 80).
const SFX_GAP = { hover: 60, place: 70, done: 400 };
// Calls into the desktop backend go through window.SignalBridge (see src/main.js).
export default function defineLogic(DCLogic) {
class Component extends DCLogic {
  constructor(props) {
    super(props);
    this.state = {
      tab: 'play',
      // Offline nicknames (saved in localStorage). Microsoft accounts live in the backend: msAccounts.
      accounts: [{ id: 'o1', name: 'Player', type: 'offline', skin: 'hood' }],
      msAccounts: [],
      integrations: { microsoft: true, discord: true },
      accountId: 'o1',
      msPhase: '',
      msCode: '',
      msUri: '',
      msErr: '',
      msCopied: false,
      accDel: '',
      accMenu: false,
      modal: null,
      nick: '',
      verMenu: false,
      versionId: 'v1',
      launching: false,
      launchStage: '',
      progress: 0,
      launched: false,
      modsView: 'catalog',
      modType: 'mod',
      modVer: 'inst',
      mr: { hits: [], total: 0, loading: true, more: false, err: '' },
      modMeta: {},
      imKind: 'mod',
      instPacks: {},
      exp: null,
      bk: null,
      skinGame: { key: '', model: '' },
      installedBy: {},
      modBusy: {},
      instMods: {},
      modDel: '',
      mcVersions: null,
      mcVersionsErr: '',
      mcLoading: false,
      mcLatest: '',
      wtWorld: null,
      wtBusy: '',
      instances: null,
      mem: null,
      ramMax: 16,
      ramClamped: false,
      running: {},
      cacheBusy: false,
      importBusy: false,
      modUpdates: {},
      updBusy: {},
      instMenu: '',
      instDel: '',
      renaming: null,
      instQuery: '',
      // Servers added in the launcher (null = first run: one well-known public server as a start).
      myServers: null,
      gameServers: [],
      serverStatus: {},
      srvModal: null,
      srvDel: '',
      news: null,
      newsErr: '',
      newsLoading: false,
      newsSeen: '',
      appVersion: '',
      gameDirInfo: null,
      usage: null,
      usageBusy: false,
      dirModal: null,
      notifOpen: false,
      notifs: [],
      upd: { checking: false, info: null, open: false, busy: false, downloaded: 0, total: 0, err: '' },
      realWorlds: null,
      realShots: null,
      modSrc: 'mr',
      modQuery: '',
      modLoader: 'inst',
      modSort: 'relevance',
      ram: 6,
      jvm: '-XX:+UseG1GC -XX:+ParallelRefProcEnabled',
      theme: 'dark',
      opts: { hide: true, snapshots: false, oldBeta: false, oldAlpha: false, autoUpdate: true, animations: true, sound: true, discord: true, backup: true, console: false, rays: true, offlineSkin: true, companion: true },
      autoSpin: false,
      showLayer: true,
      settingsTab: 'bg',
      // Settings → Интерфейс: layout of the main screen; uiEditing — the layout editor over the main screen
      ui: uiPreset('classic'),
      uiEditing: false,
      uiRects: [],
      bgMode: 'image',
      shaderTime: 'sunset',
      shaderSpeed: 100,
      bgColor: '#14202B',
      bgPreset: 'obsidian',
      bgFileUrl: '',
      bgFileType: '',
      bgFileName: '',
      bgBlur: 8,
      bgDim: 55,
      bgFileData: '',
      priority: 'normal',
      javaPath: '',
      javaList: null,
      il: null,
      backupFreq: 'launch',
      friendsOpen: false,
      // Friends of the selected profile from the Signal server: null until loaded, else { friends, incoming, outgoing }
      fr: null,
      frErr: '',
      // an offline nickname the server has not seen: its first friend request binds it to this computer
      frUnclaimed: false,
      frBusy: '',
      frDel: '',
      // lowercase nickname -> 8×8 face of its skin
      frFaces: {},
      // the presence socket: { state: off | connecting | online | reconnecting | unclaimed, retryIn?, error? }
      presence: { state: 'off' },
      // our own world opened to friends: { join, via, world } or { error } from the server, null otherwise
      myHost: null,
      nickFriend: '',
      friendMsg: '',
      friendMsgOk: true,
      skinModal: false,
      skinUrl: '',
      skinKey: 'default',
      slim: false,
      pendingSkin: '',
      pendingName: '',
      pendingSlim: false,
      skinErr: '',
      dragOver: false,
      aiPrompt: '',
      aiOpen: false,
      niOpen: false,
      niName: '',
      niLoader: 'vanilla',
      niVersion: '',
      niErr: '',
      aiStatus: 'idle',
      aiResult: null,
      aiSource: '',
      // «Собрать с ИИ»: null until asked, then whether the Signal server answers and has its OpenRouter key
      aiConfigured: null,
      aiReachable: null,
      aiNote: '',
      orKey: '',
      aiKeyOpen: false,
      dlBusy: false,
      dlDone: false,
      dlProgress: 0,
      dlStage: '',
      customInstalls: [],
      cosTab: 'cape',
      cos: emptyLook(),
      savedCos: emptyLook(),
      cosSaved: false,
      // Look on the Signal server: idle | busy | synced | empty | unclaimed | local | error
      cosSync: { state: 'idle', msg: '' }
    };
    this.angle = 0;
    this.tilt = -6;
    this.drag = null;
    this.modelEl = null;
    this.angleEl = null;
    this.stageEl = null;
    this.rootEl = null;
    this.prevEl = null;
    this.bgCanvas = null;
    this.cursor = null;
    this.headYaw = 0;
    this.px = 0;
    this.py = 0;
    this.bgLayer = null;
    this.headPitch = 0;
    this.shaderClock = 0;
    this.lastSaved = '';
  }

  componentDidMount() {
    const src = {
      hover: '/assets/sfx/hover.webm',
      // Minecraft's own sounds: a wooden button for buttons and tabs, a stone button for switches and
      // presets, a quiet block placement when a block snaps to the grid in the layout editor, and the
      // experience orb when something is done (download finished, game started)
      click: '/assets/sfx/mc-wood-click.ogg',
      toggle: '/assets/sfx/mc-stone-click.ogg',
      place: '/assets/sfx/mc-place.ogg',
      done: '/assets/sfx/mc-orb.ogg',
      open: '/assets/sfx/open.webm',
      close: '/assets/sfx/close.webm',
      launch: '/assets/sfx/launch.webm'
    };
    const volume = { hover: 0.3, place: 0.22, click: 0.45, toggle: 0.4, done: 0.45 };
    this.sfx = {};
    try {
      Object.keys(src).forEach((k) => {
        const a = new Audio(src[k]);
        a.preload = 'auto';
        a.volume = volume[k] || 0.5;
        this.sfx[k] = a;
      });
    } catch (e) {}
    try {
      this.reduceMotion = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    } catch (e) {}
    try {
      const saved = JSON.parse(window.localStorage.getItem('signal.launcher.v1') || 'null');
      if (saved && typeof saved === 'object') {
        const keep = ['ui', 'myServers', 'newsSeen', 'versionId', 'accounts', 'accountId', 'mcLatest', 'installedBy', 'modMeta', 'customInstalls', 'savedCos', 'skinUrl', 'skinKey', 'slim', 'bgMode', 'bgColor', 'bgPreset', 'bgBlur', 'bgDim', 'bgFileName', 'opts', 'ram', 'priority', 'backupFreq', 'jvm', 'javaPath'];
        const next = {};
        keep.forEach((k) => { if (saved[k] !== undefined) next[k] = saved[k]; });
        if (next.opts) next.opts = Object.assign({}, this.state.opts, next.opts);
        if (next.ui) next.ui = uiNormalize(next.ui);
        if (next.bgMode === 'shader') next.bgMode = 'image';
        if (next.savedCos) {
          // looks saved before newer slots existed lack those keys
          next.savedCos = Object.assign({}, this.state.savedCos, next.savedCos);
          next.cos = Object.assign({}, next.savedCos);
        }
        if (saved.bgFileData) { next.bgFileUrl = saved.bgFileData; next.bgFileData = saved.bgFileData; next.bgFileType = 'image'; }
        else if (next.bgMode === 'file') next.bgMode = 'image';
        this.setState(next);
      }
    } catch (e) {}
    let last = performance.now();
    const tick = (now) => {
      const dt = Math.min(64, now - last);
      last = now;
      if (this.state.autoSpin && !this.drag) this.angle = (this.angle + dt * 0.03) % 360;
      this.trackHead(dt);
      if (this.bgLayer) {
        let tx = 0;
        let ty = 0;
        if (this.cursor && this.state.opts.animations && !this.reduceMotion) {
          const vw = window.innerWidth || 1;
          const vh = window.innerHeight || 1;
          tx = -((this.cursor.x / vw) - 0.5) * 2 * 6;
          ty = -((this.cursor.y / vh) - 0.5) * 2 * 5;
        }
        const kp = Math.min(1, dt * 0.006);
        this.px += (tx - this.px) * kp;
        this.py += (ty - this.py) * kp;
        this.bgLayer.style.transform = 'translate3d(' + this.px.toFixed(2) + 'px, ' + this.py.toFixed(2) + 'px, 0)';
      }
      this.applyView();
      if (this.viewer) this.viewer.render(dt);
      this.raf = requestAnimationFrame(tick);
    };
    this.raf = requestAnimationFrame(tick);
    this.onResize = () => { this.alignBackground(); this.measureUi(); };
    window.addEventListener('resize', this.onResize);
    this.onUiKey = (e) => { if (e.key === 'Escape' && this.state.uiEditing) this.setState({ uiEditing: false }); };
    window.addEventListener('keydown', this.onUiKey);
    let savedUi = null;
    try { savedUi = (JSON.parse(window.localStorage.getItem('signal.launcher.v1') || 'null') || {}).ui; } catch (e) {}
    if (!savedUi) {
      window.SignalBridge.uiLayout().then((u) => { if (u) this.setState({ ui: uiNormalize(u) }); }).catch(() => {});
    }
    this.syncVars();
    this.loadVersions(false);
    this.loadInstances(true);
    this.loadMemory();
    this.loadAccounts().then(() => { this.syncLook(); this.connectFriends(); });
    window.SignalBridge.onPresence((st) => this.onPresenceState(st), (ev) => this.onPresenceEvent(ev));
    faceOf('/assets/img/default-skin.png').then((face) => { this.steveFace = face; }).catch(() => {});
    // Once the saved state is in: a save from before skins belonged to accounts gives the skin on the model
    // to the active account; otherwise the active account's nickname skin is checked.
    this.setState({}, () => {
      const active = (this.state.accounts || []).find((a) => a.id === this.state.accountId);
      if (active && !active.skinUrl && !active.skinFrom && this.state.skinUrl) this.adoptSkin();
      else this.refreshAccountSkin(this.state.accountId);
    });
    window.SignalBridge.onModpackDrop((path) => this.importModpack(path));
    // A .png dropped anywhere opens «Управление скином» with it.
    window.SignalBridge.onSkinDrop((file) => {
      this.play('open');
      this.setState({ skinModal: true, pendingSkin: '', pendingName: '', pendingSlim: this.state.slim, skinErr: '', dragOver: false, accMenu: false, verMenu: false, friendsOpen: false });
      this.readSkin(file);
    });
    this.loadNews(false);
    window.SignalBridge.gameDirInfo().then((i) => { if (i) this.setState({ gameDirInfo: i }); }).catch(() => {});
    this.setState({ notifs: window.SignalBridge.notifications.list() });
    this.unsubNotifs = window.SignalBridge.notifications.subscribe((list) => this.setState({ notifs: list }));
    window.SignalBridge.launcherInfo().then((i) => { if (i) this.setState({ appVersion: i.version }); }).catch(() => {});
    // «Проверять обновления при запуске»: quietly, a few seconds after start.
    if (this.state.opts.autoUpdate) this.updTimer = setTimeout(() => this.checkLauncherUpdate(true), 4000);
    window.SignalBridge.integrations().then((v) => { if (v) this.setState({ integrations: v }); }).catch(() => {});
    window.SignalBridge.onGameExit((e) => {
      if (Object.keys(this.state.running || {}).length <= 1) this.setState({ myHost: null });
      const running = Object.assign({}, this.state.running);
      delete running[e.instanceId];
      this.setState({ running: running });
      this.play(e.crashed ? 'close' : 'done');
      this.loadInstances(false);
      if (this.state.tab === 'worlds') this.loadWorlds();
      if (this.state.tab === 'shots') this.loadShots();
    });
  }

  componentDidUpdate(prevProps, prevState) {
    if (prevState && prevState.accountId !== this.state.accountId) this.syncLook();
    if (this.frKey !== undefined && this.frKey !== this.friendsKey()) this.connectFriends();
    else if (prevState && (prevState.skinUrl !== this.state.skinUrl || prevState.slim !== this.state.slim)) this.adoptSkin();
    this.syncVars();
    this.syncViewer();
    this.alignBackground();
    this.persist();
    if (prevState && prevState.ui !== this.state.ui) {
      clearTimeout(this.uiSaveTimer);
      const ui = this.state.ui;
      this.uiSaveTimer = setTimeout(() => window.SignalBridge.saveUiLayout(ui).catch(() => {}), 800);
    }
    if (this.state.uiEditing) this.measureUi();
  }

  // Root classes that lay the main screen out (CSS in launcher.html, «Settings → Интерфейс»).
  uiClasses(s) {
    const u = s.ui;
    const c = ['ui-root', 'ui-sb-' + u.sidebar, 'ui-launch-' + u.launch, 'ui-stage-' + u.stage];
    if (u.icons) c.push('ui-sb-icons');
    if (u.glass) c.push('ui-glass');
    if (u.mono) c.push('ui-launch-mono');
    Object.keys(u.hide).forEach((k) => { if (u.hide[k]) c.push('ui-hide-' + k); });
    Object.keys(u.pos).forEach((k) => c.push('ui-free-' + k));
    if (s.uiEditing) c.push('ui-editing');
    return c.join(' ');
  }

  // A layout change: `custom` marks it as the user's own (no preset). Blocks glide from where they were
  // to where they land (FLIP): measured before, re-measured after the render, animated by transform.
  setUi(patch, custom) {
    const root = this.rootEl;
    const animate = root && this.state.opts.animations && !this.reduceMotion;
    const blocks = animate ? UI_PARTS.filter(([k, , o]) => o.drag).map(([k]) => root.querySelector('[data-ui="' + k + '"]')).filter(Boolean) : [];
    const before = new Map(blocks.map((el) => [el, el.getBoundingClientRect()]));
    const ui = Object.assign({}, this.state.ui, patch, custom ? { preset: 'custom' } : null);
    this.setState({ ui: ui }, () => {
      before.forEach((a, el) => {
        if (!el.isConnected) return;
        const b = el.getBoundingClientRect();
        if (!b.width || !b.height || !a.width || !a.height) return;
        const dx = a.left - b.left, dy = a.top - b.top, sx = a.width / b.width, sy = a.height / b.height;
        if (Math.abs(dx) < 1 && Math.abs(dy) < 1 && Math.abs(sx - 1) < 0.01 && Math.abs(sy - 1) < 0.01) return;
        // a block that changes shape (menu to a top bar) would smear if scaled: it slides and fades in instead
        const reshaped = Math.abs(sx - 1) > 0.15 || Math.abs(sy - 1) > 0.15;
        const from = reshaped
          ? { transform: 'translate(' + (a.left + a.width / 2 - b.left - b.width / 2) + 'px,' + (a.top + a.height / 2 - b.top - b.height / 2) + 'px)', opacity: 0.15 }
          : { transformOrigin: '0 0', transform: 'translate(' + dx + 'px,' + dy + 'px) scale(' + sx + ',' + sy + ')' };
        el.animate([from, reshaped ? { transform: 'none', opacity: 1 } : { transformOrigin: '0 0', transform: 'none' }],
          { duration: 380, easing: 'cubic-bezier(.2,.8,.2,1)' });
      });
      // edit frames follow once the blocks have arrived
      clearTimeout(this.uiMeasureTimer);
      this.uiMeasureTimer = setTimeout(() => this.measureUi(), 420);
    });
  }

  setUiHide(key, hidden) {
    this.setUi({ hide: Object.assign({}, this.state.ui.hide, { [key]: hidden }) }, true);
  }

  // Values for Settings → Интерфейс and the layout editor over the main screen.
  uiVals(s, closeAll) {
    const u = s.ui;
    const sw = (on, toggle) => ({ on: on, swCls: on ? 'sw is-on' : 'sw', rowCls: on ? 'trow is-on' : 'trow', toggle: toggle });
    const seg = (key, list) => list.map(([v, label]) => ({
      label: label, on: u[key] === v, cls: u[key] === v ? 'seg is-active' : 'seg', pick: () => this.setUi({ [key]: v }, true)
    }));
    // preset thumbnails: [left, top, width, height] in % of a small window
    const minis = {
      classic: [[3, 6, 22, 88, 1], [44, 20, 14, 52, 0.9], [29, 78, 68, 16, 1], [78, 6, 19, 10, 0.8]],
      minimal: [[3, 6, 7, 88, 0.45], [44, 18, 14, 56, 0.9], [66, 80, 31, 13, 0.55]],
      compact: [[3, 6, 94, 11, 1], [14, 28, 13, 48, 0.9], [52, 79, 45, 15, 1]]
    };
    const hidden = (k) => !!u.hide[k];
    return {
      uiPresets: Object.keys(UI_PRESETS).map((k) => ({
        label: UI_PRESETS[k][0], desc: UI_PRESETS[k][1], on: u.preset === k, cls: u.preset === k ? 'pick is-active' : 'pick',
        mini: minis[k].map(([l, t, w, h, o]) => ({ l: l, t: t, w: w, h: h, o: o })),
        pick: () => this.setUi(uiPreset(k))
      })),
      uiCustom: u.preset === 'custom',
      uiSidebarOpts: seg('sidebar', [['left', 'Слева'], ['right', 'Справа'], ['top', 'Сверху']]),
      uiLaunchOpts: seg('launch', [['full', 'Во всю ширину'], ['center', 'По центру'], ['right', 'Справа'], ['float', 'Плавающая']]),
      uiStageOpts: seg('stage', [['left', 'Слева'], ['center', 'По центру'], ['right', 'Справа']]),
      uiIcons: sw(u.icons, () => this.setUi({ icons: !u.icons }, true)),
      uiGlass: sw(u.glass, () => this.setUi({ glass: !u.glass }, true)),
      uiMono: sw(u.mono, () => this.setUi({ mono: !u.mono }, true)),
      uiVisible: UI_HIDE.map(([k, label]) => Object.assign({ label: label }, sw(!hidden(k), () => this.setUiHide(k, !hidden(k))))),
      uiHasFree: Object.keys(u.pos).length > 0,
      uiFreeNote: 'Перетащено блоков: ' + Object.keys(u.pos).length + '. Они стоят там, куда их поставили, поверх раскладки.',
      uiUnpin: () => this.setUi({ pos: {} }, true),
      uiReset: () => this.setUi(uiPreset('classic')),
      uiEdit: () => this.setState(Object.assign({ uiEditing: true, tab: 'play' }, closeAll)),
      uiEditing: s.uiEditing,
      uiDone: () => this.setState({ uiEditing: false }),
      uiBoxes: s.uiRects.map(([k, x, y, w, h]) => {
        const part = UI_PART[k], off = hidden(k);
        return {
          label: part.label + (off ? ' · скрыт' : ''), x: x, y: y, w: w, h: h,
          cls: 'ui-box' + (part.drag ? '' : ' is-fixed') + (off ? ' is-hidden' : '') + (w < 120 || h < 40 ? ' is-small' : ''),
          canHide: !!part.hide, toggleLabel: off ? 'Вернуть' : 'Скрыть',
          toggle: () => this.setUiHide(k, !off),
          drag: (e) => this.uiDragStart(k, e)
        };
      })
    };
  }

  // Edit mode: the frames sit on the blocks' current boxes (re-measured after renders, resizes and animations).
  measureUi() {
    if (!this.state.uiEditing || !this.rootEl) return;
    const rects = [];
    UI_PARTS.forEach(([k]) => {
      const el = this.rootEl.querySelector('[data-ui="' + k + '"]');
      if (!el) return;
      const r = el.getBoundingClientRect();
      if (r.width < 2 || r.height < 2) return;
      rects.push([k, Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)]);
    });
    const key = JSON.stringify(rects);
    if (key === this.uiRectsKey) return;
    this.uiRectsKey = key;
    this.setState({ uiRects: rects });
  }

  // Edit mode: drag a block by its frame. It leaves the layout where it stands, follows the pointer on an
  // 8 px grid inside the window and is saved as x, y in % of the window; the preset becomes «Пользовательский».
  uiDragStart(k, e) {
    if (!UI_PART[k].drag || e.button !== 0 || e.target.closest('button')) return;
    const el = this.rootEl && this.rootEl.querySelector('[data-ui="' + k + '"]');
    if (!el) return;
    e.preventDefault();
    const frame = e.currentTarget;
    const W = window.innerWidth, H = window.innerHeight;
    const r = el.getBoundingClientRect();
    const snap = (v) => Math.round(v / UI_GRID) * UI_GRID;
    const put = (x, y) => {
      this.rootEl.style.setProperty('--ui-' + k + '-x', x + 'px');
      this.rootEl.style.setProperty('--ui-' + k + '-y', y + 'px');
    };
    let x = snap(r.left), y = snap(r.top);
    put(x, y);
    const pos = (px, py) => Object.assign({}, this.state.ui.pos, { [k]: { x: px / W * 100, y: py / H * 100 } });
    this.setState({ ui: Object.assign({}, this.state.ui, { preset: 'custom', pos: pos(x, y) }) });
    frame.classList.add('dragging');
    const move = (ev) => {
      const w = el.offsetWidth, h = el.offsetHeight;
      const px = x, py = y;
      x = Math.max(0, Math.min(W - w, snap(r.left + ev.clientX - e.clientX)));
      y = Math.max(0, Math.min(H - h, snap(r.top + ev.clientY - e.clientY)));
      if (x !== px || y !== py) this.play('place');
      put(x, y);
      frame.style.left = x + 'px';
      frame.style.top = y + 'px';
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      frame.classList.remove('dragging');
      this.setState({ ui: Object.assign({}, this.state.ui, { preset: 'custom', pos: pos(x, y) }) }, () => this.measureUi());
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  // The default background stands the player on its stone path: on the stage tabs the image takes the
  // smallest zoom that still covers the window and brings BG_PATH under the soles (bottom edge kept when it
  // can be). Elsewhere it is plain cover, anchored at the bottom. The layer is fixed at -12 px (parallax room).
  alignBackground() {
    const img = this.bgImg, layer = this.bgLayer;
    if (!img || !layer || !img.naturalWidth) return;
    const W = layer.clientWidth, H = layer.clientHeight, iw = img.naturalWidth, ih = img.naturalHeight;
    const cover = Math.max(W / iw, H / ih);
    let k = cover, L = (W - cover * iw) / 2, T = H - cover * ih;
    const stage = this.stageEl && this.stageEl.isConnected ? this.stageEl.getBoundingClientRect() : null;
    if (stage && stage.width) {
      const fx = stage.left + stage.width / 2 - layer.offsetLeft, fy = stage.bottom - STAGE_FEET_GAP - layer.offsetTop;
      const [ax, ay] = BG_PATH;
      const need = Math.max(fx / (ax * iw), (W - fx) / ((1 - ax) * iw), fy / (ay * ih), (H - fy) / ((1 - ay) * ih));
      k = Math.min(cover * 2, Math.max(cover, need));
      L = Math.min(0, Math.max(W - k * iw, fx - ax * k * iw));
      T = Math.min(0, Math.max(H - k * ih, fy - ay * k * ih));
    }
    const box = [L, T, k * iw, k * ih].map((v) => Math.round(v) + 'px');
    if (img.dataset.box === box.join()) return;
    const first = !img.dataset.box;
    img.dataset.box = box.join();
    Object.assign(img.style, { left: box[0], top: box[1], width: box[2], height: box[3] });
    // glide between tabs and window sizes, but not on the first placement
    if (first) requestAnimationFrame(() => { img.style.transition = 'left .5s ease, top .5s ease, width .5s ease, height .5s ease'; });
  }

  componentWillUnmount() {
    window.removeEventListener('resize', this.onResize);
    window.removeEventListener('keydown', this.onUiKey);
    clearTimeout(this.uiSaveTimer);
    clearInterval(this.timer);
    clearTimeout(this.timer2);
    clearTimeout(this.msTimer);
    clearTimeout(this.frTimer);
    clearTimeout(this.frDelTimer);
    cancelAnimationFrame(this.raf);
    clearInterval(this.dlTimer);
    clearTimeout(this.cosTimer);
    clearTimeout(this.delTimer);
    clearTimeout(this.accDelTimer);
    clearTimeout(this.instDelTimer);
    clearTimeout(this.srvDelTimer);
    clearTimeout(this.updTimer);
    if (this.unsubNotifs) this.unsubNotifs();
    if (this.viewer) { this.viewer.dispose(); this.viewer = null; }
    this.msAttempt = (this.msAttempt || 0) + 1;
  }

  persist() {
    const s = this.state;
    const data = {
      skinUrl: s.skinUrl, skinKey: s.skinKey, slim: s.slim, bgMode: s.bgMode, shaderTime: s.shaderTime,
      shaderSpeed: s.shaderSpeed, bgColor: s.bgColor, bgPreset: s.bgPreset, bgBlur: s.bgBlur, bgDim: s.bgDim,
      opts: s.opts, ram: s.ram, priority: s.priority, backupFreq: s.backupFreq,
      jvm: s.jvm, javaPath: s.javaPath,
      customInstalls: s.customInstalls, savedCos: s.savedCos, installedBy: s.installedBy, modMeta: s.modMeta, mcLatest: s.mcLatest,
      accounts: s.accounts, accountId: s.accountId, versionId: s.versionId, myServers: s.myServers, newsSeen: s.newsSeen,
      ui: s.ui
    };
    if (s.bgFileData && s.bgFileData.length < 3000000) { data.bgFileData = s.bgFileData; data.bgFileName = s.bgFileName; }
    else if (s.bgMode === 'file') data.bgMode = 'image';
    const json = JSON.stringify(data);
    if (json === this.lastSaved) return;
    this.lastSaved = json;
    try { window.localStorage.setItem('signal.launcher.v1', json); } catch (e) {}
  }

  syncVars() {
    const s = this.state;
    if (this.rootEl) {
      Object.keys(s.ui.pos).forEach((k) => {
        this.rootEl.style.setProperty('--ui-' + k + '-x', s.ui.pos[k].x + 'vw');
        this.rootEl.style.setProperty('--ui-' + k + '-y', s.ui.pos[k].y + 'vh');
      });
      if (s.skinUrl) this.rootEl.style.setProperty('--skin', 'url("' + s.skinUrl + '")');
      else this.rootEl.style.removeProperty('--skin');
    }
    if (this.prevEl) {
      if (s.pendingSkin) this.prevEl.style.setProperty('--skinprev', 'url("' + s.pendingSkin + '")');
      else this.prevEl.style.removeProperty('--skinprev');
    }
  }

  // The selected profile as the Signal Companion API knows it; null until that account is loaded.
  companionProfile() {
    const acc = this.allAccounts().find((a) => a.id === this.state.accountId);
    if (!acc) return null;
    return acc.type === 'ms' ? { kind: 'ms', id: acc.id, name: acc.name } : { kind: 'offline', id: null, name: acc.name };
  }

  // Only ids the wardrobe still has; an item that takes the cape's place (noCape) wins over the cape.
  cleanLook(look) {
    const d = this.cosDefs();
    const out = {};
    COS_SLOTS.forEach(([k]) => {
      out[k] = look && d[k].some((c) => c.id === look[k]) ? look[k] : null;
    });
    if (this.capeBlocker(out)) out.cape = null;
    return out;
  }

  // The worn item (outside the cape slot) that cannot be worn with a cape, or undefined.
  capeBlocker(look) {
    const d = this.cosDefs();
    for (const [k] of COS_SLOTS) {
      if (k === 'cape' || !look[k]) continue;
      const it = d[k].find((c) => c.id === look[k]);
      if (it && it.noCape) return it;
    }
    return undefined;
  }

  // Loads the profile's look from the Signal server. The latest request wins when profiles switch quickly.
  syncLook() {
    const profile = this.companionProfile();
    if (!profile) return;
    const req = (this.lookReq || 0) + 1;
    this.lookReq = req;
    this.setState({ cosSync: { state: 'busy', msg: '' } });
    window.SignalBridge.companionLook(profile)
      .then((r) => {
        if (req !== this.lookReq) return;
        if (r === null && window.SignalBridge.isTauri === false) this.setState({ cosSync: { state: 'local', msg: '' } });
        else if (r === null) this.setState({ cosSync: { state: 'unclaimed', msg: '' } });
        else if (r.rev > 0) {
          const look = this.cleanLook(r.look);
          this.setState({ cos: look, savedCos: look, cosSync: { state: 'synced', msg: '' } });
        } else this.setState({ cosSync: { state: 'empty', msg: '' } });
      })
      .catch((e) => { if (req === this.lookReq) this.setState({ cosSync: { state: 'error', msg: String(e) } }); });
  }

  // Sends the look for the selected profile; an offline nickname gets bound to this computer here.
  pushLook(look) {
    const profile = this.companionProfile();
    if (!profile) return;
    const req = (this.lookReq || 0) + 1;
    this.lookReq = req;
    this.setState({ cosSync: { state: 'busy', msg: '' } });
    window.SignalBridge.companionSaveLook(profile, look)
      .then((r) => { if (req === this.lookReq) this.setState({ cosSync: { state: r ? 'synced' : 'local', msg: '' } }); })
      .catch((e) => { if (req === this.lookReq) this.setState({ cosSync: { state: 'error', msg: String(e) } }); });
  }

  // The profile friends belong to, or null: «Player» is everybody's first offline name, so it gets none.
  friendsProfile() {
    const p = this.companionProfile();
    return p && !(p.kind === 'offline' && !lookupName(p.name)) ? p : null;
  }

  friendsKey() {
    const p = this.companionProfile();
    return p ? p.kind + ':' + (p.id || p.name) : '';
  }

  // A new profile: its own presence socket and friend list.
  connectFriends() {
    this.frKey = this.friendsKey();
    const profile = this.friendsProfile();
    window.SignalBridge.presenceConnect(profile).catch(() => {});
    this.frReq = (this.frReq || 0) + 1;
    this.setState({ fr: null, frErr: '', frUnclaimed: false, frDel: '', friendMsg: '' });
    if (profile) this.loadFriends();
  }

  // Friends from the server; the newest request wins.
  loadFriends() {
    const profile = this.friendsProfile();
    if (!profile) return;
    const req = (this.frReq || 0) + 1;
    this.frReq = req;
    window.SignalBridge.friendsList(profile)
      .then((r) => {
        if (req !== this.frReq) return;
        if (!r || r.unclaimed) {
          this.setState({ fr: { friends: [], incoming: [], outgoing: [] }, frUnclaimed: true, frErr: '' });
          return;
        }
        const before = this.state.fr;
        this.setState({ fr: r, frUnclaimed: false, frErr: '' });
        this.announceRequests(before, r);
        this.loadFriendFaces(r);
      })
      .catch((e) => { if (req === this.frReq) this.setState({ frErr: String(e) }); });
  }

  // A toast (kept in the bell) for every request that arrived since the previous list.
  announceRequests(before, now) {
    if (!before || this.state.frUnclaimed) return;
    const known = new Set(before.incoming.map((f) => f.uuid));
    const fresh = now.incoming.filter((f) => !known.has(f.uuid));
    if (fresh.length) this.play('done');
    fresh.forEach((f) => window.SignalBridge.toast(f.name + ' хочет добавить вас в друзья.', { kind: 'info', title: 'Заявка в друзья', notify: true }));
  }

  // Heads come from the nickname's skin, the same lookup offline accounts use.
  loadFriendFaces(r) {
    r.friends.concat(r.incoming, r.outgoing).forEach((f) => {
      const key = f.name.toLowerCase();
      if (this.state.frFaces[key]) return;
      this.resolveNickSkin(f.name)
        .then((d) => {
          if (d && d.face && d.face.length === 64) this.setState({ frFaces: Object.assign({}, this.state.frFaces, { [key]: d.face }) });
        })
        .catch(() => {});
    });
  }

  onPresenceState(st) {
    this.setState({ presence: st || { state: 'off' } });
  }

  // hello: the socket (re)connected, so the list may be stale; presence: one friend's new status.
  onPresenceEvent(ev) {
    if (!ev) return;
    if (ev.type === 'hello' || ev.type === 'friends_changed') {
      clearTimeout(this.frTimer);
      this.frTimer = setTimeout(() => this.loadFriends(), ev.type === 'hello' ? 0 : 250);
      return;
    }
    if (ev.type === 'hosting') { this.onHosting(ev); return; }
    const fr = this.state.fr;
    if (ev.type !== 'presence' || !fr || !fr.friends.some((f) => f.uuid === ev.uuid)) return;
    const seen = { status: ev.status, detail: ev.detail || null, since: ev.since || null, world: ev.world || null, join: ev.join || null, via: ev.via || null };
    this.setState({ fr: Object.assign({}, fr, { friends: fr.friends.map((f) => (f.uuid === ev.uuid ? Object.assign({}, f, seen) : f)) }) });
  }

  // The server's answer to our world opened to LAN: the address friends get, or why there is none.
  onHosting(ev) {
    const was = this.state.myHost;
    this.setState({ myHost: ev });
    if (ev.error) {
      window.SignalBridge.toast(ev.error, { kind: 'error', title: 'Мир не открыт для друзей', notify: true });
      return;
    }
    if (was && was.join === ev.join) return;
    const how = ev.via === 'direct' ? 'напрямую через роутер (UPnP)' : 'через сервер Signal';
    this.play('done');
    window.SignalBridge.toast('Друзья увидят кнопку «Подключиться к миру». Адрес ' + ev.join + ', ' + how + '.', { kind: 'success', title: 'Мир открыт для друзей' });
  }

  // «Подключиться к миру»: the address goes to the clipboard and, while a game runs, to its multiplayer list.
  joinFriend(f) {
    if (!f.join) return;
    try { if (navigator.clipboard) navigator.clipboard.writeText(f.join).catch(() => {}); } catch (e) {}
    const running = Object.keys(this.state.running || {});
    const name = f.name + (f.world ? ': ' + f.world : '');
    const version = f.detail ? ' Нужна та же версия игры: ' + f.detail + '.' : '';
    if (!running.length) {
      window.SignalBridge.toast('Адрес ' + f.join + ' скопирован. Запустите игру и вставьте его в «Сетевая игра → Прямое подключение».' + version, { kind: 'success', title: 'Мир ' + f.name });
      return;
    }
    window.SignalBridge.addFriendServer(running, name, f.join)
      .then(() => window.SignalBridge.toast('«' + name + '» добавлен в «Сетевую игру» (откройте список заново). Адрес ' + f.join + ' тоже скопирован.' + version, { kind: 'success', title: 'Мир ' + f.name }))
      .catch((e) => window.SignalBridge.toast('Адрес ' + f.join + ' скопирован, но список серверов не обновился: ' + e, { kind: 'info', title: 'Мир ' + f.name }));
  }

  sendFriendRequest() {
    const s = this.state;
    const name = s.nickFriend.trim();
    const profile = this.friendsProfile();
    const say = (msg, ok) => this.setState({ friendMsg: msg, friendMsgOk: ok });
    if (!profile) { say('Смените ник «Player» на свой, чтобы добавлять друзей', false); return; }
    if (!NICK.test(name)) { say('Никнейм: 3–16 символов, латиница, цифры и _', false); return; }
    if (name.toLowerCase() === profile.name.toLowerCase()) { say('Это ваш собственный ник', false); return; }
    const fr = s.fr || { friends: [], outgoing: [] };
    if (fr.friends.some((f) => f.name.toLowerCase() === name.toLowerCase())) { say(name + ' уже у вас в друзьях', false); return; }
    const wasUnclaimed = s.frUnclaimed;
    this.setState({ frBusy: 'add', friendMsg: '' });
    window.SignalBridge.friendsRequest(profile, name)
      .then((r) => {
        const friends = r && r.state === 'friends';
        this.setState({ frBusy: '', nickFriend: '' });
        say(friends ? name + ' теперь у вас в друзьях' : 'Заявка отправлена: ' + name, true);
        this.play(friends ? 'done' : 'place');
        // the first request bound the nickname: now it can go online
        if (wasUnclaimed) window.SignalBridge.presenceConnect(profile).catch(() => {});
        this.loadFriends();
      })
      .catch((e) => { this.setState({ frBusy: '' }); say(String(e), false); });
  }

  answerFriend(f, accept) {
    const profile = this.friendsProfile();
    if (!profile || this.state.frBusy) return;
    this.setState({ frBusy: f.uuid });
    window.SignalBridge.friendsRespond(profile, f.uuid, accept)
      .then(() => {
        this.setState({ frBusy: '' });
        if (accept) this.play('done');
        this.loadFriends();
      })
      .catch((e) => { this.setState({ frBusy: '' }); window.SignalBridge.toast(String(e), { kind: 'error', title: 'Заявка не обработана' }); });
  }

  // A friend goes after a second click («Удалить?»); a sent request is withdrawn at once.
  removeFriend(f, confirm) {
    const profile = this.friendsProfile();
    if (!profile) return;
    if (confirm && this.state.frDel !== f.uuid) {
      this.setState({ frDel: f.uuid });
      clearTimeout(this.frDelTimer);
      this.frDelTimer = setTimeout(() => this.setState({ frDel: '' }), 3000);
      return;
    }
    this.setState({ frDel: '', frBusy: f.uuid });
    window.SignalBridge.friendsRemove(profile, f.uuid)
      .then(() => { this.setState({ frBusy: '' }); this.loadFriends(); })
      .catch((e) => { this.setState({ frBusy: '' }); window.SignalBridge.toast(String(e), { kind: 'error', title: 'Не удалось удалить' }); });
  }

  // Push skin, outer layer, pose and worn cosmetics into the WebGL model.
  syncViewer() {
    const v = this.viewer;
    if (!v) return;
    const s = this.state;
    const d = this.cosDefs();
    v.setSkin(s.skinUrl || DEFAULT_SKIN, s.slim);
    v.setLayerVisible(s.showLayer);
    // the default background (bg.jpg, a sunset) lights the player like the scene; any other one, daylight
    v.setLighting(s.bgMode === 'image' || (s.bgMode === 'file' && !s.bgFileUrl) ? 'sunset' : 'neutral');
    v.setPose(s.pose);
    v.setAnimate(s.opts.animations && !this.reduceMotion);
    COS_SLOTS.forEach(([slot]) => {
      if (slot === 'aura') return;
      const it = d[slot].find((c) => c.id === s.cos[slot]);
      v.setAccessory(slot, it && it.parts ? it.parts : null);
    });
    const cape = d.cape.find((c) => c.id === s.cos.cape);
    v.setAura(s.cos.aura || null);
    // a model cape is an accessory above; only texture capes go on the cloth
    v.setCape(cape && cape.texture ? { key: cape.id, texture: cape.texture, emissive: cape.emissive } : null);
  }

  applyView() {
    if (this.modelEl) this.modelEl.style.transform = 'rotateX(' + this.tilt + 'deg) rotateY(' + this.angle + 'deg)';
    if (this.viewer) this.viewer.setView(this.angle, this.tilt, this.headYaw, this.headPitch);
    if (this.angleEl) this.angleEl.textContent = Math.round(((this.angle % 360) + 360) % 360) + '°';
  }

  trackHead(dt) {
    let yaw = 0;
    let pitch = 0;
    if (this.cursor && this.stageEl) {
      const r = this.stageEl.getBoundingClientRect();
      const cx = r.left + r.width / 2;
      const cy = r.top + r.height * 0.3;
      const nx = Math.max(-1, Math.min(1, (this.cursor.x - cx) / (r.width * 0.55)));
      const ny = Math.max(-1, Math.min(1, (this.cursor.y - cy) / (r.height * 0.6)));
      let a = ((this.angle % 360) + 540) % 360 - 180;
      yaw = Math.max(-55, Math.min(55, nx * 60 - a));
      pitch = Math.max(-28, Math.min(32, -ny * 32));
      if (Math.abs(a) > 110) { yaw = 0; pitch = 0; }
    }
    const k = Math.min(1, dt * 0.009);
    this.headYaw += (yaw - this.headYaw) * k;
    this.headPitch += (pitch - this.headPitch) * k;
  }

  // Interface sound (the speaker button in the header switches them off). A kind plays at most once per
  // SFX_GAP ms, so a burst of clicks or a fast drag in the editor stays one sound per beat.
  play(kind) {
    if (!this.state.opts.sound || !this.sfx) return;
    const a = this.sfx[kind];
    if (!a) return;
    const now = performance.now();
    const last = this.sfxAt || (this.sfxAt = {});
    if (now - (last[kind] || -Infinity) < (SFX_GAP[kind] || 80)) return;
    last[kind] = now;
    try {
      const c = a.cloneNode();
      c.volume = a.volume;
      // the orb and block placement vary in pitch, as in the game
      if (kind === 'done' || kind === 'place') { c.preservesPitch = false; c.playbackRate = 0.85 + Math.random() * 0.3; }
      const p = c.play();
      if (p && p.catch) p.catch(() => {});
    } catch (e) {}
  }

  skinModel(slim, s, withAnim) {
    const showLayer = this.state.showLayer;
    const shade = { front: 1, back: 0.72, right: 0.82, left: 0.82, top: 1.12, bottom: 0.6 };
    const faces = (u, v, w, h, d) => {
      const W = w * s, H = h * s, D = d * s;
      const list = [
        ['front', u + d, v + d, w, h, 'translateZ(' + D / 2 + 'px)'],
        ['back', u + 2 * d + w, v + d, w, h, 'rotateY(180deg) translateZ(' + D / 2 + 'px)'],
        ['right', u, v + d, d, h, 'rotateY(-90deg) translateZ(' + W / 2 + 'px)'],
        ['left', u + d + w, v + d, d, h, 'rotateY(90deg) translateZ(' + W / 2 + 'px)'],
        ['top', u + d, v, w, d, 'rotateX(90deg) translateZ(' + H / 2 + 'px)'],
        ['bottom', u + d + w, v, w, d, 'rotateX(-90deg) translateZ(' + H / 2 + 'px)']
      ];
      return list.map((f) => ({
        w: f[3] * s,
        h: f[4] * s,
        l: -f[3] * s / 2,
        t: -f[4] * s / 2,
        bp: (-f[1] * s) + 'px ' + (-f[2] * s) + 'px',
        tf: f[5],
        light: shade[f[0]]
      }));
    };
    const aw = slim ? 3 : 4;
    const ax = slim ? 5.5 : 6;
    const defs = [
      { part: 'head', x: 0, y: -10, size: [8, 8, 8], uv: [0, 0], ov: [32, 0], inflate: 1, anim: '', pivot: 4 },
      { part: 'body', x: 0, y: 0, size: [8, 12, 4], uv: [16, 16], ov: [16, 32], inflate: 0.5, anim: 'breathe', pivot: 6 },
      { part: 'rarm', x: -ax, y: 0, size: [aw, 12, 4], uv: [40, 16], ov: [40, 32], inflate: 0.5, anim: 'idle-r', pivot: -5 },
      { part: 'larm', x: ax, y: 0, size: [aw, 12, 4], uv: [32, 48], ov: [48, 48], inflate: 0.5, anim: 'idle-l', pivot: -5 },
      { part: 'rleg', x: -2, y: 12, size: [4, 12, 4], uv: [0, 16], ov: [0, 32], inflate: 0.5, anim: '', pivot: -6 },
      { part: 'lleg', x: 2, y: 12, size: [4, 12, 4], uv: [16, 48], ov: [0, 48], inflate: 0.5, anim: '', pivot: -6 }
    ];
    return defs.map((p) => {
      const w = p.size[0], h = p.size[1], d = p.size[2];
      return {
        x: p.x * s,
        y: p.y * s,
        pivot: p.pivot * s,
        anim: withAnim ? p.anim : '',
        part: withAnim ? p.part : '',
        faces: faces(p.uv[0], p.uv[1], w, h, d),
        ov: showLayer ? faces(p.ov[0], p.ov[1], w, h, d) : [],
        ovs: ((w + p.inflate) / w) + ', ' + ((h + p.inflate) / h) + ', ' + ((d + p.inflate) / d)
      };
    });
  }

  skinDefs() {
    return {
      night: { label: 'NightBuilder', rows: ['HHHHHHHH', 'HhHHHHhH', 'HSSSSSSH', 'SSSSSSSS', 'SWBSSBWS', 'SSSNNSSS', 'SSMMMMSS', 'SSSSSSSS'], pal: { H: '#3B2A1E', h: '#2C1F16', S: '#C8946A', W: '#F2F2F2', B: '#3A5BC7', N: '#A6704C', M: '#6E4230' }, skin: 'S', shirt: '#2E7D8C', pants: '#34407A' },
      kira: { label: 'Kira_Redstone', rows: ['RRRRRRRR', 'RrRRRrRR', 'RPPPPPPR', 'RPPPPPPR', 'PWGPPGWP', 'PPPPPPPP', 'PPPMMPPP', 'RPPPPPPR'], pal: { R: '#B5402A', r: '#8E2F20', P: '#F0C8A8', W: '#FFFFFF', G: '#3E9B4F', M: '#C46A5A' }, skin: 'P', shirt: '#7A3B8F', pants: '#2D2D3A' },
      helmet: { label: 'Строитель', rows: ['YYYYYYYY', 'YyYYYYyY', 'YSSSSSSY', 'SSSSSSSS', 'SKWSSWKS', 'SSSSSSSS', 'SSSDDSSS', 'SSSSSSSS'], pal: { Y: '#E2B33C', y: '#C9962A', S: '#D9A57E', K: '#2B2B2B', W: '#FFFFFF', D: '#8B5A44' }, skin: 'S', shirt: '#D2692E', pants: '#3F4A5C' },
      hood: { label: 'Худи', rows: ['AAAAAAAA', 'AaAAAAaA', 'ASSSSSSA', 'ASSSSSSA', 'AWKSSKWA', 'ASSSSSSA', 'ASSMMSSA', 'AAAAAAAA'], pal: { A: '#5B6472', a: '#4A5260', S: '#E7B98F', K: '#26323F', W: '#FFFFFF', M: '#A8644E' }, skin: 'S', shirt: '#5B6472', pants: '#2E3440' },
      copperA: { label: 'Медник', rows: ['CCCCCCCC', 'CcCCCCcC', 'CSSSSSSC', 'SSSSSSSS', 'SWTSSTWS', 'SSSSSSSS', 'SSSMMSSS', 'SSSSSSSS'], pal: { C: '#B8643A', c: '#DC9361', S: '#E2B08A', W: '#FFFFFF', T: '#2F8F7A', M: '#A0563A' }, skin: 'S', shirt: '#B8643A', pants: '#3B2A22' },
      copperB: { label: 'Патина', rows: ['GGGGGGGG', 'GgGGGGgG', 'GSSSSSSG', 'GSSSSSSG', 'GWKSSKWG', 'GSSSSSSG', 'GSSMMSSG', 'GGGGGGGG'], pal: { G: '#4FA58D', g: '#6CC3A6', S: '#C8946A', W: '#FFFFFF', K: '#1F2A2E', M: '#8B5A44' }, skin: 'S', shirt: '#3E8F7A', pants: '#2E3A38' },
      copperC: { label: 'Бронза', rows: ['BBBBBBBB', 'BbBBBBbB', 'BSSSSSSB', 'SSSSSSSS', 'SWASSAWS', 'SSSSSSSS', 'SSSDDSSS', 'SSSSSSSS'], pal: { B: '#9E7B55', b: '#8FA27A', S: '#D9A57E', W: '#FFFFFF', A: '#C9782E', D: '#8B5A44' }, skin: 'S', shirt: '#9E7B55', pants: '#3F3A30' },
      mint: { label: 'Мятный', rows: ['TTTTTTTT', 'TtTTTTtT', 'TSSSSSST', 'SSSSSSSS', 'SWDSSDWS', 'SSSSSSSS', 'SSSRRSSS', 'SSSSSSSS'], pal: { T: '#2F6F6A', t: '#245955', S: '#B57F5C', W: '#FFFFFF', D: '#1F2A2E', R: '#7E4A36' }, skin: 'S', shirt: '#3E8F7A', pants: '#3B3F4A' }
    };
  }

  head(key) {
    const defs = this.skinDefs();
    const sk = defs[key] || defs.night;
    const out = { bg: sk.pal[sk.rows[0][0]] };
    [4, 5, 8, 10].forEach((px) => {
      const parts = [];
      for (let y = 0; y < 8; y++) {
        for (let x = 0; x < 8; x++) {
          if (!x && !y) continue;
          parts.push((x * px) + 'px ' + (y * px) + 'px 0 0 ' + sk.pal[sk.rows[y][x]]);
        }
      }
      out['sh' + px] = parts.join(', ');
    });
    return out;
  }

  // Same box-shadow head as head(), from the 8×8 face the backend cut out of a real skin.
  faceHead(face) {
    const out = { bg: face[0] };
    [4, 5, 8, 10].forEach((px) => {
      const parts = [];
      for (let i = 1; i < 64; i++) parts.push(((i % 8) * px) + 'px ' + (Math.floor(i / 8) * px) + 'px 0 0 ' + face[i]);
      out['sh' + px] = parts.join(', ');
    });
    return out;
  }

  allAccounts() {
    const ms = (this.state.msAccounts || []).map((a) => ({ id: a.id, name: a.name, type: 'ms', face: a.face, skinUrl: a.skin, slim: a.slim }));
    return ms.concat(this.state.accounts || []);
  }

  loadAccounts() {
    return window.SignalBridge.listAccounts()
      .then((list) => { if (list) this.setState({ msAccounts: list }); })
      .catch(() => {});
  }

  // The skin an account puts on the model: its own (Microsoft, its nickname's or one set by hand), else the
  // default one, so the previous account's skin never stays on.
  skinOf(a) {
    return a && a.skinUrl
      ? { skinUrl: a.skinUrl, skinKey: a.skinKey || 'custom', slim: !!a.slim }
      : { skinUrl: '', skinKey: 'default', slim: false };
  }

  // Wearing an account brings its skin to the 3D model; an offline one also checks its nickname's skin.
  pickAccount(a) {
    this.setState(Object.assign({ accountId: a.id, accMenu: false }, this.skinOf(a)));
    if (a.type !== 'ms') this.refreshAccountSkin(a.id);
  }

  // Nickname -> skin (null: the name has none), shared by the nickname field and the accounts.
  // Answers are kept for NICK_SKIN_TTL; a failed lookup is dropped so the next call asks again.
  resolveNickSkin(name) {
    if (!lookupName(name)) return Promise.resolve(null);
    const cache = this.nickSkins || (this.nickSkins = new Map());
    const key = name.toLowerCase();
    const hit = cache.get(key);
    if (hit && Date.now() - hit.at < NICK_SKIN_TTL) return hit.promise;
    const entry = { at: Date.now(), done: false, data: null };
    entry.promise = window.SignalBridge.resolveOfflineSkin(name).then((data) => {
      Object.assign(entry, { done: true, data: data || null });
      return entry.data;
    }, (e) => {
      if (cache.get(key) === entry) cache.delete(key);
      throw e;
    });
    cache.set(key, entry);
    return entry.promise;
  }

  // An offline account looks its nickname's skin up again after ACCOUNT_SKIN_TTL, unless its skin was set by
  // hand. A new skin replaces the old one; «none» never removes a skin found before, because the backend also
  // answers none when Mojang is down and Ely.by has no such name. The model follows only while that account
  // is active and still wears its old skin.
  refreshAccountSkin(id) {
    const a = (this.state.accounts || []).find((x) => x.id === id);
    if (!a || a.skinFrom === 'user') return;
    if (!lookupName(a.name)) {
      // a skin an earlier version fetched for «Player» belongs to that stranger: back to Steve
      if (a.skinFrom === 'mojang' || a.skinFrom === 'elyby') this.dropNickSkin(a);
      return;
    }
    if (a.skinCheckedAt && Date.now() - a.skinCheckedAt < ACCOUNT_SKIN_TTL) return;
    this.resolveNickSkin(a.name).then((data) => {
      const s = this.state;
      const cur = (s.accounts || []).find((x) => x.id === id);
      if (!cur || cur.skinFrom === 'user') return;
      const skin = data ? { skinUrl: data.skin, slim: !!data.slim, skinKey: 'custom', face: data.face, skinFrom: data.source } : {};
      const next = { accounts: s.accounts.map((x) => (x.id === id ? Object.assign({}, x, skin, { skinCheckedAt: Date.now() }) : x)) };
      if (data && s.accountId === id && (s.skinUrl || '') === (cur.skinUrl || '')) Object.assign(next, this.skinOf(skin));
      this.setState(next);
    }).catch(() => {});
  }

  dropNickSkin(a) {
    const s = this.state;
    const plain = { skinUrl: '', slim: false, skinKey: 'default', face: null, skinFrom: '', skinCheckedAt: 0 };
    const next = { accounts: s.accounts.map((x) => (x.id === a.id ? Object.assign({}, x, plain) : x)) };
    if (s.accountId === a.id && (s.skinUrl || '') === (a.skinUrl || '')) Object.assign(next, this.skinOf(plain));
    this.setState(next);
  }

  // A skin set by hand (file, preset, reset, Classic/Slim) becomes the active offline account's own,
  // so switching accounts and back brings it back and the nickname lookup leaves it alone.
  adoptSkin() {
    const s = this.state;
    const a = (s.accounts || []).find((x) => x.id === s.accountId);
    if (!a || ((a.skinUrl || '') === (s.skinUrl || '') && !!a.slim === !!s.slim)) return;
    const own = { skinUrl: s.skinUrl, slim: !!s.slim, skinKey: s.skinKey, skinFrom: 'user', face: null };
    this.setState({ accounts: s.accounts.map((x) => (x.id === a.id ? Object.assign({}, x, own) : x)) });
    if (!s.skinUrl) return;
    faceOf(s.skinUrl).then((face) => {
      const cur = this.state.accounts.find((x) => x.id === a.id);
      if (cur && cur.skinUrl === s.skinUrl) this.setState({ accounts: this.state.accounts.map((x) => (x === cur ? Object.assign({}, x, { face }) : x)) });
    }).catch(() => {});
  }

  // «Лицензия Microsoft»: device code → browser → wait. `msAttempt` drops answers of cancelled tries.
  startMsLogin() {
    const attempt = (this.msAttempt = (this.msAttempt || 0) + 1);
    const alive = () => attempt === this.msAttempt;
    this.setState({ modal: 'ms', msPhase: 'starting', msCode: '', msErr: '', msCopied: false });
    window.SignalBridge.msLoginStart()
      .then((code) => {
        if (!alive()) return null;
        this.setState({ msPhase: 'waiting', msCode: code.userCode, msUri: code.verificationUri });
        try { navigator.clipboard.writeText(code.userCode); this.setState({ msCopied: true }); } catch (e) {}
        window.SignalBridge.openLoginPage(code.verificationUri).catch(() => {});
        return window.SignalBridge.msLoginFinish();
      })
      .then((account) => {
        if (!account || !alive()) return;
        const others = (this.state.msAccounts || []).filter((a) => a.id !== account.id);
        this.setState({ msAccounts: [account].concat(others), modal: null, msPhase: '' });
        this.pickAccount({ id: account.id, type: 'ms', skinUrl: account.skin, slim: account.slim });
        this.play('done');
        window.SignalBridge.toast('Лицензия подключена: ' + account.name + '. Онлайн-серверы и ваш скин доступны в игре.', { kind: 'success', title: 'Вход выполнен' });
      })
      .catch((e) => {
        if (!alive()) return;
        this.setState({ msPhase: 'error', msErr: String(e).replace(/^Error:\s*/, '') });
      });
  }

  cancelMsLogin() {
    this.msAttempt = (this.msAttempt || 0) + 1;
    if (this.state.msPhase === 'waiting' || this.state.msPhase === 'starting') window.SignalBridge.msLoginCancel().catch(() => {});
    this.setState({ msPhase: '' });
  }

  removeAccount(a) {
    if (this.state.accDel !== a.id) {
      clearTimeout(this.accDelTimer);
      this.setState({ accDel: a.id });
      this.accDelTimer = setTimeout(() => this.setState({ accDel: '' }), 3500);
      return;
    }
    clearTimeout(this.accDelTimer);
    const done = () => {
      const rest = this.allAccounts().filter((x) => x.id !== a.id);
      const next = { accDel: '' };
      if (this.state.accountId === a.id) Object.assign(next, { accountId: rest.length ? rest[0].id : 'o1' }, this.skinOf(rest[0]));
      this.setState(next);
      if (next.accountId) this.refreshAccountSkin(next.accountId);
      window.SignalBridge.toast(a.name + (a.type === 'ms' ? ' — вход Microsoft удалён с этого компьютера.' : ' удалён.'), { kind: 'info', title: 'Аккаунт удалён' });
    };
    if (a.type === 'ms') {
      window.SignalBridge.removeAccount(a.id)
        .then(() => { this.setState({ msAccounts: (this.state.msAccounts || []).filter((x) => x.id !== a.id) }); done(); })
        .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Аккаунт не удалён' }));
    } else {
      let rest = (this.state.accounts || []).filter((x) => x.id !== a.id);
      // Keep at least one way to play.
      if (!rest.length && !(this.state.msAccounts || []).length) rest = [{ id: 'o1', name: 'Player', type: 'offline', skin: 'hood' }];
      this.setState({ accounts: rest });
      done();
    }
  }

  makeSkin(key) {
    this.skinCache = this.skinCache || {};
    if (this.skinCache[key]) return this.skinCache[key];
    const h = this.skinDefs()[key];
    if (!h) return '';
    const c = document.createElement('canvas');
    c.width = 64;
    c.height = 64;
    const g = c.getContext('2d');
    const f = (x, y, w, hh, col) => { g.fillStyle = col; g.fillRect(x, y, w, hh); };
    const box = (u, v, w, hh, d, top, bot, side, front, back) => {
      f(u + d, v, w, d, top); f(u + d + w, v, w, d, bot);
      f(u, v + d, d, hh, side); f(u + d, v + d, w, hh, front); f(u + d + w, v + d, d, hh, side); f(u + 2 * d + w, v + d, w, hh, back);
    };
    const hair = h.pal[h.rows[0][0]];
    const skin = h.pal[h.skin];
    box(0, 0, 8, 8, 8, hair, skin, skin, skin, hair);
    for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) f(8 + x, 8 + y, 1, 1, h.pal[h.rows[y][x]]);
    for (let y = 0; y < 3; y++) { f(0, 8 + y, 8, 1, hair); f(16, 8 + y, 8, 1, hair); }
    f(24, 8, 8, 6, hair);
    box(16, 16, 8, 12, 4, h.shirt, h.shirt, h.shirt, h.shirt, h.shirt);
    f(23, 21, 2, 2, skin);
    [[40, 16], [32, 48]].forEach((uv) => { box(uv[0], uv[1], 4, 12, 4, h.shirt, skin, skin, skin, skin); f(uv[0], uv[1] + 4, 16, 4, h.shirt); });
    [[0, 16], [16, 48]].forEach((uv) => { box(uv[0], uv[1], 4, 12, 4, h.pants, '#4A4F57', h.pants, h.pants, h.pants); f(uv[0], uv[1] + 14, 16, 2, '#4A4F57'); });
    const url = c.toDataURL('image/png');
    this.skinCache[key] = url;
    return url;
  }

  readSkin(file) {
    if (!file) return;
    if (!/png$/i.test(file.type || '') && !/\.png$/i.test(file.name || '')) {
      this.setState({ skinErr: 'Нужен файл PNG. Выбран: ' + (file.name || 'неизвестный файл') });
      return;
    }
    const url = URL.createObjectURL(file);
    const img = new Image();
    img.onload = () => {
      URL.revokeObjectURL(url);
      if (img.width !== 64 || (img.height !== 64 && img.height !== 32)) {
        this.setState({ skinErr: 'Размер ' + img.width + '×' + img.height + ' не подходит — нужен 64×64 или 64×32.' });
        return;
      }
      const c = document.createElement('canvas');
      c.width = 64;
      c.height = 64;
      const g = c.getContext('2d');
      g.drawImage(img, 0, 0);
      let slim = false;
      if (img.height === 32) {
        g.drawImage(c, 0, 16, 16, 16, 16, 48, 16, 16);
        g.drawImage(c, 40, 16, 16, 16, 32, 48, 16, 16);
      } else {
        try { slim = g.getImageData(55, 20, 1, 1).data[3] === 0; } catch (e) {}
      }
      this.setState({ pendingSkin: c.toDataURL('image/png'), pendingName: file.name, pendingSlim: slim, skinErr: '' });
    };
    img.onerror = () => { URL.revokeObjectURL(url); this.setState({ skinErr: 'Не удалось прочитать изображение.' }); };
    img.src = url;
  }

  extraVals(s, accent, ver) {
    const tabs = ['play', 'news', 'installs', 'skins', 'cosmetics', 'servers', 'mods', 'shots', 'worlds', 'shop', 'settings'];
    const nav = {};
    const go = {};
    tabs.forEach((t) => {
      nav[t] = s.tab === t ? 'is-active' : '';
      go[t] = () => {
        this.setState({ tab: t, accMenu: false, verMenu: false, friendsOpen: false, aiOpen: false });
        if (t === 'worlds') this.loadWorlds();
        if (t === 'shots') this.loadShots();
        if (t === 'mods') this.refreshMods(ver.id);
        if (t === 'settings') { this.loadMemory(); this.loadJava(false); if (s.settingsTab === 'sys') this.loadUsage(); }
        if (t === 'installs') this.loadInstances(false);
        if (t === 'servers') this.refreshServers();
        if (t === 'news') { this.markNewsSeen(); this.loadNews(false); }
      };
    });
    const closeAll = { accMenu: false, verMenu: false, friendsOpen: false, notifOpen: false };

    const presets = { graphite: ['Тёмно-серый', '#1C1F24'], obsidian: ['Обсидиан', '#110C1A'], deep: ['Глубокий синий', '#0B1830'], emerald: ['Изумрудный', '#0B2A20'] };
    const bgSolid = s.bgMode === 'color' ? s.bgColor : (presets[s.bgPreset] || presets.obsidian)[1];
    const modeDefs = [
      ['shader', 'Шейдеры', 'Живая сцена', 'linear-gradient(180deg, #24305C 0%, #F19159 70%, #1A1015 71%)'],
      ['color', 'Сплошной цвет', 'Любой из палитры', s.bgColor],
      ['preset', 'Пресеты', '4 тёмные темы', 'linear-gradient(90deg, #1C1F24 25%, #110C1A 25% 50%, #0B1830 50% 75%, #0B2A20 75%)'],
      ['file', 'Свой файл', 'Фото или видео', 'repeating-linear-gradient(45deg, rgba(255,255,255,0.06) 0 8px, rgba(255,255,255,0.02) 8px 16px)']
    ];
    const timeDefs = [['sunrise', 'Рассвет'], ['day', 'День'], ['sunset', 'Закат'], ['night', 'Ночь']];
    const prioDefs = [['normal', 'Обычный', 'По умолчанию'], ['high', 'Высокий', 'Больше ресурсов'], ['realtime', 'Реального времени', 'Максимум']];
    const optDefs = {
      rays: ['Лучи солнца (godrays)', 'Рассеянные лучи и свечение в шейдерной сцене.'],
      discord: ['Discord Rich Presence', 'Показывать в Discord, во что и на какой версии вы играете.'],
      backup: ['Автоматический бэкап миров', 'Сохранять копию миров по расписанию.'],
      console: ['Консоль разработчика при запуске', 'Открывать окно логов вместе с игрой.'],
      hide: ['Скрывать лаунчер во время игры', 'Окно вернётся после выхода из Minecraft.'],
      autoUpdate: ['Проверять обновления при запуске', 'Новая версия Signal Launcher предлагается сразу после старта.'],
      snapshots: ['Показывать снапшоты', 'Экспериментальные версии появятся в списке.'],
      oldBeta: ['Старые бета-версии', 'Beta 1.0–1.8 (2010–2011).'],
      oldAlpha: ['Старые альфа-версии', 'Alpha, Infdev, Indev и Classic.'],
      sound: ['Звуки интерфейса', 'Щелчки, наведение и сигнал запуска игры.'],
      animations: ['Анимации интерфейса', 'Плавные переходы, анимация игрока и эффекты наведения.']
    };
    const toggles = (ids) => ids.map((id) => {
      const on = !!s.opts[id];
      return {
        label: optDefs[id][0], desc: optDefs[id][1], on: on, swCls: on ? 'sw is-on' : 'sw',
        toggle: () => this.setState({ opts: Object.assign({}, s.opts, { [id]: !on }) })
      };
    });

    const fv = this.friendsVals();

    const wardrobe = ['night', 'kira', 'helmet', 'hood', 'mint'].map((key) => {
      const h = this.head(key);
      const on = s.skinKey === key || (key === 'night' && s.skinKey === 'default');
      return {
        label: this.skinDefs()[key].label, bg: h.bg, sh10: h.sh10,
        btnCls: on ? 'modbtn is-installed' : 'modbtn is-get', btnLabel: on ? 'Надет ✓' : 'Надеть',
        wear: () => this.setState({ skinUrl: key === 'night' ? '' : this.makeSkin(key), skinKey: key === 'night' ? 'default' : key, slim: false })
      };
    });

    const titles = {
      play: ['Игра', ''],
      news: ['Новости', 'Обновления игры и лаунчера'],
      installs: ['Установки', 'Профили версий, загрузчики и папки игры'],
      skins: ['Скины', 'Гардероб и загрузка своих скинов'],
      servers: ['Серверы', 'Ваши серверы и прямое подключение'],
      mods: ['Моды и Сборки', 'Устанавливаются в: ' + ver.install + ' (' + ver.name + ')'],
      shots: ['Скриншоты', 'Снимки экрана выбранной установки'],
      worlds: ['Миры', 'Сохранения и резервные копии'],
      shop: ['Магазин', 'Темы, плащи и наборы скинов'],
      cosmetics: ['Косметика', 'Плащи, аксессуары и ауры — примерка на 3D-модели'],
      settings: ['Настройки', 'Фон, графика, система и лаунчер']
    };
    const defaultSkin = DEFAULT_SKIN;
    const aikar = '-XX:+UseG1GC -XX:+ParallelRefProcEnabled -XX:MaxGCPauseMillis=200 -XX:+UnlockExperimentalVMOptions -XX:+DisableExplicitGC -XX:+AlwaysPreTouch -XX:G1NewSizePercent=30 -XX:G1MaxNewSizePercent=40 -XX:G1HeapRegionSize=8M -XX:G1ReservePercent=20 -XX:G1HeapWastePercent=5 -XX:G1MixedGCCountTarget=4 -XX:InitiatingHeapOccupancyPercent=15 -XX:G1MixedGCLiveThresholdPercent=90 -XX:G1RSetUpdatingPauseTimePercent=5 -XX:SurvivorRatio=32 -XX:+PerfDisableSharedMem -XX:MaxTenuringThreshold=1';
    const jvmN = s.jvm.trim() ? s.jvm.trim().split(/\s+/).length : 0;
    const plural = pluralize;
    // Real RAM (Rust: sysinfo). The slider stops at what is free right now.
    const memTotal = s.mem ? s.mem.totalMb / 1024 : 16;
    const memAvail = s.mem ? Math.min(memTotal, s.mem.availableMb / 1024) : memTotal * 0.6;
    const memSys = Math.max(0, memTotal - memAvail);
    const memFree = Math.max(0, memAvail - s.ram);
    const memTight = s.ram > memAvail * 0.8;
    const fmtGb = (n) => decimal(n).replace(/[.,]0$/, '');
    const tg = {};
    ['sound', 'animations', 'discord', 'snapshots', 'oldBeta', 'oldAlpha', 'hide', 'console', 'backup', 'autoUpdate', 'offlineSkin', 'companion'].forEach((id) => {
      const on = !!s.opts[id];
      tg[id] = { on: on, swCls: on ? 'sw is-on' : 'sw', rowCls: on ? 'trow is-on' : 'trow', toggle: () => this.setState({ opts: Object.assign({}, s.opts, { [id]: !on }) }) };
    });
    const hex = /^#[0-9a-f]{6}$/i.test(accent) ? accent : '#2A8040';
    const rgb = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
    const mixTo = (t, k) => '#' + rgb.map((c) => Math.round(c + (t - c) * k).toString(16).padStart(2, '0')).join('');
    const pvSlim = s.pendingSkin ? s.pendingSlim : s.slim;

    return {
      tg: tg,
      discordDesc: s.integrations.discord ? 'Показывать в Discord установку и версию' : 'Нужен id приложения Discord (SIGNAL_DISCORD_APP_ID)',
      openNewInst: () => {
        this.setState({ niOpen: true, niName: '', niLoader: 'vanilla', niVersion: this.latestRelease(), niErr: '', accMenu: false, verMenu: false, friendsOpen: false });
        if (!s.mcVersions && !s.mcLoading) this.loadVersions(false);
      },
      closeNewInst: () => this.setState({ niOpen: false }),
      niOpen: s.niOpen,
      niName: s.niName,
      onNiName: (e) => this.setState({ niName: e.target.value, niErr: '' }),
      niLoaders: [['vanilla', 'Vanilla'], ['fabric', 'Fabric'], ['forge', 'Forge'], ['neoforge', 'NeoForge']].map((l) => ({
        label: l[1], on: s.niLoader === l[0], cls: s.niLoader === l[0] ? 'pick is-active' : 'pick',
        // Mod loaders only exist for releases: switching to one drops a snapshot/beta/alpha pick.
        pick: () => {
          // Keep the picked version if the loader supports it, else the newest one it does.
          const fits = this.versionChoices(l[0]);
          const keep = fits.some((v) => v.id === s.niVersion);
          this.setState(keep ? { niLoader: l[0] } : { niLoader: l[0], niVersion: fits.length ? fits[0].id : this.latestRelease() });
        }
      })),
      niVersion: s.niVersion,
      onNiVersion: (e) => this.setState({ niVersion: e.target.value }),
      niGroups: this.versionGroups(s.niLoader, s.niVersion),
      niVersionsNote: this.versionsNote(s.niLoader),
      niPreview: (s.niLoader === 'vanilla' ? this.kindDefs()[this.versionKind(s.niVersion)].prefix : ({ fabric: 'Fabric', forge: 'Forge', neoforge: 'NeoForge' })[s.niLoader]) + ' ' + s.niVersion,
      hasNiErr: !!s.niErr,
      niErr: s.niErr,
      createInst: () => {
        const name = s.niName.trim();
        if (!name) { this.setState({ niErr: 'Введите название установки' }); return; }
        if (name.length > 40) { this.setState({ niErr: 'Название — не длиннее 40 символов' }); return; }
        if (!s.niVersion) { this.setState({ niErr: 'Выберите версию игры' }); return; }
        const kind = s.niLoader === 'vanilla' ? this.versionKind(s.niVersion) : 'release';
        const kd = this.kindDefs()[kind];
        const loaderLabel = ({ vanilla: kd.prefix, fabric: 'Fabric', forge: 'Forge', neoforge: 'NeoForge' })[s.niLoader];
        const inst = {
          id: 'c' + Date.now(),
          name: loaderLabel + ' ' + s.niVersion,
          sub: (s.niLoader === 'vanilla' ? kd.sub : loaderLabel + ' Loader') + ' · новая установка',
          kind: kind,
          loader: s.niLoader === 'fabric' ? 'fabric' : (s.niLoader === 'vanilla' ? 'vanilla' : 'forge'),
          install: name,
          last: 'ещё не запускалась'
        };
        this.addInstance({ id: inst.id, name: name, version: inst.name });
        this.setState({ versionId: inst.id, niOpen: false });
        window.SignalBridge.createInstance({ id: inst.id, name: name, version: inst.name })
          .then(() => (s.niLoader === 'fabric'
            ? window.SignalBridge.ensureFabricApi({ instanceId: inst.id, version: inst.name })
              .catch((e) => window.SignalBridge.toast(String(e), { kind: 'info', title: 'Fabric API не скачан, поставьте его из каталога модов' }))
            : null))
          .then(() => this.loadInstances(false))
          .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Папка установки не создана' }));
        this.play('done');
        window.SignalBridge.toast('«' + name + '» (' + inst.name + ') выбрана в панели запуска.', { kind: 'success', title: 'Установка создана' });
      },
      openGameDir: () => window.SignalBridge.openFolder('%MINECRAFT%'),
      openLogs: () => window.SignalBridge.openFolder('%MINECRAFT%\\instances\\' + ver.id + '\\logs'),
      openShots: () => window.SignalBridge.openFolder('%MINECRAFT%\\instances\\' + ver.id + '\\screenshots'),
      winMin: () => window.SignalBridge.minimize(),
      winMax: () => window.SignalBridge.toggleMaximize(),
      winClose: () => window.SignalBridge.close(),
      showStage: s.tab === 'play' || s.tab === 'cosmetics',
      isCosmetics: s.tab === 'cosmetics',
      cosTabs: COS_SLOTS.filter(([k]) => this.cosDefs()[k].length).map((t) => ({
        label: t[1], on: s.cosTab === t[0], cls: s.cosTab === t[0] ? 'is-active' : '', pick: () => this.setState({ cosTab: t[0] })
      })),
      cosItems: (() => {
        const d = this.cosDefs();
        // A pack or a katana and a cape take the same spot on the back, so wearing one takes the other off.
        const blocker = this.capeBlocker(s.cos);
        return (d[s.cosTab] || []).map((c) => {
          const on = s.cos[s.cosTab] === c.id;
          const bumps = !on && (s.cosTab !== 'cape' ? c.noCape && s.cos.cape : !!blocker);
          const rc = { 'Обычный': '#A3ADBB', 'Редкий': '#7FB6FF', 'Эпический': '#C9A8FF', 'Легендарный': '#F2C27A' }[c.rarity] || '#A3ADBB';
          return {
            name: c.name, rarity: c.rarity, rarityColor: rc,
            preview: c.icon
              ? 'url(' + c.icon + ') center / contain no-repeat, radial-gradient(circle at 50% 62%, rgba(255,255,255,0.09), rgba(255,255,255,0) 65%), #0E1218'
              : 'radial-gradient(circle at 50% 62%, rgba(255,255,255,0.09), rgba(255,255,255,0) 65%), #0E1218',
            glow: on && c.glow && c.glow !== 'none' ? c.glow : 'none',
            on: on, cls: on ? 'cos-item is-on' : 'cos-item',
            state: on ? 'Надето · нажмите, чтобы снять' : bumps ? (s.cosTab !== 'cape' ? 'Примерить · снимет плащ' : 'Примерить · снимет предмет со спины') : 'Примерить',
            stateColor: on ? '#86DDA0' : bumps ? '#E8C27A' : '#8A94A3',
            pick: () => {
              const next = Object.assign({}, s.cos, { [s.cosTab]: on ? null : c.id });
              if (bumps && s.cosTab !== 'cape') next.cape = null;
              else if (bumps) COS_SLOTS.forEach(([k]) => { if (k !== 'cape' && d[k].some((x) => x.id === next[k] && x.noCape)) next[k] = null; });
              this.setState({ cos: next, cosSaved: false });
            }
          };
        });
      })(),
      cosSummary: (() => {
        const d = this.cosDefs();
        const names = COS_SLOTS.map(([k]) => { const it = d[k].find((c) => c.id === s.cos[k]); return it ? it.name : null; }).filter(Boolean);
        return names.length ? 'На игроке: ' + names.join(', ') + '. Покрутите модель, чтобы рассмотреть образ.' : 'Ничего не надето — выберите плащ, аксессуар или ауру.';
      })(),
      cosClear: () => this.setState({ cos: emptyLook(), cosSaved: false }),
      cosSave: () => {
        const look = this.cleanLook(s.cos);
        this.setState({ cos: look, savedCos: look, cosSaved: true });
        this.play('done');
        clearTimeout(this.cosTimer);
        this.cosTimer = setTimeout(() => this.setState({ cosSaved: false }), 1800);
        this.pushLook(look);
      },
      cosSaveLabel: !s.cosSaved ? 'Сохранить образ'
        : s.cosSync.state === 'busy' ? 'Отправка на сервер…'
        : s.cosSync.state === 'synced' ? 'Сохранено и синхронизировано ✓' : 'Сохранено на этом компьютере',
      cosSyncBadge: { busy: 'Синхронизация…', synced: 'Синхронизировано', error: 'Нет связи с сервером', local: 'Локальный предпросмотр' }[s.cosSync.state] || 'Не на сервере',
      cosSyncBadgeCls: s.cosSync.state === 'synced' ? 'local-badge is-synced' : 'local-badge',
      cosSyncNote: {
        busy: 'Синхронизация с сервером Signal…',
        synced: 'Синхронизировано с сервером Signal: образ привязан к профилю и загрузится при следующем входе.',
        empty: 'На сервере Signal образа для этого профиля ещё нет. «Сохранить образ» отправит его.',
        unclaimed: 'Ник ещё не привязан к серверу Signal. «Сохранить образ» привяжет его к этому компьютеру и отправит образ.',
        local: 'В браузерном превью образ хранится только на этом компьютере.',
        error: 'Не удалось связаться с сервером Signal, образ сохранён на этом компьютере.'
      }[s.cosSync.state] || 'Выбор сохраняется кнопкой ниже.',
      hasCosSyncErr: s.cosSync.state === 'error' && !!s.cosSync.msg,
      cosSyncErr: s.cosSync.msg,

      openAi: () => {
        this.setState({ aiOpen: true, accMenu: false, verMenu: false, friendsOpen: false });
        window.SignalBridge.aiStatus()
          .then((st) => this.setState({ aiConfigured: !!(st && st.configured), aiReachable: !!(st && st.reachable) }))
          .catch(() => this.setState({ aiConfigured: false, aiReachable: false }));
      },
      aiStatusText: s.aiConfigured ? 'ИИ подключен' : s.aiConfigured === null ? 'Проверяю ИИ…' : s.aiReachable ? 'ИИ не настроен на сервере' : 'Сервер ИИ недоступен',
      aiStatusCls: s.aiConfigured === false ? 'ai-status off' : 'ai-status',
      closeAi: () => this.setState({ aiOpen: false }),
      aiOpen: s.aiOpen,
      aiModel: 'openrouter/free',
      aiPrompt: s.aiPrompt,
      onAiPrompt: (e) => this.setState({ aiPrompt: e.target.value }),
      aiExamples: ['Хоррор сборка', 'ПвП моды', 'Уютное выживание', 'Техно и автоматизация', 'Магия и приключения', 'Максимум FPS'].map((label) => ({
        label: label, pick: () => this.setState({ aiPrompt: label }, () => this.aiGenerate())
      })),
      aiGenerate: () => this.aiGenerate(),
      aiBusy: s.aiStatus === 'loading' || s.dlBusy,
      aiBtnLabel: s.aiStatus === 'loading' ? 'Подбираю…' : 'Собрать',
      aiLoading: s.aiStatus === 'loading',
      aiLoadingText: s.orKey ? 'Модель openrouter/free подбирает моды…' : 'Подбираю моды под ваш запрос…',
      aiHasResult: s.aiStatus === 'done' && !!s.aiResult,
      aiEmpty: s.aiStatus !== 'loading' && !(s.aiStatus === 'done' && s.aiResult),
      ai: s.aiResult || { title: '', description: '', loader: '', version: '' },
      aiSourceLabel: s.aiSource === 'openrouter' ? 'Подобрано ИИ' : 'Готовый пресет',
      aiSourceColor: s.aiSource === 'openrouter' ? '#7FE3A6' : '#A3ADBB',
      hasAiNote: !!s.aiNote,
      aiNote: s.aiNote,
      aiMods: (s.aiResult ? s.aiResult.mods : []).map((mm) => ({
        name: mm.name, category: mm.category, description: mm.description, checked: !!mm.checked,
        cls: mm.checked ? 'aimod' : 'aimod off',
        toggle: () => this.setState({ aiResult: Object.assign({}, s.aiResult, { mods: s.aiResult.mods.map((x) => x.id === mm.id ? Object.assign({}, x, { checked: !x.checked }) : x) }) })
      })),
      aiChecked: (s.aiResult ? s.aiResult.mods.filter((mm) => mm.checked).length : 0) + ' из ' + (s.aiResult ? s.aiResult.mods.length : 0),
      aiGroups: (() => {
        if (!s.aiResult) return [];
        const titles = ['Оптимизация и ядро', 'Интерфейс и PvP', 'Графика и атмосфера', 'Геймплей и контент'];
        const groups = titles.map((t) => ({ title: t, mods: [] }));
        s.aiResult.mods.forEach((mm) => {
          const c = (mm.category + ' ' + mm.name).toLowerCase();
          let g = 3;
          if (/оптимиз|библиот|ядро|api|lib|core|sodium|lithium|embeddium|modernfix/.test(c)) g = 0;
          else if (/интерфейс|удобств|карт|pvp|пвп|hud|zoom/.test(c)) g = 1;
          else if (/граф|шейдер|звук|декор|атмосф|событ|iris|oculus/.test(c)) g = 2;
          const src = { modrinth: ['Modrinth', '#7FE3A6'], curseforge: ['CurseForge', '#F0A06A'], none: ['не найден', '#F2998B'] }[mm.source] || ['', ''];
          groups[g].mods.push({
            name: mm.name, category: mm.category, description: mm.description, checked: !!mm.checked,
            hasSrc: !!src[0], srcLabel: src[0], srcColor: src[1],
            size: '≈' + decimal(this.modSize(mm.name, g)) + ' МБ',
            cls: mm.checked ? 'aimod on' : 'aimod off',
            ckCls: mm.checked ? 'ck on' : 'ck',
            toggle: () => this.setState({ aiResult: Object.assign({}, s.aiResult, { mods: s.aiResult.mods.map((x) => x.id === mm.id ? Object.assign({}, x, { checked: !x.checked }) : x) }) })
          });
        });
        return groups.filter((g) => g.mods.length).map((g) => {
          const on = g.mods.filter((x) => x.checked).length;
          return Object.assign(g, { count: on + ' из ' + g.mods.length + ' ' + plural(g.mods.length, 'мод', 'мода', 'модов') });
        });
      })(),
      aiCount: s.aiResult ? s.aiResult.mods.filter((mm) => mm.checked).length : 0,
      aiWeight: (() => {
        if (!s.aiResult) return '0 МБ';
        let sum = 0;
        s.aiResult.mods.forEach((mm) => {
          if (!mm.checked) return;
          const c = (mm.category + ' ' + mm.name).toLowerCase();
          const g = /оптимиз|библиот|ядро|api|lib|core|sodium|lithium|embeddium|modernfix/.test(c) ? 0 : (/интерфейс|удобств|карт|pvp|пвп|hud|zoom/.test(c) ? 1 : (/граф|шейдер|звук|декор|атмосф|событ|iris|oculus/.test(c) ? 2 : 3));
          sum += this.modSize(mm.name, g);
        });
        return '≈' + (sum >= 10 ? Math.round(sum) : decimal(sum)) + ' МБ';
      })(),
      aiNoneChecked: !s.aiResult || !s.aiResult.mods.some((mm) => mm.checked),
      startDownload: () => this.startDownload(),
      dlBusy: s.dlBusy,
      dlDone: s.dlDone,
      dlIdle: !s.dlBusy && !s.dlDone,
      dlProgress: s.dlProgress,
      dlStage: s.dlStage,
      aiKeyOpen: s.aiKeyOpen,
      toggleAiKey: () => this.setState({ aiKeyOpen: !s.aiKeyOpen }),
      orKey: s.orKey,
      onOrKey: (e) => this.setState({ orKey: e.target.value.trim() }),
      clearOrKey: () => this.setState({ orKey: '' }),
      aiKeyState: s.orKey ? 'ключ задан' : 'ключ не задан',
      aiKeyDot: s.orKey ? '#5BD07A' : '#F0B95A',
      javaValue: s.javaPath,
      javaOptions: this.javaOptions(s, s.javaPath, 'Автоматически — Java от Mojang под каждую версию (рекомендуется)'),
      onJava: (e) => {
        const v = e.target.value;
        if (v === '__loading') return;
        if (v === '__pick') { this.pickJava().then((info) => { if (info) this.setState({ javaPath: info.path }); }); return; }
        this.setState({ javaPath: v });
      },
      javaHint: s.javaPath
        ? 'Если эта Java не подходит версии игры (например, Java 17 для 1.16.5), лаунчер сам запустит подходящую Java от Mojang и предупредит.'
        : 'Для каждой версии скачивается нужная Java от Mojang: 8 для 1.16.5 и старше, 17 или 21 для новых.',
      refreshJava: () => this.loadJava(true),
      memSummary: fmtGb(memTotal) + ' ГБ всего · свободно ' + fmtGb(memAvail) + ' ГБ',
      ramMax: s.ramMax,
      ramScale: this.ramScale(s.ramMax),
      memAria: 'Система ' + fmtGb(memSys) + ' ГБ, Minecraft ' + s.ram + ' ГБ, свободно ' + fmtGb(memFree) + ' ГБ из ' + memTotal,
      memSysPct: (memSys / memTotal * 100).toFixed(1),
      memMcPct: (s.ram / memTotal * 100).toFixed(1),
      memMcColor: memTight ? '#F0B95A' : '#4CC266',
      memHint: s.ramClamped ? 'Выделение уменьшено до ' + s.ram + ' ГБ: сейчас свободно только ' + fmtGb(memAvail) + ' ГБ' : (memTight ? 'Системе остаётся мало памяти — лучше уменьшить выделение' : (s.ram < 4 ? 'Для модовых сборок лучше выделить 6–8 ГБ' : 'Хороший запас: системе остаётся ' + fmtGb(memFree) + ' ГБ')),
      memHintColor: s.ramClamped || memTight ? '#F0C27A' : (s.ram < 4 ? '#A3ADBB' : '#86DDA0'),
      jvmCount: jvmN + ' ' + plural(jvmN, 'флаг', 'флага', 'флагов'),
      jvmIsAikar: s.jvm.trim() === aikar,
      setAikar: () => this.setState({ jvm: aikar }),
      prioNormal: s.priority !== 'realtime',
      prioNote: s.priority === 'high' ? 'Игра получает больше процессорного времени, фоновые программы — меньше.' : 'Система распределяет ресурсы сама — оптимально для большинства компьютеров.',
      backupDesc: s.opts.backup ? 'Копия миров установки ' + ({ launch: 'перед каждым запуском', daily: 'раз в день', weekly: 'раз в неделю' })[s.backupFreq] + ' · 5 последних копий' : 'Выключен — миры не копируются',
      accentHi: mixTo(255, 0.18),
      accentLo: mixTo(0, 0.28),
      accentGlow: 'rgba(' + rgb.join(',') + ',0.55)',
      nav: nav,
      go: go,
      pageTitle: titles[s.tab][0],
      pageSub: titles[s.tab][1],
      hasPageSub: !!titles[s.tab][1],
      isNews: s.tab === 'news',
      isSkins: s.tab === 'skins',
      isServers: s.tab === 'servers',
      isShots: s.tab === 'shots',
      isWorlds: s.tab === 'worlds',
      isShop: s.tab === 'shop',
      rootCls: (s.opts.animations ? '' : 'reduce ') + this.uiClasses(s),
      rootRef: (el) => { this.rootEl = el; this.syncVars(); },
      onRootMove: (e) => { this.cursor = { x: e.clientX, y: e.clientY }; },
      anyMenu: s.accMenu || s.verMenu || s.friendsOpen || s.notifOpen,
      closeMenus: () => this.setState(closeAll),
      toggleAcc: () => this.setState({ accMenu: !s.accMenu, verMenu: false, friendsOpen: false, notifOpen: false }),
      toggleVer: () => this.setState({ verMenu: !s.verMenu, accMenu: false, friendsOpen: false }),

      bgIsDefault: s.bgMode === 'image' || (s.bgMode === 'file' && !s.bgFileUrl),
      bgIsImage: s.bgMode === 'file' && !!s.bgFileUrl && s.bgFileType !== 'video',
      bgIsVideo: s.bgMode === 'file' && !!s.bgFileUrl && s.bgFileType === 'video',
      bgIsSolid: s.bgMode === 'color' || s.bgMode === 'preset',
      bgSolid: bgSolid,
      bgFileUrl: s.bgFileUrl,
      bgBlur: s.bgBlur,
      bgDim: s.bgDim,
      bgDimA: (s.bgDim / 100).toFixed(2),
      bgLayerRef: (el) => { this.bgLayer = el; },
      bgImgRef: (el) => {
        this.bgImg = el;
        if (el) el.addEventListener('load', () => this.alignBackground());
      },
      bgThumbDefault: s.bgMode === 'image' || (s.bgMode === 'file' && !s.bgFileUrl),
      bgThumbCustom: s.bgMode === 'file' && !!s.bgFileUrl && s.bgFileType !== 'video',
      bgThumbFill: (s.bgMode === 'color' || s.bgMode === 'preset') ? bgSolid : '#11161D',
      bgCurrentLabel: ({ image: 'Закат · Complementary Shaders', file: 'Ваш фон', color: 'Свой цвет', preset: (presets[s.bgPreset] || presets.obsidian)[0] })[s.bgMode] || 'Фон',
      bgCurrentSub: ({ image: 'Скриншот из галереи шейдер-пака', file: s.bgFileName || 'Файл с компьютера', color: s.bgColor, preset: bgSolid })[s.bgMode] || '',
      bgModes: [
        ['image', 'Закат (шейдеры)', 'linear-gradient(180deg, #D9B49A 0%, #C08E73 45%, #6E5A55 70%, #3A3236 100%)'],
        ['color', 'Свой цвет', s.bgColor],
        ['preset', 'Тёмные пресеты', 'linear-gradient(90deg, #1C1F24 25%, #110C1A 25% 50%, #0B1830 50% 75%, #0B2A20 75%)']
      ].map((m) => ({
        label: m[1], preview: m[2], on: s.bgMode === m[0], cls: s.bgMode === m[0] ? 'pick is-active' : 'pick',
        pick: () => this.setState({ bgMode: m[0] })
      })),
      bgIsShaderMode: s.bgMode === 'shader',
      bgIsColorMode: s.bgMode === 'color',
      bgIsPresetMode: s.bgMode === 'preset',
      bgIsFileMode: s.bgMode === 'file',
      shaderTimes: timeDefs.map((m) => ({ label: m[1], on: s.shaderTime === m[0], cls: s.shaderTime === m[0] ? 'chip is-active' : 'chip', pick: () => this.setState({ shaderTime: m[0] }) })),
      shaderSpeed: s.shaderSpeed,
      onShaderSpeed: (e) => this.setState({ shaderSpeed: Number(e.target.value) }),
      bgToggles: toggles(['rays']),
      bgColor: s.bgColor,
      onBgColor: (e) => { const v = e.target.value; this.setState({ bgColor: v }); },
      presets: Object.keys(presets).map((k) => ({ label: presets[k][0], hex: presets[k][1], on: s.bgPreset === k, cls: s.bgPreset === k ? 'pick pick-row is-active' : 'pick pick-row', pick: () => this.setState({ bgPreset: k }) })),
      bgFileLabel: s.bgFileName ? 'Выбрано: ' + s.bgFileName : 'JPG, PNG, WEBP, GIF, MP4 или WEBM с вашего компьютера',
      hasBgFile: !!s.bgFileUrl,
      onBgFile: (e) => {
        const file = e.target.files && e.target.files[0];
        if (!file) return;
        const isVideo = /^video\//.test(file.type);
        this.setState({ bgFileUrl: URL.createObjectURL(file), bgFileType: isVideo ? 'video' : 'image', bgFileName: file.name, bgMode: 'file', bgFileData: '' });
        if (!isVideo && file.size < 2200000) {
          const r = new FileReader();
          r.onload = () => this.setState({ bgFileData: String(r.result) });
          r.readAsDataURL(file);
        }
        e.target.value = '';
      },
      clearBgFile: () => this.setState({ bgFileUrl: '', bgFileType: '', bgFileName: '', bgFileData: '', bgMode: 'image' }),
      onBgBlur: (e) => this.setState({ bgBlur: Number(e.target.value) }),
      onBgDim: (e) => this.setState({ bgDim: Number(e.target.value) }),
      resetBg: () => this.setState({ bgMode: 'image', bgBlur: 8, bgDim: 55, bgFileUrl: '', bgFileType: '', bgFileName: '', bgFileData: '' }),

      stBg: s.settingsTab === 'bg',
      stGame: s.settingsTab === 'game',
      stSys: s.settingsTab === 'sys',
      stUi: s.settingsTab === 'ui',
      stUiCls: s.settingsTab === 'ui' ? 'is-active' : '',
      setStUi: () => this.setState({ settingsTab: 'ui' }),
      ...this.uiVals(s, closeAll),
      stBgCls: s.settingsTab === 'bg' ? 'is-active' : '',
      stGameCls: s.settingsTab === 'game' ? 'is-active' : '',
      stSysCls: s.settingsTab === 'sys' ? 'is-active' : '',
      setStBg: () => this.setState({ settingsTab: 'bg' }),
      setStGame: () => { this.setState({ settingsTab: 'game' }); this.loadJava(false); },
      setStSys: () => { this.setState({ settingsTab: 'sys' }); this.loadUsage(); },
      priorities: prioDefs.map((m) => ({ label: m[1], sub: m[2], on: s.priority === m[0], cls: s.priority === m[0] ? 'pick is-active' : 'pick', pick: () => this.setState({ priority: m[0] }) })),
      prioWarn: s.priority === 'realtime',
      sysToggles: toggles(['discord', 'backup', 'console', 'hide', 'autoUpdate', 'snapshots', 'sound', 'animations']),
      backupOn: !!s.opts.backup,
      backupFreq: s.backupFreq,
      onBackupFreq: (e) => this.setState({ backupFreq: e.target.value }),
      backupStatus: this.autoBackupText(s, ver),
      cacheLabel: s.cacheBusy ? 'Очистка…' : 'Очистить кэш',
      clearCache: () => {
        if (s.cacheBusy) return;
        this.setState({ cacheBusy: true });
        window.SignalBridge.clearCache()
          .then((r) => {
            this.setState({ cacheBusy: false });
            window.SignalBridge.toast(
              r.files ? 'Удалено файлов: ' + r.files + ', освобождено ' + this.fmtBytes(r.bytes) + '. Нужное скачается заново при запуске.' : 'Недокачанных файлов и временных данных не нашлось.',
              { kind: 'success', title: 'Кэш очищен' }
            );
          })
          .catch((e) => {
            this.setState({ cacheBusy: false });
            window.SignalBridge.toast(String(e), { kind: 'error', title: 'Кэш не очищен' });
          });
      },

      friendsOpen: s.friendsOpen,
      toggleFriends: () => {
        const open = !s.friendsOpen;
        this.setState({ friendsOpen: open, accMenu: false, verMenu: false, notifOpen: false, friendMsg: '', frDel: '' });
        if (open && s.frErr) this.loadFriends();
      },
      closeFriends: () => this.setState({ friendsOpen: false }),
      fr: fv,
      nickFriend: s.nickFriend,
      onNickFriend: (e) => this.setState({ nickFriend: e.target.value, friendMsg: '' }),
      onNickFriendKey: (e) => { if (e.key === 'Enter') this.sendFriendRequest(); },
      addByNick: () => this.sendFriendRequest(),
      hasFriendMsg: !!s.friendMsg,
      friendMsg: s.friendMsg,
      friendMsgColor: s.friendMsgOk ? '#86DDA0' : '#F2998B',

      stageRef: (el) => { this.stageEl = el; requestAnimationFrame(() => this.alignBackground()); },
      viewerRef: (el) => {
        if (!el) return;
        if (!this.viewer) this.viewer = new SkinViewer({ scale: STAGE_SCALE, feetFromBottom: STAGE_FEET_GAP, perspective: 1400 });
        this.viewer.mount(el);
        this.syncViewer();
        this.applyView();
      },
      openSkin: () => { this.play('open'); this.drag = null; this.setState(Object.assign({ skinModal: true, pendingSkin: '', pendingName: '', pendingSlim: s.slim, skinErr: '', dragOver: false }, closeAll)); },
      closeSkin: () => this.setState({ skinModal: false, pendingSkin: '', skinErr: '' }),
      skinModal: s.skinModal,
      previewRef: (el) => { this.prevEl = el; this.syncVars(); },
      previewParts: this.skinModel(pvSlim, 6, false),
      prevSize: 384,
      pvClassic: !pvSlim,
      pvSlim: pvSlim,
      pvClassicCls: !pvSlim ? 'is-active' : '',
      pvSlimCls: pvSlim ? 'is-active' : '',
      setPvClassic: () => this.setState(s.pendingSkin ? { pendingSlim: false } : { pendingSlim: false, slim: false }),
      setPvSlim: () => this.setState(s.pendingSkin ? { pendingSlim: true } : { pendingSlim: true, slim: true }),
      dropCls: s.dragOver ? 'dropzone is-over' : 'dropzone',
      onDragOver: (e) => { e.preventDefault(); if (!s.dragOver) this.setState({ dragOver: true }); },
      onDragLeave: () => this.setState({ dragOver: false }),
      onDropSkin: (e) => { e.preventDefault(); this.setState({ dragOver: false }); this.readSkin(e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0]); },
      onSkinFile: (e) => this.readSkin(e.target.files && e.target.files[0]),
      skinPreviewSrc: s.pendingSkin || s.skinUrl || defaultSkin,
      skinFileName: s.pendingName || (s.skinKey === 'custom' ? 'Ваш сохранённый скин' : (s.skinKey === 'default' ? 'Стандартный скин Signal' : this.skinDefs()[s.skinKey].label)),
      skinModelLabel: pvSlim ? 'Slim · руки 3px · 64×64' : 'Classic · руки 4px · 64×64',
      hasSkinErr: !!s.skinErr,
      skinErr: s.skinErr,
      applySkin: () => {
        if (s.pendingSkin) this.setState({ skinUrl: s.pendingSkin, skinKey: 'custom', slim: s.pendingSlim, skinModal: false, pendingSkin: '' });
        else this.setState({ slim: pvSlim, skinModal: false });
        this.play('done');
      },
      resetSkin: () => this.setState({ skinUrl: '', skinKey: 'default', slim: false, pendingSkin: '', pendingName: '', pendingSlim: false, skinErr: '' }),
      wardrobe: wardrobe,

      servers: this.serverCards(s),
      mockShots: [
        { src: '/assets/img/shot-taiga.webp', alt: 'Тайга у озера', file: '2026-09-28_18.42.11.png', size: '2,4 МБ' },
        { src: '/assets/img/shot-mountains.webp', alt: 'Заснеженные горы', file: '2026-09-30_21.07.55.png', size: '1,9 МБ' },
        { src: '/assets/img/shot-jungle.webp', alt: 'Джунгли', file: '2026-10-02_16.15.03.png', size: '3,1 МБ' }
      ],
      mockWorlds: [
        { name: 'Тайга у озера', mode: 'Выживание', diff: 'Сложная', version: 'Release 1.21.9', last: 'сегодня', size: '214 МБ', thumb: '/assets/img/shot-taiga.webp', thumbOpacity: 1, tint: '#1A2A22' },
        { name: 'Горная крепость', mode: 'Выживание', diff: 'Нормальная', version: 'Release 1.21', last: '3 дня назад', size: '156 МБ', thumb: '/assets/img/shot-mountains.webp', thumbOpacity: 1, tint: '#1A2230' },
        { name: 'Креативный полигон', mode: 'Творческий', diff: 'Мирная', version: 'Fabric 1.21.1', last: 'вчера', size: '88 МБ', thumb: '/assets/img/shot-jungle.webp', thumbOpacity: 1, tint: '#16301E' },
        { name: 'Техно-завод', mode: 'Выживание', diff: 'Нормальная', version: 'Forge 1.20.1', last: 'на прошлой неделе', size: '642 МБ', thumb: '/assets/img/shot-mountains.webp', thumbOpacity: 0.35, tint: '#3B2A1E' }
      ],
      shop: [
        { kind: 'Тема лаунчера', name: 'Обсидиан Pro', desc: 'Тёмная тема с фиолетовым неоновым свечением, стеклянными панелями и своим фоном.', icon: 'theme', ink: '#B9A6F2', tileBg: 'rgba(185,166,242,0.12)', tileBorder: 'rgba(185,166,242,0.3)', tags: ['Интерфейс', 'Фон'], price: 'Бесплатно', cta: 'Установить' },
        { kind: 'Плащ', name: 'Плащ «Signal»', desc: 'Градиентный плащ с логотипом лаунчера. Виден другим игрокам Signal.', icon: 'cape', ink: '#8FD3A8', tileBg: 'rgba(143,211,168,0.12)', tileBorder: 'rgba(143,211,168,0.3)', tags: ['Косметика'], price: '[ЦЕНА]', cta: 'Купить' },
        { kind: 'Набор скинов', name: 'Медный век', desc: 'Три скина в медно-бирюзовой гамме: свежая медь, патина и бронза.', icon: 'skins', ink: '#E09A5B', tileBg: 'rgba(224,154,91,0.12)', tileBorder: 'rgba(224,154,91,0.32)', tags: ['3 скина', 'Classic · Slim'], previews: [Object.assign({ label: 'Медник', lift: 6 }, this.head('copperA')), Object.assign({ label: 'Патина', lift: -8 }, this.head('copperB')), Object.assign({ label: 'Бронза', lift: 6 }, this.head('copperC'))], price: '[ЦЕНА]', cta: 'Купить' },
        { kind: 'Шейдеры', name: 'Пак «Золотой час»', desc: 'Закатные кадры с мягкими лучами для главного экрана лаунчера.', icon: 'sun', ink: '#F2C27A', tileBg: 'rgba(242,194,122,0.12)', tileBorder: 'rgba(242,194,122,0.32)', tags: ['Фон', '4K'], price: 'Бесплатно', cta: 'Установить' }
      ].map((it) => Object.assign({}, it, { isTheme: it.icon === 'theme', isCape: it.icon === 'cape', isSkins: it.icon === 'skins', isSun: it.icon === 'sun' }))
    };
  }

  // Which model the game gives the offline nickname (shown next to the skin).
  ensureSkinModel(name, version) {
    const key = name + '|' + version;
    if (this.skinModelKey === key) return;
    this.skinModelKey = key;
    window.SignalBridge.offlineSkinModel(name, version)
      .then((model) => { if (this.skinModelKey === key) this.setState({ skinGame: { key: key, model: model } }); })
      .catch(() => {});
  }

  // «⋯ → Бэкапы миров»
  openBackups(v) {
    this.setState({ instMenu: '', bk: { v: { id: v.id, install: v.install }, data: null, err: '', busy: '', confirm: '' } });
    this.loadBackups(v.id);
  }

  setBk(patch) {
    if (this.state.bk) this.setState({ bk: Object.assign({}, this.state.bk, patch) });
  }

  loadBackups(id) {
    window.SignalBridge.instanceBackups(id)
      .then((data) => { if (this.state.bk && this.state.bk.v.id === id) this.setBk({ data: data, err: '' }); })
      .catch((e) => { if (this.state.bk && this.state.bk.v.id === id) this.setBk({ err: String(e) }); });
  }

  bkConfirm(key) {
    clearTimeout(this.bkTimer);
    this.setBk({ confirm: key });
    this.bkTimer = setTimeout(() => this.setBk({ confirm: '' }), 4000);
  }

  bkCreate(w) {
    const b = this.state.bk;
    if (!b || b.busy) return;
    this.setBk({ busy: 'make:' + w.folder, confirm: '' });
    window.SignalBridge.backupInstanceWorld({ instanceId: b.v.id, folder: w.folder })
      .then((entry) => {
        this.setBk({ busy: '' });
        this.play('done');
        window.SignalBridge.toast(entry.file + ' · ' + this.fmtBytes(entry.size), { kind: 'success', title: 'Бэкап «' + w.name + '» создан' });
        this.loadBackups(b.v.id);
      })
      .catch((e) => {
        this.setBk({ busy: '' });
        window.SignalBridge.toast(String(e), { kind: 'error', title: 'Бэкап не создан' });
      });
  }

  bkRestore(entry, when) {
    const b = this.state.bk;
    if (!b || b.busy) return;
    if (this.state.running[b.v.id]) {
      window.SignalBridge.toast('Сначала закройте игру: мир нельзя заменить, пока он открыт.', { kind: 'info', title: '«' + b.v.install + '» запущена' });
      return;
    }
    const key = 'restore:' + entry.path;
    if (b.confirm !== key) { this.bkConfirm(key); return; }
    clearTimeout(this.bkTimer);
    this.setBk({ busy: key, confirm: '' });
    window.SignalBridge.restoreWorldBackup({ instanceId: b.v.id, path: entry.path })
      .then((r) => {
        this.setBk({ busy: '' });
        this.play('done');
        const safety = r.safety ? 'Прежняя версия мира тоже сохранена: ' + r.safety.split(/[\\/]/).pop() + '.' : 'Мир снова в папке saves.';
        window.SignalBridge.toast(safety, { kind: 'success', title: 'Мир «' + r.folder + '» восстановлен (бэкап от ' + when + ')', timeout: 8000 });
        this.loadBackups(b.v.id);
        if (this.state.realWorlds) this.loadWorlds();
      })
      .catch((e) => {
        this.setBk({ busy: '' });
        window.SignalBridge.toast(String(e), { kind: 'error', title: 'Мир не восстановлен' });
      });
  }

  bkDelete(entry) {
    const b = this.state.bk;
    if (!b || b.busy) return;
    const key = 'del:' + entry.path;
    if (b.confirm !== key) { this.bkConfirm(key); return; }
    clearTimeout(this.bkTimer);
    this.setBk({ confirm: '' });
    window.SignalBridge.deleteWorldBackup({ instanceId: b.v.id, path: entry.path })
      .then(() => this.loadBackups(b.v.id))
      .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Бэкап не удалён' }));
  }

  backupVals(s) {
    const b = s.bk;
    const out = { bkOpen: !!b };
    if (!b) return out;
    const d = b.data;
    out.bkTitle = 'Бэкапы миров «' + b.v.install + '»';
    out.bkLoading = !d && !b.err;
    out.bkErr = b.err;
    out.hasBkErr = !!b.err;
    out.closeBk = () => { if (!b.busy) this.setState({ bk: null }); };
    out.bkOpenSaves = () => window.SignalBridge.openFolder(d ? d.savesDir : '%MINECRAFT%\\instances\\' + b.v.id + '\\saves');
    out.bkOpenDir = () => { if (d) window.SignalBridge.openFolder(d.backupsDir); };
    out.bkReady = !!d;
    if (!d) { out.bkSub = 'Читаю папку saves…'; return out; }
    const row = (e) => {
      const when = new Date(e.created).toLocaleString(LOCALE, { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' });
      const rKey = 'restore:' + e.path;
      const dKey = 'del:' + e.path;
      const restoring = b.busy === rKey;
      return {
        when: when,
        size: this.fmtBytes(e.size),
        auto: e.auto,
        file: e.file,
        restoreLabel: restoring ? 'Восстанавливаю…' : (b.confirm === rKey ? (e.folder ? 'Заменить мир?' : 'Восстановить?') : 'Восстановить'),
        restoreCls: b.confirm === rKey ? 'bk-btn is-confirm' : 'bk-btn',
        restore: () => this.bkRestore(e, when),
        delCls: b.confirm === dKey ? 'im-del is-confirm' : 'im-del',
        delConfirm: b.confirm === dKey,
        delIdle: b.confirm !== dKey,
        del: () => this.bkDelete(e)
      };
    };
    out.bkWorlds = d.worlds.map((w) => {
      const list = d.backups.filter((e) => e.folder === w.folder).map(row);
      const making = b.busy === 'make:' + w.folder;
      return {
        name: w.name,
        meta: [w.version ? 'Minecraft ' + w.version : '', 'игра: ' + this.fmtAgo(w.lastPlayed), this.fmtBytes(w.size)].filter(Boolean).join(' · '),
        icon: w.icon ? window.SignalBridge.fileSrc(w.icon) : '',
        hasIcon: !!w.icon,
        noIcon: !w.icon,
        makeLabel: making ? 'Упаковка…' : 'Создать бэкап',
        make: () => this.bkCreate(w),
        backups: list,
        hasBackups: list.length > 0,
        noBackups: list.length === 0
      };
    });
    const orphans = d.backups.filter((e) => !e.folder).map((e) => Object.assign(row(e), { world: e.world }));
    out.bkOrphans = orphans;
    out.hasOrphans = orphans.length > 0;
    out.bkNoWorlds = d.worlds.length === 0;
    const n = d.backups.length;
    out.bkSub = d.worlds.length + ' ' + this.plural(d.worlds.length, 'мир', 'мира', 'миров') + ' · ' + n + ' ' + this.plural(n, 'бэкап', 'бэкапа', 'бэкапов') + ' · время создания — по вашим часам';
    return out;
  }

  // «⋯ → Проверить целостность файлов»
  repairInstance(v) {
    this.setState({ instMenu: '' });
    if (this.state.running[v.id]) {
      window.SignalBridge.toast('Сначала закройте игру: файлы, которые она использует, нельзя заменить.', { kind: 'info', title: '«' + v.install + '» запущена' });
      return;
    }
    if (this.repairing) {
      window.SignalBridge.toast('Дождитесь окончания текущей проверки.', { kind: 'info', title: 'Проверка уже идёт' });
      return;
    }
    this.repairing = true;
    const title = 'Проверка «' + v.install + '»';
    const pending = window.SignalBridge.toast('Сверка с манифестом Mojang…', { kind: 'loading', title: title, timeout: 0 });
    window.SignalBridge.repairInstance({
      version: v.name,
      onProgress: (p) => pending.update({ kind: 'loading', title: title + ' · ' + p.percent + '%', message: p.label, timeout: 0 })
    })
      .then((r) => {
        this.repairing = false;
        const bad = r.corrupt + r.missing;
        const parts = ['Проверено файлов: ' + r.checked + '.'];
        if (r.corrupt) parts.push('Повреждено: ' + r.corrupt + '.');
        if (r.missing) parts.push('Не хватало: ' + r.missing + '.');
        if (r.repaired) parts.push('Скачано заново: ' + r.repaired + '.');
        if (r.failed.length) parts.push('Не удалось скачать: ' + r.failed.slice(0, 3).join('; ') + (r.failed.length > 3 ? ' и ещё ' + (r.failed.length - 3) : '') + '.');
        if (r.localMissing) parts.push('Не хватает файлов, которые создаёт установщик загрузчика (' + r.localMissing + ') — переустановите загрузчик.');
        const ok = !r.failed.length && !r.localMissing;
        pending.update({
          kind: ok ? 'success' : 'error',
          title: bad === 0 && ok ? 'Файлы «' + v.install + '» в порядке' : (ok ? '«' + v.install + '» исправлена' : '«' + v.install + '»: исправлено не всё'),
          message: parts.join(' '),
          timeout: 10000,
          notify: true
        });
        if (ok) this.play('done');
      })
      .catch((e) => {
        this.repairing = false;
        pending.update({ kind: 'error', title: 'Проверка не удалась', message: String(e), timeout: 10000 });
      });
  }

  // The launcher's skin for an offline game: 64×64 PNGs (base64) with Classic and Slim arms.
  // The game picks the model for an offline nickname itself, so the resource pack gets both.
  skinVariants() {
    const src = this.state.skinUrl || DEFAULT_SKIN;
    const sourceSlim = !!this.state.slim;
    return new Promise((resolve, reject) => {
      const img = new Image();
      img.onload = () => {
        if (img.width !== 64 || (img.height !== 64 && img.height !== 32)) { reject(new Error('Скин должен быть 64×64 или 64×32')); return; }
        const c = document.createElement('canvas');
        c.width = 64;
        c.height = 64;
        const g = c.getContext('2d');
        g.drawImage(img, 0, 0);
        const base = g.getImageData(0, 0, 64, 64);
        if (img.height === 32) legacyToModern(base);
        const wide = new ImageData(new Uint8ClampedArray(base.data), 64, 64);
        const slim = new ImageData(new Uint8ClampedArray(base.data), 64, 64);
        // Right arm, left arm and both sleeve layers.
        [[40, 16], [32, 48], [40, 32], [48, 48]].forEach(([x, y]) => {
          if (sourceSlim) slimToWide(wide, x, y); else wideToSlim(slim, x, y);
        });
        const encode = (data) => {
          g.clearRect(0, 0, 64, 64);
          g.putImageData(data, 0, 0);
          return c.toDataURL('image/png').split(',')[1];
        };
        resolve({ wide: encode(wide), slim: encode(slim) });
      };
      img.onerror = () => reject(new Error('Скин не загрузился'));
      img.src = src;
    });
  }

  // «Путь к Java»: Mojang runtimes already downloaded and every system Java (backend: list_java).
  loadJava(force) {
    if (this.javaLoading || (this.state.javaList && !force)) return;
    this.javaLoading = true;
    window.SignalBridge.listJava()
      .then((list) => { this.javaLoading = false; this.setState({ javaList: list || [] }); })
      .catch(() => { this.javaLoading = false; this.setState({ javaList: [] }); });
  }

  // «Указать путь вручную…» → system file picker → check that it is a working Java.
  pickJava() {
    return window.SignalBridge.pickJavaFile()
      .then((path) => (path ? window.SignalBridge.checkJava(path) : null))
      .then((info) => {
        if (!info) return null;
        const list = (this.state.javaList || []).filter((j) => j.path.toLowerCase() !== info.path.toLowerCase());
        this.setState({ javaList: [info].concat(list) });
        return info;
      })
      .catch((e) => {
        window.SignalBridge.toast(String(e), { kind: 'error', title: 'Это не Java' });
        return null;
      });
  }

  javaLabel(j) {
    const parts = j.path.split(/[\\/]/);
    const bin = parts.lastIndexOf('bin');
    const home = bin > 0 ? parts[bin - 1] : parts[parts.length - 1];
    return 'Java ' + j.major + ' · ' + (j.source === 'mojang' ? 'от Mojang (' + home + ')' : home);
  }

  // Options of a Java select: automatic, the found ones, a custom path that is not among them.
  javaOptions(s, current, autoLabel) {
    const list = s.javaList || [];
    const out = [{ value: '', label: autoLabel }].concat(list.map((j) => ({ value: j.path, label: this.javaLabel(j) })));
    if (current && !list.some((j) => j.path === current)) out.push({ value: current, label: current });
    out.push({ value: '__pick', label: 'Указать путь вручную…' });
    if (!s.javaList) out.splice(1, 0, { value: '__loading', label: 'Ищу установленные Java…' });
    return out;
  }

  javaMajorOf(s, path) {
    const j = (s.javaList || []).find((x) => x.path === path);
    return j ? j.major : 0;
  }

  // «1.16.5», «26.3», «25w14a» → is it a Java 8 version of the game (1.16.5 and older)?
  needsJava8(mc) {
    const m = String(mc || '').match(/^1\.(\d+)(?:\.(\d+))?/);
    if (!m) return false;
    const minor = Number(m[1]);
    return minor < 16 || (minor === 16 && Number(m[2] || 0) <= 5);
  }

  // Warning for a Java that does not fit the installation's game version.
  javaFitNote(mc, major) {
    if (!major || !mc) return '';
    if (this.needsJava8(mc) && major !== 8) return 'Minecraft ' + mc + ' работает только на Java 8 — при запуске вместо Java ' + major + ' автоматически подставится Java 8 от Mojang.';
    if (!this.needsJava8(mc) && /^1\.|^\d{2}\./.test(mc) && major === 8) return 'Minecraft ' + mc + ' не запустится на Java 8 — при запуске автоматически подставится подходящая Java от Mojang.';
    return '';
  }

  // The installation's own launch settings (instance.json → launch), or null when it uses the global ones.
  instLaunchOf(s, id) {
    const inst = (s.instances || []).find((i) => i.id === id);
    return inst && inst.launch && inst.launch.custom ? inst.launch : null;
  }

  // «⋯ → Параметры запуска»
  openInstLaunch(v) {
    const s = this.state;
    const inst = (s.instances || []).find((i) => i.id === v.id);
    const saved = (inst && inst.launch) || {};
    this.setState({
      instMenu: '',
      il: {
        v: { id: v.id, install: v.install, name: v.name },
        custom: !!saved.custom,
        ramGb: Math.min(saved.ramGb || s.ram, s.ramMax),
        java: saved.java || '',
        jvmArgs: saved.jvmArgs != null && saved.custom ? saved.jvmArgs : s.jvm,
        busy: false,
        err: ''
      }
    });
    this.loadJava(false);
  }

  setIl(patch) {
    if (this.state.il) this.setState({ il: Object.assign({}, this.state.il, patch) });
  }

  saveInstLaunch() {
    const il = this.state.il;
    if (!il || il.busy) return;
    this.setIl({ busy: true, err: '' });
    const launch = { custom: il.custom, ramGb: il.ramGb, java: il.java, jvmArgs: il.jvmArgs };
    window.SignalBridge.setInstanceLaunch(Object.assign({ id: il.v.id, name: il.v.install, version: il.v.name }, launch))
      .then(() => {
        const list = (this.state.instances || []).slice();
        const i = list.findIndex((x) => x.id === il.v.id);
        if (i >= 0) list[i] = Object.assign({}, list[i], { launch: launch });
        this.setState({ il: null, instances: list });
        this.loadInstances(false);
        window.SignalBridge.toast(il.custom ? 'ОЗУ ' + il.ramGb + ' ГБ · ' + (il.java ? this.javaLabel({ path: il.java, major: this.javaMajorOf(this.state, il.java) || '?' }) : 'Java автоматически') : 'Используются общие настройки запуска.', { kind: 'success', title: 'Параметры «' + il.v.install + '» сохранены' });
      })
      .catch((e) => this.setIl({ busy: false, err: String(e) }));
  }

  instLaunchVals(s) {
    const il = s.il;
    const out = { ilOpen: !!il };
    if (!il) return out;
    const mc = this.verTarget({ name: il.v.name }).mc;
    const javaMajor = il.java ? this.javaMajorOf(s, il.java) : 0;
    const fit = il.custom ? this.javaFitNote(mc, javaMajor) : '';
    const globalJava = s.javaPath ? this.javaLabel({ path: s.javaPath, major: this.javaMajorOf(s, s.javaPath) || '?' }) : 'автоматически (Java от Mojang под версию)';
    return Object.assign(out, {
      ilTitle: 'Параметры запуска «' + il.v.install + '»',
      ilSub: il.v.name,
      ilCustom: il.custom,
      ilGlobal: !il.custom,
      ilRowCls: il.custom ? 'trow is-on' : 'trow',
      ilSwCls: il.custom ? 'sw is-on' : 'sw',
      ilToggle: () => this.setIl({ custom: !il.custom, err: '' }),
      ilGlobalSummary: 'Сейчас: ' + s.ram + ' ГБ ОЗУ · Java ' + globalJava + ' · ' + (s.jvm.trim() ? s.jvm.trim().split(/\s+/).length + ' ' + this.plural(s.jvm.trim().split(/\s+/).length, 'аргумент', 'аргумента', 'аргументов') + ' JVM' : 'без аргументов JVM'),
      ilRam: il.ramGb,
      ilRamMax: s.ramMax,
      onIlRam: (e) => this.setIl({ ramGb: Math.min(Number(e.target.value), s.ramMax) }),
      ilJava: il.java,
      ilJavaOptions: this.javaOptions(s, il.java, 'Автоматически — Java от Mojang под версию (рекомендуется)'),
      onIlJava: (e) => {
        const v = e.target.value;
        if (v === '__loading') return;
        if (v === '__pick') { this.pickJava().then((info) => { if (info) this.setIl({ java: info.path }); }); return; }
        this.setIl({ java: v });
      },
      ilJavaFit: fit,
      hasIlJavaFit: !!fit,
      ilJvm: il.jvmArgs,
      onIlJvm: (e) => this.setIl({ jvmArgs: e.target.value }),
      ilErr: il.err,
      hasIlErr: !!il.err,
      ilSaveLabel: il.busy ? 'Сохраняю…' : 'Сохранить',
      ilSave: () => this.saveInstLaunch(),
      closeIl: () => { if (!il.busy) this.setState({ il: null }); }
    });
  }

  // «Fabric 1.21.1» → what the catalog filters by for this installation.
  verTarget(ver) {
    const parts = String(ver.name || '').split(' ');
    const first = parts[0].toLowerCase();
    return {
      loader: ['fabric', 'forge', 'neoforge', 'quilt'].indexOf(first) >= 0 ? first : '',
      mc: parts.length > 1 ? parts[parts.length - 1] : ''
    };
  }

  catalogQuery(s, ver) {
    const tg = this.verTarget(ver);
    const usesLoader = s.modType === 'mod' || s.modType === 'modpack';
    let loader = s.modLoader === 'inst' ? tg.loader : (s.modLoader === 'any' ? '' : s.modLoader);
    if (!usesLoader) loader = '';
    return {
      source: s.modSrc === 'cf' ? 'cf' : 'mr',
      query: s.modQuery.trim(),
      projectType: s.modType,
      loader: loader,
      // A modpack becomes its own installation, so it is not tied to the selected one.
      gameVersion: s.modType === 'modpack' || s.modVer !== 'inst' ? '' : tg.mc,
      sort: s.modSort,
      offset: 0
    };
  }

  // Called while rendering: a changed query starts a search (typing waits 350 ms).
  ensureCatalog(q) {
    const key = JSON.stringify(q);
    if (key === this.mrKey) return;
    const prev = this.mrKey ? JSON.parse(this.mrKey) : null;
    const onlyText = !!prev && JSON.stringify(Object.assign({}, prev, { query: q.query })) === key;
    this.mrKey = key;
    clearTimeout(this.mrTimer);
    this.mrTimer = setTimeout(() => this.searchCatalog(q, false), onlyText ? 350 : 0);
  }

  searchCatalog(q, more) {
    this.mrSeq = (this.mrSeq || 0) + 1;
    const seq = this.mrSeq;
    const cur = this.state.mr;
    const { source, ...query } = q;
    this.setState({ mr: Object.assign({}, cur, { loading: true, more: !!more, err: '' }) });
    const bridge = window.SignalBridge;
    const search = source === 'cf' ? bridge.searchCurseforge.bind(bridge) : bridge.searchModrinth.bind(bridge);
    search(Object.assign({}, query, { offset: more ? cur.hits.length : 0 }))
      .then((r) => {
        if (seq !== this.mrSeq) return;
        const was = this.state.mr.hits;
        const fresh = r.hits.map((h) => Object.assign({ source: 'mr' }, h));
        const hits = more ? was.concat(fresh.filter((h) => !was.some((x) => x.projectId === h.projectId))) : fresh;
        this.setState({ mr: { hits: hits, total: r.total, loading: false, more: false, err: '' } });
      })
      .catch((e) => {
        if (seq !== this.mrSeq) return;
        // A failed new search must not leave the previous source's cards under the error.
        const keep = more ? { hits: this.state.mr.hits, total: this.state.mr.total } : { hits: [], total: 0 };
        this.setState({ mr: Object.assign({}, keep, { loading: false, more: false, err: String(e) }) });
      });
  }

  retryCatalog() {
    this.mrKey = '';
    this.setState({ mr: Object.assign({}, this.state.mr, { err: '' }) });
  }

  // installedBy[instance][key] = file. Modrinth: project id (older builds: «mr-<slug>»); CurseForge: «cf:<id>».
  iKey(h) {
    return h.source === 'cf' ? 'cf:' + h.projectId : h.projectId;
  }

  hitKey(mine, h) {
    if (mine[this.iKey(h)]) return this.iKey(h);
    return h.source !== 'cf' && mine['mr-' + h.slug] ? 'mr-' + h.slug : '';
  }

  setBusy(id, value) {
    const busy = Object.assign({}, this.state.modBusy);
    if (value) busy[id] = value; else delete busy[id];
    this.setState({ modBusy: busy });
  }

  rememberInstalled(ver, h, file) {
    const key = this.iKey(h);
    const mine = Object.assign({}, this.state.installedBy[ver.id] || {}, { [key]: file });
    this.setState({
      installedBy: Object.assign({}, this.state.installedBy, { [ver.id]: mine }),
      modMeta: Object.assign({}, this.state.modMeta, { [key]: { t: h.title, i: h.iconUrl || '' } })
    });
  }

  installHit(h) {
    const ver = this.currentVersion();
    if (h.projectType === 'mod' && !this.verTarget(ver).loader) {
      window.SignalBridge.toast('Моды ставятся в установку с Fabric, Forge или NeoForge. Выберите её в панели запуска или создайте новую.', { kind: 'error', title: 'Это ванильная установка' });
      return;
    }
    if (h.source === 'cf') { this.installCfHit(h, ver); return; }
    if (h.projectType === 'modpack') { this.installModpackHit(h); return; }
    if (h.projectType !== 'mod') { this.installAssetHit(h, ver); return; }
    this.setBusy(this.iKey(h), true);
    window.SignalBridge.installMods({
      instanceId: ver.id,
      instanceName: ver.install,
      version: ver.name,
      mods: [{ name: h.title, projectId: h.projectId }]
    })
      .then((report) => this.afterModInstall(h, ver, report))
      .catch((e) => {
        this.setBusy(this.iKey(h), false);
        window.SignalBridge.toast(String(e), { kind: 'error', title: 'Мод не установлен' });
      });
  }

  // Report of install_mods / install_curseforge for a mod.
  afterModInstall(h, ver, report) {
    this.setBusy(this.iKey(h), false);
    const main = report.installed.find((x) => !x.dependency);
    if (!main) {
      const why = report.skipped.length ? report.skipped[0].reason : 'не найден';
      window.SignalBridge.toast(h.title + ': ' + why + '.', { kind: 'error', title: 'Мод не установлен' });
      return;
    }
    this.rememberInstalled(ver, h, main.file);
    this.refreshMods(ver.id);
    const deps = report.installed.filter((x) => x.dependency).map((x) => x.file);
    const skipped = report.skipped.map((x) => x.name + ' — ' + x.reason);
    window.SignalBridge.toast(main.file + (deps.length ? ' + зависимости: ' + deps.join(', ') : '') + (skipped.length ? '. Не установлено: ' + skipped.join('; ') : ''), {
      kind: skipped.length ? 'info' : 'success',
      title: h.title + ' → «' + ver.install + '»',
      target: { tab: 'mods', view: 'installed', kind: 'mod', instanceId: ver.id }
    });
  }

  installAssetHit(h, ver) {
    const kind = h.projectType;
    const what = kind === 'shader' ? 'Шейдер' : 'Ресурс-пак';
    this.setBusy(this.iKey(h), true);
    window.SignalBridge.installAsset({ instanceId: ver.id, version: ver.name, projectId: h.projectId, kind: kind, title: h.title })
      .then((r) => this.afterAssetInstall(h, ver, r.file))
      .catch((e) => {
        this.setBusy(this.iKey(h), false);
        window.SignalBridge.toast(String(e), { kind: 'error', title: what + ' не установлен' });
      });
  }

  afterAssetInstall(h, ver, file) {
    const kind = h.projectType;
    this.setBusy(this.iKey(h), false);
    this.rememberInstalled(ver, h, file);
    this.refreshPacks(ver.id, kind);
    const hint = kind === 'shader' ? 'Включите его во вкладке «Установленные → Шейдеры».' : 'Включите его во вкладке «Установленные → Ресурс-паки».';
    window.SignalBridge.toast(file + ' · ' + hint, { kind: 'success', title: h.title + ' → «' + ver.install + '»', target: { tab: 'mods', view: 'installed', kind: kind, instanceId: ver.id } });
  }

  installModpackHit(h) {
    const key = this.iKey(h);
    if (this.state.modBusy[key]) return;
    this.setBusy(key, 'Скачивание…');
    const run = h.source === 'cf' ? 'installCurseforgeModpack' : 'installModpackProject';
    this.importModpack(h.title, (onProgress) => window.SignalBridge[run]({ projectId: h.projectId, title: h.title, onProgress: (p) => {
      this.setBusy(key, p.percent + '%');
      onProgress(p);
    } }), () => this.setBusy(key, false));
  }

  // CurseForge: authors can allow downloads only on their site — then the page opens instead.
  installCfHit(h, ver) {
    if (h.projectType === 'modpack') { this.installModpackHit(h); return; }
    const key = this.iKey(h);
    this.setBusy(key, true);
    window.SignalBridge.installCurseforge({ instanceId: ver.id, instanceName: ver.install, version: ver.name, projectId: h.projectId, kind: h.projectType, title: h.title })
      .then((report) => {
        if (h.projectType === 'mod') { this.afterModInstall(h, ver, report); return; }
        const main = report.installed[0];
        if (main) this.afterAssetInstall(h, ver, main.file);
        else this.setBusy(key, false);
      })
      .catch((e) => {
        this.setBusy(key, false);
        const text = String(e);
        if (text.indexOf('CF_BLOCKED') >= 0) {
          window.SignalBridge.toast(text.replace(/^.*CF_BLOCKED:\s*/, '') + '. Скачайте файл на сайте и положите его в папку установки.', {
            kind: 'info',
            title: h.title + ': только с сайта CurseForge',
            timeout: 9000,
            actions: [{ label: 'Открыть на CurseForge', onClick: () => window.SignalBridge.openLink(h.pageUrl) }]
          });
          return;
        }
        window.SignalBridge.toast(text, { kind: 'error', title: h.title + ' не установлен' });
      });
  }

  removeHit(h) {
    const ver = this.currentVersion();
    const mine = Object.assign({}, this.state.installedBy[ver.id] || {});
    const key = this.hitKey(mine, h);
    const file = mine[key];
    delete mine[key];
    const asset = h.projectType === 'resourcepack' || h.projectType === 'shader';
    const job = asset
      ? window.SignalBridge.removePack({ instanceId: ver.id, kind: h.projectType, name: file || '' })
      : window.SignalBridge.removeMod({ instanceId: ver.id, file: file || '' });
    job
      .then(() => {
        this.setState({ installedBy: Object.assign({}, this.state.installedBy, { [ver.id]: mine }) });
        if (asset) this.refreshPacks(ver.id, h.projectType); else this.refreshMods(ver.id);
        window.SignalBridge.toast((file || h.title) + ' удалён из «' + ver.install + '».', { kind: 'info', title: asset ? 'Пак удалён' : 'Мод удалён' });
      })
      .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Не удалось удалить' }));
  }

  // Modrinth category slugs → labels (unknown ones are shown as they come).
  catLabel(c) {
    const map = {
      adventure: 'Приключения', cursed: 'Проклятое', decoration: 'Декор', economy: 'Экономика', equipment: 'Снаряжение', food: 'Еда',
      'game-mechanics': 'Механики', library: 'Библиотека', magic: 'Магия', management: 'Управление', minigame: 'Мини-игры', mobs: 'Мобы',
      optimization: 'Оптимизация', social: 'Общение', storage: 'Хранение', technology: 'Технологии', transportation: 'Транспорт',
      utility: 'Утилиты', worldgen: 'Генерация мира', challenging: 'Хардкор', combat: 'Бой', 'kitchen-sink': 'Всё и сразу',
      lightweight: 'Лёгкая', multiplayer: 'Мультиплеер', quests: 'Квесты', audio: 'Звуки', blocks: 'Блоки', 'core-shaders': 'Core-шейдеры',
      entities: 'Существа', environment: 'Окружение', fonts: 'Шрифты', gui: 'Интерфейс', items: 'Предметы', locale: 'Язык',
      modded: 'Для модов', models: 'Модели', realistic: 'Реализм', simplistic: 'Минимализм', themed: 'Тематический', tweaks: 'Твики',
      'vanilla-like': 'Как в ванилле', atmosphere: 'Атмосфера', bloom: 'Свечение', cartoon: 'Мультяшный', 'colored-lighting': 'Цветной свет',
      fantasy: 'Фэнтези', foliage: 'Листва', high: 'Мощный ПК', low: 'Слабый ПК', medium: 'Средний ПК', 'path-tracing': 'Трассировка пути',
      pbr: 'PBR', potato: 'Очень слабый ПК', reflections: 'Отражения', screenshot: 'Для скриншотов', 'semi-realistic': 'Полуреализм', shadows: 'Тени'
    };
    return map[c] || c;
  }

  catalogCards(s, ver) {
    const loaderNames = { fabric: 'Fabric', forge: 'Forge', neoforge: 'NeoForge', quilt: 'Quilt', iris: 'Iris', optifine: 'OptiFine', canvas: 'Canvas', vanilla: 'Vanilla' };
    const fmt = (n) => {
      if (n >= 1e9) return decimal(n / 1e9) + ' млрд';
      if (n >= 1e6) return decimal(n / 1e6) + ' млн';
      if (n >= 1e3) return Math.round(n / 1e3) + ' тыс';
      return String(n);
    };
    const mine = s.installedBy[ver.id] || {};
    return s.mr.hits.map((h) => {
      const cf = h.source === 'cf';
      const pack = h.projectType === 'modpack';
      const on = !pack && !!this.hitKey(mine, h);
      const busy = s.modBusy[this.iKey(h)];
      // The author keeps downloads on curseforge.com: no install button that would only fail.
      const siteOnly = cf && h.allowDistribution === false && !on;
      const cats = h.categories || [];
      let label = pack ? 'Создать установку' : 'Установить';
      if (on) label = 'Установлен ✓';
      if (siteOnly) label = 'Открыть на CurseForge';
      if (busy) label = typeof busy === 'string' ? busy : 'Установка…';
      const pageUrl = cf ? h.pageUrl : 'https://modrinth.com/' + (h.projectType || 'mod') + '/' + (h.slug || h.projectId);
      return {
        t: h.title,
        d: h.description,
        i: h.iconUrl || '',
        hasIcon: !!h.iconUrl,
        noIcon: !h.iconUrl,
        byline: h.author ? 'от ' + h.author : (cf ? 'CurseForge' : 'Modrinth'),
        dl: fmt(h.downloads),
        follows: fmt(h.follows),
        hasFollows: !cf,
        pageTitle: cf ? 'Открыть на curseforge.com' : 'Открыть на modrinth.com',
        siteOnly: siteOnly,
        tags: cats.filter((c) => !loaderNames[c]).slice(0, 3).map((c) => this.catLabel(c)),
        loaders: cats.filter((c) => loaderNames[c]).map((c) => loaderNames[c]),
        btnCls: on ? 'modbtn is-installed' : (siteOnly ? 'modbtn is-site' : 'modbtn is-get'),
        btnLabel: label,
        page: () => { if (pageUrl) window.SignalBridge.openLink(pageUrl); },
        toggle: () => {
          if (busy) return;
          if (siteOnly) { if (pageUrl) window.SignalBridge.openLink(pageUrl); return; }
          if (on) this.removeHit(h); else this.installHit(h);
        }
      };
    });
  }

  refreshPacks(instanceId, kind) {
    window.SignalBridge.listPacks(instanceId, kind)
      .then((list) => { if (list) this.setState({ instPacks: Object.assign({}, this.state.instPacks, { [instanceId + ':' + kind]: list }) }); })
      .catch(() => {});
  }

  // «Complementary_Shaders_v5.zip» → «Complementary Shaders»
  packTitle(name) {
    const base = name.replace(/\.zip$/i, '');
    const head = base.split(/[-_+ ](?=[vr]?\d)/)[0] || base;
    return head.replace(/[-_]+/g, ' ').trim() || base;
  }

  togglePack(ver, kind, p) {
    window.SignalBridge.setPackEnabled({ instanceId: ver.id, kind: kind, name: p.name, enabled: !p.enabled })
      .then(() => this.refreshPacks(ver.id, kind))
      .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Не удалось переключить пак' }));
  }

  deletePack(ver, kind, name) {
    clearTimeout(this.delTimer);
    this.setState({ modDel: '' });
    window.SignalBridge.removePack({ instanceId: ver.id, kind: kind, name: name })
      .then(() => {
        const mine = Object.assign({}, this.state.installedBy[ver.id] || {});
        Object.keys(mine).forEach((k) => { if (mine[k] === name) delete mine[k]; });
        this.setState({ installedBy: Object.assign({}, this.state.installedBy, { [ver.id]: mine }) });
        this.refreshPacks(ver.id, kind);
        window.SignalBridge.toast(name + ' удалён из «' + ver.install + '».', { kind: 'info', title: 'Пак удалён' });
      })
      .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Не удалось удалить пак' }));
  }

  // Rows of «Установленные → Ресурс-паки / Шейдеры», shaped like the mod rows.
  packRows(s, ver, kind) {
    const pl = s.instPacks[ver.id + ':' + kind];
    if (!pl) return null;
    const mine = s.installedBy[ver.id] || {};
    const byFile = {};
    Object.keys(mine).forEach((id) => { if (s.modMeta[id]) byFile[mine[id]] = s.modMeta[id]; });
    return pl.entries.map((p) => {
      const meta = byFile[p.name];
      const key = kind + ':' + p.name;
      const confirm = s.modDel === key;
      const title = meta ? meta.t : this.packTitle(p.name);
      return {
        hasUpdate: false, updLabel: '', updBtnLabel: '', update: () => {},
        title: title,
        file: p.name + (p.isDir ? ' (папка)' : ''),
        size: this.fmtBytes(p.size),
        icon: meta && meta.i ? meta.i : '',
        hasIcon: !!(meta && meta.i),
        noIcon: !(meta && meta.i),
        on: p.enabled,
        off: !p.enabled,
        rowCls: p.enabled ? 'im-row' : 'im-row is-off',
        swCls: p.enabled ? 'sw is-on' : 'sw',
        swLabel: (p.enabled ? 'Выключить ' : 'Включить ') + title,
        delCls: confirm ? 'im-del is-confirm' : 'im-del',
        confirm: confirm,
        idle: !confirm,
        toggle: () => this.togglePack(ver, kind, p),
        del: () => {
          if (!confirm) {
            clearTimeout(this.delTimer);
            this.setState({ modDel: key });
            this.delTimer = setTimeout(() => this.setState({ modDel: '' }), 3500);
            return;
          }
          this.deletePack(ver, kind, p.name);
        }
      };
    });
  }

  // Shaders need Iris (Fabric, NeoForge) or Oculus (Forge).
  installShaderLoader(ver) {
    const tg = this.verTarget(ver);
    const oculus = tg.loader === 'forge';
    this.installHit({ projectId: oculus ? 'GchcoXML' : 'YL57xq9U', slug: oculus ? 'oculus' : 'iris', projectType: 'mod', title: oculus ? 'Oculus' : 'Iris Shaders', iconUrl: '' });
  }

  // «⋯ → Экспорт в .mrpack»
  openExport(v) {
    this.setState({ instMenu: '', exp: { v: { id: v.id, install: v.install, name: v.name }, phase: 'loading', err: '' } });
    window.SignalBridge.exportPreview(v.id)
      .then((p) => {
        const e = this.state.exp;
        if (!e || e.v.id !== v.id) return;
        const acc = this.allAccounts().find((a) => a.id === this.state.accountId);
        const sel = {};
        p.entries.forEach((x) => { sel[x.path] = x.selected; });
        this.setState({ exp: Object.assign({}, e, {
          phase: 'form', entries: p.entries, sel: sel, open: {}, label: p.version, loaderVersion: p.loaderVersion,
          name: p.name, version: '1.0.0', author: acc ? acc.name : '', summary: ''
        }) });
      })
      .catch((err) => {
        const e = this.state.exp;
        if (e && e.v.id === v.id) this.setState({ exp: Object.assign({}, e, { phase: 'error', err: String(err) }) });
      });
  }

  setExp(patch) {
    if (this.state.exp) this.setState({ exp: Object.assign({}, this.state.exp, patch) });
  }

  runExport() {
    const e = this.state.exp;
    if (!e || e.phase !== 'form') return;
    const name = (e.name || '').trim();
    const version = (e.version || '').trim();
    if (!name || !version) { this.setExp({ err: 'Укажите название и версию сборки' }); return; }
    const include = e.entries.filter((x) => e.sel[x.path]).map((x) => x.path);
    if (!include.length) { this.setExp({ err: 'Отметьте хотя бы один файл' }); return; }
    const fileName = (name + '-' + version).replace(/[\\/:*?"<>|\s]+/g, '_') + '.mrpack';
    window.SignalBridge.saveMrpackDialog(fileName)
      .then((dest) => {
        if (!dest) return;
        this.setExp({ phase: 'busy', err: '', pct: 0, stage: 'Подготовка…' });
        return window.SignalBridge.exportMrpack({
          instanceId: e.v.id,
          dest: dest,
          meta: { name: name, version: version, summary: (e.summary || '').trim(), author: (e.author || '').trim() },
          include: include,
          onProgress: (p) => this.setExp({ pct: Math.round(p.percent), stage: p.label })
        }).then((r) => {
          this.setState({ exp: null });
          this.play('done');
          window.SignalBridge.toast(r.path, {
            kind: 'success',
            title: '«' + name + '» ' + version + ' экспортирована',
            message: 'С Modrinth: ' + r.downloads + ' · в overrides: ' + r.overrides + ' · ' + this.fmtBytes(r.size) + '\n' + r.path,
            timeout: 9000,
            actions: [{ label: 'Показать файл', onClick: () => window.SignalBridge.openFolder(r.path) }]
          });
        });
      })
      .catch((err) => this.setExp({ phase: 'form', err: String(err) }));
  }

  exportVals(s) {
    const e = s.exp;
    const out = { expOpen: !!e };
    if (!e) return out;
    out.expTitle = 'Экспорт «' + e.v.install + '»';
    out.expSub = (e.label || e.v.name) + (e.loaderVersion ? ' · загрузчик ' + e.loaderVersion : '') + ' → .mrpack';
    out.expLoading = e.phase === 'loading';
    out.expFailed = e.phase === 'error';
    out.expForm = e.phase === 'form' || e.phase === 'busy';
    out.expBusy = e.phase === 'busy';
    out.expIdle = e.phase !== 'busy';
    out.expErr = e.err || '';
    out.hasExpErr = !!e.err && e.phase !== 'error';
    out.closeExp = () => { if (e.phase !== 'busy') this.setState({ exp: null }); };
    out.retryExp = () => this.openExport(e.v);
    if (!e.entries) return out;
    const field = (k) => (ev) => this.setExp({ [k]: ev.target.value, err: '' });
    out.expName = e.name;
    out.expVersion = e.version;
    out.expAuthor = e.author;
    out.expSummary = e.summary;
    out.onExpName = field('name');
    out.onExpVersion = field('version');
    out.onExpAuthor = field('author');
    out.onExpSummary = field('summary');
    out.expPct = e.pct || 0;
    out.expStage = e.stage || '';
    out.expRun = () => this.runExport();
    out.expRunLabel = e.phase === 'busy' ? 'Экспорт… ' + (e.pct || 0) + '%' : 'Сохранить .mrpack';
    const groups = [['mods', 'Моды'], ['resourcepacks', 'Ресурс-паки'], ['shaderpacks', 'Шейдеры'], ['config', 'Настройки модов (config)'], ['saves', 'Миры'], ['other', 'Прочее']];
    let remote = 0;
    let local = 0;
    let localSize = 0;
    out.expGroups = groups.map(([g, label]) => {
      const items = e.entries.filter((x) => x.group === g);
      if (!items.length) return null;
      const on = items.filter((x) => e.sel[x.path]);
      on.forEach((x) => { if (x.modrinth) remote += 1; else { local += 1; localSize += x.size; } });
      const all = on.length === items.length;
      const open = !!e.open[g];
      const setMany = (value) => {
        const sel = Object.assign({}, e.sel);
        items.forEach((x) => { sel[x.path] = value; });
        this.setExp({ sel: sel, err: '' });
      };
      return {
        label: label,
        count: on.length + ' из ' + items.length,
        all: all,
        some: on.length > 0 && !all,
        boxCls: all ? 'xp-box is-on' : (on.length ? 'xp-box is-some' : 'xp-box'),
        toggleAll: () => setMany(!all),
        open: open,
        caretRot: open ? 90 : 0,
        toggleOpen: () => this.setExp({ open: Object.assign({}, e.open, { [g]: !open }) }),
        items: items.map((x) => {
          const sel = !!e.sel[x.path];
          return {
            name: x.path.indexOf('/') >= 0 && g !== 'other' ? x.path.slice(x.path.indexOf('/') + 1) : x.path,
            size: this.fmtBytes(x.size),
            on: sel,
            boxCls: sel ? 'xp-box is-on' : 'xp-box',
            remote: x.modrinth,
            local: !x.modrinth,
            toggle: () => this.setExp({ sel: Object.assign({}, e.sel, { [x.path]: !sel }), err: '' })
          };
        })
      };
    }).filter(Boolean);
    out.expSummaryLine = 'Ссылками на Modrinth: ' + remote + ' · внутрь архива (overrides): ' + local + (local ? ' · ' + this.fmtBytes(localSize) : '');
    return out;
  }

  // Wardrobe items per slot, built once from src/catalog.items.json (the source of the published catalog).
  cosDefs() {
    if (this.cosCache) return this.cosCache;
    const asset = (p) => (p ? '/assets/cosmetics/' + p : null);
    const out = emptyLook();
    COS_SLOTS.forEach(([k]) => { out[k] = []; });
    CATALOG.items.forEach((it) => {
      if (!out[it.slot]) return;
      const parts = it.parts
        ? it.parts.map((p) => ({ attach: p.attach, url: asset(p.model) }))
        : it.model ? [{ attach: it.attach || SLOT_ATTACH[it.slot] || 'body', url: asset(it.model) }] : null;
      out[it.slot].push({
        id: it.id,
        name: (LANG === 'en' ? it.name.en : it.name.ru) || it.name.ru || it.name.en || it.id,
        rarity: RARITY[it.rarity] || RARITY.common,
        icon: asset(it.icon),
        parts,
        texture: asset(it.texture),
        emissive: asset(it.emissive),
        noCape: !!it.noCape && it.slot !== 'cape',
        glow: CAPE_GLOW[it.id] || 'none'
      });
    });
    this.cosCache = out;
    return out;
  }

  parsePack(text, prompt) {
    if (!text) return null;
    let t = String(text).replace(/```json/gi, '').replace(/```/g, '');
    const a = t.indexOf('{');
    const b = t.lastIndexOf('}');
    if (a < 0 || b <= a) return null;
    let obj;
    try { obj = JSON.parse(t.slice(a, b + 1)); } catch (e) { return null; }
    if (!obj || !Array.isArray(obj.mods)) return null;
    const mods = obj.mods.filter((m) => m && (m.name || m.title)).slice(0, 20).map((m) => ({
      name: String(m.name || m.title).slice(0, 60),
      slug: String(m.slug || '').slice(0, 64),
      category: String(m.category || 'Разное').slice(0, 30),
      description: String(m.description || m.why || m.reason || '').slice(0, 180)
    }));
    if (!mods.length) return null;
    const loaderRaw = String(obj.loader || '');
    const loader = /neo/i.test(loaderRaw) ? 'NeoForge' : (/forge/i.test(loaderRaw) ? 'Forge' : 'Fabric');
    const vm = String(obj.version || '').match(/\d+\.\d+(\.\d+)?/);
    return {
      title: this.aiPackTitle(obj.title || obj.name, prompt),
      description: String(obj.description || '').slice(0, 240),
      loader: loader,
      version: vm ? vm[0] : '1.21.1',
      mods: mods
    };
  }

  // The installation's name: the model's title unless it is only a loader and a version
  // («Fabric 1.20.1»), else the player's own idea («космос и ракеты» → «Космос и ракеты»).
  aiPackTitle(title, prompt) {
    const t = String(title || '').replace(/\s+/g, ' ').trim();
    const technical = /^(сборка|modpack)?\s*(fabric|forge|neoforge|quilt|vanilla|minecraft)?\s*(loader)?\s*[\d.]*$/i.test(t);
    if (t && !technical) return t.slice(0, 60);
    const p = String(prompt || '').replace(/\s+/g, ' ').trim().replace(/[.!?…]+$/, '');
    if (!p) return 'Сборка от ИИ';
    const short = p.length > 40 ? p.slice(0, 40).replace(/\s+\S*$/, '') + '…' : p;
    return short.charAt(0).toUpperCase() + short.slice(1);
  }

  // «Арена без лагов» twice → «Арена без лагов (2)», so installations stay apart in the lists.
  uniqueInstallName(name) {
    const taken = new Set((this.state.instances || []).map((i) => String(i.name || '').toLowerCase()));
    if (!taken.has(name.toLowerCase())) return name;
    for (let n = 2; ; n++) {
      const next = name + ' (' + n + ')';
      if (!taken.has(next.toLowerCase())) return next;
    }
  }

  fallbackPack(prompt) {
    const q = String(prompt || '').toLowerCase();
    const m = (name, category, description) => ({ name: name, category: category, description: description });
    const presets = {
      horror: { title: 'Туманный ужас', description: 'Атмосферный хоррор: существа в тумане, кровавые луны, реалистичный звук и тяжёлая тишина пещер.', loader: 'Forge', version: '1.20.1', mods: [
        m('The Man From The Fog', 'Существа', 'Фигура в тумане, которая наблюдает за игроком и преследует его.'),
        m('From The Fog', 'Существа', 'Херобрин с собственным поведением и событиями в мире.'),
        m('Cave Dweller', 'Существа', 'Охотник, который выслеживает игрока в тёмных пещерах.'),
        m('Enhanced Celestials', 'События', 'Кровавые и голубые луны с усиленным спавном мобов.'),
        m('Sound Physics Remastered', 'Звук', 'Реверберация и затухание звука в пещерах и помещениях.'),
        m('AmbientSounds', 'Звук', 'Живые фоновые звуки леса, пещер и ночи.'),
        m('Embeddium', 'Оптимизация', 'Порт Sodium для Forge: стабильный FPS даже в густом тумане.'),
        m('Oculus', 'Графика', 'Шейдеры на Forge — для по-настоящему тёмных ночей.'),
        m('Corpse', 'Выживание', 'После смерти остаётся тело с вещами, их можно вернуть.'),
        m('Just Enough Items (JEI)', 'Интерфейс', 'Просмотр рецептов и предметов.')
      ] },
      pvp: { title: 'Арена без лагов', description: 'Лёгкая PvP-сборка: максимум FPS, чистый интерфейс, зум и полезная информация в бою.', loader: 'Fabric', version: '1.21.1', mods: [
        m('Fabric API', 'Библиотека', 'Базовая библиотека для модов Fabric.'),
        m('Sodium', 'Оптимизация', 'Современный рендер: высокий и ровный FPS в бою.'),
        m('Lithium', 'Оптимизация', 'Ускоряет игровую логику без изменения механик.'),
        m('FerriteCore', 'Оптимизация', 'Снижает потребление оперативной памяти.'),
        m('ImmediatelyFast', 'Оптимизация', 'Быстрее отрисовывает интерфейс, частицы и текст.'),
        m('Entity Culling', 'Оптимизация', 'Не рендерит сущности, которых не видно.'),
        m('Zoomify', 'Удобство', 'Плавный зум, чтобы видеть противника издалека.'),
        m('AppleSkin', 'Интерфейс', 'Показывает насыщение и восстановление от еды.'),
        m('Better Ping Display', 'Интерфейс', 'Точный пинг игроков в списке Tab.'),
        m('Dynamic FPS', 'Оптимизация', 'Снижает нагрузку, когда окно игры свёрнуто.'),
        m('Mod Menu', 'Интерфейс', 'Список модов и их настройки из главного меню.')
      ] },
      tech: { title: 'Индустрия 2.0', description: 'Механизмы, автоматизация и цифровое хранилище: от первых шестерёнок до заводов.', loader: 'NeoForge', version: '1.21.1', mods: [
        m('Create', 'Технологии', 'Механизмы на вращении: прессы, конвейеры, поезда.'),
        m('Mekanism', 'Технологии', 'Энергия, переработка руды и высокотехнологичные машины.'),
        m('Applied Energistics 2', 'Хранение', 'Цифровая сеть хранения и автокрафт.'),
        m('Storage Drawers', 'Хранение', 'Компактные ящики для больших объёмов ресурсов.'),
        m('Sophisticated Backpacks', 'Снаряжение', 'Рюкзаки с улучшениями и фильтрами.'),
        m('Sophisticated Core', 'Библиотека', 'Нужен для Sophisticated Backpacks.'),
        m('Just Enough Items (JEI)', 'Интерфейс', 'Рецепты всех машин в одном окне.'),
        m('Jade', 'Интерфейс', 'Подсказка о блоке или машине под прицелом.'),
        m("Xaero's Minimap", 'Карта', 'Миникарта, чтобы не заблудиться между цехами.'),
        m('Sodium', 'Оптимизация', 'Держит FPS на больших заводах.')
      ] },
      magic: { title: 'Тайные тропы', description: 'Магия, заклинания и новые измерения для долгого приключения.', loader: 'NeoForge', version: '1.21.1', mods: [
        m("Iron's Spells 'n Spellbooks", 'Магия', 'Сотни заклинаний, школы магии и боссы.'),
        m('Ars Nouveau', 'Магия', 'Конструктор собственных заклинаний и магическая автоматизация.'),
        m('The Twilight Forest', 'Измерения', 'Сумеречный лес с боссами, лабиринтами и сокровищами.'),
        m('Waystones', 'Удобство', 'Телепорты между путевыми камнями.'),
        m('Curios API', 'Библиотека', 'Слоты для амулетов и колец.'),
        m('GeckoLib', 'Библиотека', 'Анимации существ и предметов.'),
        m('playerAnimator', 'Библиотека', 'Анимации игрока при сотворении заклинаний.'),
        m('Jade', 'Интерфейс', 'Информация о блоке под прицелом.'),
        m('Just Enough Items (JEI)', 'Интерфейс', 'Рецепты предметов и ритуалов.'),
        m("Xaero's World Map", 'Карта', 'Полноэкранная карта исследованного мира.')
      ] },
      perf: { title: 'Чистый FPS', description: 'Ванильный геймплей с максимальной оптимизацией для слабых и средних ПК.', loader: 'Fabric', version: '1.21.1', mods: [
        m('Fabric API', 'Библиотека', 'Базовая библиотека для модов Fabric.'),
        m('Sodium', 'Оптимизация', 'Главный мод производительности рендера.'),
        m('Lithium', 'Оптимизация', 'Оптимизация физики, ИИ и логики мира.'),
        m('FerriteCore', 'Оптимизация', 'Меньше оперативной памяти.'),
        m('ModernFix', 'Оптимизация', 'Быстрая загрузка и меньше памяти.'),
        m('ImmediatelyFast', 'Оптимизация', 'Ускоренный рендер интерфейса и текста.'),
        m('Entity Culling', 'Оптимизация', 'Скрывает невидимые сущности.'),
        m('More Culling', 'Оптимизация', 'Дополнительное отсечение невидимых граней.'),
        m('Dynamic FPS', 'Оптимизация', 'Экономит ресурсы в свёрнутом окне.'),
        m('Sodium Extra', 'Оптимизация', 'Дополнительные настройки графики.'),
        m("Reese's Sodium Options", 'Интерфейс', 'Удобное меню настроек Sodium.')
      ] },
      cozy: { title: 'Уютная долина', description: 'Неспешное выживание: фермерство, кухня, декор и красивый мир.', loader: 'Fabric', version: '1.21.1', mods: [
        m('Fabric API', 'Библиотека', 'Базовая библиотека для модов Fabric.'),
        m('Sodium', 'Оптимизация', 'Плавная картинка без просадок.'),
        m('Iris Shaders', 'Графика', 'Шейдеры для тёплых закатов.'),
        m("Farmer's Delight Refabricated", 'Фермерство', 'Новые культуры, кухня и блюда.'),
        m('Supplementaries', 'Декор', 'Банки, указатели, флюгеры и другие мелочи.'),
        m('Moonlight Lib', 'Библиотека', 'Нужна для Supplementaries.'),
        m('Waystones', 'Удобство', 'Быстрые перемещения между домом и фермой.'),
        m('Balm', 'Библиотека', 'Нужна для Waystones.'),
        m("Xaero's Minimap", 'Карта', 'Миникарта с путевыми точками.'),
        m('AppleSkin', 'Интерфейс', 'Сытость и восстановление от еды.'),
        m('Jade', 'Интерфейс', 'Что за блок перед вами.'),
        m('Mouse Tweaks', 'Удобство', 'Быстрая работа с инвентарём мышью.')
      ] }
    };
    let key = 'cozy';
    if (/хорр|страш|ужас|жут|horror|scary/.test(q)) key = 'horror';
    else if (/пвп|pvp|арен|бой|сраж|кит/.test(q)) key = 'pvp';
    else if (/техн|tech|завод|автомат|механ|индустр/.test(q)) key = 'tech';
    else if (/маги|magic|заклин|волшеб|приключ|рпг|rpg|данж|adventure/.test(q)) key = 'magic';
    else if (/fps|фпс|оптимиз|слаб|производ|лаг/.test(q)) key = 'perf';
    const p = presets[key];
    return { title: p.title, description: p.description, loader: p.loader, version: p.version, mods: p.mods.slice() };
  }

  async aiGenerate() {
    const prompt = (this.state.aiPrompt || '').trim() || 'уютное выживание';
    if (this.state.aiStatus === 'loading') return;
    this.setState({ aiStatus: 'loading', aiResult: null, aiNote: '', dlProgress: 0, dlStage: '', dlDone: false, dlBusy: false });
    let result = null;
    let note = '';
    try {
      // Rust command generate_ai_modpack asks the Signal server, which holds the OpenRouter key.
      const text = await window.SignalBridge.generateModpack(prompt);
      result = this.parsePack(text, prompt);
      if (!result) throw new Error('ответ не в формате JSON');
    } catch (e) {
      note = 'ИИ недоступен (' + ((e && e.message) || e) + ') — показан готовый пресет.';
      console.warn('[signal] generate_ai_modpack:', e);
      result = null;
    }
    const source = result ? 'openrouter' : 'fallback';
    if (!result) {
      await new Promise((r) => setTimeout(r, 900));
      result = this.fallbackPack(prompt);
    }
    result.mods = result.mods.map((mm, i) => Object.assign({ id: 'm' + i, checked: true }, mm));
    // Every mod is looked up on Modrinth, then CurseForge, for this loader and version: what is not there
    // stays in the list unticked, and the install takes exactly the projects found.
    try {
      const found = await window.SignalBridge.resolveAiMods(result.loader + ' ' + result.version, result.mods.map((mm) => ({ name: mm.name, slug: mm.slug || '' })));
      if (Array.isArray(found) && found.length === result.mods.length) {
        result.mods = result.mods.map((mm, i) => {
          const f = found[i];
          return Object.assign({}, mm, { source: f.source, projectId: f.projectId || null, url: f.url || '', checked: f.source !== 'none' },
            f.title && f.title !== mm.name ? { name: f.title } : null);
        });
        const missing = result.mods.filter((mm) => mm.source === 'none').length;
        if (missing) note = (note ? note + ' ' : '') + 'Не найдено для ' + result.loader + ' ' + result.version + ': ' + missing + ' — сняты с установки.';
      }
    } catch (e) {
      note = (note ? note + ' ' : '') + 'Каталоги модов сейчас не ответили — моды будут искаться при установке.';
    }
    this.setState({ aiStatus: 'done', aiResult: result, aiSource: source, aiNote: note });
  }

  startDownload() {
    const r = this.state.aiResult;
    if (!r || this.state.dlBusy) return;
    const mods = r.mods.filter((mm) => mm.checked);
    if (!mods.length) return;
    const id = 'c' + Date.now();
    const name = r.loader + ' ' + r.version;
    const title = this.uniqueInstallName(r.title);
    this.setState({ dlBusy: true, dlDone: false, dlProgress: 0, dlStage: 'Поиск модов на Modrinth…' });
    const fromCf = mods.filter((mm) => mm.source === 'curseforge' && mm.projectId);
    const fromMr = mods.filter((mm) => !fromCf.includes(mm));
    const onProgress = (p) => this.setState({ dlProgress: Math.max(this.state.dlProgress, p.percent), dlStage: p.label });
    window.SignalBridge.installMods({
      instanceId: id,
      instanceName: title,
      version: name,
      mods: fromMr.map((mm) => (mm.projectId ? { name: mm.name, projectId: mm.projectId } : { name: mm.name })),
      onProgress: onProgress
    })
      .then(async (report) => {
        // A CurseForge mod counts only when its own jar came down; otherwise (no access, no file for this
        // version, a file the author keeps on curseforge.com) the same name is looked for on Modrinth.
        const cfFailed = [];
        for (const mm of fromCf) {
          this.setState({ dlStage: 'CurseForge: ' + mm.name + '…' });
          try {
            const cf = await window.SignalBridge.installCurseforge({ instanceId: id, instanceName: title, version: name, projectId: mm.projectId, kind: 'mod', title: mm.name });
            const installed = (cf && cf.installed) || [];
            report.installed = report.installed.concat(installed);
            if (installed.some((x) => !x.dependency)) report.skipped = report.skipped.concat((cf && cf.skipped) || []);
            else cfFailed.push({ mm: mm, reason: ((cf && cf.skipped) || []).map((x) => x.reason).join(', ') || 'файл не скачан' });
          } catch (e) {
            cfFailed.push({ mm: mm, reason: String(e) });
          }
        }
        if (cfFailed.length) {
          this.setState({ dlStage: 'CurseForge не отдал модов: ' + cfFailed.length + ', ищу их на Modrinth…' });
          let alt = { installed: [], skipped: [] };
          try {
            alt = await window.SignalBridge.installMods({ instanceId: id, instanceName: title, version: name, mods: cfFailed.map((f) => ({ name: f.mm.name })), onProgress: onProgress });
          } catch (e) {
            console.warn('[signal] Modrinth fallback:', e);
          }
          report.installed = report.installed.concat(alt.installed);
          const found = new Set(alt.installed.filter((x) => !x.dependency).map((x) => x.name));
          cfFailed.filter((f) => !found.has(f.mm.name))
            .forEach((f) => report.skipped.push({ name: f.mm.name, reason: 'CurseForge: ' + f.reason + '; на Modrinth замены нет' }));
        }
        // The loader and its libraries now, not at the first «Играть»: its build is pinned for these mods,
        // and a broken loader shows up here instead of as a crash on start.
        this.setState({ dlStage: r.loader + ' ' + r.version + ': установка загрузчика…' });
        report.loaderVersion = await window.SignalBridge.installLoader({ instanceId: id, version: name, onProgress: onProgress });
        // What is really in mods/ decides: a file the report lists but the folder lacks is not installed,
        // and a pack none of whose chosen mods came down is not a pack.
        const files = await window.SignalBridge.listMods(id);
        const onDisk = new Set(files.map((f) => f.file));
        const lost = report.installed.filter((x) => x.file && !onDisk.has(x.file));
        report.installed = report.installed.filter((x) => !lost.includes(x));
        lost.forEach((x) => report.skipped.push({ name: x.name, reason: 'файл не появился в папке mods' }));
        report.jars = files.filter((f) => f.enabled).length;
        if (!report.installed.some((x) => !x.dependency)) {
          await window.SignalBridge.deleteInstance(id).catch(() => {});
          throw new Error('Ни один из выбранных модов не скачался, папка mods осталась пустой. Проверьте соединение и соберите ещё раз.');
        }
        return report;
      })
      .then((report) => {
        const n = report.jars;
        const deps = report.installed.filter((x) => x.dependency).length;
        this.addInstance({ id: id, name: title, version: name, mods: n, loaderVersion: report.loaderVersion || null });
        this.setState({ dlBusy: false, dlDone: true, dlProgress: 100, dlStage: 'Готово', versionId: id });
        this.loadInstances(false);
        this.refreshMods(id);
        this.play('done');
        const skipped = report.skipped.map((x) => x.name + ' — ' + x.reason);
        window.SignalBridge.toast(
          [
            deps ? 'Скачано модов: ' + n + ', из них зависимостей: ' + deps + '.' : 'Скачано модов: ' + n + '.',
            'Сборка выбрана в панели запуска.',
            skipped.length ? 'Пропущено: ' + skipped.join('; ') + '.' : ''
          ].filter(Boolean).join(' '),
          { kind: skipped.length ? 'info' : 'success', title: 'Сборка «' + title + '» установлена', timeout: skipped.length ? 12000 : 6000, notify: true, target: { tab: 'installs', instanceId: id } }
        );
      })
      .catch((e) => {
        this.setState({ dlBusy: false, dlProgress: 0, dlStage: '' });
        window.SignalBridge.toast(String(e), { kind: 'error', title: 'Сборка не скачалась' });
      });
  }

  // Mojang's version types → labels. `prefix` is what launch_minecraft parses ("Snapshot 26.4-snapshot-3").
  kindDefs() {
    return {
      release: { prefix: 'Release', group: 'Релизы', sub: 'Vanilla' },
      snapshot: { prefix: 'Snapshot', group: 'Снапшоты', sub: 'Снапшот' },
      old_beta: { prefix: 'Beta', group: 'Бета-версии', sub: 'Старая бета' },
      old_alpha: { prefix: 'Alpha', group: 'Альфа-версии', sub: 'Старая альфа' }
    };
  }

  kindAllowed(kind) {
    const o = this.state.opts;
    if (kind === 'snapshot') return !!o.snapshots;
    if (kind === 'old_beta') return !!o.oldBeta;
    if (kind === 'old_alpha') return !!o.oldAlpha;
    return true;
  }

  latestRelease() {
    const mv = this.state.mcVersions;
    return (mv && mv.latestRelease) || this.state.mcLatest || '1.21.9';
  }

  versionKind(id) {
    const mv = this.state.mcVersions;
    const v = mv && mv.versions.find((x) => x.id === id);
    return v && this.kindDefs()[v.kind] ? v.kind : 'release';
  }

  loadVersions(manual) {
    this.setState({ mcLoading: true });
    return window.SignalBridge.listVersions()
      .then((list) => {
        this.setState({ mcVersions: list, mcVersionsErr: '', mcLoading: false, mcLatest: list.latestRelease || this.state.mcLatest });
        if (manual) {
          window.SignalBridge.toast('Релиз ' + list.latestRelease + ', снапшот ' + list.latestSnapshot + ' · всего ' + list.versions.length + ' версий' + (list.offline ? ' (из кэша, нет связи с Mojang)' : '') + '.', { kind: list.offline ? 'info' : 'success', title: 'Список версий обновлён' });
        }
      })
      .catch((e) => {
        this.setState({ mcVersionsErr: String(e), mcLoading: false });
        if (manual) window.SignalBridge.toast(String(e), { kind: 'error', title: 'Список версий не загрузился' });
      });
  }

  // Versions for «Новая установка», grouped by type and filtered by the settings toggles.
  // Fabric/Forge/NeoForge get releases only.
  versionChoices(loader) {
    const mv = this.state.mcVersions;
    if (!mv) {
      const ids = ['1.21.9', '1.21.8', '1.21.4', '1.21.1', '1.21', '1.20.6', '1.20.4', '1.20.1', '1.19.4', '1.19.2', '1.18.2', '1.16.5', '1.12.2'];
      if (this.state.mcLatest && ids.indexOf(this.state.mcLatest) < 0) ids.unshift(this.state.mcLatest);
      return ids.map((id) => ({ id: id, kind: 'release', installed: false }));
    }
    if (loader === 'vanilla') return mv.versions.filter((v) => this.kindAllowed(v.kind));
    // Oldest release each loader can install: Fabric 1.14, Forge 1.12.2 (new installer), NeoForge 1.20.2.
    const releases = mv.versions.filter((v) => v.kind === 'release');
    const oldest = ({ fabric: '1.14', forge: '1.12.2', neoforge: '1.20.2' })[loader];
    const cut = releases.findIndex((v) => v.id === oldest);
    return cut < 0 ? releases : releases.slice(0, cut + 1);
  }

  versionGroups(loader, selected) {
    const kd = this.kindDefs();
    const choices = this.versionChoices(loader);
    // Keep the current pick listed so the select never shows an empty value.
    if (selected && !choices.some((v) => v.id === selected)) choices.unshift({ id: selected, kind: this.versionKind(selected), installed: false });
    return Object.keys(kd).map((k) => {
      const opts = choices.filter((v) => (kd[v.kind] ? v.kind : 'release') === k);
      return {
        label: kd[k].group + ' · ' + opts.length,
        options: opts.map((v) => ({ value: v.id, label: v.id + (v.installed ? '  · скачана' : '') }))
      };
    }).filter((g) => g.options.length);
  }

  versionsNote(loader) {
    const s = this.state;
    if (!s.mcVersions) return s.mcVersionsErr ? 'Нет связи с Mojang — показан короткий список релизов.' : 'Загружаю список версий с Mojang…';
    if (loader !== 'vanilla') {
      const since = ({ fabric: '1.14', forge: '1.12.2', neoforge: '1.20.2' })[loader];
      return 'Только релизы с ' + since + '. Загрузчик скачается и установится сам при первом запуске.';
    }
    const hidden = [];
    if (!s.opts.snapshots) hidden.push('снапшоты');
    if (!s.opts.oldBeta) hidden.push('бета');
    if (!s.opts.oldAlpha) hidden.push('альфа');
    const base = s.mcVersions.offline ? 'Список из кэша (нет связи с Mojang). ' : '';
    return base + (hidden.length ? 'Скрыты: ' + hidden.join(', ') + ' — включаются в «Настройки → Система и Лаунчер».' : 'Показаны все типы версий.');
  }

  toggleMod(ver, f) {
    window.SignalBridge.setModEnabled({ instanceId: ver.id, file: f.file, enabled: !f.enabled })
      .then((next) => {
        const mine = Object.assign({}, this.state.installedBy[ver.id] || {});
        Object.keys(mine).forEach((k) => { if (mine[k] === f.file) mine[k] = next; });
        const list = (this.state.instMods[ver.id] || []).map((x) => (x.file === f.file ? Object.assign({}, x, { file: next, enabled: !f.enabled }) : x));
        this.setState({
          installedBy: Object.assign({}, this.state.installedBy, { [ver.id]: mine }),
          instMods: Object.assign({}, this.state.instMods, { [ver.id]: list })
        });
        this.refreshMods(ver.id);
      })
      .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Не удалось переключить мод' }));
  }

  deleteMod(ver, file) {
    clearTimeout(this.delTimer);
    this.setState({ modDel: '' });
    window.SignalBridge.removeMod({ instanceId: ver.id, file: file })
      .then(() => {
        const mine = Object.assign({}, this.state.installedBy[ver.id] || {});
        Object.keys(mine).forEach((k) => { if (mine[k] === file) delete mine[k]; });
        const list = (this.state.instMods[ver.id] || []).filter((x) => x.file !== file);
        this.setState({
          installedBy: Object.assign({}, this.state.installedBy, { [ver.id]: mine }),
          instMods: Object.assign({}, this.state.instMods, { [ver.id]: list })
        });
        this.refreshMods(ver.id);
        window.SignalBridge.toast(file + ' удалён из «' + ver.install + '».', { kind: 'info', title: 'Мод удалён' });
      })
      .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Не удалось удалить мод' }));
  }

  // «sodium-fabric-0.6.0+mc1.21.1.jar.disabled» → «Sodium Fabric»
  modTitle(file) {
    const base = file.replace(/\.disabled$/i, '').replace(/\.jar$/i, '');
    const name = base.split(/[-_+ ](?=v?\d)/)[0] || base;
    return name.replace(/[-_]+/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
  }

  installedModRows(s, ver) {
    const files = s.instMods[ver.id];
    if (!files) return null;
    const mine = s.installedBy[ver.id] || {};
    const byFile = {};
    Object.keys(mine).forEach((id) => { if (s.modMeta[id]) byFile[mine[id]] = s.modMeta[id]; });
    return files.map((f) => {
      const m = byFile[f.file];
      const confirm = s.modDel === f.file;
      const u = ((s.modUpdates[ver.id] || {}).map || {})[f.file];
      const updating = !!s.updBusy[f.file];
      return {
        hasUpdate: !!u,
        updLabel: u ? u.current + ' → ' + u.latest : '',
        updBtnLabel: updating ? 'Обновляю…' : 'Обновить',
        update: () => { if (u && !updating) this.updateMod(ver, u); },
        title: m ? m.t : (u && u.title) || this.modTitle(f.file),
        file: f.file,
        size: this.fmtBytes(f.size),
        icon: m && m.i ? m.i : '',
        hasIcon: !!(m && m.i),
        noIcon: !(m && m.i),
        on: f.enabled,
        off: !f.enabled,
        rowCls: (f.enabled ? 'im-row' : 'im-row is-off') + (u ? ' has-update' : ''),
        swCls: f.enabled ? 'sw is-on' : 'sw',
        swLabel: (f.enabled ? 'Выключить ' : 'Включить ') + (m ? m.t : f.file),
        delCls: confirm ? 'im-del is-confirm' : 'im-del',
        delLabel: confirm ? 'Удалить?' : '',
        confirm: confirm,
        idle: !confirm,
        toggle: () => this.toggleMod(ver, f),
        del: () => {
          if (!confirm) {
            clearTimeout(this.delTimer);
            this.setState({ modDel: f.file });
            this.delTimer = setTimeout(() => this.setState({ modDel: '' }), 3500);
            return;
          }
          this.deleteMod(ver, f.file);
        }
      };
    });
  }

  transferWorld(w, v) {
    if (this.state.wtBusy) return;
    this.setState({ wtBusy: v.id });
    window.SignalBridge.transferWorld({ path: w.path, instanceId: v.id, instanceName: v.install, version: v.name })
      .then((dest) => {
        this.setState({ wtBusy: '', wtWorld: null });
        this.play('done');
        window.SignalBridge.toast(dest, { kind: 'success', title: '«' + w.name + '» → «' + v.install + '»', timeout: 7000, target: { tab: 'worlds' } });
        this.loadWorlds();
      })
      .catch((e) => {
        this.setState({ wtBusy: '' });
        window.SignalBridge.toast(String(e), { kind: 'error', title: 'Мир не перенесён' });
      });
  }

  // Installations from .minecraft/instances. `migrate` writes instance.json for installs that
  // older builds kept only in localStorage.
  loadInstances(migrate) {
    return window.SignalBridge.listInstances()
      .then((list) => {
        list = list || [];
        const legacy = migrate ? (this.state.customInstalls || []).filter((c) => !list.some((i) => i.id === c.id)) : [];
        if (!legacy.length) {
          this.setState({ instances: list, customInstalls: [] });
          return null;
        }
        return Promise.all(legacy.map((c) => window.SignalBridge.createInstance({ id: c.id, name: c.install, version: c.name }).catch(() => null)))
          .then(() => this.loadInstances(false));
      })
      .catch((e) => {
        console.error('[signal] list_instances:', e);
        this.setState({ instances: [] });
      });
  }

  // Optimistic entry until the next list_instances.
  addInstance(info) {
    const list = (this.state.instances || []).filter((i) => i.id !== info.id);
    const entry = Object.assign({ created: Date.now(), lastPlayed: null, playTime: 0, lastBackup: null, lastExitCode: null, mods: 0, worlds: 0 }, info);
    this.setState({ instances: [entry].concat(list) });
  }

  loadMemory() {
    window.SignalBridge.systemMemory()
      .then((mem) => {
        if (!mem) return;
        // While a game runs the free memory is the game's, so the limit is kept as it was.
        if (Object.keys(this.state.running).length) { this.setState({ mem: mem }); return; }
        const max = Math.max(2, Math.min(64, Math.floor(mem.availableMb / 1024)));
        const next = { mem: mem, ramMax: max };
        if (this.state.ram > max) { next.ram = max; next.ramClamped = true; }
        this.setState(next);
      })
      .catch(() => {});
  }

  // Labels under the RAM slider: 2, 4, 8, 16… up to the limit, without crowding the last one.
  ramScale(max) {
    const span = Math.max(1, max - 2);
    const marks = [2, 4, 8, 16, 32, 64].filter((v) => v < max && (max - v) / span > 0.12);
    return marks.concat([max]).map((v) => ({ label: v + ' ГБ', left: ((v - 2) / span * 100).toFixed(2) }));
  }

  autoBackupText(s, ver) {
    if (!s.opts.backup) return 'Автобэкап выключен — включите его в «Настройки → Система и Лаунчер»';
    const freq = ({ launch: 'перед каждым запуском', daily: 'раз в день', weekly: 'раз в неделю' })[s.backupFreq];
    const last = ver.lastBackup ? 'последний для «' + ver.install + '»: ' + this.fmtWhen(ver.lastBackup) : 'для «' + ver.install + '» ещё не делался';
    return 'Автобэкап ' + freq + ' · ' + last + ' · .minecraft\\backups\\auto';
  }

  fmtWhen(ms) {
    const d = new Date(ms);
    const hm = d.toLocaleTimeString(LOCALE, { hour: '2-digit', minute: '2-digit' });
    const day = Math.floor((new Date().setHours(0, 0, 0, 0) - new Date(ms).setHours(0, 0, 0, 0)) / 864e5);
    if (day <= 0) return 'сегодня, ' + hm;
    if (day === 1) return 'вчера, ' + hm;
    return d.toLocaleDateString(LOCALE) + ', ' + hm;
  }

  fmtPlayTime(sec) {
    if (sec < 3600) return Math.max(1, Math.round(sec / 60)) + ' мин';
    return decimal(sec / 3600, sec < 36000 ? 1 : 0) + ' ч';
  }

  // instance.json → an entry of the installation lists.
  instanceEntry(i) {
    const label = i.version || '';
    const low = label.toLowerCase();
    const loader = low.startsWith('fabric') ? 'fabric' : (low.startsWith('forge') || low.startsWith('neoforge') ? 'forge' : 'vanilla');
    const kind = low.startsWith('snapshot') ? 'snapshot' : (low.startsWith('beta') ? 'old_beta' : (low.startsWith('alpha') ? 'old_alpha' : 'release'));
    const parts = [loader === 'fabric' ? 'Fabric Loader' : (loader === 'forge' ? (low.startsWith('neoforge') ? 'NeoForge' : 'Forge') : this.kindDefs()[kind].sub)];
    if (i.pack) parts.unshift('Сборка ' + i.pack);
    if (i.loaderVersion && loader !== 'vanilla') parts[parts.length - 1] += ' ' + i.loaderVersion;
    if (i.mods) parts.push(i.mods + ' ' + this.plural(i.mods, 'мод', 'мода', 'модов'));
    if (i.playTime >= 60) parts.push(this.fmtPlayTime(i.playTime) + ' в игре');
    if (i.lastExitCode != null && i.lastExitCode !== 0) parts.push('последний запуск завершился сбоем');
    return {
      id: i.id, name: label, sub: parts.join(' · '), kind: kind, loader: loader, install: i.name,
      last: i.lastPlayed ? this.fmtWhen(i.lastPlayed) : 'ещё не запускалась',
      lastBackup: i.lastBackup || null,
      icon: i.icon || null,
      onDisk: true
    };
  }

  // «Импорт сборки» / drag & drop: .mrpack → new installation, progress in a toast.
  // `source`: a .mrpack path, or (catalog) a title plus `run(onProgress)` that downloads and imports.
  importModpack(source, run, finished) {
    // A plain field, not state: two quick drops land before setState applies.
    if (this.importing) {
      window.SignalBridge.toast('Дождитесь окончания текущего импорта.', { kind: 'info', title: 'Сборка уже импортируется' });
      if (finished) finished();
      return;
    }
    const file = String(source).split(/[\\/]/).pop();
    this.importing = true;
    this.setState({ importBusy: true });
    const pending = window.SignalBridge.toast((run ? 'Скачивание ' + file + '…' : 'Чтение ' + file + '…'), { kind: 'loading', title: 'Импорт сборки', timeout: 0 });
    const onProgress = (p) => pending.update({ kind: 'loading', title: 'Импорт сборки · ' + p.percent + '%', message: p.label, timeout: 0 });
    (run ? run(onProgress) : window.SignalBridge.importModpack({ path: source, onProgress: onProgress }))
      .then((r) => {
        if (finished) finished();
        this.addInstance({ id: r.instanceId, name: r.name, version: r.version, loaderVersion: r.loaderVersion, icon: r.icon });
        this.importing = false;
        this.setState({ importBusy: false, versionId: r.instanceId });
        this.loadInstances(false);
        this.play('done');
        const loader = r.loaderVersion ? r.version.split(' ')[0] + ' ' + r.loaderVersion : '';
        const blocked = r.blocked || [];
        pending.update({
          kind: blocked.length ? 'info' : 'success',
          title: 'Сборка «' + r.name + '» установлена',
          message: [
            'Скачано файлов: ' + r.files + '.',
            r.overrides ? 'Файлов настроек: ' + r.overrides + '.' : '',
            r.version + ' выбрана в панели запуска.',
            loader ? 'Загрузчик ' + loader + ' скачается при первом запуске.' : '',
            r.blocked && r.blocked.length
              ? 'Только с сайта CurseForge (скачайте вручную в папку установки): ' + r.blocked.slice(0, 4).map((b) => b.name).join(', ') + (r.blocked.length > 4 ? ' и ещё ' + (r.blocked.length - 4) : '') + '.'
              : ''
          ].filter(Boolean).join(' '),
          timeout: blocked.length ? 0 : 9000,
          notify: blocked.length > 0,
          target: { tab: 'installs', instanceId: r.instanceId },
          actions: blocked.length
            ? [{ label: 'Папка установки', onClick: () => window.SignalBridge.openFolder('%MINECRAFT%\\instances\\' + r.instanceId) }]
              .concat(blocked[0].url ? [{ label: 'Первый файл на CurseForge', onClick: () => window.SignalBridge.openLink(blocked[0].url) }] : [])
            : []
        });
      })
      .catch((e) => {
        if (finished) finished();
        this.importing = false;
        this.setState({ importBusy: false });
        pending.update({ kind: 'error', title: 'Сборка не импортирована', message: String(e), timeout: 12000 });
      });
  }

  pickAndImport() {
    // One system file picker at a time.
    if (this.picking || this.importing) return;
    this.picking = true;
    window.SignalBridge.pickModpack()
      .then((path) => {
        this.picking = false;
        if (path) this.importModpack(path);
      })
      .catch((e) => {
        this.picking = false;
        window.SignalBridge.toast(String(e), { kind: 'error', title: 'Файл не выбран' });
      });
  }

  // ------------------------------------------------------------------ game folder

  loadUsage() {
    if (this.usageLoading) return;
    this.usageLoading = true;
    this.setState({ usageBusy: true });
    window.SignalBridge.diskUsage()
      .then((u) => { this.usageLoading = false; this.setState({ usage: u, usageBusy: false }); })
      .catch(() => { this.usageLoading = false; this.setState({ usageBusy: false }); });
  }

  // «Сменить папку…» / «Стандартная папка»: pick (or take) a folder, look inside, offer the modes.
  pickGameDir(path) {
    const choose = path ? Promise.resolve(path) : window.SignalBridge.pickFolder('Новая папка игры');
    choose
      .then((p) => {
        if (!p) return null;
        return window.SignalBridge.checkGameDir(p).then((check) => {
          const info = this.state.gameDirInfo || {};
          // Sensible default: a Minecraft folder is used as is, an empty one gets a copy.
          const mode = check.minecraft || (check.exists && !check.empty) ? 'use' : 'copy';
          this.setState({ dirModal: { path: p, check: check, mode: mode, busy: false, label: '', percent: 0, err: '', isDefault: !!info.isDefault } });
        });
      })
      .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Папка не выбрана' }));
  }

  confirmDirChange() {
    const m = this.state.dirModal;
    if (!m || m.busy) return;
    if (Object.keys(this.state.running).length) {
      this.setState({ dirModal: Object.assign({}, m, { err: 'Сначала закройте игру: папку нельзя менять, пока Minecraft запущен.' }) });
      return;
    }
    this.setState({ dirModal: Object.assign({}, m, { busy: true, err: '', label: m.mode === 'use' ? 'Переключаю…' : 'Подготовка…', percent: 0 }) });
    window.SignalBridge.changeGameDir({
      path: m.path,
      mode: m.mode,
      onProgress: (p) => this.setState((st) => ({ dirModal: st.dirModal && Object.assign({}, st.dirModal, { label: p.label, percent: p.percent }) }))
    })
      .then((info) => {
        const how = { use: 'Лаунчер теперь работает с этой папкой.', copy: 'Данные скопированы, старая папка осталась на месте.', move: 'Данные перенесены.' }[m.mode];
        window.SignalBridge.toastAfterReload(info.path, { kind: 'success', title: 'Папка игры изменена. ' + how, target: { tab: 'settings', settingsTab: 'sys' } });
        window.location.reload();
      })
      .catch((e) => this.setState((st) => ({ dirModal: st.dirModal && Object.assign({}, st.dirModal, { busy: false, err: String(e) }) })));
  }

  usageView(s) {
    const u = s.usage;
    const defs = {
      versions: ['Версии игры', '#5BD07A'],
      libraries: ['Библиотеки', '#4FA3FF'],
      assets: ['Ассеты: звуки и текстуры', '#B9A6F2'],
      runtime: ['Java', '#F2C27A'],
      instances: ['Установки: моды, миры, настройки', '#E09A5B'],
      backups: ['Бэкапы миров', '#7FD3C7'],
      cache: ['Кэш лаунчера', '#9CC9F0'],
      other: ['Прочее: общие миры, логи, скриншоты', '#6F7884']
    };
    if (!u) return { rows: [], segs: [], summary: s.usageBusy ? 'Считаю место…' : '' };
    const total = Math.max(1, u.total);
    const parts = u.parts.filter((p) => p.bytes > 0);
    return {
      rows: parts.map((p) => ({ label: defs[p.key][0], color: defs[p.key][1], size: this.fmtBytes(p.bytes) })),
      segs: parts.map((p) => ({ color: defs[p.key][1], w: (p.bytes / total * 100).toFixed(2), label: defs[p.key][0] + ' — ' + this.fmtBytes(p.bytes) })),
      summary: 'Занято ' + this.fmtBytes(u.total) + (u.free != null ? ' · свободно на диске ' + this.fmtBytes(u.free) : '')
    };
  }

  // ------------------------------------------------------------------ notifications

  // Where a history entry leads: { console } | { update } | { tab, instanceId?, view?, settingsTab? }
  openTarget(t) {
    this.setState({ notifOpen: false });
    if (!t) return;
    if (t.console) { window.SignalBridge.console.open(t.console); return; }
    if (t.update) {
      if (this.state.upd.info && this.state.upd.info.available) this.setUpd({ open: true });
      else this.checkLauncherUpdate(false);
      return;
    }
    const next = { accMenu: false, verMenu: false };
    if (t.tab) next.tab = t.tab;
    if (t.instanceId && this.versionList().some((v) => v.id === t.instanceId)) next.versionId = t.instanceId;
    if (t.view) next.modsView = t.view === 'installed' ? 'installed' : 'catalog';
    if (t.kind) next.imKind = t.kind;
    if (t.settingsTab) next.settingsTab = t.settingsTab;
    this.setState(next);
    if (t.tab === 'worlds') this.loadWorlds();
    if (t.tab === 'installs') this.loadInstances(false);
    if (t.tab === 'mods' && t.instanceId) {
      this.refreshMods(t.instanceId);
      if (t.kind && t.kind !== 'mod') this.refreshPacks(t.instanceId, t.kind);
    }
    if (t.tab === 'settings' && t.settingsTab === 'sys') this.loadUsage();
  }

  notifCards(s) {
    const colors = { error: '#F2998B', success: '#5BD07A', info: '#8EC2FF' };
    return s.notifs.map((n) => ({
      title: n.title || n.message,
      message: n.title ? n.message : '',
      hasMessage: !!(n.title && n.message),
      when: this.fmtWhen(n.at),
      color: colors[n.kind] || colors.info,
      cls: n.read ? 'nf-item' : 'nf-item is-new',
      linked: !!n.target,
      open: () => this.openTarget(n.target),
      remove: () => window.SignalBridge.notifications.remove(n.id)
    }));
  }

  // ------------------------------------------------------------------ launcher updates

  setUpd(patch) {
    this.setState((st) => ({ upd: Object.assign({}, st.upd, patch) }));
  }

  // `quiet`: the start-up check — no toasts unless there is something new.
  checkLauncherUpdate(quiet) {
    if (this.state.upd.checking || this.state.upd.busy) return;
    this.setUpd({ checking: true, err: '' });
    window.SignalBridge.checkLauncherUpdate()
      .then((info) => {
        this.setUpd({ checking: false, info: info });
        if (!info.configured) {
          if (!quiet) window.SignalBridge.toast('В этой сборке не задан адрес обновлений.', { kind: 'info', title: 'Обновления не настроены' });
        } else if (info.available) {
          if (quiet) {
            window.SignalBridge.toast('Сейчас у вас ' + info.current + '.', {
              kind: 'info', title: 'Доступна новая версия Signal Launcher v' + info.version, timeout: 12000, notify: true, target: { update: true },
              actions: [{ label: 'Подробнее', onClick: () => this.setUpd({ open: true }) }]
            });
          } else {
            this.setUpd({ open: true });
          }
        } else if (!quiet) {
          window.SignalBridge.toast('У вас последняя версия: ' + info.current + '.', { kind: 'success', title: 'Обновлений нет', notify: false });
        }
      })
      .catch((e) => {
        this.setUpd({ checking: false, err: String(e) });
        if (!quiet) window.SignalBridge.toast(String(e), { kind: 'error', title: 'Проверка обновлений не удалась' });
      });
  }

  installLauncherUpdate() {
    if (this.state.upd.busy) return;
    this.setUpd({ busy: true, downloaded: 0, total: 0, err: '' });
    window.SignalBridge.installLauncherUpdate({ onProgress: (p) => this.setUpd({ downloaded: p.downloaded, total: p.total || 0 }) })
      .then(() => this.setUpd({ busy: false, open: false }))
      .catch((e) => this.setUpd({ busy: false, err: String(e) }));
  }

  // ------------------------------------------------------------------ servers

  serverList() {
    const s = this.state;
    const mine = (s.myServers || [{ id: 's-hypixel', name: 'Hypixel', addr: 'mc.hypixel.net' }]).map((x) => Object.assign({ mine: true }, x));
    const taken = new Set(mine.map((x) => x.addr.toLowerCase()));
    const game = (s.gameServers || [])
      .filter((g) => !taken.has(g.address.toLowerCase()))
      .map((g) => ({ id: 'g:' + g.address, name: g.name, addr: g.address, icon: g.icon, source: g.source, mine: false }));
    return mine.concat(game);
  }

  // Tab opened / «Обновить список»: servers from the game's servers.dat, then a ping for each.
  refreshServers() {
    window.SignalBridge.savedServers()
      .catch(() => [])
      .then((game) => {
        this.setState({ gameServers: game || [] });
        const mine = (this.state.myServers || [{ addr: 'mc.hypixel.net' }]).map((x) => x.addr);
        const all = mine.concat((game || []).map((g) => g.address));
        this.pingAll(all.filter((a, i) => all.findIndex((b) => b.toLowerCase() === a.toLowerCase()) === i));
      });
  }

  pingAll(addrs) {
    // Functional updates: several pings can finish in the same tick.
    this.setState((st) => {
      const next = Object.assign({}, st.serverStatus);
      addrs.forEach((a) => { next[a.toLowerCase()] = Object.assign({}, next[a.toLowerCase()] || {}, { pending: true }); });
      return { serverStatus: next };
    });
    addrs.forEach((a) => {
      window.SignalBridge.pingServer(a)
        .catch((e) => ({ online: false, error: String(e) }))
        .then((r) => this.setState((st) => ({ serverStatus: Object.assign({}, st.serverStatus, { [a.toLowerCase()]: Object.assign({ at: Date.now() }, r) }) })));
    });
  }

  saveServerModal() {
    const m = this.state.srvModal;
    if (!m) return;
    const addr = m.addr.trim().replace(/^minecraft:\/\//i, '');
    if (!/^(\[[0-9a-f:]+\]|[^\s/:\[\]]+)(:\d{1,5})?$/i.test(addr)) {
      this.setState({ srvModal: Object.assign({}, m, { err: 'Адрес вида play.example.org или 192.168.1.20:25565' }) });
      return;
    }
    if (m.mode === 'direct') {
      this.setState({ srvModal: null });
      this.connectServer(addr);
      return;
    }
    const mine = this.state.myServers || [{ id: 's-hypixel', name: 'Hypixel', addr: 'mc.hypixel.net' }];
    if (mine.some((x) => x.addr.toLowerCase() === addr.toLowerCase())) {
      this.setState({ srvModal: Object.assign({}, m, { err: 'Этот сервер уже в списке' }) });
      return;
    }
    const name = m.name.trim().slice(0, 40) || addr;
    this.setState({ myServers: mine.concat([{ id: 's' + Date.now(), name: name, addr: addr }]), srvModal: null });
    this.pingAll([addr]);
  }

  removeServer(sv) {
    if (this.state.srvDel !== sv.id) {
      clearTimeout(this.srvDelTimer);
      this.setState({ srvDel: sv.id });
      this.srvDelTimer = setTimeout(() => this.setState({ srvDel: '' }), 3500);
      return;
    }
    clearTimeout(this.srvDelTimer);
    const mine = this.state.myServers || [{ id: 's-hypixel', name: 'Hypixel', addr: 'mc.hypixel.net' }];
    this.setState({ myServers: mine.filter((x) => x.id !== sv.id), srvDel: '' });
  }

  // «Подключиться»: the selected installation starts and joins the server right away.
  connectServer(addr) {
    const ver = this.currentVersion();
    window.SignalBridge.toast('«' + ver.install + '» (' + ver.name + ') запустится и сразу подключится к ' + addr + '.', { kind: 'info', title: 'Подключение к серверу', timeout: 5000 });
    this.launch(addr);
  }

  serverCards(s) {
    const tints = ['#8FD3A8', '#9CC9F0', '#E09A5B', '#B9A6F2', '#F2C27A'];
    const num = (n) => Number(n).toLocaleString(LOCALE);
    return this.serverList().map((sv, i) => {
      const st = s.serverStatus[sv.addr.toLowerCase()];
      const pending = !st || !!st.pending;
      const online = !pending && !!st.online;
      const ping = online ? st.latencyMs : null;
      const bars = ping == null ? 0 : (ping < 50 ? 4 : (ping < 100 ? 3 : (ping < 150 ? 2 : 1)));
      const players = online && st.playersOnline != null ? num(st.playersOnline) + '/' + num(st.playersMax) + ' игроков' : '';
      const icon = (st && st.favicon) || sv.icon || '';
      const confirm = s.srvDel === sv.id;
      return {
        name: sv.name,
        addr: sv.addr,
        tint: tints[i % tints.length],
        hasIcon: !!icon,
        noIcon: !icon,
        iconSrc: icon,
        ver: online && st.version ? st.version : (sv.mine ? 'в лаунчере' : 'из игры'),
        motd: online ? (st.motd || 'Без описания') : (sv.source ? 'Из игры · ' + sv.source : 'Добавлен в лаунчере'),
        status: pending ? 'Проверка…' : (online ? 'Онлайн' : 'Недоступен'),
        statusColor: pending ? '#8A94A3' : (online ? (ping < 150 ? '#5BD07A' : '#F0B95A') : '#8A94A3'),
        meta: pending ? 'пинг…' : (online ? [players, ping + ' мс'].filter(Boolean).join(' · ') : (st.error || 'нет ответа')),
        off: !online,
        pingLabel: ping == null ? 'Нет связи' : 'Пинг ' + ping + ' мс, ' + bars + ' из 4 делений',
        bars: [6, 9, 12, 15].map((h, idx) => ({ h: h, c: idx < bars ? '#2ecc71' : 'rgba(255,255,255,0.16)', g: idx < bars ? '0 0 6px rgba(46,204,113,0.6)' : 'none' })),
        connect: () => this.connectServer(sv.addr),
        canRemove: sv.mine,
        delCls: confirm ? 'srv-del is-confirm' : 'srv-del',
        delLabel: confirm ? 'Удалить?' : '',
        delIdle: !confirm,
        delConfirm: confirm,
        remove: () => this.removeServer(sv)
      };
    });
  }

  // ------------------------------------------------------------------ news

  loadNews(force) {
    if (this.newsLoading) return;
    this.newsLoading = true;
    this.setState({ newsLoading: true });
    window.SignalBridge.newsFeed(force)
      .then((feed) => {
        this.newsLoading = false;
        this.setState({ news: feed, newsErr: '', newsLoading: false });
        if (this.state.tab === 'news') this.markNewsSeen(feed);
        if (force) window.SignalBridge.toast(feed.offline ? 'Нет связи с Mojang — показаны сохранённые новости.' : 'Лента обновлена.', { kind: feed.offline ? 'info' : 'success', title: 'Новости', timeout: 3000 });
      })
      .catch((e) => {
        this.newsLoading = false;
        this.setState({ newsErr: String(e), newsLoading: false });
      });
  }

  // Everything up to the newest item counts as read (the badge on «Новости»).
  markNewsSeen(feed) {
    const items = ((feed || this.state.news) || {}).items || [];
    if (items.length && items[0].date > (this.state.newsSeen || '')) this.setState({ newsSeen: items[0].date });
  }

  newsBadge(s) {
    const items = (s.news && s.news.items) || [];
    let since = s.newsSeen;
    if (!since) {
      const d = new Date(Date.now() - 14 * 864e5);
      since = d.toISOString().slice(0, 10);
    }
    const n = items.filter((i) => i.date > since).length;
    return n > 9 ? '9+' : (n ? String(n) : '');
  }

  openNewsItem(it) {
    if (it.link) {
      window.SignalBridge.openLink(it.link).catch((e) => window.SignalBridge.toast(String(e), { kind: 'info', title: 'Ссылка не открылась' }));
    } else if (it.contentPath) {
      window.SignalBridge.readPatchNotes({ title: it.title, meta: this.newsTag(it) + ' · ' + this.newsDate(it.date), contentPath: it.contentPath });
    }
  }

  newsTag(it) {
    return ({ news: 'Новости Java', release: 'Релиз', snapshot: 'Снапшот' })[it.kind] + (it.version ? ' · ' + it.version : '');
  }

  newsDate(date) {
    const today = new Date().toISOString().slice(0, 10);
    const yesterday = new Date(Date.now() - 864e5).toISOString().slice(0, 10);
    if (date === today) return 'сегодня';
    if (date === yesterday) return 'вчера';
    const [y, m, d] = date.split('-').map(Number);
    return new Date(y, m - 1, d).toLocaleDateString(LOCALE);
  }

  newsCards(s) {
    const items = (s.news && s.news.items) || [];
    const colors = { news: '#7FD08F', release: '#9CC9F0', snapshot: '#F2B07A' };
    const fallback = ['/assets/img/news-castle.webp', '/assets/img/news-rainbow.webp', '/assets/img/news-house.webp', '/assets/img/shot-mountains.webp'];
    return items.map((it, i) => ({
      title: it.title,
      text: it.text,
      tag: this.newsTag(it),
      tagColor: colors[it.kind] || '#7FD08F',
      date: this.newsDate(it.date),
      img: it.image ? window.SignalBridge.fileSrc(it.image) : fallback[i % fallback.length],
      alt: '',
      cta: it.link ? 'Читать на minecraft.net' : 'Читать патчноуты',
      isFresh: !!s.newsSeen && it.date > s.newsSeen,
      open: () => this.openNewsItem(it)
    }));
  }

  setModUpdates(id, patch) {
    const all = Object.assign({}, this.state.modUpdates);
    all[id] = Object.assign({}, all[id] || {}, patch);
    this.setState({ modUpdates: all });
  }

  checkUpdates(ver) {
    const cur = this.state.modUpdates[ver.id];
    if (cur && cur.checking) return;
    this.setModUpdates(ver.id, { checking: true });
    window.SignalBridge.checkModUpdates(ver.id)
      .then((r) => {
        const map = {};
        r.updates.forEach((u) => { map[u.file] = u; });
        this.setModUpdates(ver.id, { checking: false, map: map, known: r.known, unknown: r.unknown, at: Date.now() });
        const n = r.updates.length;
        window.SignalBridge.toast(
          (n ? 'Доступно ' + n + ' ' + this.plural(n, 'обновление', 'обновления', 'обновлений') + '.' : 'Все моды, известные Modrinth, актуальны.') +
            (r.unknown ? ' Не нашлось на Modrinth: ' + r.unknown + ' (CurseForge или собранные вручную).' : ''),
          { kind: n ? 'info' : 'success', title: '«' + ver.install + '»: проверка обновлений', notify: n > 0, target: { tab: 'mods', view: 'installed', instanceId: ver.id } }
        );
      })
      .catch((e) => {
        this.setModUpdates(ver.id, { checking: false });
        window.SignalBridge.toast(String(e), { kind: 'error', title: 'Обновления не проверены' });
      });
  }

  // One mod; `quiet` for «Обновить все». Resolves to true on success.
  updateMod(ver, u, quiet) {
    const busy = Object.assign({}, this.state.updBusy, { [u.file]: true });
    this.setState({ updBusy: busy });
    const unbusy = () => {
      const b = Object.assign({}, this.state.updBusy);
      delete b[u.file];
      return b;
    };
    return window.SignalBridge.updateMod({ instanceId: ver.id, file: u.file, versionId: u.versionId })
      .then((name) => {
        const mine = Object.assign({}, this.state.installedBy[ver.id] || {});
        Object.keys(mine).forEach((k) => { if (mine[k] === u.file) mine[k] = name; });
        const map = Object.assign({}, (this.state.modUpdates[ver.id] || {}).map || {});
        delete map[u.file];
        this.setState({ updBusy: unbusy(), installedBy: Object.assign({}, this.state.installedBy, { [ver.id]: mine }) });
        this.setModUpdates(ver.id, { map: map });
        this.refreshMods(ver.id);
        if (!quiet) window.SignalBridge.toast(u.file + ' → ' + name, { kind: 'success', title: (u.title || 'Мод') + ' обновлён до ' + u.latest, target: { tab: 'mods', view: 'installed', instanceId: ver.id } });
        return true;
      })
      .catch((e) => {
        this.setState({ updBusy: unbusy() });
        window.SignalBridge.toast(String(e), { kind: 'error', title: (u.title || u.file) + ' не обновлён' });
        return false;
      });
  }

  updateAll(ver) {
    const list = Object.values((this.state.modUpdates[ver.id] || {}).map || {});
    if (!list.length) return;
    const pending = window.SignalBridge.toast('0 из ' + list.length, { kind: 'loading', title: 'Обновление модов', timeout: 0 });
    let ok = 0;
    list.reduce((chain, u, i) => chain.then(() => this.updateMod(ver, u, true).then((done) => {
      if (done) ok += 1;
      pending.update({ kind: 'loading', title: 'Обновление модов', message: (i + 1) + ' из ' + list.length, timeout: 0 });
    })), Promise.resolve()).then(() => {
      pending.update({ kind: ok === list.length ? 'success' : 'info', title: 'Обновлено ' + ok + ' из ' + list.length, message: '«' + ver.install + '»', timeout: 6000, notify: true, target: { tab: 'mods', view: 'installed', instanceId: ver.id } });
    });
  }

  duplicateInstance(v) {
    const name = (v.install + ' (копия)').slice(0, 40);
    this.setState({ instMenu: '' });
    const pending = window.SignalBridge.toast('Копирую моды, настройки и миры…', { kind: 'loading', title: 'Дублирование «' + v.install + '»', timeout: 0 });
    window.SignalBridge.duplicateInstance({ id: v.id, name: name })
      .then((newId) => {
        const mine = this.state.installedBy[v.id];
        if (mine) this.setState({ installedBy: Object.assign({}, this.state.installedBy, { [newId]: Object.assign({}, mine) }) });
        this.loadInstances(false);
        pending.update({ kind: 'success', title: 'Создана «' + name + '»', message: 'Копия без логов и статистики игры.', timeout: 5000, target: { tab: 'installs', instanceId: newId } });
      })
      .catch((e) => pending.update({ kind: 'error', title: 'Не удалось дублировать', message: String(e), timeout: 9000 }));
  }

  deleteInstance(v) {
    if (this.state.running[v.id]) {
      window.SignalBridge.toast('Сначала закройте игру этой установки.', { kind: 'info', title: '«' + v.install + '» запущена' });
      return;
    }
    if (this.state.instDel !== v.id) {
      clearTimeout(this.instDelTimer);
      this.setState({ instDel: v.id });
      this.instDelTimer = setTimeout(() => this.setState({ instDel: '' }), 4000);
      return;
    }
    clearTimeout(this.instDelTimer);
    window.SignalBridge.deleteInstance(v.id)
      .then(() => {
        const drop = (obj) => { const o = Object.assign({}, obj); delete o[v.id]; return o; };
        this.setState({
          instDel: '', instMenu: '',
          versionId: this.state.versionId === v.id ? 'v1' : this.state.versionId,
          installedBy: drop(this.state.installedBy), instMods: drop(this.state.instMods), modUpdates: drop(this.state.modUpdates),
          instances: (this.state.instances || []).filter((i) => i.id !== v.id)
        });
        this.loadInstances(false);
        window.SignalBridge.toast('Папка установки удалена вместе с модами, настройками и мирами.', { kind: 'info', title: '«' + v.install + '» удалена' });
      })
      .catch((e) => {
        this.setState({ instDel: '' });
        window.SignalBridge.toast(String(e), { kind: 'error', title: 'Установка не удалена' });
      });
  }

  saveRename() {
    const r = this.state.renaming;
    if (!r) return;
    const name = r.name.trim();
    if (!name) { this.setState({ renaming: Object.assign({}, r, { err: 'Введите название' }) }); return; }
    if (name.length > 40) { this.setState({ renaming: Object.assign({}, r, { err: 'Не длиннее 40 символов' }) }); return; }
    window.SignalBridge.renameInstance({ id: r.id, name: name })
      .then(() => {
        this.setState({ renaming: null, instances: (this.state.instances || []).map((i) => (i.id === r.id ? Object.assign({}, i, { name: name }) : i)) });
        this.loadInstances(false);
      })
      .catch((e) => this.setState({ renaming: Object.assign({}, r, { err: String(e) }) }));
  }

  plural(n, one, few, many) {
    return pluralize(n, one, few, many);
  }

  currentVersion() {
    const versions = this.versionList();
    return versions.find((v) => v.id === this.state.versionId) || versions[0];
  }

  refreshMods(instanceId) {
    // The shader list knows whether Iris is among the mods.
    if (this.state.instPacks[instanceId + ':shader']) this.refreshPacks(instanceId, 'shader');
    window.SignalBridge.listMods(instanceId)
      .then((list) => { if (list) this.setState({ instMods: Object.assign({}, this.state.instMods, { [instanceId]: list }) }); })
      .catch(() => {});
  }

  loadWorlds() {
    window.SignalBridge.listWorlds()
      .then((list) => { if (list) this.setState({ realWorlds: list }); })
      .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Миры не прочитались' }));
  }

  loadShots() {
    window.SignalBridge.listScreenshots()
      .then((list) => { if (list) this.setState({ realShots: list }); })
      .catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Скриншоты не прочитались' }));
  }

  backupWorld(w) {
    const pending = window.SignalBridge.toast('Упаковываю «' + w.name + '» в zip…', { kind: 'loading', title: 'Бэкап мира', timeout: 0 });
    return window.SignalBridge.backupWorld(w.path)
      .then((zip) => pending.update({ kind: 'success', title: 'Бэкап «' + w.name + '» готов', message: zip, timeout: 7000, target: { tab: 'worlds' } }))
      .catch((e) => pending.update({ kind: 'error', title: 'Бэкап не удался', message: String(e), timeout: 9000 }));
  }

  fmtBytes(b) {
    if (b >= 1e9) return decimal(b / 1e9) + ' ГБ';
    if (b >= 1e6) return Math.round(b / 1e6) + ' МБ';
    return Math.max(1, Math.round(b / 1e3)) + ' КБ';
  }

  fmtAgo(ms) {
    if (!ms) return 'неизвестно';
    const days = (Date.now() - ms) / 864e5;
    if (days < 1) return 'сегодня';
    if (days < 2) return 'вчера';
    // «5 дней назад», not «5 дн. назад»: an abbreviation's period would read as a sentence end to the translator.
    const n = Math.floor(days);
    if (days < 7) return n + ' ' + this.plural(n, 'день', 'дня', 'дней') + ' назад';
    return new Date(ms).toLocaleDateString(LOCALE);
  }

  worldCards(s, mock) {
    if (!s.realWorlds) {
      // Browser preview: the first two sample worlds pretend to live in the shared .minecraft/saves.
      return mock.map((m, i) => Object.assign({}, m, {
        canTransfer: i < 2,
        transfer: () => this.setState({ wtWorld: { name: m.name, path: '%MINECRAFT%\\saves\\' + m.name, version: m.version.split(' ').pop(), instanceId: '' } })
      }));
    }
    const modes = ['Выживание', 'Творческий', 'Приключение', 'Наблюдатель'];
    const diffs = ['Мирная', 'Лёгкая', 'Нормальная', 'Сложная'];
    const tints = ['#1A2A22', '#1A2230', '#16301E', '#3B2A1E'];
    const versions = this.versionList();
    return s.realWorlds.map((w, i) => {
      const inst = versions.find((v) => v.id === w.instanceId);
      return {
        name: w.name,
        mode: w.hardcore ? 'Хардкор' : (modes[w.gameType] || 'Выживание'),
        diff: w.difficulty == null ? 'сложность —' : (diffs[w.difficulty] || 'сложность —'),
        version: (w.version ? 'Minecraft ' + w.version : 'версия неизвестна') + ' · ' + (inst ? inst.install : w.instanceName),
        last: this.fmtAgo(w.lastPlayed),
        size: this.fmtBytes(w.size),
        thumb: w.icon ? window.SignalBridge.fileSrc(w.icon) : '/assets/img/shot-taiga.webp',
        thumbOpacity: w.icon ? 1 : 0.35,
        tint: tints[i % tints.length],
        backup: () => this.backupWorld(w),
        canTransfer: !w.instanceId,
        transfer: () => this.setState({ wtWorld: w }),
        play: () => this.setState(inst ? { versionId: inst.id, tab: 'play' } : { tab: 'play' })
      };
    });
  }

  shotCards(s, mock) {
    if (!s.realShots) return mock;
    return s.realShots.map((sh) => ({
      src: window.SignalBridge.fileSrc(sh.path),
      alt: sh.name,
      file: sh.name,
      size: this.fmtBytes(sh.size) + ' · ' + sh.instanceName,
      open: () => window.SignalBridge.openFolder(sh.path),
      copy: () => {
        try { navigator.clipboard.writeText(sh.path); } catch (e) {}
        window.SignalBridge.toast(sh.path, { kind: 'info', title: 'Путь к снимку скопирован', timeout: 3000 });
      }
    }));
  }

  modSize(name, group) {
    let h = 0;
    for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
    const r = (h % 1000) / 1000;
    const range = [[0.3, 2.4], [0.2, 1.4], [0.5, 3.8], [1.6, 12]][group] || [0.5, 3];
    return range[0] + r * (range[1] - range[0]);
  }

  versionList() {
    const s = this.state;
    const mv = s.mcVersions;
    const latest = this.latestRelease();
    const when = (id) => {
      const v = mv && mv.versions.find((x) => x.id === id);
      return v && v.releaseTime ? ' · вышла ' + new Date(v.releaseTime).toLocaleDateString(LOCALE) : '';
    };
    const disk = s.instances || [];
    const onDisk = (id) => disk.find((i) => i.id === id);
    const lastOf = (id) => (onDisk(id) && onDisk(id).lastPlayed ? this.fmtWhen(onDisk(id).lastPlayed) : 'ещё не запускалась');
    const backupOf = (id) => (onDisk(id) && onDisk(id).lastBackup) || null;
    const live = [{ id: 'v1', name: 'Release ' + latest, sub: 'Последний релиз' + when(latest), kind: 'release', loader: 'vanilla', install: 'Последний релиз', last: lastOf('v1'), lastBackup: backupOf('v1') }];
    if (s.opts.snapshots && mv && mv.latestSnapshot && mv.latestSnapshot !== latest) {
      live.push({ id: 'vsnap', name: 'Snapshot ' + mv.latestSnapshot, sub: 'Последний снапшот' + when(mv.latestSnapshot), kind: 'snapshot', loader: 'vanilla', install: 'Последний снапшот', last: lastOf('vsnap'), lastBackup: backupOf('vsnap') });
    }
    // Until list_instances answers, installs from older builds stand in for the disk list.
    const rest = s.instances
      ? disk.filter((i) => i.id !== 'v1' && i.id !== 'vsnap').map((i) => this.instanceEntry(i))
      : (s.customInstalls || []);
    return live.concat(rest);
  }

  launch(server) {
    if (this.state.launching) return;
    const s = this.state;
    const versions = this.versionList();
    const ver = versions.find((v) => v.id === s.versionId) || versions[0];
    if (s.running[ver.id]) {
      window.SignalBridge.toast('«' + ver.install + '» уже запущена. Закройте игру или выберите другую установку.', { kind: 'info', title: 'Игра уже идёт' });
      return;
    }
    const own = this.instLaunchOf(s, ver.id);
    const allAcc = this.allAccounts();
    const acc = allAcc.find((a) => a.id === this.state.accountId) || allAcc[0];
    clearTimeout(this.timer2);
    this.setState({ launching: true, launched: false, progress: 0, launchStage: 'Подготовка…', verMenu: false, accMenu: false });
    // Offline: the launcher's skin goes into the game as a resource pack (a failure only skips it).
    const offline = !acc || acc.type !== 'ms';
    const skinJob = offline && s.opts.offlineSkin
      ? this.skinVariants().catch((e) => { console.warn('[signal] skin pack:', e); return null; })
      : Promise.resolve(null);
    skinJob.then((skin) => window.SignalBridge.launchMinecraft({
      version: ver.name,
      username: acc ? acc.name : 'Player',
      accountId: acc && acc.type === 'ms' ? acc.id : null,
      discord: !!s.opts.discord && !!s.integrations.discord,
      ramGb: own ? own.ramGb : this.state.ram,
      jvmArgs: own ? (own.jvmArgs || '') : this.state.jvm,
      javaPath: own ? (own.java || null) : (s.javaPath || null),
      instanceId: ver.id,
      instanceName: ver.install,
      priority: s.priority,
      hideLauncher: !!s.opts.hide,
      backup: s.opts.backup ? s.backupFreq : '',
      openConsole: !!s.opts.console,
      server: server || null,
      skin: skin,
      companion: s.opts.companion !== false,
      // Real progress from the Rust downloader («Скачивание библиотек (12/48)…» and so on).
      onProgress: (p) => this.setState({ progress: Math.max(this.state.progress, p.percent), launchStage: p.label })
    }))
      .then((result) => {
        const running = result && result.mode === 'started' ? Object.assign({}, this.state.running, { [ver.id]: true }) : this.state.running;
        this.setState({ progress: 100, launched: true, launchStage: '', running: running });
        this.loadInstances(false);
        this.play('done');
        this.timer2 = setTimeout(() => this.setState({ launching: false, launched: false, progress: 0 }), 2600);
      })
      .catch((e) => {
        console.error('[signal] launch_minecraft:', e);
        this.play('close');
        this.setState({ launching: false, launched: false, progress: 0, launchStage: '' });
      });
  }

  // The friends drawer: rows with heads, statuses and actions, the socket's state and the header badge.
  friendsVals() {
    const s = this.state;
    const fr = s.fr || { friends: [], incoming: [], outgoing: [] };
    const looks = {
      playing_singleplayer: { dot: '#5BD07A', text: '#86DDA0', label: 'Играет', order: 0 },
      hosting_p2p: { dot: '#B48CFF', text: '#CDB8FF', label: 'Открыл мир', order: 0 },
      in_menu: { dot: '#4FA3FF', text: '#8EC2FF', label: 'В лаунчере', order: 1 },
      offline: { dot: '#5D6672', text: '#8A94A3', label: 'Не в сети', order: 2 }
    };
    const ago = (iso) => {
      const min = Math.max(0, Math.round((Date.now() - Date.parse(iso)) / 60000));
      if (!iso || isNaN(min)) return '';
      if (min < 1) return 'только что';
      if (min < 60) return min + ' мин';
      if (min < 48 * 60) return Math.floor(min / 60) + ' ч';
      return Math.floor(min / 1440) + ' дн';
    };
    const head = (name) => {
      const face = s.frFaces[name.toLowerCase()] || this.steveFace;
      const h = face && face.length === 64 ? this.faceHead(face) : this.head('default');
      return { bg: h.bg, sh: h.sh4 };
    };
    const person = (f) => Object.assign({ name: f.name, isOffline: f.kind === 'offline', busy: s.frBusy === f.uuid }, head(f.name));
    const friends = fr.friends
      .map((f) => Object.assign({}, f, { look: looks[f.status] || looks.offline }))
      .sort((a, b) => a.look.order - b.look.order || a.name.toLowerCase().localeCompare(b.name.toLowerCase()))
      .map((f) => {
        const on = f.status !== 'offline';
        const time = f.status !== 'in_menu' && f.since ? ago(f.since) : '';
        const hosting = f.status === 'hosting_p2p';
        const label = hosting && f.world ? f.look.label + ' «' + f.world + '»' : f.look.label;
        return Object.assign(person(f), {
          on: on,
          dot: f.look.dot, textColor: f.look.text, fade: on ? 1 : 0.55,
          detail: [label, on ? f.detail : '', on ? time : ''].filter(Boolean).join(' · '),
          canJoin: hosting && !!f.join,
          joinTitle: f.join ? f.join + (f.via === 'direct' ? ' · напрямую' : ' · через сервер Signal') : '',
          join: () => this.joinFriend(f),
          delCls: s.frDel === f.uuid ? 'acc-del is-confirm' : 'acc-del',
          delConfirm: s.frDel === f.uuid,
          delIdle: s.frDel !== f.uuid,
          del: () => this.removeFriend(f, true)
        });
      });
    const online = friends.filter((f) => f.on);
    const offline = friends.filter((f) => !f.on);
    const incoming = fr.incoming.map((f) => Object.assign(person(f), {
      when: ago(f.at),
      accept: () => this.answerFriend(f, true),
      decline: () => this.answerFriend(f, false)
    }));
    const outgoing = fr.outgoing.map((f) => Object.assign(person(f), { when: ago(f.at), cancel: () => this.removeFriend(f, false) }));
    const p = s.presence || { state: 'off' };
    const link = {
      online: ['#5BD07A', 'В сети'],
      connecting: ['#F0B95A', 'Подключение…'],
      reconnecting: ['#F2998B', p.retryIn ? 'Нет связи · повтор через ' + p.retryIn + ' с' : 'Нет связи'],
      unclaimed: ['#8A94A3', 'Не в сети'],
      off: ['#8A94A3', 'Не подключено']
    }[p.state] || ['#8A94A3', 'Не подключено'];
    const profile = this.friendsProfile();
    const loaded = !!s.fr;
    return {
      online, offline, incoming, outgoing,
      hasOnline: online.length > 0,
      hasOffline: offline.length > 0,
      hasIncoming: incoming.length > 0,
      hasOutgoing: outgoing.length > 0,
      onlineCount: online.length,
      offlineCount: offline.length,
      incomingCount: incoming.length,
      outgoingCount: outgoing.length,
      linkColor: link[0],
      linkText: link[1],
      linkTitle: p.error || '',
      noProfile: !profile,
      unclaimed: !!profile && s.frUnclaimed,
      loading: !!profile && !loaded && !s.frErr,
      hasErr: !!profile && !!s.frErr,
      err: s.frErr,
      retry: () => { this.setState({ frErr: '' }); this.loadFriends(); },
      empty: !!profile && loaded && !s.frUnclaimed && !friends.length && !incoming.length && !outgoing.length,
      myName: profile ? profile.name : '',
      hosting: !!(s.myHost && s.myHost.join),
      hostWorld: s.myHost && s.myHost.world ? '«' + s.myHost.world + '»' : 'Ваш мир',
      hostJoin: s.myHost ? s.myHost.join || '' : '',
      hostVia: s.myHost && s.myHost.via === 'direct' ? 'напрямую через роутер (UPnP)' : 'через сервер Signal',
      copyHost: () => {
        try { if (navigator.clipboard && s.myHost) navigator.clipboard.writeText(s.myHost.join).catch(() => {}); } catch (e) {}
        window.SignalBridge.toast(s.myHost ? s.myHost.join : '', { kind: 'success', title: 'Адрес скопирован', timeout: 2500 });
      },
      adding: s.frBusy === 'add',
      addLabel: s.frBusy === 'add' ? 'Отправка…' : 'Добавить',
      // header badge: requests first, else friends online
      badge: incoming.length || online.length,
      hasBadge: incoming.length + online.length > 0,
      badgeCls: incoming.length ? 'fr-badge is-req' : 'fr-badge',
      buttonLabel: incoming.length ? 'Друзья: заявок ' + incoming.length : 'Друзья: в сети ' + online.length
    };
  }

  renderVals() {
    const s = this.state;
    const accent = this.props.accent ?? '#2A8040';
    const vers = this.versionList();
    const go = (tab) => () => this.setState({ tab: tab, accMenu: false, verMenu: false });

    const accounts = this.allAccounts().map((a) => Object.assign({}, a, a.face && a.face.length === 64 ? this.faceHead(a.face) : this.head(a.skin), {
      delCls: s.accDel === a.id ? 'acc-del is-confirm' : 'acc-del',
      delLabel: s.accDel === a.id ? 'Удалить?' : '',
      delConfirm: s.accDel === a.id,
      delIdle: s.accDel !== a.id,
      del: () => this.removeAccount(a),
      isMs: a.type === 'ms',
      isOffline: a.type === 'offline',
      active: a.id === s.accountId,
      rowCls: a.id === s.accountId ? 'item is-active' : 'item',
      pick: () => this.pickAccount(a)
    }));
    const cur = accounts.find((a) => a.active) || accounts[0];

    const versions = vers.map((v) => Object.assign({}, v, {
      // An installation shows its own name («Арена без лагов»), the game version goes under it;
      // the built-in «Последний релиз/снапшот» keep the version as the title.
      title: v.onDisk && v.install ? v.install : v.name,
      titleSub: v.onDisk && v.install ? v.name + ' · ' + v.sub : v.sub,
      hasIcon: !!v.icon,
      iconSrc: v.icon ? window.SignalBridge.fileSrc(v.icon) : '',
      isVanilla: !v.icon && v.loader === 'vanilla',
      isForge: !v.icon && v.loader === 'forge',
      isFabric: !v.icon && v.loader === 'fabric',
      menuOpen: s.instMenu === v.id,
      menuLabel: 'Действия с «' + v.install + '»',
      toggleMenu: () => this.setState({ instMenu: s.instMenu === v.id ? '' : v.id, instDel: '' }),
      // Built-in «Последний релиз/снапшот» can be copied (once launched) but not renamed or deleted.
      canEdit: !!v.onDisk,
      canDuplicate: !!v.onDisk || (s.instances || []).some((i) => i.id === v.id),
      rename: () => this.setState({ renaming: { id: v.id, name: v.install, err: '' }, instMenu: '' }),
      duplicate: () => this.duplicateInstance(v),
      exportIt: () => this.openExport(v),
      launchIt: () => this.openInstLaunch(v),
      ownLaunch: !!this.instLaunchOf(s, v.id),
      backupsIt: () => this.openBackups(v),
      repairIt: () => this.repairInstance(v),
      delLabel: s.instDel === v.id ? 'Точно удалить? Папка будет стёрта' : 'Удалить',
      delCls: s.instDel === v.id ? 'inst-act is-danger is-confirm' : 'inst-act is-danger',
      del: () => this.deleteInstance(v),
      selected: v.id === s.versionId,
      rowCls: v.id === s.versionId ? 'item is-active' : 'item',
      pick: () => {
        this.setState({ versionId: v.id, verMenu: false });
        if (s.tab === 'mods') {
          this.refreshMods(v.id);
          if (s.imKind !== 'mod') this.refreshPacks(v.id, s.imKind);
        }
      },
      playIt: () => this.setState({ versionId: v.id, tab: 'play' }),
      openDir: () => window.SignalBridge.openFolder('%MINECRAFT%\\instances\\' + v.id)
    }));
    const ver = versions.find((v) => v.selected) || versions[0];

    const titles = {
      play: ['Игра', 'Ваш скин в 3D · перетащите, чтобы повернуть на 360°'],
      installs: ['Установки', 'Профили версий, загрузчики и папки игры'],
      mods: ['Моды и Сборки', 'Устанавливаются в: ' + ver.install + ' (' + ver.name + ')'],
      settings: ['Настройки лаунчера', 'Память, Java и поведение лаунчера']
    };

    let step = 'Проверка файлов';
    if (s.progress >= 30) step = 'Загрузка ресурсов';
    if (s.progress >= 60) step = 'Подготовка Java';
    if (s.progress >= 85) step = 'Запуск игры';
    let playLabel = cur.isOffline ? 'ИГРАТЬ ОФЛАЙН' : 'ИГРАТЬ';
    let playSub = ver.name;
    if (s.launching && !s.launched) { playLabel = s.progress < 99 ? 'ЗАГРУЗКА…' : 'ЗАПУСК…'; playSub = (s.launchStage || step) + ' · ' + s.progress + '%'; }
    if (s.launched) { playLabel = 'ИГРА ЗАПУЩЕНА'; playSub = ver.name; }
    const inGame = !!s.running[ver.id] && !s.launching;
    if (inGame) { playLabel = 'В ИГРЕ'; playSub = ver.name + ' · окно вернётся после выхода'; }

    const cq = this.catalogQuery(s, ver);
    if (s.tab === 'mods' && s.modsView === 'catalog') this.ensureCatalog(cq);
    const catalog = this.catalogCards(s, ver);
    const instFiles = s.instMods[ver.id];
    const installedCount = instFiles ? instFiles.filter((f) => f.enabled).length : 0;

    const optDefs = [
      { id: 'hide', label: 'Скрывать лаунчер во время игры', desc: 'Окно вернётся после выхода из Minecraft.' },
      { id: 'snapshots', label: 'Показывать снапшоты', desc: 'Экспериментальные версии появятся в списке.' },
      { id: 'autoUpdate', label: 'Обновлять лаунчер автоматически', desc: 'Загрузка в фоне, установка при следующем запуске.' },
      { id: 'animations', label: 'Анимации интерфейса', desc: 'Плавные переходы и эффекты наведения.' },
      { id: 'sound', label: 'Звуки интерфейса', desc: 'Щелчки, наведение и сигнал запуска игры.' }
    ];
    const opts = optDefs.map((o) => {
      const on = !!s.opts[o.id];
      return Object.assign({}, o, {
        on: on,
        swCls: on ? 'sw is-on' : 'sw',
        toggle: () => this.setState({ opts: Object.assign({}, s.opts, { [o.id]: !on }) })
      });
    });

    const nick = s.nick;
    const valid = /^[A-Za-z0-9_]{3,16}$/.test(nick);
    const taken = this.allAccounts().some((a) => a.name.toLowerCase() === nick.toLowerCase());
    let nickHint = '3–16 символов: латинские буквы, цифры и _';
    let nickHintColor = '#8A94A3';
    if (nick && !valid) { nickHint = 'Только латиница, цифры и _, от 3 до 16 символов'; nickHintColor = '#F2998B'; }
    else if (nick && taken) { nickHint = 'Такой никнейм уже добавлен'; nickHintColor = '#F2998B'; }
    else if (nick) { nickHint = 'Никнейм подходит'; nickHintColor = '#7FD99A'; }

    const modalTitles = { choose: 'Добавить аккаунт', ms: 'Вход Microsoft', offline: 'Автономный режим' };

    const base = {
      accent: accent,
      isPlay: s.tab === 'play',
      isInstalls: s.tab === 'installs',
      isMods: s.tab === 'mods',
      isSettings: s.tab === 'settings',
      navPlay: s.tab === 'play' ? 'is-active' : '',
      navInstalls: s.tab === 'installs' ? 'is-active' : '',
      navMods: s.tab === 'mods' ? 'is-active' : '',
      navSettings: s.tab === 'settings' ? 'is-active' : '',
      goPlay: go('play'),
      goInstalls: go('installs'),
      goMods: go('mods'),
      goSettings: go('settings'),

      accounts: accounts,
      accountCount: accounts.length,
      cur: cur,
      accMenu: s.accMenu,
      accRot: s.accMenu ? 180 : 0,
      toggleAcc: () => this.setState({ accMenu: !s.accMenu, verMenu: false }),
      anyMenu: s.accMenu || s.verMenu || s.notifOpen,
      closeMenus: () => this.setState({ accMenu: false, verMenu: false, notifOpen: false }),

      versions: versions,
      ver: ver,
      verMenu: s.verMenu,
      verRot: s.verMenu ? 180 : 0,
      toggleVer: () => this.setState({ verMenu: !s.verMenu, accMenu: false }),

      onAnyClick: (e) => {
        const el = e.target && e.target.closest ? e.target.closest('button, a') : null;
        if (!el || el.disabled) return;
        this.play(el.getAttribute('data-sfx') || 'click');
      },
      onAnyHover: (e) => {
        const el = e.target && e.target.closest ? e.target.closest('[data-hover]') : null;
        if (!el || (e.relatedTarget && el.contains(e.relatedTarget))) return;
        this.play('hover');
      },
      soundOn: !!s.opts.sound,
      soundOff: !s.opts.sound,
      toggleSound: () => this.setState({ opts: Object.assign({}, s.opts, { sound: !s.opts.sound }) }),

      skinSize: 640,
      skinOwner: cur.name,
      modelRef: (el) => { this.modelEl = el; this.applyView(); },
      angleRef: (el) => { this.angleEl = el; this.applyView(); },
      dragStart: (e) => {
        try { e.currentTarget.setPointerCapture(e.pointerId); } catch (err) {}
        this.drag = { x: e.clientX, y: e.clientY, a: this.angle, t: this.tilt };
      },
      dragMove: (e) => {
        if (!this.drag) return;
        this.angle = this.drag.a + (e.clientX - this.drag.x) * 0.6;
        this.tilt = Math.max(-35, Math.min(25, this.drag.t - (e.clientY - this.drag.y) * 0.3));
        this.applyView();
      },
      dragEnd: () => { this.drag = null; },
      viewFront: () => { this.angle = 0; this.tilt = -10; this.setState({ autoSpin: false }); this.applyView(); },
      viewSide: () => { this.angle = 90; this.tilt = -10; this.setState({ autoSpin: false }); this.applyView(); },
      viewBack: () => { this.angle = 180; this.tilt = -10; this.setState({ autoSpin: false }); this.applyView(); },
      autoSpin: s.autoSpin,
      spinOff: !s.autoSpin,
      toggleSpin: () => this.setState({ autoSpin: !s.autoSpin }),
      poseIdle: s.pose === 'idle',
      poseWalk: s.pose === 'walk',
      poseIdleCls: s.pose === 'idle' ? 'is-active' : '',
      poseWalkCls: s.pose === 'walk' ? 'is-active' : '',
      setIdle: () => this.setState({ pose: 'idle' }),
      setWalk: () => this.setState({ pose: 'walk' }),
      showLayer: s.showLayer,
      layerSw: s.showLayer ? 'sw is-on' : 'sw',
      toggleLayer: (e) => { if (e && e.preventDefault) e.preventDefault(); this.setState({ showLayer: !s.showLayer }); },

      progress: s.progress,
      playLabel: playLabel,
      playSub: playSub,
      iconPlay: !s.launching && !inGame && !cur.isOffline,
      iconOffline: !s.launching && !inGame && cur.isOffline,
      iconSpin: s.launching && !s.launched,
      iconDone: s.launched || inGame,
      launch: () => this.launch(),
      toggleConsole: () => window.SignalBridge.console.toggle(ver.id),

      modalOpen: !!s.modal,
      modalTitle: modalTitles[s.modal] || '',
      stepChoose: s.modal === 'choose',
      stepMs: s.modal === 'ms',
      stepOffline: s.modal === 'offline',
      notChoose: !!s.modal && s.modal !== 'choose',
      openAdd: () => this.setState({ accMenu: false, modal: 'choose', nick: '' }),
      closeModal: () => { this.cancelMsLogin(); this.setState({ modal: null }); },
      backModal: () => { this.cancelMsLogin(); this.setState({ modal: 'choose' }); },
      chooseMs: () => this.startMsLogin(),
      msOff: !s.integrations.microsoft,
      msOptionDesc: s.integrations.microsoft ? 'Вход через учётную запись Microsoft: онлайн-серверы, ваш скин и Realms.' : 'Временно недоступен: у лаунчера пока нет своего id приложения Azure.',
      msOptionColor: s.integrations.microsoft ? '#A3ADBB' : '#F0B95A',
      msOn: !!s.integrations.microsoft,
      msStarting: s.msPhase === 'starting',
      msWaiting: s.msPhase === 'waiting',
      msFailed: s.msPhase === 'error',
      msCode: s.msCode,
      msHost: (s.msUri || 'https://www.microsoft.com/link').replace(/^https?:\/\/(www\.)?/, ''),
      msErr: s.msErr,
      msCopyLabel: s.msCopied ? 'Скопирован ✓' : 'Копировать код',
      msCopy: () => {
        try { navigator.clipboard.writeText(s.msCode); } catch (e) {}
        this.setState({ msCopied: true });
      },
      msOpenPage: () => window.SignalBridge.openLoginPage(s.msUri || 'https://www.microsoft.com/link').catch((e) => window.SignalBridge.toast(String(e), { kind: 'error', title: 'Браузер не открылся' })),
      msRetry: () => this.startMsLogin(),
      chooseOffline: () => {
        this.setState({ modal: 'offline' });
        window.SignalBridge.warmSkinLookup().catch(() => {});
      },
      nick: nick,
      onNick: (e) => this.setState({ nick: e.target.value }),
      onNickKey: (e) => { if (e.key === 'Enter') base.addOffline(); },
      nickHint: nickHint,
      nickHintColor: nickHintColor,
      nickInvalid: !valid || taken,
      nickPreview: nick || 'Никнейм',
      newHead: this.head('hood'),
      // Saved at once with Steve and the dialog closes; the nickname's skin (Mojang, else Ely.by) is fetched
      // in the background and put on the model and the avatar when it arrives (refreshAccountSkin).
      addOffline: () => {
        if (!valid || taken) return;
        const id = 'a' + Date.now();
        const next = {
          accounts: (s.accounts || []).concat([{ id: id, name: nick.trim(), type: 'offline', skin: 'hood' }]),
          accountId: id,
          modal: null,
          nick: ''
        };
        Object.assign(next, this.skinOf(null));
        this.setState(next, () => this.refreshAccountSkin(id));
      },

      viewCatalog: s.modsView !== 'installed',
      viewInstalled: s.modsView === 'installed',
      segCatalog: s.modsView !== 'installed' ? 'is-active' : '',
      segInstalled: s.modsView === 'installed' ? 'is-active' : '',
      setCatalog: () => this.setState({ modsView: 'catalog' }),
      setInstalled: () => {
        this.setState({ modsView: 'installed' });
        this.refreshMods(ver.id);
        if (s.imKind !== 'mod') this.refreshPacks(ver.id, s.imKind);
      },
      catalog: catalog,
      installedCount: installedCount,

      ram: s.ram,
      onRam: (e) => this.setState({ ram: Math.min(Number(e.target.value), s.ramMax), ramClamped: false }),
      jvm: s.jvm,
      onJvm: (e) => this.setState({ jvm: e.target.value }),
      opts: opts,
      isDark: s.theme === 'dark',
      isSys: s.theme === 'system',
      themeDark: s.theme === 'dark' ? 'is-active' : '',
      themeSys: s.theme === 'system' ? 'is-active' : '',
      setDark: () => this.setState({ theme: 'dark' }),
      setSys: () => this.setState({ theme: 'system' })
    };
    const extra = this.extraVals(s, accent, ver);
    extra.menuVersions = versions.filter((v) => v.selected || this.kindAllowed(v.kind || 'release'));

    // Catalog filters
    const tg = this.verTarget(ver);
    const loaderNames = { fabric: 'Fabric', forge: 'Forge', neoforge: 'NeoForge', quilt: 'Quilt' };
    const typeDefs = [['mod', 'Моды'], ['modpack', 'Сборки'], ['resourcepack', 'Ресурс-паки'], ['shader', 'Шейдеры']];
    extra.typeChips = typeDefs.map(([k, label]) => ({
      label: label,
      on: s.modType === k,
      cls: s.modType === k ? 'seg is-active' : 'seg',
      pick: () => this.setState(s.modType === k ? {} : { modType: k, modQuery: '' })
    }));
    extra.srcChips = [['mr', 'Modrinth', '#1BD96A'], ['cf', 'CurseForge', '#F16436']].map(([k, label, dot]) => ({
      label: label,
      dot: dot,
      on: s.modSrc === k,
      cls: s.modSrc === k ? 'seg is-active' : 'seg',
      pick: () => this.setState({ modSrc: k })
    }));
    extra.foundLabel = 'Найдено на ' + (cq.source === 'cf' ? 'CurseForge' : 'Modrinth') + ':';
    extra.isModpackType = s.modType === 'modpack';
    extra.showLoader = s.modType === 'mod' || s.modType === 'modpack';
    extra.showGameVer = s.modType !== 'modpack';
    extra.loaderOptions = [{ value: 'inst', label: 'Как у установки' + (tg.loader ? ' (' + loaderNames[tg.loader] + ')' : '') }, { value: 'any', label: 'Любой' }]
      .concat(Object.keys(loaderNames).map((k) => ({ value: k, label: loaderNames[k] })));
    extra.modLoader = s.modLoader;
    extra.onModLoader = (e) => this.setState({ modLoader: e.target.value });
    extra.verOptions = [{ value: 'inst', label: tg.mc ? 'Minecraft ' + tg.mc : 'Как у установки' }, { value: 'any', label: 'Любая версия' }];
    extra.modVer = s.modVer;
    extra.onModVer = (e) => this.setState({ modVer: e.target.value });
    extra.modSort = s.modSort;
    extra.onModSort = (e) => this.setState({ modSort: e.target.value });
    extra.modQuery = s.modQuery;
    extra.onModQuery = (e) => this.setState({ modQuery: e.target.value });
    extra.searchPlaceholder = ({ mod: 'Поиск модов: Sodium, JEI, Create…', modpack: 'Поиск сборок: Fabulously Optimized, Cobblemon…', resourcepack: 'Поиск ресурс-паков: Faithful, Fresh Animations…', shader: 'Поиск шейдеров: Complementary, BSL…' })[s.modType];
    const mr = s.mr;
    extra.mrLoading = mr.loading && !mr.more;
    extra.mrError = !!mr.err;
    extra.mrErr = mr.err;
    // CurseForge refusing (403) or unreachable: a note in the list, the source stays where it is.
    extra.mrErrIsCf = cq.source === 'cf';
    extra.mrErrTitle = cq.source === 'cf' ? 'CurseForge временно недоступен или отклонил запрос' : 'Каталог не загрузился';
    extra.mrErrHint = cq.source === 'cf' ? 'Попробуйте позже или переключитесь на Modrinth — там те же моды чаще всего тоже есть.' : '';
    extra.switchToMr = () => this.setState({ modSrc: 'mr' });
    extra.mrRetry = () => this.retryCatalog();
    extra.foundCount = mr.total.toLocaleString(LOCALE);
    extra.hasResults = !mr.loading && !mr.err && mr.hits.length > 0;
    extra.noResults = !mr.loading && !mr.err && mr.hits.length === 0;
    extra.showCards = mr.hits.length > 0 && !(mr.loading && !mr.more);
    extra.hasMore = !mr.err && mr.hits.length > 0 && mr.hits.length < mr.total;
    extra.moreLabel = mr.more ? 'Загружаю…' : 'Показать ещё · ' + Math.min(20, mr.total - mr.hits.length);
    extra.showMore = () => { if (!mr.loading) this.searchCatalog(JSON.parse(this.mrKey), true); };
    extra.resetFilters = () => this.setState({ modQuery: '', modLoader: 'inst', modVer: 'inst' });
    extra.filterNote = s.modType === 'modpack'
      ? 'Сборка станет новой установкой со своим загрузчиком и версией'
      : (s.modType === 'mod' && !tg.loader
        ? '«' + ver.install + '» — ванильная установка: моды ставятся только в установки с Fabric, Forge или NeoForge'
        : 'Ставится в «' + ver.install + '» · ' + ver.name);
    extra.filterNoteCls = s.modType === 'mod' && !tg.loader ? 'mr-note is-warn' : 'mr-note';

    // «Установленные»: mods, resource packs, shaders
    const kind = s.imKind;
    const imRows = kind === 'mod' ? this.installedModRows(s, ver) : this.packRows(s, ver, kind);
    const imOn = imRows ? imRows.filter((r) => r.on).length : 0;
    const folder = ({ mod: 'mods', resourcepack: 'resourcepacks', shader: 'shaderpacks' })[kind];
    extra.imKinds = [['mod', 'Моды'], ['resourcepack', 'Ресурс-паки'], ['shader', 'Шейдеры']].map(([k, label]) => ({
      label: label,
      on: kind === k,
      cls: kind === k ? 'chip is-active' : 'chip',
      pick: () => {
        this.setState({ imKind: k, modDel: '' });
        if (k === 'mod') this.refreshMods(ver.id); else this.refreshPacks(ver.id, k);
      }
    }));
    extra.imIsMods = kind === 'mod';
    extra.imMods = imRows || [];
    extra.imLoading = !imRows;
    extra.imEmpty = !!imRows && imRows.length === 0;
    extra.imCount = instFiles ? instFiles.length : 0;
    const word = kind === 'mod' ? 'включено' : (kind === 'shader' ? 'активен' : 'включено');
    extra.imSummary = imRows
      ? (imRows.length ? imRows.length + ' ' + this.plural(imRows.length, 'файл', 'файла', 'файлов') + ' в «' + ver.install + '» · ' + word + ' ' + imOn + (imRows.length > imOn ? ', выключено ' + (imRows.length - imOn) : '') : 'Папка ' + folder + ' установки «' + ver.install + '» пуста')
      : 'Читаю папку ' + folder + '…';
    extra.imFolderLabel = 'Папка ' + folder;
    extra.imEmptyTitle = ({ mod: 'Модов пока нет', resourcepack: 'Ресурс-паков пока нет', shader: 'Шейдеров пока нет' })[kind];
    extra.imEmptyText = kind === 'mod' ? 'Установите их из каталога или соберите сборку с ИИ.' : 'Найдите их в каталоге Modrinth или положите .zip в папку ' + folder + '.';
    extra.imBrowse = () => this.setState({ modsView: 'catalog', modType: kind });
    const shaders = s.instPacks[ver.id + ':shader'];
    extra.needIris = kind === 'shader' && !!shaders && !shaders.iris;
    extra.irisText = tg.loader === 'forge'
      ? 'Шейдеры на Forge работают через мод Oculus — без него паки из этой папки игра не загрузит.'
      : (tg.loader ? 'Шейдеры работают через мод Iris — без него паки из этой папки игра не загрузит.' : 'Шейдерам нужен мод Iris, а он ставится только в установку с Fabric или NeoForge.');
    extra.canIris = !!tg.loader && tg.loader !== 'quilt';
    extra.irisLabel = s.modBusy[tg.loader === 'forge' ? 'GchcoXML' : 'YL57xq9U'] ? 'Установка…' : 'Установить ' + (tg.loader === 'forge' ? 'Oculus' : 'Iris');
    extra.installIris = () => this.installShaderLoader(ver);
    extra.shaderNote = kind === 'shader' && !!shaders && shaders.entries.length > 1 ? 'Активным может быть только один шейдер — включение другого выключит текущий.' : '';
    extra.hasShaderNote = !!extra.shaderNote;

    Object.assign(extra, this.exportVals(s));
    Object.assign(extra, this.backupVals(s));
    Object.assign(extra, this.instLaunchVals(s));

    // Skin in an offline game
    const offlineAcc = !!cur && !cur.isMs;
    if (offlineAcc && s.opts.offlineSkin) this.ensureSkinModel(cur.name, ver.name);
    const gameModel = s.skinGame.key === (cur ? cur.name : '') + '|' + ver.name ? s.skinGame.model : '';
    extra.skinGameNote = !offlineAcc
      ? 'Аккаунт Microsoft: в игре виден скин из вашего профиля Minecraft.'
      : (!s.opts.offlineSkin
        ? 'Показ скина в офлайн-игре выключен: «Настройки → Система и Лаунчер».'
        : (gameModel
          ? 'В игре («' + ver.name + '», офлайн) модель ' + (gameModel === 'slim' ? 'Slim' : 'Classic') + ': для ника «' + cur.name + '» её выбирает сама игра, скин подгоняется под неё.'
          : 'Скин попадёт в офлайн-игру через ресурс-пак — без модов.'));
    extra.imRefresh = () => this.refreshMods(ver.id);
    const upd = s.modUpdates[ver.id] || {};
    const updCount = Object.keys(upd.map || {}).length;
    extra.imCheckLabel = upd.checking ? 'Проверяю…' : 'Проверить обновления';
    extra.imCheck = () => this.checkUpdates(ver);
    extra.imHasUpdates = updCount > 0;
    extra.imUpdateAllLabel = 'Обновить все (' + updCount + ')';
    extra.imUpdateAll = () => this.updateAll(ver);
    if (upd.at && imRows && imRows.length) {
      extra.imSummary += updCount
        ? ' · обновлений: ' + updCount
        : ' · все известные Modrinth моды актуальны' + (upd.unknown ? ' (не найдено на Modrinth: ' + upd.unknown + ')' : '');
    }

    // Interface language
    extra.langOptions = LANGS.map(([value, label]) => ({ value: value, label: label }));
    extra.langValue = LANG;
    extra.onLang = (e) => setLang(e.target.value);

    // Game folder and disk usage
    const gi = s.gameDirInfo;
    const uv = this.usageView(s);
    extra.gameDirPath = gi ? gi.path : '…';
    extra.gameDirNote = gi ? (gi.isDefault ? 'Стандартная папка — её же использует официальный лаунчер' : 'Своя папка · стандартная: ' + gi.defaultPath) : '';
    extra.gameDirCustom = !!(gi && !gi.isDefault);
    extra.changeGameDir = () => this.pickGameDir();
    extra.resetGameDir = () => { if (gi) this.pickGameDir(gi.defaultPath); };
    extra.usageRows = uv.rows;
    extra.usageSegs = uv.segs;
    extra.usageSummary = uv.summary;
    extra.hasUsage = uv.rows.length > 0;
    extra.refreshUsage = () => this.loadUsage();
    extra.usageRefreshLabel = s.usageBusy ? 'Считаю…' : 'Пересчитать';
    const dm = s.dirModal;
    extra.dirOpen = !!dm;
    extra.dirPath = dm ? dm.path : '';
    extra.dirCheck = dm ? (dm.check.minecraft ? 'В этой папке уже есть данные Minecraft' : (!dm.check.exists ? 'Папки ещё нет — она будет создана' : (dm.check.empty ? 'Папка пустая' : 'В папке есть другие файлы'))) : '';
    const canCopy = !!dm && (!dm.check.exists || dm.check.empty);
    const modeDefs = [
      ['use', 'Использовать как есть', 'Лаунчер начнёт работать с выбранной папкой. Текущая останется на месте.', true, ''],
      ['copy', 'Скопировать всё сюда', 'Версии, Java, установки и миры скопируются в новую папку' + (s.usage ? ' (' + this.fmtBytes(s.usage.total) + ')' : '') + '. Старая папка останется.', canCopy, 'Нужна пустая папка'],
      ['move', 'Перенести', 'Скопировать и удалить старую папку. На том же диске — мгновенно.', canCopy && !(dm && dm.isDefault), dm && dm.isDefault ? 'Стандартную .minecraft использует и официальный лаунчер — её можно только скопировать' : 'Нужна пустая папка']
    ];
    extra.dirModes = dm ? modeDefs.map((d) => ({
      label: d[1], desc: d[3] ? d[2] : d[4], on: dm.mode === d[0], off: !d[3],
      cls: 'dir-mode' + (dm.mode === d[0] ? ' is-on' : '') + (d[3] ? '' : ' is-off'),
      pick: () => { if (d[3] && !dm.busy) this.setState({ dirModal: Object.assign({}, dm, { mode: d[0], err: '' }) }); }
    })) : [];
    extra.dirBusy = !!(dm && dm.busy);
    extra.dirIdle = !(dm && dm.busy);
    extra.dirLabel = dm ? dm.label : '';
    extra.dirPct = dm ? Math.round(dm.percent || 0) : 0;
    extra.dirErr = dm ? dm.err : '';
    extra.hasDirErr = !!(dm && dm.err);
    extra.dirAction = dm && dm.busy ? 'Подождите…' : 'Сменить папку';
    extra.confirmDir = () => this.confirmDirChange();
    extra.pickAnotherDir = () => { if (!(dm && dm.busy)) this.pickGameDir(); };
    extra.closeDir = () => { if (!(dm && dm.busy)) this.setState({ dirModal: null }); };

    // Notification centre
    const unread = s.notifs.filter((n) => !n.read).length;
    extra.notifOpen = s.notifOpen;
    extra.toggleNotifs = () => {
      const open = !s.notifOpen;
      this.setState({ notifOpen: open, accMenu: false, verMenu: false, friendsOpen: false });
      if (open) window.SignalBridge.notifications.markAllRead();
    };
    extra.hasUnread = unread > 0;
    extra.bellLabel = unread ? 'Уведомления: ' + unread + ' новых' : 'Уведомления';
    extra.notifItems = this.notifCards(s);
    extra.noNotifs = s.notifs.length === 0;
    extra.hasNotifs = s.notifs.length > 0;
    extra.clearNotifs = () => window.SignalBridge.notifications.clear();

    // Launcher updates
    const lu = s.upd;
    const ui = lu.info;
    const mb = (b) => decimal(b / 1048576);
    const pct = lu.total ? Math.min(100, Math.round(lu.downloaded / lu.total * 100)) : 0;
    extra.appVersion = s.appVersion || '…';
    extra.updCheckLabel = lu.checking ? 'Проверяю…' : 'Проверить обновления лаунчера';
    extra.checkUpdates = () => this.checkLauncherUpdate(false);
    extra.updStatus = lu.err
      ? lu.err
      : (!ui ? 'Версия ' + (s.appVersion || '…') : (!ui.configured ? 'Версия ' + ui.current + ' · источник обновлений не задан в этой сборке' : (ui.available ? 'Доступна версия ' + ui.version + ' (у вас ' + ui.current + ')' : 'Версия ' + ui.current + ' — последняя')));
    extra.updHasNew = !!(ui && ui.available);
    extra.openUpdate = () => this.setUpd({ open: true });
    extra.updOpen = !!(lu.open && ui && ui.available);
    extra.updTitle = ui && ui.version ? 'Доступна новая версия Signal Launcher v' + ui.version : '';
    extra.updMeta = ui ? 'Сейчас у вас ' + ui.current + (ui.date ? ' · выпуск от ' + ui.date.split('-').reverse().join('.') : '') : '';
    extra.updNotes = (ui && ui.notes) || 'Заметок к выпуску нет.';
    extra.updBusy = lu.busy;
    extra.updIdle = !lu.busy;
    extra.updPct = pct;
    extra.updProgress = lu.total ? 'Скачано ' + mb(lu.downloaded) + ' из ' + mb(lu.total) + ' МБ · ' + pct + '%' : (lu.downloaded ? 'Скачано ' + mb(lu.downloaded) + ' МБ' : 'Подключаюсь…');
    extra.updErr = lu.err;
    extra.hasUpdErr = !!lu.err && !!lu.open;
    extra.updInstall = () => this.installLauncherUpdate();
    extra.updInstallLabel = lu.busy ? (pct >= 100 ? 'Устанавливаю…' : 'Скачиваю ' + pct + '%') : 'Обновить сейчас';
    extra.closeUpdate = () => { if (!lu.busy) this.setUpd({ open: false, err: '' }); };

    // «Серверы»
    extra.noServers = extra.servers.length === 0;
    extra.openAddServer = () => this.setState({ srvModal: { mode: 'add', name: '', addr: '', err: '' } });
    extra.openDirect = () => this.setState({ srvModal: { mode: 'direct', name: '', addr: '', err: '' } });
    extra.refreshServersBtn = () => this.refreshServers();
    extra.srvRefreshLabel = Object.values(s.serverStatus).some((x) => x.pending) ? 'Проверяю…' : 'Обновить список';
    const sm = s.srvModal;
    extra.srvOpen = !!sm;
    extra.srvIsAdd = !!sm && sm.mode === 'add';
    extra.srvTitle = sm && sm.mode === 'direct' ? 'Прямое подключение' : 'Добавить сервер';
    extra.srvSub = sm && sm.mode === 'direct' ? 'Игра «' + ver.install + '» запустится и сразу подключится' : 'Сервер появится в списке с пингом и онлайном';
    extra.srvAction = sm && sm.mode === 'direct' ? 'Подключиться' : 'Добавить';
    extra.srvName = sm ? sm.name : '';
    extra.srvAddr = sm ? sm.addr : '';
    extra.srvErr = sm ? sm.err : '';
    extra.hasSrvErr = !!(sm && sm.err);
    extra.onSrvName = (e) => this.setState({ srvModal: Object.assign({}, s.srvModal, { name: e.target.value, err: '' }) });
    extra.onSrvAddr = (e) => this.setState({ srvModal: Object.assign({}, s.srvModal, { addr: e.target.value, err: '' }) });
    extra.saveSrv = () => this.saveServerModal();
    extra.closeSrv = () => this.setState({ srvModal: null });

    // «Новости»
    const cards = this.newsCards(s);
    extra.newsTop = cards.slice(0, 1);
    extra.newsRest = cards.slice(1);
    extra.hasNews = cards.length > 0;
    extra.newsEmpty = !cards.length;
    extra.newsEmptyText = s.newsLoading ? 'Загружаю новости Minecraft…' : (s.newsErr ? s.newsErr : 'Новостей пока нет.');
    extra.newsStatus = s.news
      ? 'Официальная лента Mojang: новости Java Edition, релизы и снапшоты' + (s.news.offline ? ' · нет связи, показаны сохранённые' : '') + ' · обновлено ' + this.fmtWhen(s.news.updatedAt)
      : (s.newsLoading ? 'Загружаю ленту…' : 'Лента недоступна');
    extra.refreshNews = () => this.loadNews(true);
    extra.newsRefreshLabel = s.newsLoading ? 'Обновляю…' : 'Обновить';
    extra.newsBadge = this.newsBadge(s);
    extra.hasNewsBadge = !!extra.newsBadge;

    // «Установки»: search, «⋯» actions, rename modal, import.
    const iq = s.instQuery.trim().toLowerCase();
    extra.installsList = versions.filter((v) => !iq || (v.install + ' ' + v.name + ' ' + v.sub).toLowerCase().indexOf(iq) >= 0);
    extra.noInstallsFound = !!iq && extra.installsList.length === 0;
    extra.instQuery = s.instQuery;
    extra.onInstQuery = (e) => this.setState({ instQuery: e.target.value });
    extra.importPack = () => this.pickAndImport();
    extra.importLabel = s.importBusy ? 'Импорт…' : 'Импорт .mrpack';
    extra.renameOpen = !!s.renaming;
    extra.renameName = s.renaming ? s.renaming.name : '';
    extra.renameErr = s.renaming ? s.renaming.err : '';
    extra.hasRenameErr = !!(s.renaming && s.renaming.err);
    extra.onRenameName = (e) => this.setState({ renaming: Object.assign({}, s.renaming, { name: e.target.value, err: '' }) });
    extra.saveRename = () => this.saveRename();
    extra.closeRename = () => this.setState({ renaming: null });
    extra.imOpenDir = () => window.SignalBridge.openFolder('%MINECRAFT%\\instances\\' + ver.id + '\\' + ({ mod: 'mods', resourcepack: 'resourcepacks', shader: 'shaderpacks' })[s.imKind]);

    const mv = s.mcVersions;
    extra.verInfo = mv
      ? 'Mojang: релиз ' + mv.latestRelease + ', снапшот ' + mv.latestSnapshot + ' · в списках ' + mv.versions.filter((v) => this.kindAllowed(v.kind)).length + ' из ' + mv.versions.length + ' версий' + (mv.offline ? ' · из кэша, нет связи' : '')
      : (s.mcVersionsErr ? 'Список не загрузился: ' + s.mcVersionsErr : 'Загружаю список версий с Mojang…');
    extra.verRefreshLabel = s.mcLoading ? 'Обновляю…' : 'Обновить список версий';
    extra.refreshVersions = () => { if (!s.mcLoading) this.loadVersions(true); };

    const wt = s.wtWorld;
    extra.wtOpen = !!wt;
    extra.wtName = wt ? wt.name : '';
    extra.wtMeta = wt ? (wt.version ? 'Minecraft ' + wt.version : 'версия неизвестна') + ' · общая папка .minecraft' : '';
    extra.closeWt = () => { if (!s.wtBusy) this.setState({ wtWorld: null }); };
    extra.wtTargets = wt ? vers.map((v) => {
      const busy = s.wtBusy === v.id;
      const same = !!wt.version && v.name.split(' ').pop() === wt.version;
      return {
        install: v.install, name: v.name,
        isVanilla: v.loader === 'vanilla', isForge: v.loader === 'forge', isFabric: v.loader === 'fabric',
        same: same, busy: busy, idle: !busy,
        cls: busy ? 'wt-row is-busy' : 'wt-row',
        pick: () => this.transferWorld(wt, v)
      };
    }) : [];

    extra.worlds = this.worldCards(s, extra.mockWorlds);
    extra.shots = this.shotCards(s, extra.mockShots);
    extra.noWorlds = !!s.realWorlds && s.realWorlds.length === 0;
    extra.noShots = !!s.realShots && s.realShots.length === 0;
    extra.shotsCount = s.realShots
      ? s.realShots.length + ' ' + this.plural(s.realShots.length, 'снимок', 'снимка', 'снимков') + ' · все установки и общая папка'
      : 'Пример · реальные снимки появятся в приложении';
    if (s.realWorlds) {
      extra.backupStatus = 'Найдено ' + s.realWorlds.length + ' ' + this.plural(s.realWorlds.length, 'мир', 'мира', 'миров') + ' · ' + this.autoBackupText(s, ver);
    }
    extra.backupAll = () => {
      const list = s.realWorlds || [];
      if (!list.length) {
        window.SignalBridge.toast('Миров для бэкапа пока нет.', { kind: 'info', title: 'Бэкап' });
        return;
      }
      list.reduce((chain, w) => chain.then(() => this.backupWorld(w)), Promise.resolve());
    };
    return Object.assign(base, extra);
  }
}

return Component;
}

// --- Skin texture helpers (offline skin pack, see skinVariants) ---------------------------------

function copyPx(src, dst, sx, sy, dx, dy, w, h, flipX) {
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const from = ((sy + y) * 64 + sx + (flipX ? w - 1 - x : x)) * 4;
      const to = ((dy + y) * 64 + dx + x) * 4;
      for (let k = 0; k < 4; k++) dst.data[to + k] = src.data[from + k];
    }
  }
}

function clearPx(img, x0, y0, w, h) {
  for (let y = y0; y < y0 + h; y++) img.data.fill(0, (y * 64 + x0) * 4, (y * 64 + x0 + w) * 4);
}

// A 64×32 skin → 64×64: the left leg and arm are mirrored copies of the right ones (as the game does).
function legacyToModern(img) {
  const src = new ImageData(new Uint8ClampedArray(img.data), 64, 64);
  [[4, 16, 20, 48, 4, 4], [8, 16, 24, 48, 4, 4], [0, 20, 24, 52, 4, 12], [4, 20, 20, 52, 4, 12], [8, 20, 16, 52, 4, 12], [12, 20, 28, 52, 4, 12],
    [44, 16, 36, 48, 4, 4], [48, 16, 40, 48, 4, 4], [40, 20, 40, 52, 4, 12], [44, 20, 36, 52, 4, 12], [48, 20, 32, 52, 4, 12], [52, 20, 44, 52, 4, 12]]
    .forEach(([sx, sy, dx, dy, w, h]) => copyPx(src, img, sx, sy, dx, dy, w, h, true));
}

// Arm block at (x, y), 16×16. Slim faces: top 3, bottom 3, side 4, front 3, side 4, back 3 px wide.
function slimToWide(img, x, y) {
  const src = new ImageData(new Uint8ClampedArray(img.data), 64, 64);
  clearPx(img, x, y, 16, 16);
  const widen = (sx, sy, dx, dy, h) => { copyPx(src, img, sx, sy, dx, dy, 3, h); copyPx(src, img, sx + 2, sy, dx + 3, dy, 1, h); };
  widen(x + 4, y, x + 4, y, 4);            // top
  widen(x + 7, y, x + 8, y, 4);            // bottom
  copyPx(src, img, x, y + 4, x, y + 4, 4, 12);          // outer side
  widen(x + 4, y + 4, x + 4, y + 4, 12);   // front
  copyPx(src, img, x + 7, y + 4, x + 8, y + 4, 4, 12);  // inner side
  widen(x + 11, y + 4, x + 12, y + 4, 12); // back
}

function wideToSlim(img, x, y) {
  const src = new ImageData(new Uint8ClampedArray(img.data), 64, 64);
  clearPx(img, x, y, 16, 16);
  copyPx(src, img, x + 4, y, x + 4, y, 3, 4);           // top
  copyPx(src, img, x + 8, y, x + 7, y, 3, 4);           // bottom
  copyPx(src, img, x, y + 4, x, y + 4, 4, 12);          // outer side
  copyPx(src, img, x + 4, y + 4, x + 4, y + 4, 3, 12);  // front
  copyPx(src, img, x + 8, y + 4, x + 7, y + 4, 4, 12);  // inner side
  copyPx(src, img, x + 12, y + 4, x + 11, y + 4, 3, 12); // back
}
