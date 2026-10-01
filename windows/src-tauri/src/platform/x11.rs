// X11 specifics — the Tier 1 path. Under a Wayland session this runs against
// XWayland, which is how the island gets the three things Wayland refuses to
// give a normal client: a global cursor position, a global button state, and a
// window it can place at the top centre of a monitor.
//
// One XQueryPointer call replaces both GetCursorPos and GetAsyncKeyState: it
// returns the root position and the button mask together.
//
// x11rb is pure Rust and speaks the wire protocol itself, so building this
// needs no libx11-dev — only a live `:0` to talk to at runtime.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use tauri::{AppHandle, WebviewWindow};
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SO, SK};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClipOrdering, ConnectionExt as _, KeyButMask, Rectangle, Window,
};

/// Title of the island window. The settings window is `Settings — Coucou` and
/// the helper window is `coucou`, so this picks the island out of the three
/// this process owns.
const ISLAND_TITLE: &str = "Coucou";

// Reconnecting on every one of the 60 polls a second would be absurd, so each
// thread keeps one connection. `thread_local` rather than a shared mutex: the
// poll loop owns its thread, and `target_monitor` only ever calls from the
// command thread, so there is never contention to solve.
//
// (A plain comment, not a doc comment: rustdoc does not document macro
// invocations, so `///` here is a warning rather than documentation.)
thread_local! {
    static X: RefCell<Option<X11>> = const { RefCell::new(None) };
}

struct X11 {
    conn: x11rb::rust_connection::RustConnection,
    /// The root window whose coordinates are the ones the island needs: on X11
    /// there is one root for all monitors, so root coordinates are already
    /// screen-global and need no per-monitor offset — which is exactly what
    /// `GetCursorPos` gave us on Windows.
    root: Window,
    /// The island's own X window, resolved the first time a shape is applied.
    /// `0` means "not looked up yet"; XIDs are never reused while a window
    /// lives, and the island lives for the whole run.
    island: Cell<Window>,
    /// A rejected input-shape request is logged once, not once per frame.
    shape_warned: Cell<bool>,
}

impl X11 {
    fn open() -> Option<X11> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[screen_num].root;
        Some(X11 {
            conn,
            root,
            island: Cell::new(0),
            shape_warned: Cell::new(false),
        })
    }
}

/// With X, `QueryPointer` answers; without X there is no global pointer at all
/// and the webview has to drive `onCursor` from its own DOM events.
pub fn pointer_source() -> &'static str {
    if x_available() {
        "poll"
    } else {
        "dom"
    }
}

/// Can this process reach an X display at all? Cached, because it is asked for
/// once at boot and the answer does not change within a run.
fn x_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| X11::open().is_some())
}

/// Forces the GTK backend onto X11 when an X display is reachable.
///
/// This has to happen before Tauri initializes GTK, which is why it is a
/// function the caller invokes at the top of `run()` rather than something
/// read from the window config. Without it, a Wayland session gets the Wayland
/// backend, `set_position` becomes a no-op, and the island lands wherever the
/// compositor felt like putting it.
///
/// An explicit `GDK_BACKEND` in the environment always wins — that is the
/// escape hatch for someone who wants the degraded native-Wayland build.
///
/// Returns true when the app will be running on X11.
pub fn prefer_x11() -> bool {
    if std::env::var_os("GDK_BACKEND").is_some() {
        // The user already chose. Report which backend that choice implies.
        return std::env::var("GDK_BACKEND")
            .map(|v| v.eq_ignore_ascii_case("x11"))
            .unwrap_or(false);
    }
    if !x_available() {
        return false;
    }
    std::env::set_var("GDK_BACKEND", "x11");
    true
}

/// Runs `f` against this thread's connection, opening one on first use.
///
/// When `f` returns nothing the connection is dropped rather than kept. `f` can
/// only return `None` because the query failed — a pointer sitting outside the
/// root or a zeroed mask are still answers — so "nothing" is the signal that
/// the display went away (suspend, XWayland restart) and that the next call
/// should try a fresh one. A healthy connection is simply put back.
fn with_x<T>(f: impl FnOnce(&X11) -> Option<T>) -> Option<T> {
    X.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = X11::open();
        }
        let out = slot.as_ref().and_then(f);
        if out.is_none() {
            *slot = None;
        }
        out
    })
}

/// Cursor position in physical screen coordinates.
pub fn cursor_physical() -> Option<(f64, f64)> {
    with_x(|x| {
        let reply = x.conn.query_pointer(x.root).ok()?.reply().ok()?;
        Some((reply.root_x as f64, reply.root_y as f64))
    })
}

/// True while the left mouse button is held — same query as the position, so
/// the poll pays for both with one round trip.
pub fn left_button_down() -> bool {
    with_x(|x| {
        let reply = x.conn.query_pointer(x.root).ok()?.reply().ok()?;
        Some(reply.mask.contains(KeyButMask::BUTTON1))
    })
    .unwrap_or(false)
}

/// On X11 the window's input region is pinned to the island's own rectangle
/// instead of being derived from the cursor, so the poll loop must leave
/// `set_ignore_cursor_events` alone — see `set_input_rect`.
pub fn shape_follows_rect() -> bool {
    true
}

/// Gives the island window an input region shaped like the island itself.
///
/// `rect` is window-logical: `Some` restricts input to that rectangle, `None`
/// hands the whole window the mouse — which is what the collapsed wake strip
/// needs, because there the window *is* the strip.
///
/// This deliberately does not go through `WebviewWindow::set_ignore_cursor_events`.
/// That flag is computed from the cursor, and the cursor read only advances
/// while XWayland is being handed pointer events — which is precisely what the
/// flag controls. The two feed each other: pass-through on, so the pointer read
/// freezes outside the island, so `accept` stays false, so the island can never
/// be clicked again. Deriving the region from the rectangle the front end
/// already pushes breaks the loop because it needs no cursor at all.
///
/// An empty rectangle is meaningful rather than a failure: it is the island at
/// zero height, mid-animation, and it should not swallow a click while it is
/// still invisible.
pub fn set_input_rect(win: &WebviewWindow, rect: Option<(f64, f64, f64, f64)>) {
    let scale = win.scale_factor().unwrap_or(1.0);
    // XShape clips the region to the window itself, so a rectangle that runs
    // past an edge is harmless — and deliberately not clamped to `inner_size`,
    // which right after a resize still reports the *previous* size and would
    // otherwise cut the island's region down to the old window.
    let (x, y, w, h) = match rect {
        None => {
            // Whole window: the collapsed wake strip, where the window *is* the
            // strip. If the size is unavailable, leave the region alone rather
            // than guess one — a wrong guess here either swallows clicks or
            // makes the island untouchable.
            let Ok(inner) = win.inner_size() else { return };
            (0.0, 0.0, inner.width as f64, inner.height as f64)
        }
        Some((rx, ry, rw, rh)) => {
            let x0 = (rx * scale).floor().max(0.0);
            let y0 = (ry * scale).floor().max(0.0);
            let x1 = ((rx + rw) * scale).ceil();
            let y1 = ((ry + rh) * scale).ceil();
            (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
        }
    };
    let shape = Rectangle {
        x: x as i16,
        y: y as i16,
        width: w.min(u16::MAX as f64) as u16,
        height: h.min(u16::MAX as f64) as u16,
    };

    with_x(|x| {
        // Always `Some`: `with_x` reads `None` as "the display went away" and
        // drops the connection, and a window we cannot find is not that.
        if shape_supported(x) {
            if let Some(id) = island_xid(x) {
                let sent = match x.conn.shape_rectangles(
                    SO::SET,
                    SK::INPUT,
                    ClipOrdering::UNSORTED,
                    id,
                    0,
                    0,
                    std::slice::from_ref(&shape),
                ) {
                    Ok(cookie) => cookie.check().is_ok(),
                    Err(_) => false,
                };
                if !sent && !x.shape_warned.replace(true) {
                    crate::log::line(
                        "input region refused — the island will take the whole window".to_string(),
                    );
                }
            }
        }
        Some(())
    });
}

/// Asked once per connection: XWayland ships SHAPE, but a bare X server might not.
fn shape_supported(x: &X11) -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        x.conn
            .extension_information(x11rb::protocol::shape::X11_EXTENSION_NAME)
            .ok()
            .flatten()
            .is_some()
    })
}

/// This process's island window, found by pid and exact title.
///
/// Both windows we own share `WM_CLASS`, so the title is what separates the
/// island from the settings window. The lookup happens once and is then cached
/// for the life of the connection, so a title that later changes with the
/// island's content cannot lose us the window.
fn island_xid(x: &X11) -> Option<Window> {
    let cached = x.island.get();
    if cached != 0 {
        return Some(cached);
    }
    let pid_atom = intern(x, b"_NET_WM_PID")?;
    let name_atom = intern(x, b"_NET_WM_NAME")?;
    let utf8 = intern(x, b"UTF8_STRING")?;
    let pid = std::process::id();

    let tree = x.conn.query_tree(x.root).ok()?.reply().ok()?;
    for &w in tree.children.iter() {
        if property_u32(x, w, pid_atom) != Some(pid) {
            continue;
        }
        // `_NET_WM_NAME` is UTF-8; `WM_NAME` is the Latin-1 fallback for a
        // window manager that has not set the modern one.
        let title = property_bytes(x, w, name_atom, utf8)
            .or_else(|| property_bytes(x, w, AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()));
        let title = title.map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string());
        if title.as_deref() != Some(ISLAND_TITLE) {
            continue;
        }
        x.island.set(w);
        return Some(w);
    }
    None
}

fn intern(x: &X11, name: &[u8]) -> Option<Atom> {
    Some(x.conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
}

fn property_u32(x: &X11, w: Window, property: Atom) -> Option<u32> {
    let reply = x
        .conn
        .get_property(false, w, property, AtomEnum::CARDINAL, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    // Bound first: the iterator borrows `reply`, which must outlive it.
    let value = reply.value32()?.next();
    value
}

fn property_bytes(x: &X11, w: Window, property: Atom, kind: Atom) -> Option<Vec<u8>> {
    let reply = x.conn.get_property(false, w, property, kind, 0, 256).ok()?.reply().ok()?;
    let bytes: Vec<u8> = reply.value8()?.collect();
    Some(bytes)
}

/// Keeps clicks from stealing focus from whatever the user was actually doing.
///
/// The X11 equivalent of WS_EX_NOACTIVATE is "tell the window manager this
/// window does not want keyboard focus" — `set_focusable(false)` — which is
/// also what keeps it out of Alt-Tab alongside `skipTaskbar`. Note that this
/// is a *request*; a compositor is free to ignore it, so verify on the target
/// desktop (plan §12, R4).
pub fn make_non_activating(win: &WebviewWindow) {
    let _ = win.set_focusable(false);
    let _ = win.set_skip_taskbar(true);
    let _ = win.set_always_on_top(true);
}

/// Temporarily allow activation so a text field inside the island can be typed
/// in, then take it away again.
pub fn set_activating(win: &WebviewWindow, activating: bool) {
    let _ = win.set_focusable(!activating);
}

/// wry's GTK backend installs its drag handler on the webview it creates, and
/// GTK has no second inner window claiming the target the way WebView2 does —
/// so there is nothing to revoke. Kept as a real function (rather than cfg'ing
/// the call site) so the island code stays platform-free.
pub fn unblock_webview_drops(_app: &AppHandle) {}

/// The numeric uid of this process, as a string. Used to check the peer of the
/// relay socket rather than to name it — `$XDG_RUNTIME_DIR` is already per-uid.
pub fn current_user_key() -> Option<String> {
    // `/proc/self` is owned by the process it names, so its uid is our euid.
    // Avoids pulling in libc for one call.
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_key_is_numeric() {
        let key = current_user_key().expect("/proc/self should be readable");
        assert!(key.parse::<u32>().is_ok(), "uid should be numeric, got {key}");
    }

    #[test]
    fn pointer_source_is_a_known_value() {
        assert!(matches!(pointer_source(), "poll" | "dom"));
    }
}
