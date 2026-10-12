const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = s => document.querySelector(s);

const setTheme = t => $('#theme').href = `themes/${t}.css`;
listen('theme', e => setTheme(e.payload));

const THEMES = [['windows', 'Windows 11'], ['macos', 'macOS'], ['gnome', 'GNOME'], ['kde', 'KDE Plasma']];
const BARS = [['taskbar', 'Taskbar', 'One bar per monitor, styled by the theme'], ['toolbar-dock', 'Toolbar + Dock', 'Status bar on top, app dock at the bottom']];
const LAYOUTS = [['dwindle', 'Dwindle'], ['master-stack', 'Master + stack'], ['columns', 'Columns'], ['rows', 'Rows'], ['grid', 'Grid'], ['monocle', 'Monocle']];

let cfg = null;

function toast(text, error = false) {
  const t = $('#toast');
  t.textContent = text;
  t.className = error ? 'show error' : 'show';
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => (t.className = ''), error ? 6000 : 1600);
}

// Every change saves the whole config; Rust validates, applies what changed, or tells us why not.
// Start from a fresh copy: pins, layout or theme may have changed from the bar/tray meanwhile,
// and a stale copy would silently put them back.
let saving = Promise.resolve();
function save(patch) {
  saving = saving.then(async () => {
    const fresh = (await invoke('get_settings')).config;
    try {
      await invoke('set_settings', { config: { ...fresh, ...patch } });
      toast('Saved');
    } catch (e) {
      toast(String(e), true);
    }
    await load(); // show what is actually in effect
  });
  return saving;
}
// changes made elsewhere (tray, bar, dock) show up when you come back to this window
addEventListener('focus', () => load());

function cards(el, items, current, onPick, extra) {
  el.replaceChildren(...items.map(([id, name, desc]) => {
    const b = document.createElement('button');
    b.className = 'card';
    b.setAttribute('role', 'radio');
    b.setAttribute('aria-checked', id === current);
    b.innerHTML = extra(id);
    const label = document.createElement('span');
    label.className = 'label';
    label.textContent = name;
    b.append(label);
    if (desc) {
      const small = document.createElement('small');
      small.textContent = desc;
      b.append(small);
    }
    b.onclick = () => onPick(id);
    return b;
  }));
}

const appName = path => (path || '').split(/[\\!]/).pop().replace(/\.exe$/i, '');

function renderPins() {
  const list = $('#pinned');
  if (!cfg.pinned.length) {
    const li = document.createElement('li');
    li.className = 'empty';
    li.textContent = 'Nothing pinned yet.';
    return list.replaceChildren(li);
  }
  list.replaceChildren(...cfg.pinned.map((path, i) => {
    const li = document.createElement('li');
    const img = new Image();
    img.alt = '';
    invoke('file_icon', { path }).then(src => src && (img.src = src));
    const text = document.createElement('span');
    text.className = 'grow';
    const strong = document.createElement('strong');
    strong.textContent = appName(path);
    const small = document.createElement('small');
    small.textContent = path;
    text.append(strong, small);
    const move = (to, label, icon) => {
      const b = document.createElement('button');
      b.className = 'icon-btn';
      b.innerHTML = icon;
      b.setAttribute('aria-label', label);
      b.disabled = to < 0 || to >= cfg.pinned.length;
      b.onclick = () => {
        const p = [...cfg.pinned];
        [p[i], p[to]] = [p[to], p[i]];
        save({ pinned: p });
      };
      return b;
    };
    const remove = document.createElement('button');
    remove.className = 'icon-btn';
    remove.innerHTML = icons.close;
    remove.setAttribute('aria-label', `Unpin ${appName(path)}`);
    remove.onclick = () => save({ pinned: cfg.pinned.filter(p => p !== path) });
    li.append(img, text, move(i - 1, 'Move up', icons.chevronUp), move(i + 1, 'Move down', icons.chevronUp.replace('m18 15-6-6-6 6', 'm6 9 6 6 6-6')), remove);
    return li;
  }));
}

async function load() {
  const info = await invoke('get_settings');
  cfg = info.config;
  setTheme(cfg.theme);
  cards($('#themes'), THEMES, cfg.theme, id => save({ theme: id }), id => `<span class="preview" data-preview="${id}"><i></i><i></i><i></i></span>`);
  cards($('#bars'), BARS, cfg.bars, id => save({ bars: id }), id => `<span class="preview" data-preview="${id}"><i></i><i></i><i></i></span>`);
  cards($('#layouts'), LAYOUTS, cfg.tiling_layout, id => save({ tiling_layout: id }), id => icons.layout[id]);
  $('#auto_tile').checked = cfg.auto_tile;
  $('#gap').value = cfg.gap;
  $('#gap-val').textContent = cfg.gap + ' px';
  $('#wp-current').textContent = cfg.wallpaper || 'Not managed by Fix UI (Windows keeps its own)';
  $('#wallpaper_interval').value = String(cfg.wallpaper_interval);
  $('#wp-next').disabled = !cfg.wallpaper;
  for (const k of document.querySelectorAll('.keys')) if (document.activeElement !== k) k.value = cfg[k.dataset.field];
  $('#hide_windows_taskbar').checked = cfg.hide_windows_taskbar;
  if (document.activeElement !== $('#update_url')) $('#update_url').value = cfg.update_url;
  $('#auto_update').checked = cfg.auto_update;
  renderWidgets();
  $('#config-path').textContent = info.config_path;
  $('#version').textContent = info.version;
  renderPins();
}

$('#auto_tile').onchange = e => save({ auto_tile: e.target.checked });
$('#hide_windows_taskbar').onchange = e => save({ hide_windows_taskbar: e.target.checked });
$('#gap').oninput = e => ($('#gap-val').textContent = e.target.value + ' px');
$('#gap').onchange = e => save({ gap: Number(e.target.value) });
$('#wallpaper_interval').onchange = e => save({ wallpaper_interval: Number(e.target.value) });
$('#wp-image').onclick = () => invoke('pick_wallpaper', { folder: false }).then(p => p && load());
$('#wp-folder').onclick = () => invoke('pick_wallpaper', { folder: true }).then(p => p && load());
$('#wp-next').onclick = () => invoke('next_wallpaper').then(() => toast('Changing wallpaper…'));
$('#open-config').onclick = () => invoke('open_config_folder');

// ---- widgets
async function renderWidgets() {
  const { installed, enabled } = await invoke('list_widgets');
  const list = $('#widget-list');
  if (!installed.length) {
    const li = document.createElement('li');
    li.className = 'empty';
    li.textContent = 'No widgets installed.';
    return list.replaceChildren(li);
  }
  list.replaceChildren(...installed.map(w => {
    const li = document.createElement('li');
    const text = document.createElement('span');
    text.className = 'grow';
    const strong = document.createElement('strong');
    strong.textContent = w.name;
    const small = document.createElement('small');
    small.textContent = `${w.description ? w.description + ' · ' : ''}${w.id} · ${w.width}×${w.height} · ${w.corner}`;
    text.append(strong, small);
    const toggle = document.createElement('input');
    toggle.type = 'checkbox';
    toggle.setAttribute('role', 'switch');
    toggle.setAttribute('aria-label', `Show ${w.name}`);
    toggle.checked = enabled.includes(w.id);
    toggle.onchange = () => save({ widgets: toggle.checked ? [...cfg.widgets.filter(x => x !== w.id), w.id] : cfg.widgets.filter(x => x !== w.id) });
    li.append(text, toggle);
    return li;
  }));
}
$('#open-widgets').onclick = () => invoke('open_widgets_folder');
$('#reload-widgets').onclick = () => invoke('reload_widgets').then(() => (renderWidgets(), toast('Widgets reloaded')), e => toast(String(e), true));

// ---- updates
$('#update_url').onchange = e => save({ update_url: e.target.value.trim() });
$('#auto_update').onchange = e => save({ auto_update: e.target.checked });
$('#check-update').onclick = async () => {
  const status = $('#update-status'), notes = $('#update-notes'), install = $('#install-update');
  status.textContent = 'Checking…';
  notes.textContent = '';
  install.hidden = true;
  try {
    const u = await invoke('check_update');
    status.textContent = u ? `Version ${u.version} is available (you have ${u.current})` : `You're up to date (${$('#version').textContent})`;
    notes.textContent = u?.notes || '';
    install.hidden = !u;
  } catch (e) {
    status.textContent = 'Couldn’t check for updates';
    notes.textContent = String(e);
  }
};
$('#install-update').onclick = async () => {
  $('#install-update').disabled = true;
  $('#update-status').textContent = 'Downloading… Fix UI will restart.';
  try {
    await invoke('install_update');
  } catch (e) {
    $('#update-status').textContent = 'Update failed';
    $('#update-notes').textContent = String(e);
    $('#install-update').disabled = false;
  }
};

// ---- shortcut recorder: "ctrl+alt+t" style, as the hotkey parser expects
const MODS = ['Control', 'Alt', 'Shift', 'Meta'];
function combo(e) {
  if (MODS.includes(e.key)) return null; // wait for the real key
  const key = e.code.replace(/^Key/, '').replace(/^Digit/, '').toLowerCase();
  const mods = [e.ctrlKey && 'ctrl', e.altKey && 'alt', e.shiftKey && 'shift', e.metaKey && 'super'].filter(Boolean);
  return mods.length ? [...mods, key].join('+') : null; // a bare key would hijack normal typing
}
for (const input of document.querySelectorAll('.keys')) {
  input.onfocus = () => {
    input.value = 'Press keys…';
    invoke('pause_hotkeys', { paused: true }); // or pressing a current shortcut would fire it
  };
  input.onblur = () => {
    input.value = cfg[input.dataset.field];
    invoke('pause_hotkeys', { paused: false });
  };
  input.onkeydown = e => {
    e.preventDefault();
    if (e.key === 'Escape') return input.blur();
    const c = combo(e);
    if (!c) return;
    input.blur();
    save({ [input.dataset.field]: c });
  };
}

// ---- sidebar: highlight the section at the top of the view (the last one when scrolled to the end)
const main = $('main');
const links = [...document.querySelectorAll('.side a')];
const sections = [...document.querySelectorAll('main section')];
function markCurrent() {
  const atEnd = main.scrollTop + main.clientHeight >= main.scrollHeight - 4;
  const current = atEnd ? sections.at(-1) : sections.filter(s => s.offsetTop - main.offsetTop <= main.scrollTop + 40).at(-1) || sections[0];
  for (const a of links) a.toggleAttribute('aria-current', a.getAttribute('href') === '#' + current.id);
}
main.addEventListener('scroll', markCurrent, { passive: true });
markCurrent();

$('.logo').innerHTML = icons.layout['master-stack'];
load();
