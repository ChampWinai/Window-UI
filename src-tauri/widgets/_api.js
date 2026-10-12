// Fix UI widget API. Include with <script src="/_api.js"></script>.
// Widgets run in a sandboxed frame with no direct access to the app; every call is a message to the
// host, which only answers the requests listed here.
window.FixUI = (() => {
  let next = 0;
  const pending = new Map();
  addEventListener('message', e => {
    const p = pending.get(e.data && e.data.id);
    if (!p) return;
    pending.delete(e.data.id);
    'error' in e.data ? p.reject(new Error(e.data.error)) : p.resolve(e.data.ok);
  });
  const call = (type, args) =>
    new Promise((resolve, reject) => {
      const id = ++next;
      pending.set(id, { resolve, reject });
      parent.postMessage({ id, type, args }, '*');
    });
  return {
    /** volume, network, battery, bluetooth, input language, notification count */
    system: () => call('system'),
    /** { cpu: 0-1, memory: 0-1, memory_used, memory_total } (bytes) */
    stats: () => call('stats'),
    /** [{ app, title, artist, playing }] */
    media: () => call('media'),
    /** album art of `app`'s current track as a data: URL, or null */
    mediaThumb: app => call('mediaThumb', { app }),
    /** action: 'prev' | 'next' | 'toggle' */
    mediaControl: (app, action) => call('mediaControl', { app, action }),
  };
})();
