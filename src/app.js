// Bearite-PC launcher window.
// Data below is placeholder state; later stages will fill it from the real game and mods.

const tauri = window.__TAURI__;
const appWindow = tauri && tauri.window ? tauri.window.getCurrentWindow() : null;

const invoke = tauri && tauri.core ? tauri.core.invoke : null;
const listen = tauri && tauri.event ? tauri.event.listen : null;

const GITHUB_URL = 'https://github.com/Sp1KLe/Bearite-PC';
const NICK_MAX = 24;

const state = {
  game: { found: false, name: 'Super Bear Adventure', pkg: 'com.Earthkwak.Platformer' },
  mods: [
    { id: 'bearite', name: 'Bearite', meta: 'v1.2.0 · Bearite · встроенный · нативных: 1 · настроек: 5', enabled: true },
  ],
  logLines: [],
};

const $ = (id) => document.getElementById(id);

// ---------- toast ----------
let toastTimer = null;
function toast(message) {
  const el = $('toast');
  el.textContent = message;
  el.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('show'), 2600);
}

// ---------- log ----------
function renderLog() {
  $('logbox').textContent = state.logLines.length ? state.logLines.join('\n') : '(пусто)';
}

function log(message) {
  const time = new Date().toLocaleTimeString('ru-RU');
  state.logLines.push('[' + time + '] ' + message);
  renderLog();
}

function copyText(text) {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    return navigator.clipboard.writeText(text);
  }
  return new Promise((resolve, reject) => {
    const area = document.createElement('textarea');
    area.value = text;
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand('copy');
    area.remove();
    ok ? resolve() : reject(new Error('copy failed'));
  });
}

function copyLog() {
  if (!state.logLines.length) {
    toast('Лог пуст, копировать нечего');
    return;
  }
  copyText(state.logLines.join('\n'))
    .then(() => toast('Лог скопирован'))
    .catch(() => toast('Не удалось скопировать лог'));
}

function clearLog() {
  state.logLines = [];
  renderLog();
  toast('Лог очищен');
}

$('log-refresh').addEventListener('click', () => { renderLog(); toast('Лог обновлён'); });
$('log-copy').addEventListener('click', copyLog);
$('log-clear').addEventListener('click', clearLog);
$('set-copy-log').addEventListener('click', copyLog);
$('set-clear-log').addEventListener('click', clearLog);

// ---------- pages ----------
function showPage(name) {
  document.querySelectorAll('.page').forEach((p) => p.classList.toggle('active', p.id === 'page-' + name));
  document.querySelectorAll('.nav-item[data-page]').forEach((b) => b.classList.toggle('active', b.dataset.page === name));
  if (name === 'log') renderLog();
}

document.querySelectorAll('.nav-item[data-page]').forEach((b) => {
  b.addEventListener('click', () => showPage(b.dataset.page));
});

// ---------- game card ----------
function renderGame() {
  const g = state.game;
  $('game-name').textContent = g.name;
  $('game-status').textContent = g.found ? g.name + ' найдена' : 'Игра будет скачана';
  $('game-meta').textContent = g.found ? 'Готова · ' + g.pkg : 'Скачается при первом запуске';
}

async function refreshEnv() {
  if (!invoke) return;
  try {
    const info = await invoke('env_info');
    state.game.found = !!info.game_ready;
    renderGame();
  } catch (e) { /* keep the previous state */ }
}

// ---------- mods ----------
function renderModsCount() {
  const on = state.mods.filter((m) => m.enabled).length;
  $('mods-count').textContent = 'Включено ' + on + ' из ' + state.mods.length;
}

function renderMods() {
  const list = $('mods-list');
  list.textContent = '';
  state.mods.forEach((m) => {
    const row = document.createElement('div');
    row.className = 'mod';

    const icon = document.createElement('img');
    icon.className = 'mod-icon';
    icon.src = 'assets/logo.png';
    icon.alt = '';
    icon.draggable = false;

    const info = document.createElement('div');
    info.className = 'mod-info';
    const name = document.createElement('div');
    name.className = 'mod-name';
    name.textContent = m.name;
    const meta = document.createElement('div');
    meta.className = 'mod-meta';
    meta.textContent = m.meta;
    info.append(name, meta);

    const sw = document.createElement('label');
    sw.className = 'switch';
    const input = document.createElement('input');
    input.type = 'checkbox';
    input.checked = m.enabled;
    input.addEventListener('change', () => {
      m.enabled = input.checked;
      log('Мод «' + m.name + '» ' + (m.enabled ? 'включён' : 'выключен'));
      renderModsCount();
    });
    const track = document.createElement('span');
    track.className = 'track';
    const thumb = document.createElement('span');
    thumb.className = 'thumb';
    sw.append(input, track, thumb);

    row.append(icon, info, sw);
    list.append(row);
  });
  renderModsCount();
}

$('btn-install').addEventListener('click', () => {
  log('Нажато «Установить мод» (установка модов пока не подключена)');
  toast('Установку модов подключим позже');
});

// ---------- nickname ----------
function readNick() {
  try { return localStorage.getItem('nick') || ''; } catch (e) { return ''; }
}
function saveNick(value) {
  try { localStorage.setItem('nick', value); } catch (e) { /* storage may be unavailable */ }
}
function updateNickCount() {
  $('nick-count').textContent = $('nick').value.length + '/' + NICK_MAX;
}

$('nick').value = readNick().slice(0, NICK_MAX);
updateNickCount();
$('nick').addEventListener('input', () => {
  updateNickCount();
  saveNick($('nick').value.trim());
});
$('nick').addEventListener('change', () => log('Ник сохранён: ' + $('nick').value.trim()));

// ---------- launching the game ----------
let launching = false;

function consentGiven() {
  try { return localStorage.getItem('consent') === '1'; } catch (e) { return false; }
}
function saveConsent() {
  try { localStorage.setItem('consent', '1'); } catch (e) { /* storage may be unavailable */ }
}

// phase: idle | working | running | error; progress 0..100, or -1 when unknown
function applyLaunchState(s) {
  launching = s.phase === 'working' || s.phase === 'running';
  const btn = $('btn-play');
  btn.disabled = launching;
  btn.classList.toggle('working', s.phase === 'working' && s.progress >= 0);
  btn.classList.toggle('indeterminate', s.phase === 'working' && s.progress < 0);
  btn.classList.toggle('running', s.phase === 'running');
  btn.style.setProperty('--p', Math.max(0, s.progress) + '%');
  $('play-label').textContent = launching ? s.text : 'Играть с модами';
  if (s.phase === 'error') {
    const text = String(s.text || 'Ошибка запуска');
    toast(text.length > 140 ? text.slice(0, 140) + '… (подробности в логе)' : text);
  }
  if (s.phase === 'idle' || s.phase === 'error') refreshEnv();
}

async function startLaunch() {
  applyLaunchState({ phase: 'working', text: 'Подготовка…', progress: -1 });
  try {
    await invoke('play');
  } catch (e) {
    applyLaunchState({ phase: 'idle', text: '', progress: 0 });
    toast(String(e));
  }
}

async function onPlay() {
  if (launching) return;
  if (!invoke) { toast('Запуск работает только в приложении лаунчера'); return; }
  let info = { ready: true };
  try { info = await invoke('env_info'); } catch (e) { /* try to launch anyway */ }
  if (!info.ready && !consentGiven()) {
    $('consent').hidden = false;
    return;
  }
  startLaunch();
}

$('btn-play').addEventListener('click', onPlay);
$('consent-no').addEventListener('click', () => { $('consent').hidden = true; });
$('consent-yes').addEventListener('click', () => {
  saveConsent();
  $('consent').hidden = true;
  startLaunch();
});

if (listen) {
  listen('launch-state', (e) => applyLaunchState(e.payload));
  listen('launch-log', (e) => log(String(e.payload)));
}

// ---------- save button ----------
$('btn-save').addEventListener('click', () => {
  log('Нажато «Загрузить сохранение» (пока не подключено)');
  toast('Загрузка сохранения пока не подключена');
});

// ---------- help ----------
$('help-open-log').addEventListener('click', () => showPage('log'));
$('help-github').addEventListener('click', () => {
  if (tauri && tauri.core) {
    tauri.core.invoke('open_url', { url: GITHUB_URL }).catch(() => toast('Не удалось открыть страницу'));
  } else {
    window.open(GITHUB_URL, '_blank');
  }
});

// ---------- window buttons ----------
function closeWindow() {
  if (appWindow) appWindow.close();
  else window.close();
}
$('btn-close').addEventListener('click', closeWindow);
$('btn-exit').addEventListener('click', closeWindow);
$('btn-min').addEventListener('click', () => { if (appWindow) appWindow.minimize(); });

// ---------- window dragging (frameless window) ----------
document.querySelectorAll('.topbar, .logo-cell, .topbar > *:not(.controls), .topbar .brand, .topbar .chip')
  .forEach((el) => el.setAttribute('data-tauri-drag-region', ''));

// ---------- start ----------
function showPaths(data, mods) {
  $('mods-path').textContent = mods;
  $('set-mods-path').textContent = mods;
  $('set-data-path').textContent = data;
  $('consent-path').textContent = data;
}
showPaths('%LOCALAPPDATA%\\Bearite-PC', '%LOCALAPPDATA%\\Bearite-PC\\mods');
if (invoke) {
  invoke('paths').then((p) => showPaths(p.data, p.mods)).catch(() => {});
}
renderGame();
refreshEnv();
renderMods();
renderLog(); // the log starts empty
