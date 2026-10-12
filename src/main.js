// Entry point: wires the launcher UI to the Rust backend and mounts it.
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';

import '@fontsource/onest/400.css';
import '@fontsource/onest/500.css';
import '@fontsource/onest/600.css';
import '@fontsource/onest/700.css';
import '@fontsource/unbounded/500.css';
import '@fontsource/unbounded/600.css';
import '@fontsource/unbounded/700.css';
import '@fontsource/jetbrains-mono/400.css';
import '@fontsource/jetbrains-mono/500.css';
import './app.css';

import template from './launcher.html?raw';
import defineLogic from './launcher.logic.js';
import { mountDesign } from './dc-runtime.js';
import { toast } from './toast.js';
import { gameConsole } from './console.js';
import { newsReader } from './news-reader.js';
import { notifications } from './notify.js';
import { t, lang, setLang, decimal } from './i18n/index.js';

document.documentElement.lang = lang;

// True inside the Tauri window, false when the frontend is opened in a normal browser (npm run dev).
const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
const appWindow = isTauri ? getCurrentWindow() : null;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
// Browser preview only: an in-memory stand-in for instances/<id>/mods.
const demoMods = {};
// Browser preview only: resourcepacks/ and shaderpacks/ of each installation.
const demoPacks = {};
// Browser preview only: the CurseForge API needs the build's key and sends no CORS headers, so the
// preview shows a few sample projects.
const cfPage = (type, slug) => 'https://www.curseforge.com/minecraft/' + type + '/' + slug;
const demoCf = {
  samples: {
    mod: [
      { projectId: '238222', slug: 'jei', title: 'Just Enough Items (JEI)', description: 'View Items and Recipes', author: 'mezz', downloads: 321000000, categories: ['API and Library', 'forge', 'neoforge', 'fabric'], allowDistribution: true, pageUrl: cfPage('mc-mods', 'jei') },
      { projectId: '306612', slug: 'fabric-api', title: 'Fabric API', description: 'Core API library for the Fabric toolchain', author: 'modmuss50', downloads: 140000000, categories: ['API and Library', 'fabric'], allowDistribution: true, pageUrl: cfPage('mc-mods', 'fabric-api') },
      { projectId: '32274', slug: 'journeymap', title: 'JourneyMap', description: 'Real-time mapping in game or in a web browser as you explore.', author: 'techbrew', downloads: 230000000, categories: ['Map and Information', 'forge', 'fabric'], allowDistribution: false, pageUrl: cfPage('mc-mods', 'journeymap') }
    ],
    modpack: [
      { projectId: '715572', slug: 'all-the-mods-9', title: 'All the Mods 9', description: 'Kitchen-sink pack for Minecraft 1.20.1', author: 'ATMTeam', downloads: 9000000, categories: ['Multiplayer', 'forge'], allowDistribution: true, pageUrl: cfPage('modpacks', 'all-the-mods-9') }
    ],
    resourcepack: [
      { projectId: '236821', slug: 'faithful-32x', title: 'Faithful 32x', description: 'Vanilla look with double the resolution', author: 'Faithful', downloads: 20000000, categories: ['32x'], allowDistribution: true, pageUrl: cfPage('texture-packs', 'faithful-32x') }
    ],
    shader: [
      { projectId: '385587', slug: 'complementary-shaders', title: 'Complementary Shaders', description: 'Shaders with balanced looks and performance', author: 'EminGT', downloads: 15000000, categories: ['Realistic'], allowDistribution: true, pageUrl: cfPage('shaders', 'complementary-shaders') }
    ]
  }
};
// Browser preview only: worlds and backups for «Бэкапы миров».
const demoBackups = {};
const demoBackupsOf = (instanceId) => demoBackups[instanceId] || (demoBackups[instanceId] = {
  worlds: [
    { folder: 'Новый мир', name: 'Новый мир', version: '1.21.1', lastPlayed: Date.now() - 3600e3, size: 18400000, icon: null },
    { folder: 'Survival', name: 'Survival', version: '1.21.1', lastPlayed: Date.now() - 5 * 864e5, size: 64200000, icon: null }
  ],
  backups: [
    { file: 'Survival_2026-10-05_18-42-10.zip', path: 'C:\\demo\\backups\\auto\\Survival_2026-10-05_18-42-10.zip', world: 'Survival', folder: 'Survival', created: Date.now() - 5 * 864e5, size: 51200000, auto: true },
    { file: 'Old_world_2026-09-20_12-00-00.zip', path: 'C:\\demo\\backups\\Old_world_2026-09-20_12-00-00.zip', world: 'Old_world', folder: null, created: Date.now() - 20 * 864e5, size: 9100000, auto: false }
  ]
});
const demoPackList = (instanceId, kind) => {
  const all = demoPacks[instanceId] || (demoPacks[instanceId] = { resourcepack: [], shader: [] });
  return all[kind] || (all[kind] = []);
};
// Browser preview only: installations kept in localStorage instead of .minecraft/instances.
const DEMO_INSTANCES = 'signal.demo.instances';
const demoInstances = {
  read() {
    try { return JSON.parse(localStorage.getItem(DEMO_INSTANCES) || '[]'); } catch (e) { return []; }
  },
  write(list) {
    try { localStorage.setItem(DEMO_INSTANCES, JSON.stringify(list)); } catch (e) {}
  },
  upsert(id, patch) {
    const list = this.read();
    const i = list.findIndex((x) => x.id === id);
    const base = { id, name: id, version: '', created: Date.now(), lastPlayed: null, playTime: 0, lastBackup: null, lastExitCode: null, mods: 0, worlds: 0 };
    if (i >= 0) list[i] = Object.assign(list[i], patch); else list.push(Object.assign(base, patch));
    this.write(list);
  }
};
const fmtMinutes = (sec) => (sec < 60 ? 'меньше минуты' : sec < 3600 ? Math.round(sec / 60) + ' мин' : decimal(sec / 3600) + ' ч');
const gameExitHandlers = [];
const modpackDropHandlers = [];
const skinDropHandlers = [];
// Browser preview only: a believable game log (with a warning, an error and a stack trace)
// so the console can be tried without Java. `simulateGameExit({ crashed: true })` ends it.
const demoLog = [
  ['[main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.9', 'info'],
  ['[main/INFO]: Loading 14 mods:', 'info'],
  ['[main/WARN]: Mod sodium-extra uses a deprecated API', 'warn'],
  ['[Render thread/INFO]: Backend library: LWJGL version 3.3.3', 'info'],
  ['[Render thread/WARN]: Missing sound for event: minecraft:item.goat_horn.play', 'warn'],
  ['[Render thread/INFO]: Reloading ResourceManager: vanilla, fabric', 'info'],
  ['[Worker-Main-3/ERROR]: Failed to load texture: modid:textures/block/missing.png', 'error'],
  ['java.io.FileNotFoundException: modid:textures/block/missing.png', 'error'],
  ['\tat net.minecraft.class_3300.method_14486(class_3300.java:57)', 'trace'],
  ['\tat net.minecraft.class_1049.method_4659(class_1049.java:38)', 'trace'],
  ['\t... 8 more', 'trace'],
  ['[Render thread/INFO]: Created: 1024x512x4 minecraft:textures/atlas/blocks.png-atlas', 'info'],
  ['[Render thread/INFO]: Sound engine started', 'info']
];
let demoLine = 0;
let demoErrorLine = null;
function demoStream(instanceId) {
  let i = 0;
  const timer = setInterval(() => {
    const chunk = demoLog.slice(i, i + 3).map(([text, level]) => {
      demoLine += 1;
      if (text.startsWith('java.io.')) demoErrorLine = demoLine;
      return { n: demoLine, level, text: level === 'trace' || text.startsWith('java.') ? text : '[' + new Date().toLocaleTimeString('ru-RU') + '] ' + text, err: false };
    });
    if (!chunk.length) { clearInterval(timer); return; }
    gameConsole.push({ instanceId, lines: chunk, skipped: 0 });
    i += 3;
  }, 350);
}

// Browser preview only: a pretend Microsoft account so the sign-in modal can be clicked through.
const demoMs = { accounts: [], cancelled: false };

// Every call from the UI to the desktop backend goes through this bridge.
window.SignalBridge = {
  isTauri,
  toast,

  /**
   * «ИГРАТЬ» → Rust: launch_minecraft. The backend downloads what is missing from Mojang and emits
   * `download-progress` ({ stage, label, percent, current, total }); this shows it in a toast and
   * passes it to `onProgress` for the PLAY button, then turns the toast into «Игра запускается!».
   * The button switches to «ЗАГРУЗКА… / ЗАПУСК…» and plays the launch sound (launch() in launcher.logic.js).
   * @returns {Promise<{ mode: string, title: string, message: string }>}
   */
  async launchMinecraft({ version, instanceId, instanceName, username, accountId, ramGb, jvmArgs, priority, hideLauncher, backup, discord, server, skin, javaPath, openConsole, onProgress }) {
    const pending = toast(`Подготовка игровых файлов версии ${version}…`, { kind: 'loading', title: `Запуск ${version}`, timeout: 0 });
    const report = (p) => {
      pending.update({ kind: 'loading', title: `Запуск ${version} · ${p.percent}%`, message: p.label, timeout: 0 });
      if (onProgress) onProgress(p);
    };
    let unlisten = null;
    // The console session starts with the launch, so download errors show up there too.
    gameConsole.begin(instanceId, instanceName || version, { open: openConsole });
    try {
      let result;
      if (isTauri) {
        unlisten = await listen('download-progress', (event) => report(event.payload));
        result = await invoke('launch_minecraft', { version, instanceId, instanceName, username, accountId, ramGb, jvmArgs, priority, hideLauncher, backup, discord, server, skin, javaPath: javaPath || null });
      } else {
        // Browser preview: replay the stages the desktop app reports.
        for (const [label, percent] of [
          ['Загрузка манифеста версий…', 1], ['Загрузка client.jar…', 6], ['Скачивание библиотек (31/62)…', 29],
          ['Скачивание ассетов (1940/3876)…', 64], ['Скачивание Java (java-runtime-delta)…', 90], ['Запуск игры…', 99]
        ]) {
          report({ label, percent });
          await delay(450);
        }
        result = { mode: 'browser', title: 'Игра запускается!', message: 'Это браузерный режим — в приложении здесь стартует Java.' };
        demoInstances.upsert(instanceId, { name: instanceName, version, lastPlayed: Date.now() });
        demoStream(instanceId);
      }
      pending.update({ kind: 'success', title: result.title, message: result.message, timeout: 6000, notify: false });
      // The chosen Java did not fit this version: the game runs on Mojang's, and the player is told.
      if (result.javaNote) toast(result.javaNote, { kind: 'info', title: 'Java заменена при запуске', timeout: 10000, notify: true });
      return result;
    } catch (e) {
      pending.update({ kind: 'error', title: 'Не удалось запустить игру', message: String(e), timeout: 10000, target: { console: instanceId } });
      gameConsole.fail(instanceId, String(e));
      throw e;
    } finally {
      if (unlisten) unlisten();
    }
  },

  /**
   * Catalog «Установить» and «Скачать и установить сборку» → Rust: install_mods. Mods come from
   * Modrinth (with required dependencies) into .minecraft/instances/<instanceId>/mods.
   * Progress arrives as `mods-progress` events.
   * @returns {Promise<{ installed: {name,projectId,file,dependency}[], skipped: {name,reason}[], modsDir: string }>}
   */
  async installMods({ instanceId, instanceName, version, mods, onProgress }) {
    if (!isTauri) {
      for (let i = 0; i < mods.length; i++) {
        if (onProgress) onProgress({ label: `Скачивание ${mods[i].name} (${i + 1}/${mods.length})…`, percent: Math.round((100 * i) / mods.length) });
        await delay(220);
      }
      const installed = mods.map((m) => ({ name: m.name, projectId: m.projectId || m.name, file: m.name.toLowerCase().replace(/[^a-z0-9]+/g, '-') + '.jar', dependency: false }));
      const files = demoMods[instanceId] || (demoMods[instanceId] = []);
      installed.forEach((m) => {
        if (!files.some((f) => f.file === m.file)) files.push({ file: m.file, size: 400000 + m.file.length * 91000, enabled: true });
      });
      return { installed, skipped: [], modsDir: '' };
    }
    let unlisten = null;
    try {
      if (onProgress) unlisten = await listen('mods-progress', (event) => onProgress(event.payload));
      return await invoke('install_mods', { instanceId, instanceName, version, mods });
    } finally {
      if (unlisten) unlisten();
    }
  },

  // Files in the installation's mods folder: [{ file, size, enabled }].
  async listMods(instanceId) {
    if (isTauri) return invoke('list_mods', { instanceId });
    return (demoMods[instanceId] || []).map((f) => Object.assign({}, f));
  },

  async removeMod({ instanceId, file }) {
    if (isTauri) return invoke('remove_mod', { instanceId, file });
    demoMods[instanceId] = (demoMods[instanceId] || []).filter((f) => f.file !== file);
  },

  // Mod manager switch: renames .jar ↔ .jar.disabled and returns the new file name.
  async setModEnabled({ instanceId, file, enabled }) {
    if (isTauri) return invoke('set_mod_enabled', { instanceId, file, enabled });
    const f = (demoMods[instanceId] || []).find((x) => x.file === file);
    if (!f) throw new Error('Файл не найден: ' + file);
    f.enabled = enabled;
    f.file = enabled ? f.file.replace(/\.disabled$/, '') : f.file + '.disabled';
    return f.file;
  },

  /**
   * Game versions from Mojang's manifest (Rust caches it for offline use).
   * @returns {Promise<{ latestRelease, latestSnapshot, offline, versions: { id, kind, releaseTime, installed }[] }>}
   */
  async listVersions() {
    if (isTauri) return invoke('list_versions');
    const res = await fetch('https://piston-meta.mojang.com/mc/game/version_manifest_v2.json');
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const m = await res.json();
    return {
      latestRelease: m.latest.release,
      latestSnapshot: m.latest.snapshot,
      offline: false,
      versions: m.versions.map((v) => ({ id: v.id, kind: v.type, releaseTime: v.releaseTime, installed: false }))
    };
  },

  // «Перенести в установку…»: copies a shared world into .minecraft/instances/<instanceId>/saves.
  async transferWorld({ path, instanceId, instanceName, version }) {
    if (!isTauri) {
      await delay(700);
      return '.minecraft\\instances\\' + instanceId + '\\saves\\' + path.split(/[\\/]/).pop();
    }
    return invoke('transfer_world', { path, instanceId, instanceName, version });
  },

  // Developer console: open / toggle for an installation (see console.js).
  console: {
    open: (instanceId) => gameConsole.open(instanceId),
    toggle: (instanceId) => gameConsole.toggle(instanceId)
  },

  // Interface language: 'ru' | 'en'; switching reloads the window.
  lang,
  setLang,

  // Notification centre (the bell): the history store from notify.js.
  notifications,

  // «Сменить папку…»: { path, defaultPath, isDefault }
  async gameDirInfo() {
    if (isTauri) return invoke('game_dir_info');
    return { path: 'C:\\Users\\demo\\AppData\\Roaming\\.minecraft', defaultPath: 'C:\\Users\\demo\\AppData\\Roaming\\.minecraft', isDefault: true };
  },

  // System folder picker; returns a path or null.
  async pickFolder(title) {
    if (isTauri) {
      const path = await openDialog({ directory: true, multiple: false, title });
      return typeof path === 'string' ? path : null;
    }
    return 'D:\\Games\\Minecraft';
  },

  // { exists, empty, minecraft, same }
  async checkGameDir(path) {
    if (isTauri) return invoke('check_game_dir', { path });
    return { exists: false, empty: true, minecraft: false, same: false };
  },

  // mode: 'use' | 'copy' | 'move'; progress: { label, percent }
  async changeGameDir({ path, mode, onProgress }) {
    if (!isTauri) {
      if (mode !== 'use') {
        for (let i = 1; i <= 5; i++) { if (onProgress) onProgress({ label: 'Копирование файлов (' + i * 2000 + '/10000)…', percent: i * 20 }); await delay(250); }
      }
      return { path, defaultPath: 'C:\\Users\\demo\\AppData\\Roaming\\.minecraft', isDefault: false };
    }
    let unlisten = null;
    try {
      if (onProgress) unlisten = await listen('gamedir-progress', (event) => onProgress(event.payload));
      return await invoke('change_game_dir', { path, mode });
    } finally {
      if (unlisten) unlisten();
    }
  },

  // { path, total, free, parts: [{ key, bytes }] }
  async diskUsage() {
    if (isTauri) return invoke('disk_usage');
    await delay(400);
    const mb = 1048576;
    return { path: 'C:\\Users\\demo\\AppData\\Roaming\\.minecraft', total: 1473 * mb, free: 31 * 1024 * mb, parts: [
      { key: 'versions', bytes: 98 * mb }, { key: 'libraries', bytes: 189 * mb }, { key: 'assets', bytes: 803 * mb }, { key: 'runtime', bytes: 237 * mb },
      { key: 'instances', bytes: 61 * mb }, { key: 'backups', bytes: 12 * mb }, { key: 'cache', bytes: 1 * mb }, { key: 'other', bytes: 72 * mb }] };
  },

  // A message that should survive a window reload (shown on the next start).
  toastAfterReload(message, opts) {
    try { sessionStorage.setItem('signal.afterReload', JSON.stringify({ message, opts })); } catch (e) {}
  },

  // { version, updatesConfigured }
  async launcherInfo() {
    if (isTauri) return invoke('launcher_info');
    return { version: '0.1.0', updatesConfigured: true };
  },

  /**
   * Self-update (Rust: updater.rs) → { configured, available, current, version, notes, date }.
   * The browser preview pretends a newer version exists so the dialog can be tried.
   */
  async checkLauncherUpdate() {
    if (isTauri) return invoke('check_launcher_update');
    await delay(700);
    return { configured: true, available: true, current: '0.1.0', version: '0.2.0', notes: 'Демо-режим: так будут выглядеть заметки к выпуску из GitHub Releases.\n— Новая вкладка\n— Исправления', date: '2026-10-08' };
  },

  // Download (onProgress({ downloaded, total })), signature check, install; the app restarts.
  async installLauncherUpdate({ onProgress }) {
    if (!isTauri) {
      const total = 9_800_000;
      for (let d = 0; d <= total; d += 1_400_000) { if (onProgress) onProgress({ downloaded: Math.min(d, total), total }); await delay(250); }
      toast('В приложении здесь установится новая версия и лаунчер перезапустится.', { kind: 'info', title: 'Демо-режим' });
      return;
    }
    let unlisten = null;
    try {
      if (onProgress) unlisten = await listen('update-progress', (event) => onProgress(event.payload));
      await invoke('install_launcher_update');
    } finally {
      if (unlisten) unlisten();
    }
  },

  // { microsoft, discord }: whether this build has the Azure / Discord application ids.
  async integrations() {
    return isTauri ? invoke('integrations') : { microsoft: true, discord: false };
  },

  /**
   * «Войти через Microsoft», step 1 → { userCode, verificationUri, expiresIn }.
   * Step 2 (msLoginFinish) resolves when the code is confirmed: { id, name, skin, slim, face }.
   */
  async msLoginStart() {
    if (isTauri) return invoke('ms_login_start');
    demoMs.cancelled = false;
    await delay(500);
    return { userCode: 'DEMO-CODE', verificationUri: 'https://www.microsoft.com/link', expiresIn: 900 };
  },

  async msLoginFinish() {
    if (isTauri) return invoke('ms_login_finish');
    await delay(4000);
    if (demoMs.cancelled) throw new Error('Вход отменён');
    const face = Array.from({ length: 64 }, (_, i) => (i < 16 || i % 8 === 0 || i % 8 === 7 ? '#2b1d14' : (i === 33 || i === 38 ? '#3a5bc7' : '#c8946a')));
    const account = { id: 'demo-uuid', name: 'Demo_Player', skin: null, slim: false, face };
    demoMs.accounts = [account].concat(demoMs.accounts.filter((a) => a.id !== account.id));
    return account;
  },

  async msLoginCancel() {
    if (isTauri) return invoke('ms_login_cancel');
    demoMs.cancelled = true;
  },

  // Microsoft accounts saved by the backend (no tokens): [{ id, name, skin, slim, face }]
  async listAccounts() {
    return isTauri ? invoke('list_accounts') : demoMs.accounts.slice();
  },

  async removeAccount(id) {
    if (isTauri) return invoke('remove_account', { id });
    demoMs.accounts = demoMs.accounts.filter((a) => a.id !== id);
  },

  // microsoft.com/link in the default browser (the backend allows only Microsoft's sign-in pages).
  async openLoginPage(url) {
    if (isTauri) return invoke('open_login_page', { url });
    // Browser preview: no real sign-in page (the demo account needs none).
    toast(url, { kind: 'info', title: 'Здесь откроется страница входа', timeout: 3000 });
  },

  /** «Путь к Java»: [{ path, major, source: 'mojang' | 'system' }], newest first. */
  async listJava() {
    if (isTauri) return invoke('list_java');
    await delay(300);
    return [
      { path: 'C:\\Users\\demo\\AppData\\Roaming\\.minecraft\\runtime\\java-runtime-delta\\windows-x64\\java-runtime-delta\\bin\\javaw.exe', major: 21, source: 'mojang' },
      { path: 'C:\\Program Files\\Eclipse Adoptium\\jdk-17.0.12.7-hotspot\\bin\\javaw.exe', major: 17, source: 'system' },
      { path: 'C:\\Program Files\\Java\\jre1.8.0_491\\bin\\javaw.exe', major: 8, source: 'system' }
    ];
  },

  /** «Указать путь вручную…»: java.exe / javaw.exe → { path, major, source } or an error. */
  async checkJava(path) {
    if (isTauri) return invoke('check_java', { path });
    return { path, major: 21, source: 'system' };
  },

  async pickJavaFile() {
    if (isTauri) {
      const path = await openDialog({ multiple: false, directory: false, title: t('Выберите java.exe или javaw.exe'), filters: [{ name: 'Java', extensions: ['exe'] }] });
      return typeof path === 'string' ? path : null;
    }
    return 'C:\\Program Files\\Zulu\\zulu-21\\bin\\javaw.exe';
  },

  /** «Параметры запуска» of an installation → instance.json (`launch`). */
  async setInstanceLaunch({ id, name, version, custom, ramGb, java, jvmArgs }) {
    if (isTauri) return invoke('set_instance_launch', { id, name, version, custom, ramGb, java: java || '', jvmArgs: jvmArgs || '' });
    demoInstances.upsert(id, { name, version, launch: { custom, ramGb, java, jvmArgs } });
  },

  // «Импорт сборки»: the system file picker for .mrpack. Returns a path or null.
  async pickModpack() {
    if (isTauri) {
      const path = await openDialog({ multiple: false, directory: false, title: 'Сборка Modrinth', filters: [{ name: 'Сборка Modrinth (.mrpack)', extensions: ['mrpack'] }] });
      return typeof path === 'string' ? path : null;
    }
    return 'C:\\Users\\demo\\Downloads\\Fabulously Optimized-15.0.0.mrpack';
  },

  /**
   * .mrpack → new installation (Rust: mrpack.rs). Progress: `import-progress`.
   * @returns {Promise<{ instanceId, name, version, loaderVersion, files, serverOnly, overrides, icon }>}
   */
  async importModpack({ path, onProgress }) {
    if (!isTauri) {
      for (const [label, percent] of [['Чтение сборки…', 1], ['Скачивание файлов сборки (12/48)…', 25], ['Скачивание файлов сборки (48/48)…', 90], ['Копирование настроек сборки (overrides)…', 92]]) {
        if (onProgress) onProgress({ label, percent });
        await delay(400);
      }
      const id = 'm' + Date.now();
      demoInstances.upsert(id, { name: 'Fabulously Optimized', version: 'Fabric 26.3', loaderVersion: '0.19.5', pack: 'Fabulously Optimized 15.0.0', mods: 46 });
      return { instanceId: id, name: 'Fabulously Optimized', version: 'Fabric 26.3', loaderVersion: '0.19.5', files: 48, serverOnly: 0, overrides: 51, icon: null };
    }
    let unlisten = null;
    try {
      if (onProgress) unlisten = await listen('import-progress', (event) => onProgress(event.payload));
      return await invoke('import_modpack', { path });
    } finally {
      if (unlisten) unlisten();
    }
  },

  /**
   * Live Modrinth catalog (Rust: modrinth.rs → /v2/search).
   * @param {{ query, projectType: 'mod'|'modpack'|'resourcepack'|'shader', loader, gameVersion, sort, offset }} query
   * @returns {Promise<{ hits: { projectId, slug, projectType, title, description, author, downloads, follows, iconUrl, categories, dateModified }[], total, offset }>}
   */
  async searchModrinth(query) {
    if (isTauri) return invoke('search_modrinth', { query });
    // Browser preview: the same request straight to Modrinth (it allows CORS).
    const kind = query.projectType;
    const facets = [['project_type:' + kind]];
    if ((kind === 'mod' || kind === 'modpack') && query.loader) facets.push(['categories:' + query.loader]);
    if (query.gameVersion && kind !== 'modpack') facets.push(['versions:' + query.gameVersion]);
    const params = new URLSearchParams({ query: (query.query || '').trim(), facets: JSON.stringify(facets), index: query.sort || 'relevance', offset: String(query.offset || 0), limit: '20' });
    const res = await fetch('https://api.modrinth.com/v2/search?' + params);
    if (!res.ok) throw new Error('Modrinth: HTTP ' + res.status);
    const data = await res.json();
    return {
      total: data.total_hits || 0,
      offset: query.offset || 0,
      hits: (data.hits || []).map((h) => ({
        projectId: h.project_id, slug: h.slug, projectType: h.project_type, title: h.title, description: h.description, author: h.author,
        downloads: h.downloads || 0, follows: h.follows || 0,
        iconUrl: h.icon_url && h.icon_url.startsWith('https://cdn.modrinth.com/') ? h.icon_url : null,
        categories: h.display_categories || [], dateModified: h.date_modified
      }))
    };
  },

  /** Resource pack / shader from the catalog → resourcepacks/ | shaderpacks/ of the installation. */
  async installAsset({ instanceId, version, projectId, kind, title }) {
    if (isTauri) return invoke('install_asset', { instanceId, version, projectId, kind });
    await delay(600);
    const file = String(title || projectId).replace(/[^A-Za-z0-9]+/g, '-') + '.zip';
    const list = demoPackList(instanceId, kind);
    if (!list.some((p) => p.name === file)) list.push({ name: file, size: 900000 + file.length * 70000, isDir: false, enabled: false });
    return { file, version: '1.0.0' };
  },

  /** A modpack from the catalog becomes a new installation. Progress: `import-progress`. */
  async installModpackProject({ projectId, title, onProgress }) {
    if (!isTauri) {
      if (onProgress) onProgress({ label: 'Скачивание сборки…', percent: 0 });
      await delay(500);
      return this.importModpack({ path: title + '.mrpack', onProgress });
    }
    let unlisten = null;
    try {
      if (onProgress) unlisten = await listen('import-progress', (event) => onProgress(event.payload));
      return await invoke('install_modpack_project', { projectId });
    } finally {
      if (unlisten) unlisten();
    }
  },

  /** @returns {Promise<{ entries: { name, size, isDir, enabled }[], folder, iris }>} */
  async listPacks(instanceId, kind) {
    if (isTauri) return invoke('list_packs', { instanceId, kind });
    const hasIris = (demoMods[instanceId] || []).some((f) => /^iris/i.test(f.file));
    return { entries: demoPackList(instanceId, kind).map((p) => Object.assign({}, p)), folder: '%MINECRAFT%\\instances\\' + instanceId + '\\' + (kind === 'shader' ? 'shaderpacks' : 'resourcepacks'), iris: hasIris };
  },

  async setPackEnabled({ instanceId, kind, name, enabled }) {
    if (isTauri) return invoke('set_pack_enabled', { instanceId, kind, name, enabled });
    const list = demoPackList(instanceId, kind);
    if (kind === 'shader' && enabled) list.forEach((p) => { p.enabled = false; });
    const p = list.find((x) => x.name === name);
    if (!p) throw new Error('Пак ' + name + ' не найден');
    p.enabled = enabled;
  },

  async removePack({ instanceId, kind, name }) {
    if (isTauri) return invoke('remove_pack', { instanceId, kind, name });
    const all = demoPacks[instanceId];
    if (all) all[kind] = demoPackList(instanceId, kind).filter((p) => p.name !== name);
  },

  /**
   * «Экспорт в .mrpack», step 1.
   * @returns {Promise<{ name, version, loaderVersion, entries: { path, group, size, isDir, selected, modrinth }[] }>}
   */
  async exportPreview(instanceId) {
    if (isTauri) return invoke('export_preview', { instanceId });
    await delay(500);
    const inst = demoInstances.read().find((i) => i.id === instanceId);
    const mods = (demoMods[instanceId] || []).map((f, i) => ({ path: 'mods/' + f.file, group: 'mods', size: f.size, isDir: false, selected: f.enabled, modrinth: i % 4 !== 3 }));
    const packs = ['resourcepack', 'shader'].flatMap((kind) => demoPackList(instanceId, kind).map((p) => ({ path: (kind === 'shader' ? 'shaderpacks/' : 'resourcepacks/') + p.name, group: kind === 'shader' ? 'shaderpacks' : 'resourcepacks', size: p.size, isDir: false, selected: true, modrinth: true })));
    return {
      name: inst ? inst.name : 'Моя сборка',
      version: inst ? inst.version : 'Fabric 1.21.1',
      loaderVersion: inst ? inst.loaderVersion || '0.16.9' : '0.16.9',
      entries: mods.concat(packs, [
        { path: 'config/sodium-options.json', group: 'config', size: 1200, isDir: false, selected: true, modrinth: false },
        { path: 'config/iris.properties', group: 'config', size: 300, isDir: false, selected: true, modrinth: false },
        { path: 'options.txt', group: 'other', size: 5400, isDir: false, selected: true, modrinth: false },
        { path: 'servers.dat', group: 'other', size: 420, isDir: false, selected: false, modrinth: false },
        { path: 'saves/World', group: 'saves', size: 8400000, isDir: true, selected: false, modrinth: false }
      ])
    };
  },

  /** The system «Save as» dialog for the .mrpack. Returns a path or null. */
  async saveMrpackDialog(defaultName) {
    if (isTauri) {
      const path = await saveDialog({ title: t('Экспорт сборки'), defaultPath: defaultName, filters: [{ name: t('Сборка Modrinth (.mrpack)'), extensions: ['mrpack'] }] });
      return typeof path === 'string' ? path : null;
    }
    return 'C:\\Users\\demo\\Downloads\\' + defaultName;
  },

  /**
   * Step 2: writes the .mrpack. Progress: `export-progress`.
   * @returns {Promise<{ path, downloads, overrides, size }>}
   */
  async exportMrpack({ instanceId, dest, meta, include, onProgress }) {
    if (!isTauri) {
      for (const [label, percent] of [['Сверка файлов с Modrinth…', 10], ['Упаковка overrides…', 60], ['Запись modrinth.index.json…', 95]]) {
        if (onProgress) onProgress({ label, percent });
        await delay(350);
      }
      const remote = include.filter((p) => /^(mods|resourcepacks|shaderpacks)\//.test(p)).length;
      return { path: dest, downloads: remote, overrides: include.length - remote, size: 48000 };
    }
    let unlisten = null;
    try {
      if (onProgress) unlisten = await listen('export-progress', (event) => onProgress(event.payload));
      return await invoke('export_mrpack', { instanceId, dest, meta, include });
    } finally {
      if (unlisten) unlisten();
    }
  },

  /**
   * CurseForge catalog (Rust: curseforge.rs → /v1/mods/search), same query as searchModrinth.
   * Hits carry `source: 'cf'`, `allowDistribution` and `pageUrl`.
   */
  async searchCurseforge(query) {
    if (isTauri) return invoke('search_curseforge', { query });
    // Browser preview: the API needs a key and has no CORS, so a few sample projects stand in.
    await delay(400);
    const sample = demoCf.samples[query.projectType] || [];
    const q = (query.query || '').trim().toLowerCase();
    const hits = sample.filter((h) => !q || h.title.toLowerCase().includes(q)).map((h) => Object.assign({ source: 'cf', projectType: query.projectType, follows: 0, dateModified: '', iconUrl: null }, h));
    return { hits: hits.slice(query.offset || 0, (query.offset || 0) + 20), total: hits.length, offset: query.offset || 0 };
  },

  /** Mod (with dependencies), resource pack or shader from CurseForge → same report as installMods. */
  async installCurseforge({ instanceId, instanceName, version, projectId, kind, title }) {
    if (isTauri) return invoke('install_curseforge', { instanceId, instanceName, version, projectId, kind });
    await delay(600);
    const h = Object.values(demoCf.samples).flat().find((x) => x.projectId === projectId);
    if (h && !h.allowDistribution) throw new Error('CF_BLOCKED: автор разрешил скачивать «' + title + '» только с сайта CurseForge');
    const file = String(title).replace(/[^A-Za-z0-9]+/g, '-') + (kind === 'mod' ? '.jar' : '.zip');
    if (kind === 'mod') {
      const files = demoMods[instanceId] || (demoMods[instanceId] = []);
      if (!files.some((f) => f.file === file)) files.push({ file, size: 700000, enabled: true });
    } else {
      const list = demoPackList(instanceId, kind);
      if (!list.some((p) => p.name === file)) list.push({ name: file, size: 1200000, isDir: false, enabled: false });
    }
    return { installed: [{ name: file, projectId, file, dependency: false }], skipped: [], modsDir: '' };
  },

  /** CurseForge modpack → new installation. Progress: `import-progress`. */
  async installCurseforgeModpack({ projectId, title, onProgress }) {
    if (!isTauri) {
      const r = await this.importModpack({ path: title + '.zip', onProgress });
      return Object.assign(r, { name: title, blocked: [{ name: 'mods/OptiFine_1.20.1_HD_U_I6.jar', url: 'https://www.curseforge.com/minecraft/mc-mods/optifine/files/1' }] });
    }
    let unlisten = null;
    try {
      if (onProgress) unlisten = await listen('import-progress', (event) => onProgress(event.payload));
      return await invoke('install_curseforge_modpack', { projectId });
    } finally {
      if (unlisten) unlisten();
    }
  },

  /**
   * Offline account: the skin of this nickname (Mojang, else Ely.by).
   * @returns {Promise<{ source: 'mojang'|'elyby', skin: string, slim: boolean, face: string[] } | null>}
   */
  async resolveOfflineSkin(nickname) {
    if (isTauri) return invoke('resolve_offline_skin', { nickname });
    // Browser preview: Mojang's API sends no CORS headers, so no lookup here.
    await delay(400);
    return null;
  },

  /** Which model the game gives an offline nickname in this version: 'slim' | 'wide'. */
  async offlineSkinModel(name, version) {
    if (isTauri) return invoke('offline_skin_model', { name, version });
    return name.length % 2 ? 'slim' : 'wide';
  },

  /**
   * «Бэкапы миров» of an installation.
   * @returns {Promise<{ worlds: { folder, name, version, lastPlayed, size, icon }[], backups: { file, path, world, folder, created, size, auto }[], savesDir, backupsDir }>}
   */
  async instanceBackups(instanceId) {
    if (isTauri) return invoke('instance_backups', { instanceId });
    await delay(250);
    const d = demoBackupsOf(instanceId);
    return { worlds: d.worlds.map((w) => Object.assign({}, w)), backups: d.backups.map((b) => Object.assign({}, b)), savesDir: '%MINECRAFT%\\instances\\' + instanceId + '\\saves', backupsDir: '%MINECRAFT%\\backups\\instances\\' + instanceId };
  },

  async backupInstanceWorld({ instanceId, folder }) {
    if (isTauri) return invoke('backup_instance_world', { instanceId, folder });
    await delay(700);
    const d = demoBackupsOf(instanceId);
    const created = Date.now();
    const file = folder.replace(/[^\p{L}\p{N}_-]/gu, '_') + '_' + new Date(created).toISOString().slice(0, 19).replace('T', '_').replace(/:/g, '-') + '.zip';
    const entry = { file, path: 'C:\\demo\\backups\\' + file, world: folder, folder, created, size: 6400000, auto: false };
    d.backups.unshift(entry);
    return entry;
  },

  async restoreWorldBackup({ instanceId, path }) {
    if (isTauri) return invoke('restore_world_backup', { instanceId, path });
    await delay(900);
    const d = demoBackupsOf(instanceId);
    const b = d.backups.find((x) => x.path === path);
    if (!b) throw new Error('Это не бэкап этой установки');
    return { folder: b.world, safety: 'C:\\demo\\backups\\' + b.world + '_safety.zip' };
  },

  async deleteWorldBackup({ instanceId, path }) {
    if (isTauri) return invoke('delete_world_backup', { instanceId, path });
    const d = demoBackupsOf(instanceId);
    d.backups = d.backups.filter((x) => x.path !== path);
  },

  /**
   * «Проверить целостность файлов»: sha1 of client.jar, libraries and assets; damaged files are
   * downloaded again. Progress: `repair-progress`.
   * @returns {Promise<{ versionId, checked, corrupt, missing, repaired, failed: string[], localMissing }>}
   */
  async repairInstance({ version, onProgress }) {
    if (!isTauri) {
      for (const [label, percent] of [['Сверка профилей версии с манифестом Mojang…', 1], ['Проверка файлов (1980/3961)…', 36], ['Проверка файлов (3961/3961)…', 70], ['Перекачивание файлов (1/1)…', 99]]) {
        if (onProgress) onProgress({ label, percent });
        await delay(400);
      }
      return { versionId: version, checked: 3961, corrupt: 1, missing: 0, repaired: 1, failed: [], localMissing: 0 };
    }
    let unlisten = null;
    try {
      if (onProgress) unlisten = await listen('repair-progress', (event) => onProgress(event.payload));
      return await invoke('repair_instance', { version });
    } finally {
      if (unlisten) unlisten();
    }
  },

  /**
   * Server List Ping → { online, playersOnline, playersMax, latencyMs, motd, version, favicon, resolved, error }.
   * Never rejects for an offline server: `online: false` with `error`.
   */
  async pingServer(address) {
    if (isTauri) return invoke('ping_server', { address });
    await delay(300 + (address.length % 5) * 120);
    return { online: true, playersOnline: 12, playersMax: 100, latencyMs: 40 + (address.length % 7) * 15, motd: 'Демо-режим: настоящий пинг работает в приложении', version: '1.21.x', favicon: null, resolved: address, error: null };
  },

  // Servers saved in game (servers.dat): [{ name, address, icon, source }]
  async savedServers() {
    return isTauri ? invoke('saved_servers') : [];
  },

  // «Новости» → { items: [{ id, kind, title, text, date, image, link, contentPath, version }], updatedAt, offline }
  async newsFeed(force) {
    if (isTauri) return invoke('news_feed', { force: !!force });
    // Browser preview: the same feed straight from Mojang (it allows CORS), images by URL.
    const base = 'https://launchercontent.mojang.com';
    const [news, notes] = await Promise.all([fetch(base + '/v2/news.json').then((r) => r.json()), fetch(base + '/v2/javaPatchNotes.json').then((r) => r.json())]);
    const items = news.entries
      .filter((e) => (e.newsType || []).includes('Java'))
      .map((e) => ({ id: e.id, kind: 'news', title: e.title, text: e.text, date: String(e.date).slice(0, 10), image: base + ((e.newsPageImage || e.playPageImage || {}).url || ''), link: e.readMoreLink, contentPath: null, version: null }))
      .concat(notes.entries.map((e) => ({ id: e.id, kind: e.type === 'release' ? 'release' : 'snapshot', title: e.title, text: e.shortText, date: String(e.date).slice(0, 10), image: base + ((e.image || {}).url || ''), link: null, contentPath: e.contentPath, version: e.version })))
      .sort((a, b) => (a.date < b.date ? 1 : -1))
      .slice(0, 18);
    return { items, updatedAt: Date.now(), offline: false };
  },

  // Patch notes body (HTML from Mojang; shown through the reader's tag allowlist).
  async newsArticle(contentPath) {
    if (isTauri) return invoke('news_article', { contentPath });
    const r = await fetch('https://launchercontent.mojang.com/v2/' + contentPath).then((x) => x.json());
    return r.body || '';
  },

  // Patch notes in the in-app reader: { title, meta, contentPath }.
  readPatchNotes({ title, meta, contentPath }) {
    newsReader.open({ title, meta, load: () => window.SignalBridge.newsArticle(contentPath) });
  },

  // minecraft.net / Modrinth links in the default browser (the backend checks the host).
  async openLink(url) {
    if (isTauri) return invoke('open_link', { url });
    toast(url, { kind: 'info', title: 'Откроется в браузере', timeout: 3000 });
  },

  // A .mrpack dropped on the window (Tauri drag & drop): handler(path).
  onModpackDrop(handler) {
    modpackDropHandlers.push(handler);
  },

  // A .png dropped on the window: handler(File) — the skin editor's own loader takes it from there.
  onSkinDrop(handler) {
    skinDropHandlers.push(handler);
  },

  // Mod manager: { updates: [{ file, projectId, title, current, latest, versionId }], known, unknown }
  async checkModUpdates(instanceId) {
    if (isTauri) return invoke('check_mod_updates', { instanceId });
    await delay(800);
    const files = demoMods[instanceId] || [];
    const updates = files.slice(0, 1).map((f) => ({ file: f.file, projectId: 'demo', title: '', current: '1.0.0', latest: '1.2.0', versionId: 'demo-version' }));
    return { updates, known: files.length, unknown: 0 };
  },

  // Replaces a mod with the given Modrinth version; returns the new file name.
  async updateMod({ instanceId, file, versionId }) {
    if (isTauri) return invoke('update_mod', { instanceId, file, versionId });
    await delay(700);
    const f = (demoMods[instanceId] || []).find((x) => x.file === file);
    if (f) f.file = f.file.replace(/\.jar/, '-1.2.0.jar');
    return f ? f.file : file;
  },

  async renameInstance({ id, name }) {
    if (isTauri) return invoke('rename_instance', { id, name });
    demoInstances.upsert(id, { name });
  },

  // Returns the new installation id.
  async duplicateInstance({ id, name }) {
    if (isTauri) return invoke('duplicate_instance', { id, name });
    await delay(500);
    const src = demoInstances.read().find((x) => x.id === id) || { version: 'Release 26.3' };
    const newId = 'c' + Date.now();
    demoInstances.upsert(newId, Object.assign({}, src, { id: newId, name, lastPlayed: null, playTime: 0, created: Date.now() }));
    return newId;
  },

  async deleteInstance(id) {
    if (isTauri) return invoke('delete_instance', { id });
    demoInstances.write(demoInstances.read().filter((x) => x.id !== id));
  },

  /**
   * Installations from .minecraft/instances/<id>/instance.json, most recently played first:
   * [{ id, name, version, created, lastPlayed, playTime, lastBackup, lastExitCode, mods, worlds }]
   */
  async listInstances() {
    return isTauri ? invoke('list_instances') : demoInstances.read().sort((a, b) => (b.lastPlayed || 0) - (a.lastPlayed || 0));
  },

  // «Новая установка»: the folder and instance.json are created right away.
  async createInstance({ id, name, version }) {
    if (isTauri) return invoke('create_instance', { id, name, version });
    demoInstances.upsert(id, { name, version });
  },

  // { totalMb, availableMb } for the memory slider.
  async systemMemory() {
    if (isTauri) return invoke('system_memory');
    const gb = Math.max(8, navigator.deviceMemory || 16);
    return { totalMb: gb * 1024, availableMb: Math.round(gb * 1024 * 0.55) };
  },

  // «Очистить кэш» → { files, bytes }
  async clearCache() {
    if (isTauri) return invoke('clear_cache');
    await delay(500);
    return { files: 3, bytes: 18400000 };
  },

  /**
   * Subscribes to the end of a game session: { instanceId, instanceName, code, crashed, seconds, log, crashReport }.
   * The bridge itself shows the «игра закрыта / аварийно закрылась» toast.
   */
  onGameExit(handler) {
    gameExitHandlers.push(handler);
  },

  // Real worlds and screenshots from .minecraft and every installation; null in the browser preview.
  async listWorlds() {
    return isTauri ? invoke('list_worlds') : null;
  },

  async listScreenshots() {
    return isTauri ? invoke('list_screenshots') : null;
  },

  async backupWorld(path) {
    if (!isTauri) {
      await delay(600);
      return '.minecraft\\backups\\world.zip';
    }
    return invoke('backup_world', { path });
  },

  // Local file → URL the WebView can show (Tauri asset protocol, limited to .minecraft).
  fileSrc(path) {
    return isTauri ? convertFileSrc(path) : path;
  },

  // «Собрать с ИИ» → Rust: generate_ai_modpack(prompt). Returns the model's raw JSON text.
  async generateModpack(prompt) {
    if (!isTauri) throw new Error('генерация доступна только в приложении');
    return invoke('generate_ai_modpack', { prompt });
  },

  // «Папка игры», «Открыть .minecraft», «Логи», «Скриншоты» → Rust: open_folder(path)
  async openFolder(path) {
    if (!isTauri) {
      toast(path, { kind: 'info', title: 'Открыть папку', timeout: 3000 });
      return;
    }
    try {
      await invoke('open_folder', { path });
    } catch (e) {
      toast(String(e), { kind: 'error', title: 'Папка не открылась' });
    }
  },

  minimize() { if (appWindow) appWindow.minimize(); },
  toggleMaximize() { if (appWindow) appWindow.toggleMaximize(); },
  close() { if (appWindow) appWindow.close(); }
};

// Game exit (Rust: game::watch_game). A crash gets a toast with the log and crash report.
function announceGameExit(e) {
  const where = '«' + e.instanceName + '»';
  gameConsole.end(e);
  if (e.crashed) {
    const actions = [{ label: 'Показать причину', onClick: () => gameConsole.revealReason(e.instanceId) }];
    actions.push({ label: 'Открыть лог', onClick: () => window.SignalBridge.openFolder(e.log) });
    if (e.crashReport) actions.push({ label: 'Отчёт о сбое', onClick: () => window.SignalBridge.openFolder(e.crashReport) });
    toast((e.reason ? e.reason.title + '. ' : '') + where + ' · сессия ' + fmtMinutes(e.seconds) + '.', {
      kind: 'error',
      title: 'Игра аварийно закрылась (код ' + (e.code == null ? '?' : e.code) + ')',
      timeout: 0,
      actions,
      target: { console: e.instanceId }
    });
  } else {
    toast(where + ' · сыграно ' + fmtMinutes(e.seconds) + '.', { kind: 'info', title: 'Игра закрыта', timeout: 5000 });
  }
  gameExitHandlers.forEach((h) => { try { h(e); } catch (err) { console.error(err); } });
}
if (isTauri) {
  listen('game-exit', (event) => announceGameExit(event.payload));
  listen('game-log', (event) => gameConsole.push(event.payload));
}
// Drag & drop onto the window, by extension: .mrpack → import a modpack, .png → skin editor,
// anything else → a hint. Tauri delivers file paths (HTML5 drop events do not reach the page
// while its own drag & drop is on); the browser preview gets File objects instead.
const dropVeil = document.createElement('div');
dropVeil.className = 'drop-veil';
dropVeil.hidden = true;
dropVeil.innerHTML = '<div class="drop-card"><svg width="34" height="34" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3v12M7 10l5 5 5-5"></path><path d="M5 19h14"></path></svg><b></b><span></span></div>';
document.body.appendChild(dropVeil);
const ext = (name) => (String(name || '').match(/\.([a-z0-9]+)$/i) || [])[1]?.toLowerCase() || '';
const UNSUPPORTED = 'Поддерживаются только сборки .mrpack и скины .png';
function showVeil(kind) {
  const texts = {
    mrpack: ['Отпустите, чтобы импортировать сборку', 'Файл Modrinth .mrpack станет новой установкой'],
    png: ['Отпустите, чтобы сменить скин', 'PNG 64×64 или 64×32 откроется в «Управлении скином»'],
    other: ['Этот файл не подойдёт', UNSUPPORTED],
    any: ['Отпустите файл', 'Сборка .mrpack или скин .png']
  }[kind];
  dropVeil.querySelector('b').textContent = t(texts[0]);
  dropVeil.querySelector('span').textContent = t(texts[1]);
  dropVeil.classList.toggle('is-bad', kind === 'other');
  dropVeil.hidden = false;
}
const kindOf = (names) => (names.some((n) => ext(n) === 'mrpack') ? 'mrpack' : names.some((n) => ext(n) === 'png') ? 'png' : 'other');
const dropSkin = (file) => skinDropHandlers.forEach((h) => h(file));
const notSupported = () => toast(UNSUPPORTED + '.', { kind: 'info', title: 'Этот файл не подойдёт' });
if (isTauri) {
  getCurrentWebview().onDragDropEvent(async (event) => {
    const { type, paths } = event.payload;
    if (type === 'enter') showVeil(kindOf(paths || []));
    else if (type === 'leave') dropVeil.hidden = true;
    else if (type === 'drop') {
      dropVeil.hidden = true;
      const list = paths || [];
      const pack = list.find((p) => ext(p) === 'mrpack');
      const png = list.find((p) => ext(p) === 'png');
      if (pack) modpackDropHandlers.forEach((h) => h(pack));
      else if (png) {
        try {
          const bytes = await invoke('read_skin_png', { path: png });
          dropSkin(new File([bytes], png.split(/[\\/]/).pop(), { type: 'image/png' }));
        } catch (e) {
          toast(String(e), { kind: 'error', title: 'Скин не прочитался' });
        }
      } else if (list.length) notSupported();
    }
  });
} else {
  let depth = 0;
  window.addEventListener('dragenter', (e) => {
    if (!e.dataTransfer || ![...e.dataTransfer.types].includes('Files')) return;
    depth += 1;
    // Names are hidden until the drop; the MIME type tells PNGs apart.
    const types = [...e.dataTransfer.items].map((i) => i.type);
    showVeil(types.includes('image/png') ? 'png' : 'any');
  });
  window.addEventListener('dragleave', () => { depth = Math.max(0, depth - 1); if (!depth) dropVeil.hidden = true; });
  window.addEventListener('dragover', (e) => { if (!dropVeil.hidden) e.preventDefault(); });
  window.addEventListener('drop', (e) => {
    depth = 0;
    dropVeil.hidden = true;
    // The skin editor's drop zone handles its own drops.
    if (e.defaultPrevented || !e.dataTransfer) return;
    e.preventDefault();
    const files = [...(e.dataTransfer.files || [])];
    const pack = files.find((f) => ext(f.name) === 'mrpack');
    const png = files.find((f) => ext(f.name) === 'png');
    if (pack) modpackDropHandlers.forEach((h) => h(pack.name));
    else if (png) dropSkin(png);
    else if (files.length) notSupported();
  });
}

// A message left by the previous page (e.g. after the game folder changed).
try {
  const pending = JSON.parse(sessionStorage.getItem('signal.afterReload') || 'null');
  sessionStorage.removeItem('signal.afterReload');
  if (pending) setTimeout(() => toast(pending.message, pending.opts || {}), 600);
} catch (e) {}

newsReader.configure({
  openLink: (url) => window.SignalBridge.openLink(url).catch((e) => toast(String(e), { kind: 'info', title: 'Ссылка не открылась' }))
});

gameConsole.configure({
  loadSnapshot: (instanceId) => (isTauri ? invoke('console_snapshot', { instanceId }) : Promise.resolve([])),
  openFolder: (path) => window.SignalBridge.openFolder(path)
});
// Browser preview only: window.SignalBridge.simulateGameExit({ crashed: true }) shows both toasts.
// Not in the app, where a fake «game crashed» should never be possible.
if (!isTauri) window.SignalBridge.simulateGameExit = (patch = {}) => announceGameExit(Object.assign(
  {
    instanceId: 'v1', instanceName: 'Последний релиз', code: patch.crashed ? -1 : 0, crashed: false, seconds: 2520,
    log: '%MINECRAFT%\\instances\\v1\\signal-launch.log', crashReport: null,
    reason: patch.crashed ? {
      title: 'Игра упала: Exception in world tick',
      hint: 'Причина — в первой строке исключения ниже и в отчёте о сбое.',
      detail: 'java.io.FileNotFoundException: modid:textures/block/missing.png',
      line: demoErrorLine
    } : null
  },
  patch
));

mountDesign({
  template,
  defineLogic,
  container: document.getElementById('app'),
  props: { accent: '#2A8040' }
});
