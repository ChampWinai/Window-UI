const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { Menu } = window.__TAURI__.menu;

// one page, three jobs: "bar" (single taskbar), "toolbar" (top) or "dock" (bottom); see rebuild_bars
document.body.dataset.role = window.__TAURI__.window.getCurrentWindow().label.split('-').pop();

const apps = document.getElementById('apps');
const time = document.querySelector('#clock .time');
const date = document.querySelector('#clock .date');

const setTheme = t => document.getElementById('theme').href = `themes/${t}.css`;
invoke('theme').then(setTheme);
listen('theme', e => setTheme(e.payload));

const popup = async items => (await Menu.new({ items })).popup();
let active = null, lastKey = '';

const start = document.getElementById('start');
start.onclick = () => invoke('toggle_launcher');
// the dock has no tray, so settings also live on the start button's menu
start.oncontextmenu = e => {
  e.preventDefault();
  popup([{ text: 'Fix UI settings', action: () => invoke('open_settings') }]);
};
const settingsBtn = document.getElementById('settings');
settingsBtn.innerHTML = icons.settings;
settingsBtn.onclick = () => invoke('open_settings');

// ---- apps: one button per app (pinned first, then running in first-seen order)
// ponytail: icons cached per exe forever; prune if long sessions grow memory
const groupIcons = new Map();
const seenApps = [];
// "C:\…\Code.exe" -> Code, "shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude" -> Claude
const appName = path => (path || '').split(/[\\!]/).pop().replace(/\.exe$/i, '');

function iconOf(g) {
  if (!groupIcons.has(g.key)) {
    // the exe's own icon is stable; fall back to the window's (elevated apps hide their exe path)
    const first = g.path ? invoke('file_icon', { path: g.path }) : Promise.resolve(null);
    groupIcons.set(g.key, first.then(src => src || (g.windows[0] && invoke('window_icon', { hwnd: g.windows[0].hwnd }))));
  }
  return groupIcons.get(g.key);
}

function groups(s) {
  const map = new Map();
  for (const path of s.pinned) map.set(path, { key: path, path, pinned: true, windows: [] });
  for (const w of s.windows) {
    const key = w.exe || `hwnd:${w.hwnd}`;
    if (!map.has(key)) map.set(key, { key, path: w.exe, pinned: false, windows: [] });
    map.get(key).windows.push(w);
  }
  for (const key of map.keys()) if (!seenApps.includes(key)) seenApps.push(key);
  const list = [...map.values()];
  // stable cycling order regardless of z-order
  for (const g of list) g.windows.sort((a, b) => a.hwnd - b.hwnd);
  return list.sort((a, b) => (b.pinned - a.pinned) || (a.pinned ? s.pinned.indexOf(a.key) - s.pinned.indexOf(b.key) : seenApps.indexOf(a.key) - seenApps.indexOf(b.key)));
}

function appButton(g) {
  const b = document.createElement('button');
  const isActive = g.windows.some(w => w.hwnd === active);
  const label = g.windows.length ? (g.windows.find(w => w.hwnd === active) || g.windows[0]).title : appName(g.path);
  b.title = g.windows.length > 1 ? `${appName(g.path) || label} (${g.windows.length} windows)` : label;
  b.setAttribute('aria-pressed', isActive);
  b.classList.toggle('running', g.windows.length > 0);
  b.dataset.count = Math.min(g.windows.length, 3);
  const img = new Image();
  img.alt = '';
  iconOf(g).then(src => src && (img.src = src));
  const span = document.createElement('span');
  span.textContent = label;
  b.append(img, span);

  // clicking focuses our bar, so `active` is the window that was in front before the click
  b.onclick = () => {
    if (!g.windows.length) return invoke('launch_app', { path: g.path });
    const i = g.windows.findIndex(w => w.hwnd === active);
    if (g.windows.length === 1) return invoke(i === 0 ? 'minimize_window' : 'focus_window', { hwnd: g.windows[0].hwnd }).then(tick);
    invoke('focus_window', { hwnd: g.windows[(i + 1) % g.windows.length].hwnd }).then(tick); // several: cycle
  };
  b.onauxclick = e => e.button === 1 && g.path && invoke('launch_app', { path: g.path }); // middle: new window
  b.oncontextmenu = e => {
    e.preventDefault();
    const items = [];
    if (g.path) {
      items.push({ text: g.pinned ? 'Unpin' : 'Pin', action: () => invoke('set_pinned', { path: g.path, pinned: !g.pinned }).then(tick) });
      items.push({ text: 'New window', action: () => invoke('launch_app', { path: g.path }) });
    }
    if (g.windows.length) {
      items.push({ text: g.windows.length > 1 ? 'Close all windows' : 'Close window', action: () => g.windows.forEach(w => invoke('close_window', { hwnd: w.hwnd })) });
    }
    if (items.length) popup(items);
  };
  return b;
}

// ---- virtual desktops + tiling layout
const workspaces = document.getElementById('workspaces');
const layoutBtn = document.getElementById('layout');
const titleEl = document.getElementById('title');
const LAYOUT_NAMES = { 'dwindle': 'Dwindle', 'master-stack': 'Master + stack', 'columns': 'Columns', 'rows': 'Rows', 'grid': 'Grid', 'monocle': 'Monocle' };
let wmKey = '';

function renderWm(s) {
  const key = JSON.stringify([s.desktops, s.layout, s.auto_tile]);
  if (key === wmKey) return;
  wmKey = key;
  const d = s.desktops;
  workspaces.replaceChildren(...d.names.map((name, i) => {
    const b = document.createElement('button');
    b.className = 'pill ws';
    b.textContent = i + 1;
    b.title = name || `Desktop ${i + 1}`;
    if (i === d.current) b.setAttribute('aria-current', 'true');
    b.onclick = () => invoke('switch_desktop', { index: i });
    b.oncontextmenu = e => {
      e.preventDefault();
      if (d.names.length > 1) popup([{ text: `Remove ${b.title}`, action: () => invoke('remove_desktop', { index: i }) }]);
    };
    return b;
  }), Object.assign(document.createElement('button'), {
    className: 'pill ws add', textContent: '+', title: 'New desktop', onclick: () => invoke('new_desktop'),
  }));
  layoutBtn.innerHTML = icons.layout[s.layout];
  layoutBtn.title = `Tiling: ${LAYOUT_NAMES[s.layout]}${s.auto_tile ? '' : ' (auto-tile off)'} · click: next layout`;
  layoutBtn.classList.toggle('off', !s.auto_tile);
}
workspaces.onwheel = e => invoke('step_desktop', { forward: e.deltaY > 0 });
layoutBtn.onclick = () => invoke('next_layout').then(tick);
layoutBtn.oncontextmenu = e => {
  e.preventDefault();
  popup([
    { text: 'Auto-tile windows', checked: !layoutBtn.classList.contains('off'), action: () => invoke('toggle_auto_tile').then(tick) },
    ...Object.entries(LAYOUT_NAMES).map(([id, text]) => ({ text, action: () => invoke('set_layout', { layout: id }).then(tick) })),
  ]);
};

async function tick() {
  const s = await invoke('taskbar_state');
  if (s.active !== null) active = s.active;
  renderWm(s);
  titleEl.textContent = s.active_title;
  const list = groups(s);
  // only rebuild when something changed, otherwise clicks land on a replaced element
  const key = JSON.stringify([active, list.map(g => [g.key, g.pinned, g.windows.map(w => [w.hwnd, w.title])])]);
  if (key !== lastKey) {
    lastKey = key;
    apps.replaceChildren(...list.map(appButton));
  }
  const now = new Date();
  time.textContent = now.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  date.textContent = now.toLocaleDateString([], { weekday: 'short', day: 'numeric', month: 'short' });
}

tick();
setInterval(tick, 500);

// ---- tray: app icons, language, bluetooth, network, volume, battery, notifications
const $ = id => document.getElementById(id);
const net = $('net'), vol = $('vol'), bat = $('bat'), bt = $('bt'), lang = $('lang'), bell = $('bell');
const appTray = $('apptray'), appIcons = $('appicons'), chevron = $('chevron');
let level = null;

// other apps' tray icons: keep one button per icon and update it in place, so a click
// isn't lost to a re-render (some icons, like temperature readouts, change every second)
const trayButtons = new Map();
let trayOrder = '';

function trayButton(key) {
  const b = document.createElement('button');
  b.className = 'pill trayicon';
  b.append(new Image());
  b.onclick = () => invoke('tray_click', { key, button: 'left' });
  b.ondblclick = () => invoke('tray_click', { key, button: 'double' });
  b.oncontextmenu = e => {
    e.preventDefault();
    invoke('tray_click', { key, button: 'right' });
  };
  return b;
}

// first-seen order: some apps delete + re-add their icon on every update, which would
// otherwise shuffle it to the end and move whatever you were about to click
const traySeen = [];

function renderTray(list) {
  for (const t of list) if (!traySeen.includes(t.key)) traySeen.push(t.key);
  list.sort((a, b) => traySeen.indexOf(a.key) - traySeen.indexOf(b.key));
  for (const t of list) {
    if (!trayButtons.has(t.key)) trayButtons.set(t.key, trayButton(t.key));
    const b = trayButtons.get(t.key);
    b.title = t.tip;
    b.setAttribute('aria-label', t.tip || 'App icon');
    const img = b.firstChild;
    if (t.icon && img.getAttribute('src') !== t.icon) img.src = t.icon;
  }
  const order = list.map(t => t.key).join('\n');
  if (order !== trayOrder) {
    trayOrder = order;
    for (const k of trayButtons.keys()) if (!list.some(t => t.key === k)) trayButtons.delete(k);
    appIcons.replaceChildren(...list.map(t => trayButtons.get(t.key)));
  }
  appTray.hidden = !list.length;
}

function setCollapsed(c) {
  appTray.classList.toggle('collapsed', c);
  chevron.setAttribute('aria-expanded', !c);
  chevron.title = c ? 'Show app icons' : 'Hide app icons';
  try { localStorage.setItem('trayCollapsed', c ? '1' : ''); } catch {}
}
chevron.innerHTML = icons.chevronUp;
chevron.onclick = () => setCollapsed(!appTray.classList.contains('collapsed'));
try { setCollapsed(!!localStorage.getItem('trayCollapsed')); } catch {}

async function status() {
  invoke('tray_icons').then(renderTray);
  const s = await invoke('system_status');
  lang.textContent = s.language;

  bt.hidden = s.bluetooth === null;
  bt.innerHTML = icons.bluetooth;
  bt.classList.toggle('off', !s.bluetooth);
  bt.title = s.bluetooth ? 'Bluetooth on' : 'Bluetooth off';

  const count = s.notifications || 0;
  bell.innerHTML = icons.bell + (count ? `<span class="badge">${Math.min(count, 99)}</span>` : '');
  bell.title = count ? `${count} notifications` : 'Notifications';

  const n = s.network;
  net.innerHTML = n.kind === 'wifi' ? icons.wifi(n.bars) : n.kind === 'ethernet' ? icons.ethernet : icons.offline;
  net.title = n.kind === 'none' ? 'Not connected' : n.name;

  const v = s.volume;
  level = v && v.level;
  vol.innerHTML = icons.volume(v);
  vol.title = !v ? 'No audio device' : v.muted ? 'Muted' : `Volume ${Math.round(v.level * 100)}%`;

  const b = s.battery;
  bat.hidden = !b;
  if (b) {
    bat.innerHTML = icons.battery(b.percent, b.charging) + `<span>${b.percent}%</span>`;
    bat.title = `Battery ${b.percent}%${b.charging ? ', charging' : ''}`;
    bat.classList.toggle('low', b.percent <= 15 && !b.charging);
  }
}

net.onclick = () => invoke('open_uri', { uri: 'ms-availablenetworks:' });
vol.onclick = bat.onclick = bt.onclick = () => invoke('toggle_flyout', { page: 'quick' });
bell.onclick = $('clock').onclick = () => invoke('toggle_flyout', { page: 'notifications' });
lang.onclick = () => invoke('next_language').then(() => setTimeout(status, 200));
vol.onwheel = e => {
  if (level === null) return;
  level = Math.min(1, Math.max(0, level + (e.deltaY < 0 ? 0.02 : -0.02)));
  invoke('set_volume', { capture: false, level }).then(status);
};
status();
setInterval(status, 2000);
