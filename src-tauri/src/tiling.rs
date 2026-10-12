//! Tiling window manager: layouts are pure functions of (window count, area); the engine keeps a
//! stable window order per monitor and re-applies the layout whenever that set changes.
use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::win;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    Dwindle,
    MasterStack,
    Columns,
    Rows,
    Grid,
    Monocle,
}

pub const LAYOUTS: [Layout; 6] = [Layout::Dwindle, Layout::MasterStack, Layout::Columns, Layout::Rows, Layout::Grid, Layout::Monocle];

impl Layout {
    pub fn next(self) -> Layout {
        LAYOUTS[(LAYOUTS.iter().position(|&l| l == self).unwrap() + 1) % LAYOUTS.len()]
    }
}

/// x, y, width, height
pub type Rect = (i32, i32, i32, i32);

/// Two parts side by side (`across`) or stacked, `gap` apart.
fn split((x, y, w, h): Rect, across: bool, gap: i32) -> (Rect, Rect) {
    if across {
        let a = (w - gap) / 2;
        ((x, y, a, h), (x + a + gap, y, w - a - gap, h))
    } else {
        let a = (h - gap) / 2;
        ((x, y, w, a), (x, y + a + gap, w, h - a - gap))
    }
}

/// `n` equal slices of `r`, `gap` apart.
fn slices((x, y, w, h): Rect, n: i32, across: bool, gap: i32) -> Vec<Rect> {
    let total = if across { w } else { h } - gap * (n - 1);
    (0..n)
        .map(|i| {
            // spread the rounding remainder so the last slice ends exactly at the edge
            let start = total * i / n + gap * i;
            let size = total * (i + 1) / n - total * i / n;
            if across { (x + start, y, size, h) } else { (x, y + start, w, size) }
        })
        .collect()
}

pub fn rects(layout: Layout, n: usize, area: Rect, gap: i32) -> Vec<Rect> {
    let n = n as i32;
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![area];
    }
    match layout {
        Layout::Monocle => vec![area; n as usize],
        Layout::Columns => slices(area, n, true, gap),
        Layout::Rows => slices(area, n, false, gap),
        Layout::MasterStack => {
            let (master, stack) = split(area, true, gap);
            std::iter::once(master).chain(slices(stack, n - 1, false, gap)).collect()
        }
        Layout::Dwindle => {
            // each window takes half of what's left, alternating side-by-side and stacked
            let mut out = Vec::new();
            let mut rest = area;
            for i in 0..n - 1 {
                let (a, b) = split(rest, i % 2 == 0, gap);
                out.push(a);
                rest = b;
            }
            out.push(rest);
            out
        }
        Layout::Grid => {
            let cols = (n as f64).sqrt().ceil() as i32;
            let rows = (n + cols - 1) / cols;
            slices(area, rows, false, gap)
                .into_iter()
                .enumerate()
                .flat_map(|(r, row)| {
                    // the last row may be short; its windows share the full width
                    let in_row = if r as i32 == rows - 1 { n - cols * (rows - 1) } else { cols };
                    slices(row, in_row, true, gap)
                })
                .collect()
        }
    }
}

// ---- engine ------------------------------------------------------------

/// monitor -> window order (first seen first), so windows keep their slot as others come and go
static ORDER: Mutex<Option<HashMap<isize, Vec<isize>>>> = Mutex::new(None);

/// Re-tiles every monitor whose set of tileable windows changed, or all of them with `force`.
pub fn tick(layout: Layout, gap: i32, force: bool) {
    let mut by_monitor: HashMap<isize, Vec<isize>> = HashMap::new();
    for h in win::tileable_windows() {
        by_monitor.entry(win::monitor_of(h)).or_default().push(h);
    }
    let mut guard = ORDER.lock().unwrap();
    let order = guard.get_or_insert_with(HashMap::new);
    order.retain(|mon, _| by_monitor.contains_key(mon));
    for (mon, current) in by_monitor {
        let list = order.entry(mon).or_default();
        let before = list.clone();
        list.retain(|h| current.contains(h));
        list.extend(current.iter().filter(|h| !before.contains(h)));
        if force || *list != before {
            let area = win::work_area(mon, gap);
            for (&h, r) in list.iter().zip(rects(layout, list.len(), area, gap)) {
                win::place_window(h, r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    #[test]
    fn layouts_fill_the_area_without_overlap() {
        let area = (10, 20, 1900, 1000);
        for layout in LAYOUTS {
            for n in 0..9 {
                let r = rects(layout, n, area, 8);
                assert_eq!(r.len(), n, "{layout:?} n={n}");
                for (i, &a) in r.iter().enumerate() {
                    assert!(a.2 > 0 && a.3 > 0, "{layout:?} n={n} empty {a:?}");
                    assert!(a.0 >= area.0 && a.1 >= area.1 && a.0 + a.2 <= area.0 + area.2 && a.1 + a.3 <= area.1 + area.3, "{layout:?} n={n} outside {a:?}");
                    if layout != Layout::Monocle {
                        for &b in &r[i + 1..] {
                            assert!(!overlaps(a, b), "{layout:?} n={n} {a:?} overlaps {b:?}");
                        }
                    }
                }
                // the right/bottom edges are reached: no leftover strip
                if n > 0 {
                    assert_eq!(r.iter().map(|a| a.0 + a.2).max(), Some(area.0 + area.2), "{layout:?} n={n}");
                    assert_eq!(r.iter().map(|a| a.1 + a.3).max(), Some(area.1 + area.3), "{layout:?} n={n}");
                }
            }
        }
    }
}
