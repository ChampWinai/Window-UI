const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = s => document.querySelector(s);

const setTheme = t => $('#theme').href = `themes/${t}.css`;
invoke('theme').then(setTheme);
listen('theme', e => setTheme(e.payload));

// slider fill is drawn with a gradient stop at --p
const paint = input => input.style.setProperty('--p', input.value + '%');

function slider(id, capture) {
  const input = $('#' + id), val = $(`#${id}-val`), mute = $(`#${id}-mute`);
  input.oninput = () => {
    paint(input);
    val.textContent = input.value + '%';
    invoke('set_volume', { capture, level: input.value / 100 });
  };
  mute.onclick = () => invoke('toggle_mute', { capture }).then(refresh);
  return v => {
    mute.innerHTML = capture ? icons.mic(v) : icons.volume(v);
    mute.classList.toggle('muted', !v || v.muted);
    mute.disabled = input.disabled = !v;
    // don't fight the user's drag
    if (v && document.activeElement !== input) input.value = Math.round(v.level * 100);
    paint(input);
    val.textContent = v ? input.value + '%' : '—';
  };
}
const setOut = slider('out', false), setIn = slider('in', true);

function deviceList(el, list) {
  el.replaceChildren(...list.map(d => {
    const b = document.createElement('button');
    b.className = 'device';
    b.setAttribute('role', 'radio');
    b.setAttribute('aria-checked', d.default);
    b.innerHTML = icons.check;
    const name = document.createElement('span');
    name.textContent = d.name;
    b.append(name);
    b.onclick = () => invoke('set_default_device', { id: d.id }).then(refresh);
    return b;
  }));
}

// "Spotify.exe" -> Spotify, "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic" -> ZuneMusic
const appName = id => id.replace(/\.exe$/i, '').split('!').pop().split('.').pop();
const thumbs = new Map(); // app + title -> Promise<data url>

function card(m) {
  const key = m.app + '\n' + m.title;
  const el = document.createElement('article');
  el.className = 'card';
  const art = new Image();
  art.alt = '';
  if (!thumbs.has(key)) thumbs.set(key, invoke('media_thumb', { app: m.app }));
  thumbs.get(key).then(src => {
    if (!src) return;
    art.src = src;
    el.style.setProperty('--art', `url("${src}")`);
  });

  const info = document.createElement('div');
  info.className = 'info';
  const app = document.createElement('small');
  app.textContent = appName(m.app);
  const title = document.createElement('strong');
  title.textContent = m.title || 'Unknown';
  const artist = document.createElement('span');
  artist.textContent = m.artist;

  const controls = document.createElement('div');
  controls.className = 'controls';
  for (const [action, icon, label] of [
    ['prev', icons.prev, 'Previous'],
    ['toggle', m.playing ? icons.pause : icons.play, m.playing ? 'Pause' : 'Play'],
    ['next', icons.next, 'Next'],
  ]) {
    const b = document.createElement('button');
    b.innerHTML = icon;
    b.setAttribute('aria-label', label);
    if (action === 'toggle') b.className = 'play';
    // players update their state a beat after the command
    b.onclick = () => invoke('media_control', { app: m.app, action }).then(() => setTimeout(refresh, 300));
    controls.append(b);
  }
  info.append(app, title, artist, controls);
  el.append(art, info);
  return el;
}

let open = false, page = 'quick', devKey = '', mediaKey = '', notifKey = '';

// ---- quick page: radio tiles + bluetooth devices
function tile(id, kind, uri, icon) {
  const el = $('#' + id), main = el.querySelector('.tile-main');
  el.querySelector('.tile-icon').innerHTML = icon;
  el.querySelector('.tile-more').innerHTML = icons.chevronRight;
  el.querySelector('.tile-more').onclick = () => invoke('open_uri', { uri });
  main.onclick = () => {
    const on = main.getAttribute('aria-pressed') !== 'true';
    set(on); // optimistic; radios take a moment to switch
    invoke('set_radio', { kind, on }).then(refreshRadios);
  };
  const set = on => {
    el.hidden = on === null;
    main.setAttribute('aria-pressed', !!on);
    el.querySelector('small').textContent = on ? 'On' : 'Off';
  };
  return set;
}
const setWifi = tile('tile-wifi', 'wifi', 'ms-availablenetworks:', icons.wifi(5));
const setBt = tile('tile-bt', 'bluetooth', 'ms-settings:bluetooth', icons.bluetooth);

async function refreshRadios() {
  const r = await invoke('radios');
  setWifi(r.wifi);
  setBt(r.bluetooth);
  $('#tile-bt small').textContent = r.bluetooth && r.devices.length ? r.devices.join(', ') : r.bluetooth ? 'On' : 'Off';
  $('#bt-section').hidden = !r.devices.length;
  $('#bt-devices').replaceChildren(...r.devices.map(name => {
    const li = document.createElement('li');
    li.innerHTML = icons.bluetooth;
    li.append(name);
    return li;
  }));
}

for (const b of document.querySelectorAll('[data-uri]')) b.onclick = () => invoke('open_uri', { uri: b.dataset.uri });
$('#open-settings').onclick = () => invoke('open_settings');

// ---- notifications page
const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
function ago(ms) {
  const s = (ms - Date.now()) / 1000;
  for (const [unit, size] of [['day', 86400], ['hour', 3600], ['minute', 60]]) {
    if (Math.abs(s) >= size) return rtf.format(Math.round(s / size), unit);
  }
  return rtf.format(0, 'second');
}

function emptyState(text, action) {
  const div = document.createElement('div');
  div.className = 'empty';
  div.innerHTML = icons.bell;
  const p = document.createElement('p');
  p.textContent = text;
  div.append(p);
  if (action) div.append(action);
  return div;
}

async function refreshNotifs() {
  const list = await invoke('notifications');
  const key = JSON.stringify(list);
  if (key === notifKey) return;
  notifKey = key;
  const box = $('#notifs');
  $('#clear-all').hidden = !list?.length;
  if (list === null) {
    const b = document.createElement('button');
    b.className = 'link';
    b.textContent = 'Open Windows notification center';
    b.onclick = () => invoke('open_uri', { uri: 'ms-actioncenter:' });
    return box.replaceChildren(emptyState('Windows hasn’t allowed this app to read notifications.', b));
  }
  if (!list.length) return box.replaceChildren(emptyState('No new notifications'));
  box.replaceChildren(...list.map(n => {
    const el = document.createElement('article');
    el.className = 'notif';
    const head = document.createElement('header');
    const app = document.createElement('small');
    app.textContent = `${n.app || 'App'} · ${ago(n.time)}`;
    const x = document.createElement('button');
    x.className = 'icon-btn';
    x.innerHTML = icons.close;
    x.setAttribute('aria-label', 'Dismiss');
    x.onclick = () => invoke('dismiss_notification', { id: n.id }).then(refreshNotifs);
    head.append(app, x);
    const title = document.createElement('strong');
    title.textContent = n.title;
    const body = document.createElement('p');
    body.textContent = n.body;
    el.append(head, title, body);
    return el;
  }));
}
$('#clear-all').onclick = () => invoke('dismiss_notification', { id: null }).then(refreshNotifs);

// ---- calendar
let month = new Date();
function renderCalendar() {
  const y = month.getFullYear(), m = month.getMonth(), today = new Date();
  $('#cal-title').textContent = month.toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
  // 2023-01-01 was a Sunday
  const heads = [...Array(7)].map((_, i) => {
    const s = document.createElement('span');
    s.className = 'dow';
    s.textContent = new Date(2023, 0, 1 + i).toLocaleDateString(undefined, { weekday: 'narrow' });
    return s;
  });
  const cells = [...Array(new Date(y, m, 1).getDay())].map(() => document.createElement('span'));
  for (let d = 1; d <= new Date(y, m + 1, 0).getDate(); d++) {
    const c = document.createElement('span');
    c.textContent = d;
    if (d === today.getDate() && m === today.getMonth() && y === today.getFullYear()) c.className = 'today';
    cells.push(c);
  }
  $('#cal-grid').replaceChildren(...heads, ...cells);
}
$('#cal-prev').innerHTML = icons.chevronLeft;
$('#cal-next').innerHTML = icons.chevronRight;
$('#cal-prev').onclick = () => { month = new Date(month.getFullYear(), month.getMonth() - 1, 1); renderCalendar(); };
$('#cal-next').onclick = () => { month = new Date(month.getFullYear(), month.getMonth() + 1, 1); renderCalendar(); };

async function refresh() {
  if (page === 'notifications') return refreshNotifs();
  const [audio, media] = await Promise.all([invoke('audio_state'), invoke('media_state')]);
  setOut(audio.output);
  setIn(audio.input);
  const dk = JSON.stringify([audio.outputs, audio.inputs]);
  if (dk !== devKey) {
    devKey = dk;
    deviceList($('#outputs'), audio.outputs);
    deviceList($('#inputs'), audio.inputs);
  }
  const mk = JSON.stringify(media);
  if (mk !== mediaKey) {
    mediaKey = mk;
    $('#media-section').hidden = !media.length;
    $('#media').replaceChildren(...media.map(card));
  }
}

listen('flyout-open', e => {
  open = true;
  page = e.payload;
  document.body.dataset.page = page;
  if (page === 'notifications') {
    month = new Date();
    renderCalendar();
  } else {
    refreshRadios();
  }
  refresh();
});
window.addEventListener('blur', () => open = false);
setInterval(() => open && refresh(), 1000);
// bluetooth enumeration is slower; poll it less
setInterval(() => open && page === 'quick' && refreshRadios(), 3000);
