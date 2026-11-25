// Bearite-PC launcher window.
// All data below is placeholder state; later stages will fill it from the real game and mods.

const tauri = window.__TAURI__;
const appWindow = tauri && tauri.window ? tauri.window.getCurrentWindow() : null;

const state = {
  game: { found: true, name: 'Super Bear Adventure', version: '1.3.0.8', pkg: 'com.Earthkwak.Platformer' },
  mods: [
    { id: 'bearite', name: 'Bearite', desc: 'Встроенный мод: карты, мини-игры, гардероб', enabled: true },
  ],
  logLines: [],
};

const $ = (id) => document.getElementById(id);

// ---------- log ----------
function log(message) {
  const time = new Date().toLocaleTimeString('ru-RU');
  state.logLines.push('[' + time + '] ' + message);
  $('logbox').textContent = state.logLines.join('\n');
}

// ---------- toast ----------
let toastTimer = null;
function toast(message) {
  const el = $('toast');
  el.textContent = message;
  el.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('show'), 2600);
}

// ---------- pages ----------
function showPage(name) {
  document.querySelectorAll('.page').forEach((p) => p.classList.toggle('active', p.id === 'page-' + name));
  document.querySelectorAll('.nav-item[data-page]').forEach((b) => b.classList.toggle('active', b.dataset.page === name));
}

document.querySelectorAll('.nav-item[data-page]').forEach((b) => {
  b.addEventListener('click', () => showPage(b.dataset.page));
});

// ---------- game card ----------
function renderGame() {
  const g = state.game;
  $('game-name').textContent = g.name;
  $('game-status').textContent = g.found ? g.name + ' найдена' : g.name + ' не найдена';
  $('game-meta').textContent = g.found ? 'Версия ' + g.version + ' · ' + g.pkg : 'Игра пока не подключена';
}

// ---------- mods ----------
function renderMods() {
  const list = $('mods-list');
  list.textContent = '';
  state.mods.forEach((m) => {
    const row = document.createElement('div');
    row.className = 'mod';

    const info = document.createElement('div');
    info.className = 'mod-info';
    const name = document.createElement('div');
    name.className = 'mod-name';
    name.textContent = m.name;
    const desc = document.createElement('div');
    desc.className = 'mod-desc';
    desc.textContent = m.desc;
    info.append(name, desc);

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

    row.append(info, sw);
    list.append(row);
  });
  renderModsCount();
}

function renderModsCount() {
  const on = state.mods.filter((m) => m.enabled).length;
  $('mods-count').textContent = 'Включено ' + on + ' из ' + state.mods.length;
}

// ---------- buttons ----------
$('btn-play').addEventListener('click', () => {
  log('Нажато «Играть с модами» (запуск игры будет подключён на следующем этапе)');
  toast('Запуск игры подключим на следующем этапе');
});
$('btn-save').addEventListener('click', () => {
  log('Нажато «Загрузить сохранение» (пока не подключено)');
  toast('Загрузка сохранения пока не подключена');
});

function closeWindow() {
  if (appWindow) appWindow.close();
  else window.close();
}
$('btn-close').addEventListener('click', closeWindow);
$('btn-exit').addEventListener('click', closeWindow);
$('btn-min').addEventListener('click', () => { if (appWindow) appWindow.minimize(); });

// ---------- window dragging (frameless window) ----------
document.querySelectorAll('.topbar, .logo-cell, .topbar > *:not(.controls):not(.version), .topbar .brand, .topbar .chip')
  .forEach((el) => el.setAttribute('data-tauri-drag-region', ''));

// ---------- start ----------
renderGame();
renderMods();
log('Bearite-PC запущен');
