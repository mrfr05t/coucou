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

/// The directories a Linux distribution keeps its GStreamer plugins in — the
/// multiarch layouts first, then the plain one. Only those that exist are used,
/// so an AppImage built on one machine keeps working on another.
#[cfg(all(unix, not(target_os = "macos")))]
const SYSTEM_GSTREAMER_DIRS: [&str; 5] = [
    "/usr/lib/x86_64-linux-gnu/gstreamer-1.0",
    "/usr/lib/aarch64-linux-gnu/gstreamer-1.0",
    "/usr/lib/i386-linux-gnu/gstreamer-1.0",
    "/usr/lib64/gstreamer-1.0",
    "/usr/lib/gstreamer-1.0",
];

/// The system GStreamer plugin directories `exists` accepts, in the order
/// GStreamer should try them.
///
/// Split out so the choice can be tested without a filesystem and without
/// touching the process environment.
#[cfg(all(unix, not(target_os = "macos")))]
fn system_plugin_dirs(exists: impl Fn(&str) -> bool) -> Vec<String> {
    SYSTEM_GSTREAMER_DIRS
        .iter()
        .filter(|dir| exists(dir))
        .map(|dir| (*dir).to_string())
        .collect()
}

/// Windows has no AppImage and nothing to repair here.
#[cfg(windows)]
pub fn repair_appimage_gstreamer() {}

/// Points GStreamer at the system plugins when running from an AppImage.
///
/// The AppImage bundles GStreamer's core libraries but none of its plugins, and
/// its launcher sets `GST_PLUGIN_SYSTEM_PATH_1_0` to `$APPDIR/usr/lib/gstreamer-1.0`
/// — a directory that does not exist. That *replaces* the path GStreamer was
/// built with, so it registers no dynamic plugins at all: no `appsrc`, no
/// `decodebin`, no audio sink. WebKit's answer to a missing sink is an
/// `AudioContext` that never starts plus a `decodeAudioData` promise that never
/// settles — a silent island, with nothing in the log to say why.
///
/// `GST_PLUGIN_PATH_1_0` is additive, so the system directories go there and
/// anything an AppImage does ship of its own keeps working.
///
/// Must be called before Tauri builds anything: the web process inherits this
/// environment, and it is where GStreamer initializes.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn repair_appimage_gstreamer() {
    let Some(appdir) = std::env::var_os("APPDIR") else {
        return; // Not an AppImage.
    };
    if std::path::Path::new(&appdir)
        .join("usr/lib/gstreamer-1.0")
        .is_dir()
    {
        // This AppImage brought its own plugins; the paths are its business.
        return;
    }
    let mut dirs = system_plugin_dirs(|dir| std::path::Path::new(dir).is_dir());
    if let Some(existing) = std::env::var_os("GST_PLUGIN_PATH_1_0") {
        if !existing.is_empty() {
            dirs.push(existing.to_string_lossy().into_owned());
        }
    }
    if dirs.is_empty() {
        crate::log::line(
            "AppImage: no GStreamer plugin directory found — sound will not play".to_string(),
        );
        return;
    }
    crate::log::line(format!(
        "AppImage: pointing GStreamer at the system plugins ({})",
        dirs.join(":")
    ));
    std::env::set_var("GST_PLUGIN_PATH_1_0", dirs.join(":"));
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn only_the_plugin_directories_that_exist_are_offered() {
        // Distributions disagree about where they put these, so a directory
        // that is not there has to be skipped rather than pointed at — pointing
        // GStreamer at a path that does not exist is the whole bug this fixes.
        assert_eq!(
            system_plugin_dirs(|dir| dir == "/usr/lib/x86_64-linux-gnu/gstreamer-1.0"),
            vec!["/usr/lib/x86_64-linux-gnu/gstreamer-1.0".to_string()]
        );
        assert_eq!(
            system_plugin_dirs(|dir| dir.starts_with("/usr/lib6")),
            vec!["/usr/lib64/gstreamer-1.0".to_string()]
        );
    }

    #[test]
    fn a_machine_with_no_plugin_directory_offers_nothing() {
        // The caller turns this into a log line instead of an empty path, which
        // would otherwise look like "no plugins wanted" to GStreamer.
        assert!(system_plugin_dirs(|_| false).is_empty());
    }
}
