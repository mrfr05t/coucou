// Everything that differs between Windows and Linux lives behind this module.
//
// The island's design needs three things the operating system has to provide:
// where the pointer is, whether a button is held, and whether clicking the
// window may take focus. Windows answers all three with Win32; X11 answers all
// three with XQueryPointer + WM_HINTS; Wayland answers none of them, which is
// why the Linux build talks to X11 (XWayland) and reports `pointer_source =
// "dom"` when it cannot.
//
// Nothing else in the app may reach for a platform primitive directly — if a
// call needs `cfg`, it belongs in here.

#[cfg(windows)]
mod win32;
#[cfg(all(unix, not(target_os = "macos")))]
mod x11;

#[cfg(windows)]
use win32 as imp;
#[cfg(all(unix, not(target_os = "macos")))]
use x11 as imp;

/// As shown in the settings window and in the build's own docs.
#[cfg(windows)]
pub const PLATFORM: &str = "windows";
#[cfg(all(unix, not(target_os = "macos")))]
pub const PLATFORM: &str = "linux";

/// `"poll"` when the Rust side streams 60 Hz `cursor` events at the frontend,
/// `"dom"` when there is no global cursor query to be had and the webview has
/// to fall back to its own pointer events.
pub fn pointer_source() -> &'static str {
    imp::pointer_source()
}

/// Cursor position in physical screen coordinates.
pub fn cursor_physical() -> Option<(f64, f64)> {
    imp::cursor_physical()
}

/// True when the window's input region is shaped by the island's own rectangle
/// rather than flipped by the cursor poll.
///
/// That is the case on X11/XWayland, where the cursor read and the click-through
/// flag control each other (see `x11::set_input_rect`); Windows has a cursor
/// query that never freezes, so there the poll still decides.
pub fn shape_follows_rect() -> bool {
    imp::shape_follows_rect()
}

/// Sets the window's input region. `rect` is window-logical coordinates:
/// `Some` restricts input to the island, `None` gives the whole window the
/// mouse. A no-op where `shape_follows_rect()` is false.
pub fn set_input_rect(win: &tauri::WebviewWindow, rect: Option<(f64, f64, f64, f64)>) {
    imp::set_input_rect(win, rect)
}

/// True while the left button is held — the only hint we get that a drag is
/// in flight before it reaches the window.
pub fn left_button_down() -> bool {
    imp::left_button_down()
}

/// Keeps clicks from stealing focus from whatever the user was actually doing,
/// and keeps the window out of Alt-Tab.
pub fn make_non_activating(win: &tauri::WebviewWindow) {
    imp::make_non_activating(win)
}

/// Temporarily allows activation so a text field inside the island can be typed
/// in, then takes it away again.
pub fn set_activating(win: &tauri::WebviewWindow, activating: bool) {
    imp::set_activating(win, activating)
}

/// Windows only: lets dropped files reach the app. Elsewhere wry's own drag
/// handler is already the one Tauri listens to, so this is a no-op.
pub fn unblock_webview_drops(app: &tauri::AppHandle) {
    imp::unblock_webview_drops(app)
}

/// A stable identity for "this same user", used to check that the relay socket
/// we are about to write to really is ours: the SID on Windows, the uid on
/// Linux. Not needed to *name* the socket on Linux — `$XDG_RUNTIME_DIR` is
/// already per-uid — but it is the defence-in-depth equivalent of the SID that
/// Windows bakes into the pipe name.
pub fn current_user_key() -> Option<String> {
    imp::current_user_key()
}

/// Chooses the display backend, or reports that the platform already made the
/// choice. **Must be called before Tauri builds anything**, because GTK reads
/// `GDK_BACKEND` the first time it initializes and a decision made after that
/// point is ignored.
///
/// `true` means "the island will be able to place itself and read a global
/// cursor"; `false` means the degraded path.
///
/// Written as two cfg'd items rather than cfg'd blocks inside one body: a
/// `#[cfg]` on a trailing block is a statement, not an expression, and leaves
/// the function with no tail value on exactly one of the two platforms.
#[cfg(windows)]
pub fn prefer_x11() -> bool {
    // Windows is the platform the island was designed on; nothing to choose.
    true
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn prefer_x11() -> bool {
    imp::prefer_x11()
}
