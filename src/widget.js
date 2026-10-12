// Widget host. The widget itself runs in a sandboxed iframe on another origin (widget://), so it
// can't reach window.__TAURI__ or our commands; it can only post the requests answered below.
const { invoke } = window.__TAURI__.core;

// window label: widget-<generation>-<id>
const id = window.__TAURI__.window.getCurrentWindow().label.split('-').slice(2).join('-');

const frame = document.createElement('iframe');
frame.sandbox = 'allow-scripts'; // no allow-same-origin: opaque origin, no storage, no parent access
frame.src = `http://widget.localhost/${encodeURIComponent(id)}/index.html`;
frame.title = id;
document.body.append(frame);

const API = {
  system: () => invoke('system_status'),
  stats: () => invoke('system_stats'),
  media: () => invoke('media_state'),
  mediaThumb: a => invoke('media_thumb', { app: String(a.app) }),
  mediaControl: a => invoke('media_control', { app: String(a.app), action: String(a.action) }),
};

addEventListener('message', async e => {
  if (e.source !== frame.contentWindow) return;
  const { id: callId, type, args } = e.data || {};
  const handler = Object.hasOwn(API, type) ? API[type] : null;
  const reply = handler
    ? await handler(args || {}).then(ok => ({ ok }), err => ({ error: String(err) }))
    : { error: `unknown request: ${type}` };
  frame.contentWindow.postMessage({ id: callId, ...reply }, '*');
});
