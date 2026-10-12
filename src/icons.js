// Inline SVG icons (24px grid, stroke = currentColor). Only static markup and numbers go in here.
const svg = body => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
const dim = on => on ? '' : ' opacity="0.3"';

const icons = {
  volume(v) {
    const speaker = '<path d="M11 5 6 9H2v6h4l5 4z"/>';
    if (!v || v.muted || v.level === 0) return svg(speaker + '<path d="m22 9-6 6M16 9l6 6"/>');
    return svg(speaker + '<path d="M15.5 8.5a5 5 0 0 1 0 7"/>' + (v.level > 0.5 ? '<path d="M19 5a10 10 0 0 1 0 14"/>' : ''));
  },
  mic(v) {
    const mic = '<rect x="9" y="2" width="6" height="12" rx="3"/><path d="M5 10a7 7 0 0 0 14 0M12 17v5"/>';
    return svg(mic + (!v || v.muted ? '<path d="m3 3 18 18"/>' : ''));
  },
  wifi(bars) {
    const lit = Math.ceil((bars * 3) / 5); // 0-5 bars -> 0-3 arcs
    return svg(
      `<path d="M2 8.8a15 15 0 0 1 20 0"${dim(lit >= 3)}/>` +
      `<path d="M5 12.6a10 10 0 0 1 14 0"${dim(lit >= 2)}/>` +
      `<path d="M8.5 16.4a5 5 0 0 1 7 0"${dim(lit >= 1)}/>` +
      '<path d="M12 20h.01" stroke-width="3"/>'
    );
  },
  ethernet: svg('<rect x="9" y="2" width="6" height="6" rx="1"/><rect x="2" y="16" width="6" height="6" rx="1"/><rect x="16" y="16" width="6" height="6" rx="1"/><path d="M12 8v4M5 16v-4h14v4"/>'),
  offline: svg('<path d="M2 8.8a15 15 0 0 1 20 0M5 12.6a10 10 0 0 1 14 0M8.5 16.4a5 5 0 0 1 7 0" opacity="0.3"/><path d="m3 3 18 18"/>'),
  battery(percent, charging) {
    const w = Math.max(1, Math.round((14 * percent) / 100));
    return svg(
      '<rect x="2" y="7" width="18" height="10" rx="2.5"/><path d="M22 11v2"/>' +
      `<rect x="4" y="9" width="${w}" height="6" rx="1" fill="currentColor" stroke="none"${charging ? ' opacity="0.35"' : ''}/>` +
      (charging ? '<path d="m12 8-3 4h4l-3 4" stroke-width="1.8"/>' : '')
    );
  },
  bluetooth: svg('<path d="m7 7 10 10-5 5V2l5 5L7 17"/>'),
  bell: svg('<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>'),
  chevronUp: svg('<path d="m18 15-6-6-6 6"/>'),
  chevronLeft: svg('<path d="m15 18-6-6 6-6"/>'),
  chevronRight: svg('<path d="m9 18 6-6-6-6"/>'),
  close: svg('<path d="M18 6 6 18M6 6l12 12"/>'),
  sliders: svg('<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"/>'),
  check: svg('<path d="m5 12 5 5 9-9"/>'),
  settings: svg('<path d="M12.2 2h-.4a2 2 0 0 0-2 2v.2a2 2 0 0 1-1 1.7l-.4.3a2 2 0 0 1-2 0l-.2-.1a2 2 0 0 0-2.7.7l-.2.4a2 2 0 0 0 .7 2.7l.2.1a2 2 0 0 1 1 1.7v.5a2 2 0 0 1-1 1.8l-.2.1a2 2 0 0 0-.7 2.7l.2.4a2 2 0 0 0 2.7.7l.2-.1a2 2 0 0 1 2 0l.4.3a2 2 0 0 1 1 1.7v.2a2 2 0 0 0 2 2h.4a2 2 0 0 0 2-2v-.2a2 2 0 0 1 1-1.7l.4-.3a2 2 0 0 1 2 0l.2.1a2 2 0 0 0 2.7-.7l.2-.4a2 2 0 0 0-.7-2.7l-.2-.1a2 2 0 0 1-1-1.8v-.5a2 2 0 0 1 1-1.7l.2-.1a2 2 0 0 0 .7-2.7l-.2-.4a2 2 0 0 0-2.7-.7l-.2.1a2 2 0 0 1-2 0l-.4-.3a2 2 0 0 1-1-1.7V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>'),
  // tiling layouts, drawn as their tiles inside a 20x16 screen
  layout: Object.fromEntries(Object.entries({
    'dwindle': '<path d="M12 4v16M12 12h8M16 12v8"/>',
    'master-stack': '<path d="M12 4v16M12 9.3h8M12 14.6h8"/>',
    'columns': '<path d="M8.7 4v16M15.3 4v16"/>',
    'rows': '<path d="M2 9.3h20M2 14.6h20"/>',
    'grid': '<path d="M12 4v16M2 12h20"/>',
    'monocle': '<rect x="5" y="7" width="14" height="10" rx="1"/>',
  }).map(([k, inner]) => [k, svg('<rect x="2" y="4" width="20" height="16" rx="2"/>' + inner)])),
  play: svg('<path d="M7 4v16l13-8z" fill="currentColor"/>'),
  pause: svg('<path d="M7 4h3v16H7zM14 4h3v16h-3z" fill="currentColor" stroke="none"/>'),
  prev: svg('<path d="M18 5v14L8 12z" fill="currentColor"/><path d="M6 5v14"/>'),
  next: svg('<path d="M6 5v14l10-7z" fill="currentColor"/><path d="M18 5v14"/>'),
};
