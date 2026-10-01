// Island window: placement on the chosen display, the two window sizes
// (full panel / invisible wake strip), click-through and the cursor poll.
//
// There is no notch on a PC, so the island is a black shape drawn at the top
// centre of the main display inside a borderless, transparent, always-on-top
// window that never takes focus.
//
// Everything OS-specific — cursor query, button state, focus, drag unblocking —
// is reached through `crate::platform`. This file is the logic that is the same
// on every platform.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::platform;

/// Logical size of the full window — the largest island view, like the macOS panel.
pub const PANEL_W: f64 = 720.0;
pub const PANEL_H: f64 = 320.0;
/// Logical size of the invisible strip that wakes the island when it is hidden.
pub const STRIP_W: f64 = 240.0;
pub const STRIP_H: f64 = 6.0;

pub const WINDOW_LABEL: &str = "island";

/// Margin around the island that still counts as "on the island", in logical px.
/// Wider than the macOS 6 pt because a click must never be swallowed.
const HIT_MARGIN: f64 = 14.0;

#[derive(Serialize, Clone)]
pub struct CursorPayload {
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Clone)]
pub struct ScreenInfo {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// The island shape in window-logical coordinates, pushed by the front end.
/// The poll thread owns the click-through decision so it lands in the same 16 ms
/// tick as the cursor read — an IPC round trip here loses clicks.
#[derive(Clone, Copy, Default)]
pub struct IslandRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Wakes / parks the cursor poll thread so a hidden island costs literally nothing.
pub struct PollGate {
    active: Mutex<bool>,
    cv: Condvar,
    pub collapsed: AtomicBool,
    pub rect: Mutex<IslandRect>,
    /// Mirrors the window flag so we only call into Win32 when it changes.
    ignoring: AtomicBool,
    /// Last input region actually handed to the platform, so a shape that has
    /// not changed costs neither a request nor a log line.
    last_shape: Mutex<Option<Shape>>,
}

/// What `sync_input_shape` last applied, in window-logical coordinates.
type Shape = (bool, i64, i64, i64, i64);

impl PollGate {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(false),
            cv: Condvar::new(),
            collapsed: AtomicBool::new(true),
            rect: Mutex::new(IslandRect::default()),
            ignoring: AtomicBool::new(false),
            last_shape: Mutex::new(None),
        }
    }

    pub fn set_rect(&self, rect: IslandRect) {
        let mut guard = self.rect.lock().unwrap();
        let was_blank = guard.w == 0.0 && guard.h == 0.0;
        let becomes_blank = rect.w == 0.0 && rect.h == 0.0;
        let size_changed = guard.w != rect.w || guard.h != rect.h;
        *guard = rect;
        drop(guard);

        // Only the two edges are logged: the first shape the front end ever
        // pushes, and the moment it withdraws it. Click-through can never turn
        // off with no shape, and "it never woke up" is invisible without this.
        if size_changed && (was_blank || becomes_blank) {
            crate::log::line(format!(
                "island rect {w:.0}x{h:.0} at ({x:.0},{y:.0})",
                w = rect.w, h = rect.h, x = rect.x, y = rect.y
            ));
        }
    }

    /// Forces the next poll tick to re-apply the flag (after a window resize).
    pub fn forget_ignore_state(&self) {
        self.ignoring.store(false, Ordering::Relaxed);
    }

    pub fn set_active(&self, on: bool) {
        let mut guard = self.active.lock().unwrap();
        *guard = on;
        self.cv.notify_all();
    }

    fn wait_until_active(&self) {
        let mut guard = self.active.lock().unwrap();
        while !*guard {
            guard = self.cv.wait(guard).unwrap();
        }
    }

    fn is_active(&self) -> bool {
        *self.active.lock().unwrap()
    }
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(WINDOW_LABEL)
}

// Cursor, button state, drag unblocking and focus policy live in
// `crate::platform` — see `platform/win32.rs` and `platform/x11.rs`. This file
// only ever asks the seam for an answer.

fn monitor_contains(m: &Monitor, x: f64, y: f64) -> bool {
    let p = m.position();
    let s = m.size();
    x >= p.x as f64
        && x < (p.x + s.width as i32) as f64
        && y >= p.y as f64
        && y < (p.y + s.height as i32) as f64
}

/// The display the island lives on: the primary one, or the one under the cursor.
fn target_monitor(app: &AppHandle, pref: &str) -> Option<Monitor> {
    let monitors = app.available_monitors().ok()?;
    if pref == "cursor" {
        if let Some((cx, cy)) = platform::cursor_physical() {
            if let Some(m) = monitors.iter().find(|m| monitor_contains(m, cx, cy)) {
                return Some(m.clone());
            }
        }
    }
    app.primary_monitor()
        .ok()
        .flatten()
        .or_else(|| monitors.into_iter().next())
}

pub fn screen_info(app: &AppHandle, pref: &str) -> ScreenInfo {
    match target_monitor(app, pref) {
        Some(m) => {
            let scale = m.scale_factor();
            let p = m.position();
            let s = m.size();
            ScreenInfo {
                x: p.x as f64 / scale,
                y: p.y as f64 / scale,
                width: s.width as f64 / scale,
                height: s.height as f64 / scale,
                scale,
            }
        }
        None => ScreenInfo { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0, scale: 1.0 },
    }
}

/// Places and sizes the window. `collapsed` picks the wake strip instead of the panel.
pub fn apply_geometry(app: &AppHandle, pref: &str, collapsed: bool) {
    let Some(win) = window(app) else { return };
    let Some(m) = target_monitor(app, pref) else { return };

    let scale = m.scale_factor();
    let mp = *m.position();
    let ms = *m.size();

    let (lw, lh) = if collapsed { (STRIP_W, STRIP_H) } else { (PANEL_W, PANEL_H) };
    let pw = (lw * scale).round().max(1.0) as u32;
    let ph = (lh * scale).round().max(1.0) as u32;
    let x = mp.x + (ms.width as i32 - pw as i32) / 2;
    let y = mp.y;

    // GTK will not shrink a window declared `resizable: false` — `resize` is a
    // silent no-op there, where Windows' SetWindowPos resizes regardless of
    // WS_THICKFRAME. So the island has to become resizable before every
    // placement, and it must *stay* resizable: locking it again makes GTK fall
    // back to the window's default size (240x6, which GTK itself clamps to 200
    // high), undoing the very resize we just asked for. Not offering a resize
    // grip is therefore a Windows-only guarantee — here the size is authoritative
    // because the island immediately re-asserts it on the next placement.
    #[cfg(unix)]
    let _ = win.set_resizable(true);
    let _ = win.set_size(PhysicalSize::new(pw, ph));
    let _ = win.set_position(PhysicalPosition::new(x, y));
    // Moving across displays can rescale the window: re-assert the physical size.
    let _ = win.set_size(PhysicalSize::new(pw, ph));
    let _ = win.set_always_on_top(true);
    crate::log::line(format!("placed the island: {pw}x{ph} at ({x},{y})"));
    // A placement is exactly when GTK may re-assert its own input region, so the
    // island's shape goes back on straight after.
    sync_input_shape(app, collapsed);
}

/// Hands the window an input region shaped like the island, so it takes the
/// mouse where it is drawn and nowhere else.
///
/// Only meaningful where `platform::shape_follows_rect()` — see
/// `platform::set_input_rect` for why X11 needs this instead of a cursor-driven
/// flag. Elsewhere it returns before touching anything.
///
/// `collapsed` is passed in rather than read from the gate because at boot the
/// window has already been sized while the gate still holds its initial value.
pub fn sync_input_shape(app: &AppHandle, collapsed: bool) {
    if !platform::shape_follows_rect() {
        return;
    }
    let Some(shared) = app.try_state::<crate::Shared>() else { return };
    let r = *shared.gate.rect.lock().unwrap();

    // Collapsed: the window *is* the wake strip, so all of it takes the mouse.
    // Expanded: the island's own rectangle — including a zero height, which is
    // the island mid-animation and must not swallow a click while invisible.
    let (key, described) = if collapsed {
        ((true, 0, 0, 0, 0), "full window (wake strip)".to_string())
    } else if r.w > 0.0 && r.h > 0.0 {
        (
            (false, r.x.round() as i64, r.y.round() as i64, r.w.round() as i64, r.h.round() as i64),
            format!("{w:.0}x{h:.0} at ({x:.0},{y:.0})", w = r.w, h = r.h, x = r.x, y = r.y),
        )
    } else {
        ((false, 0, 0, 0, 0), "empty — island invisible".to_string())
    };

    {
        let mut last = shared.gate.last_shape.lock().unwrap();
        if *last == Some(key) {
            return;
        }
        *last = Some(key);
    }
    crate::log::line(format!("input shape: {described}"));
    if let Some(win) = window(app) {
        platform::set_input_rect(&win, if collapsed { None } else { Some((r.x, r.y, r.w, r.h)) });
    }
}

/// Position, size and scale of the monitor the island lives on. Any change here
/// means the island has to be placed again.
fn current_screen_key(app: &AppHandle) -> Option<(i32, i32, u32, u32, u64)> {
    let pref = app
        .try_state::<crate::Shared>()
        .map(|s| s.settings.lock().unwrap().screen.clone())
        .unwrap_or_else(|| "primary".into());
    let m = target_monitor(app, &pref)?;
    let p = m.position();
    let size = m.size();
    Some((p.x, p.y, size.width, size.height, m.scale_factor().to_bits()))
}

/// Emits `cursor` (window-logical coordinates) at ~60 Hz while the island is
/// visible. Parked on a condvar the rest of the time.
pub fn spawn_cursor_poll(app: AppHandle, gate: Arc<PollGate>) {
    std::thread::spawn(move || {
        let mut was_down = false;
        // On the rect-driven path, whether the cursor was last seen inside the
        // island — only used to log the transition once.
        let mut was_inside = false;
        // Remembered across wakes so a display change while hidden is noticed the
        // moment the island comes back.
        let mut last_screen: Option<(i32, i32, u32, u32, u64)> = None;
        loop {
            gate.wait_until_active();
            let mut last = (f64::MIN, f64::MIN);
            let mut ticks: u32 = 0;
            while gate.is_active() {
                std::thread::sleep(Duration::from_millis(16));

                // Monitors get plugged in, unplugged, rearranged and rescaled, and
                // an island pinned to coordinates that no longer exist is an island
                // nobody can reach. Checked about twice a second — the cursor poll
                // is already running, so this costs one monitor query.
                ticks = ticks.wrapping_add(1);
                if ticks % 30 == 0 {
                    let now = current_screen_key(&app);
                    if now.is_some() && now != last_screen {
                        let first = last_screen.is_none();
                        last_screen = now;
                        if !first {
                            crate::log::line("display layout changed — repositioning".to_string());
                            let _ = app.emit_to(WINDOW_LABEL, "screen-changed", ());
                        }
                    }
                }

                let Some(win) = window(&app) else { continue };
                let Ok(origin) = win.outer_position() else { continue };
                let scale = win.scale_factor().unwrap_or(1.0);
                let Some((cx, cy)) = platform::cursor_physical() else { continue };
                let x = (cx - origin.x as f64) / scale;
                let y = (cy - origin.y as f64) / scale;
                let size = match win.inner_size() {
                    Ok(s) => (s.width as f64 / scale, s.height as f64 / scale),
                    Err(_) => (PANEL_W, PANEL_H),
                };
                if (x - last.0).abs() < 1.0 && (y - last.1).abs() < 1.0 {
                    continue;
                }
                last = (x, y);

                // The island shape, with a small entry margin so the decision is
                // already made by the time a moving cursor reaches a button.
                let r = *gate.rect.lock().unwrap();
                let on_island = r.w > 0.0
                    && x >= r.x - HIT_MARGIN
                    && x <= r.x + r.w + HIT_MARGIN
                    && y >= r.y - HIT_MARGIN
                    && y <= r.y + r.h + HIT_MARGIN;

                // A file being dragged has to be able to find us. WS_EX_TRANSPARENT
                // — what click-through is on Windows — hides the window from
                // WindowFromPoint, so OLE finds no drop target and shows the "no
                // drop" cursor. macOS has no such problem: AppKit delivers drags to
                // registered destinations whatever ignoresMouseEvents says. So while
                // a button is held anywhere over the panel, the whole panel takes
                // the mouse, which also makes the drop zone as forgiving as the Mac's.
                // A press may be the start of a drag: make sure the drop target is
                // ours before the file arrives.
                let down = platform::left_button_down();
                if down && !was_down {
                    let handle = app.clone();
                    let _ = app.run_on_main_thread(move || platform::unblock_webview_drops(&handle));
                }
                was_down = down;

                let dragging = down
                    && x >= 0.0
                    && x <= size.0
                    && y >= 0.0
                    && y <= size.1;

                let accept = on_island || dragging;

                if platform::shape_follows_rect() {
                    // Nothing to flip: `sync_input_shape` already gave the window
                    // the island's own rectangle, so it takes the mouse wherever
                    // the island is drawn whether or not the cursor read is
                    // telling the truth. All the poll can usefully report is that
                    // read itself — on XWayland it can freeze for minutes while
                    // the pointer is over surfaces XWayland gets no events for,
                    // and "the island is unclickable" has exactly one cause
                    // hiding in that number. Tested against the rectangle the
                    // window actually uses, margin and all, so the line says
                    // where the click would really land.
                    let in_shape = r.w > 0.0
                        && r.h > 0.0
                        && x >= r.x
                        && x <= r.x + r.w
                        && y >= r.y
                        && y <= r.y + r.h;
                    if in_shape != was_inside {
                        was_inside = in_shape;
                        if !gate.collapsed.load(Ordering::Relaxed) {
                            crate::log::line(format!(
                                "cursor {} island — cursor=({x:.0},{y:.0}) rect=({:.0},{:.0} {w:.0}x{h:.0})",
                                if in_shape { "inside" } else { "outside" },
                                r.x, r.y, w = r.w, h = r.h,
                            ));
                        }
                    }
                } else if
                    // While the island is hidden, this flag belongs to
                    // set_collapsed — which sets it and then parks this poll — and
                    // the cursor is normally nowhere near the island shape. A tick
                    // already in flight must therefore not recompute it: it would
                    // flip click-through back on, and with the poll parked nothing
                    // would ever flip it off again. The wake strip wakes on a DOM
                    // mouseenter, which click-through swallows, so that state has
                    // no way back out.
                    !gate.collapsed.load(Ordering::Relaxed)
                    && gate.ignoring.load(Ordering::Relaxed) == accept
                {
                    gate.ignoring.store(!accept, Ordering::Relaxed);
                    // One line per transition, not per tick: this is the seam
                    // where cursor space, the island rect and the window flag
                    // all meet, and on a new platform "it is stuck clicking
                    // through" has exactly one cause hiding in these numbers.
                    crate::log::line(format!(
                        "click-through {} — cursor=({x:.0},{y:.0}) rect=({:.0},{:.0} {w:.0}x{h:.0})",
                        if accept { "off: window takes the mouse" } else { "on: passes through" },
                        r.x, r.y, w = r.w, h = r.h,
                    ));
                    let _ = win.set_ignore_cursor_events(!accept);
                }

                let _ = win.emit("cursor", CursorPayload { x, y });
            }
        }
    });
}

pub fn set_ignore_cursor(app: &AppHandle, ignore: bool) {
    if let Some(win) = window(app) {
        let _ = win.set_ignore_cursor_events(ignore);
    }
}
