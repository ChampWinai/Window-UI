const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const q = document.getElementById('q');
const list = document.getElementById('list');
let wins = [], shown = [], sel = 0;

const setTheme = t => document.getElementById('theme').href = `themes/${t}.css`;
invoke('theme').then(setTheme);
listen('theme', e => setTheme(e.payload));

const open = async w => {
  await invoke('focus_window', { hwnd: w.hwnd });
  getCurrentWindow().hide();
};

function render() {
  const s = q.value.toLowerCase();
  shown = wins.filter(w => w.title.toLowerCase().includes(s));
  sel = Math.max(0, Math.min(sel, shown.length - 1));
  list.replaceChildren(...shown.map((w, i) => {
    const li = document.createElement('li');
    li.textContent = w.title;
    li.setAttribute('role', 'option');
    li.setAttribute('aria-selected', i === sel);
    li.onclick = () => open(w);
    return li;
  }));
  list.children[sel]?.scrollIntoView({ block: 'nearest' });
}

listen('launcher-open', async () => {
  wins = await invoke('list_windows');
  q.value = '';
  sel = 0;
  render();
  q.focus();
});

q.oninput = () => { sel = 0; render(); };
q.onkeydown = e => {
  if (e.key === 'ArrowDown') sel++;
  else if (e.key === 'ArrowUp') sel--;
  else if (e.key === 'Enter') return shown[sel] && open(shown[sel]);
  else if (e.key === 'Escape') return getCurrentWindow().hide();
  else return;
  e.preventDefault();
  render();
};
