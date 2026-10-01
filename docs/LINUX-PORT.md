# Coucou for Linux — porting plan

**Status:** implemented on Ubuntu 26.04 / GNOME Shell 50 / Wayland (XWayland).
Phases 0–4 are done and evidenced; Phase 5 is partly done — §10 items marked
`[x]` were verified live, the rest need a human hand (a real pointer and a real
hook run) or a clean CI runner. This document keeps the original plan text and
records what actually happened underneath it, so the reasoning survives.

**Scope:** take `windows/` (the Tauri 2 port) and make it build and run on Linux.
The macOS app (`NotchBuddy/`) is **not** in scope — it is AppKit + SwiftUI
(`NSPanel`, `NSScreen.notch`, Accessibility, AppleScript) and has no Linux
equivalent. Porting it would be a GTK rewrite of ~10k lines, not a port.

**Why `windows/` and not `NotchBuddy/`:** the Windows port is framework-free
TypeScript + Canvas 2D on the front, and a Rust backend whose Win32 usage is
concentrated in a handful of files rather than spread thin. Tauri 2 itself
supports Linux (GTK + WebKitGTK). Roughly 85 % of `windows/` needs no change at
all.

---

## 0. Verdict in one paragraph

Feasible. The wire protocol, the frontend, and most of the backend port
directly. Three things need real design work: the **relay IPC** (named pipe →
Unix socket), the **pointer/windowing layer** (Win32 global cursor polling has
no Wayland equivalent), and **packaging/CI**. There is also a silent foot-gun in
`keyring`: without an explicit Linux feature it falls back to an in-memory mock
store and API keys silently evaporate.

The pointer layer is the one genuine architectural decision, and it is argued in
§2. The recommendation is **X11/XWayland first**, because on this machine's
desktop (GNOME 50 / Wayland) the Wayland path cannot do three of the things the
island depends on — and two of them are unfixable without a compositor
protocol GNOME does not implement.

---

## 1. What ports unchanged

Inventory, so the change list in §3 can be read against it.

| Area | Files | Status |
|---|---|---|
| Frontend | `windows/src/**` (all `.ts`), `index.html`, `settings.html`, `src/style.css` | unchanged, except §6 |
| Mochi engine | `src/mochi/engine.ts` | unchanged (Canvas 2D) |
| State machine | `src/island/fsm.ts`, `src/core/state.ts`, `src/core/layout.ts` | unchanged |
| Views | `src/views/*` | unchanged |
| Chat | `src/core/chat.ts` | unchanged |
| Sound | `src/core/sound.ts` | unchanged (verify autoplay, §12) |
| Tauri commands | `claude.rs`, `integrations.rs`, `files.rs`, `tray.rs` | unchanged |
| Hook merge logic | `hooks.rs` — `merged()`, `entry_is_ours()`, the LCS diff | unchanged (only its path/time helpers) |
| Settings schema | `settings.rs` — `Settings`, `load`, `save` | unchanged (only its dirs) |
| Wire protocol | one newline-terminated JSON object per connection; `allow`/`deny` written back; 800 ms ACK + 108 s decision waits; hook budgets 300 ms / 2 s / 110 s | **unchanged — this is the invariant** |
| Build tooling | `vite`, `package.json`, `tsconfig`, `pack.mjs` (partially, §7) | mostly unchanged |

---

## 2. The one real design decision: pointer, click-through, positioning

This section is the heart of the plan. Everything else is mechanical.

### 2.1 What the Windows build actually does

`island::spawn_cursor_poll` (`src-tauri/src/island.rs:275`) runs a 60 Hz loop
and does three things with **global** Win32 state:

1. `cursor_physical()` (`island.rs:117`) — `GetCursorPos` → global cursor in
   physical screen coords.
2. `left_button_down()` (`island.rs:158`) — `GetAsyncKeyState(VK_LBUTTON)` →
   is a drag probably in flight.
3. `set_ignore_cursor()` (`island.rs:364`) — `WS_EX_TRANSPARENT`-style
   click-through, toggled on/off depending on whether the cursor is inside the
   island rect (`gate.rect`, pushed by the frontend at `island.ts:491` via
   `set_island_rect`).

Plus `make_non_activating` / `set_activating` (`island.rs:237`, `:247`) which
toggle `WS_EX_NOACTIVATE` so clicks never steal focus from the terminal, and
`unblock_webview_drops` (`island.rs:134`) which is a WebView2/OLE-specific hack.

The frontend consumes this as a `cursor` event → `onCursor(x, y)`
(`island.ts:562`), which drives gaze (Mochi's eyes), `mouseInIsland`, and the
FSM's `mouseEntered`/`mouseLeft`.

### 2.2 What each backend gives us

**Windows:** everything above, natively.

**X11:** everything above, natively. `XQueryPointer` returns root-window
position *and* the button mask in one call — a direct 1:1 replacement for
`GetCursorPos` + `GetAsyncKeyState`. Input shape is the X Shape extension, but
the region is taken from the island rectangle rather than from the cursor — see
§2.6, which is the one place this port deliberately departs from the Windows
design. Window placement is absolute. Focus control is `WM_HINTS` /
`set_focusable`.

**Wayland:** three hard walls.

| Need | Wayland reality |
|---|---|
| Global cursor position | **No protocol.** tao hard-codes it: `util::cursor_position(is_wayland)` returns `Ok((0,0))` when `is_wayland` is true (verified in tao `src/platform_impl/linux/util.rs`). A normal client is told the pointer position only while it is over its own surface. |
| Global left-button state | **No protocol.** Same restriction. |
| Place a window at top-centre of a monitor | **No protocol.** `xdg_toplevel` has no "move to (x, y)" request; `gtk_window_move` is a no-op. The only standard way to pin a surface to a screen edge is `wlr-layer-shell`, which **GNOME does not implement**. |

So on Wayland the island cannot be positioned, cannot see the cursor, and
cannot know when a drag starts. Layer-shell would solve positioning but is a
wlroots-compositors-only protocol (Sway, Hyprland, KDE's KWin implements it too
— but not GNOME).

### 2.3 Measurements taken on this machine

The dev machine is `ubuntu:GNOME`, `XDG_SESSION_TYPE=wayland`, GNOME Shell 50.1.
Rather than reason about this abstractly, three probes were run against the live
session using PyGObject/GTK 3.24 — throwaway scripts written for the purpose and
deleted afterwards, since they only print numbers. The numbers are kept below;
the probes themselves are reproducible from the commands shown.

**X11 backend (`GDK_BACKEND=x11`, i.e. XWayland at `:0`):**

```
n_monitors = 1   [0,0 6144x3456]  primary
pointer    = (4122, 1159)         -- real, matches an independent XQueryPointer call
w.move(2712, 0) -> get_position() = (2712, 0)   -- top-centre placement honoured
```

**Native Wayland backend:**

```
n_monitors = 0   (still 0 after 4 s in the main loop)
primary    = None
pointer    = (0, 0)               -- tao's stub, confirmed
window pos = (0, 0)               -- w.move() had no effect
```

Ground truth from the compositor itself:

```
$ wayland-info | grep wl_output
(nothing — 42 globals advertised, no wl_output among them)
$ gdbus call --session --dest org.gnome.Mutter.DisplayConfig ... GetCurrentState
(uint32 2, @a(...) [], @a(...) [], {...})   -- both monitor arrays empty
$ xrandr --query
Screen 0: current 6144 x 3456    -- no outputs listed
```

**Interpretation, stated honestly:** a Wayland compositor with *no* `wl_output`
global is abnormal — every real GNOME session advertises outputs. This
particular session is in a degraded or unusual state (possibly a virtual or
remote display; the cause was not determined). It is **not** proof that
"GTK cannot enumerate monitors on Wayland" in general.

But it does not change the recommendation, because the *other* two walls —
no global cursor, no client-side positioning — are properties of the Wayland
protocol itself, not of this machine. This session merely adds a third reason to
lead with X11.

Also verified on this machine: XWayland is running at `:0` with the `SHAPE`
extension present; `libX11.so.6`, `libXext.so.6`, `libgtk-3.so.0`,
`libwebkit2gtk-4.1.so.0` are installed.

### 2.4 Recommendation

**Tier 1 — X11 / XWayland, full parity. Port the Windows architecture 1:1.**

Force the GTK backend with `GDK_BACKEND=x11` on Wayland sessions (XWayland is
present and rootless on Ubuntu by default; confirmed running here). On native
X11 sessions it is the natural backend anyway. This gets:

- global cursor → `XQueryPointer` (position + button mask, one round trip)
- click-through → X Shape, region driven by the island rectangle (§2.6)
- top-centre placement → `apply_geometry` unchanged
- non-activating window → `set_focusable(false)`

**Tier 2 — native Wayland, degraded. Not a launch requirement.** Documented as
a known-degradation path: island runs, hooks/IPC/chat/secrets all work, but
gaze only tracks the pointer while it is over the island and the window's
position is compositor-chosen. Needs layer-shell or a GNOME extension for real
parity; out of scope for the first port.

### 2.5 One architectural improvement worth taking

While porting, make the **wake path event-driven instead of poll-driven.**

Today the collapsed island is a 240×6 strip that is deliberately *not*
click-through (`set_collapsed` calls `set_ignore_cursor(&app, false)`), so the
poll sees the cursor enter the strip and wakes the island. On X11 a stale
`XQueryPointer` result is possible when the pointer is over a native Wayland
window — but the strip is a real X surface, so a pointer over it generates
genuine enter/motion events regardless of how stale the global query is.

So: feed `onCursor` from GDK enter/motion events *while collapsed*, and keep the
global poll for gaze and for the click-through decision while expanded. This
makes wake-up correct even where the global query is not, and it is the same
mechanism the frontend already uses outside Tauri (`island.ts:556`). Low risk,
and it leaves Tier 2 a smaller step later.

> **What was implemented instead.** The poll is *parked* while collapsed rather
> than fed by GDK events: `set_collapsed(true)` sets `cursor_poll_parked`, so
> the hidden island polls at zero Hz as §2.5's "zero polling" comment promises,
> and wake comes from the DOM `mouseenter` on `#wake-strip` — which is
> registered unconditionally (`island.ts:529`, not gated on `pointerSource`) and
> is sized to exactly fill the collapsed 240×6 viewport
> (`style.css:56`: `position:fixed; top:0; left:50%; translateX(-50%);
> width:240px; height:6px`). The Rust side's job while collapsed is therefore
> only to make the strip *receive* clicks: `set_collapsed` shrinks the window and
> gives the whole window the input region.

### 2.6 Why the input region is the island's rectangle, not the cursor

On Windows the poll computes `accept = cursor inside gate.rect` and flips
`WS_EX_TRANSPARENT` to match. On XWayland that loop can deadlock, because the
two halves feed each other:

- `XQueryPointer` only advances while XWayland is being handed pointer events,
  and what decides that is precisely the input shape.
- With the shape in its "outside" state (a 2×2 sliver), the pointer read stops
  moving.
- A read frozen outside the rect keeps `accept` false forever, so the shape
  never grows, so the read never advances.

Measured here: during a card the input shape sat at `(0,0) 2x2` for **108 s
straight** while the cursor read was frozen at `(4055,1048)` — outside the
`640x160` approval rect. Every click passed through to the window underneath
and the card could not be clicked at all. The read was not broken everywhere:
it tracked fine over the Settings window (a large X surface) and over the
island's *lower* half — which is exactly why the island visibly reacted when the
pointer was over Settings but not when it was over the card.

So on Linux the shape does not consult the cursor:

- collapsed → the whole window takes the mouse (the window *is* the 240x6 strip);
- expanded → exactly `gate.rect`, which the frontend already pushes on every
  geometry change (`applyGeometry` → `set_island_rect`).

Implemented by `platform::x11::set_input_rect` (SHAPE, `ShapeSet`/`Input`)
applied from `island::sync_input_shape`, which `apply_geometry` calls after
every placement and `set_island_rect` calls on every push.
`platform::shape_follows_rect()` reports whether this is the case, and the poll
skips `set_ignore_cursor_events` entirely when it is — there is no flag to flip.
The poll still logs one line per inside/outside transition, because that cursor
read is the one remaining thing here that is hard to trust.

Two details worth keeping:

- The region is **not** clamped to `win.inner_size()`. Right after a resize GTK
  still reports the *previous* size, and clamping against it cut the approval
  region down to the old window. XShape clips the region to the window itself,
  so overshooting is harmless — which is also why the collapsed shape reads back
  as `1440x640` on a 480x12 window and still covers all of it.
- An empty rectangle (island at zero height, mid-animation) is meaningful: it
  means "takes nothing", so an invisible island never swallows a click.

Verified live: with a card up `xshape` reports input shape `(80,0) 1280x320`
(= `640x160` logical at `(40,0)`, exactly `islandSize("expanded","approval")`);
collapsed it reports the whole strip; and two `PermissionRequest` cards were
answered `allow` by clicking the pill (`ui decide allow` →
`hook id=… answered allow`).

---

## 3. File-by-file change list

Exact files, exact functions, exact cfg gates. This is the work order.

### 3.1 `windows/Cargo.toml` (workspace)

No structural change. Optionally add `[target.'cfg(target_os = "linux")'.workspace.dependencies]`
if §8 decides to share a dependency. `members = ["src-tauri", "hook"]` stays.

### 3.2 `windows/src-tauri/Cargo.toml`

```toml
# remove from [dependencies]:
#   windows = { version = "0.61", features = [...] }
#   keyring = { version = "3", features = ["windows-native"] }

[target.'cfg(windows)'.dependencies]
windows = { version = "0.61", features = [ /* as today */ ] }
keyring = { version = "3", features = ["windows-native"] }

[target.'cfg(target_os = "linux")'.dependencies]
keyring = { version = "3", features = ["sync-secret-service"], default-features = false }
x11rb = "0.13"          # §8, replaces GetCursorPos + GetAsyncKeyState
libc = "0.2"            # only if needed for geteuid fallback; prefer keyring/x11rb alone
```

The `keyring` split is **not optional**. With only `windows-native` declared,
`keyring` 3.6.3 on Linux compiles with no credential-store feature and silently
uses its **mock** store — `secrets::set` would return `Ok`, `present` would
return false on next launch, and the user's API key would be gone with no error
anywhere. Verified against the crate docs: "If no features are enabled … the
mock credential store is used."

Feature name must be confirmed against keyring 3.6.3 at implementation time
(`linux-native` vs `sync-secret-service` vs `linux-native-sync-persistent`) —
see §12.

> **Resolved at implementation: `sync-secret-service`.** "It compiles" would not
> have been enough. keyring's default-store cascade (`src/lib.rs`) selects the
> kernel keyring when only `linux-native` is set — `keyutils.rs` writes to
> `KeyRingIdentifier::Session`, so the key dies with the login session. Selecting
> `org.freedesktop.secrets` requires a `*-secret-service` feature, which in turn
> needs `libdbus-1-dev` at build time. Verified end to end: a `set` in one
> process and a `get` in a fresh process round-trips, which a mock store cannot
> do (`NoEntry`). See §12 R3.

### 3.3 `src-tauri/src/lib.rs`

| Line | Today | Change |
|---|---|---|
| `15` | `use std::os::windows::process::CommandExt;` | `#[cfg(windows)] use std::os::windows::process::CommandExt;` |
| `32` | `const CREATE_NO_WINDOW: u32 = 0x0800_0000;` | `#[cfg(windows)] const CREATE_NO_WINDOW: u32 = 0x0800_0000;` |
| `125–135` | `open_url` → `rundll32.exe shell32.dll,ShellExecution_RunW` | cfg branch: `#[cfg(unix)]` → `Command::new("xdg-open").arg(url).spawn()` |
| `138–155` | `open_in_vscode` → `creation_flags(...)` + `explorer` fallback | cfg the `.creation_flags()` call away; `#[cfg(unix)]` fallback → `Command::new("xdg-open").arg(path)` |
| `158–174` | `find_on_path` walks `%PATH%` × `%PATHEXT%` | `#[cfg(unix)]` variant: walk `$PATH`, test `path.is_file()` + exec bit, no extension list |
| `29–46` | `BootInfo { settings, screen, version, hook_path }` | add `platform: String` (`"windows"` / `"linux"`) and `pointer_source: String` (`"poll"` / `"dom"`) — consumed by §6 |
| `run()` setup | calls `island::make_non_activating(&win)` after creating windows | unchanged call site; the function body becomes cfg'd (§3.5) |

`Command::new` usage must be audited: `CREATE_NO_WINDOW` is Windows-only and
every `.creation_flags(CREATE_NO_WINDOW)` needs `#[cfg(windows)]` on the
statement. There are 4 occurrences in `lib.rs` (lines 32, 131, 148) plus 1 in
`hooks.rs`.

### 3.4 `src-tauri/src/island.rs` (368 lines) — split into portable + platform

**Stays in `island.rs`, no cfg needed** (portable):

- consts `PANEL_W`, `PANEL_H`, `STRIP_W`, `STRIP_H`, `WINDOW_LABEL`, `HIT_MARGIN`
- `CursorPayload`, `ScreenInfo`, `IslandRect`, `PollGate` + `impl PollGate`
- `window()` (`:113`)
- `monitor_contains()` (`:162`)
- `target_monitor()` (`:172`) — calls `platform::cursor_physical()`
- `screen_info()` (`:187`)
- `apply_geometry()` (`:206`) — `set_size` / `set_position` / `set_always_on_top`
  are all Tauri APIs with Linux implementations
- `current_screen_key()` (`:262`)
- `spawn_cursor_poll()` (`:275`) — structure unchanged; the two Win32 calls it
  makes become `platform::…` calls
- `set_ignore_cursor()` (`:364`) — calls Tauri's `set_ignore_cursor_events`,
  which tao implements on Linux via `input_shape_combine_region` (verified in
  tao `platform_impl/linux/event_loop.rs`, `WindowRequest::CursorIgnoreEvents`)

**Moves to a new `src-tauri/src/platform/win32.rs`:**

- `cursor_physical()` (`:117`) — `GetCursorPos`
- `left_button_down()` (`:158`) — `GetAsyncKeyState`
- `hwnd_of()` (`:227`)
- `revoke_render_widget()` (`:144`) and all of `unblock_webview_drops` (`:134`)
- `make_non_activating()` (`:237`) — `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`
- `set_activating()` (`:247`)
- the `windows::…` imports at lines 15–24

**New `src-tauri/src/platform/x11.rs`:**

- `cursor_physical()` — `x11rb` `QueryPointer` on the root window, returns
  `(root_x, root_y)`. One connection opened lazily, reused.
- `left_button_down()` — same call's `mask` field, `Button1Mask`.
- `make_non_activating()` / `set_activating()` — map to Tauri
  `win.set_focusable(false|true)`; add `win.set_skip_taskbar(true)` (Tauri's
  config `skipTaskbar` is documented as working on Windows *and* Linux).
- `unblock_webview_drops()` — **no-op.** The WebView2/OLE hack has no
  equivalent: wry's GTK backend installs its own drag handler
  (`wry/src/webkitgtk/drag_drop.rs`, verified) which *does* deliver positions in
  `DragDropEvent::Enter/Over/Drop` — better than Windows, which loses them.

**New `src-tauri/src/platform/mod.rs`:**

```rust
#[cfg(windows)] mod win32;
#[cfg(all(unix, not(target_os = "macos")))] mod x11;

#[cfg(windows)] pub use win32::*;
#[cfg(all(unix, not(target_os = "macos")))] pub use x11::*;
```

Lighter alternative, if a new directory feels heavy: keep everything in
`island.rs` and put `#[cfg(...)]` on the function bodies. Functionally
equivalent; the module split is recommended only because `island.rs` is already
368 lines and ~40 % of it is Win32.

**`PollGate` behaviour is unchanged** — `is_active`, `wait_until_active`,
`set_active`, `set_rect`, `forget_ignore_state`, `collapsed` all stay.

### 3.5 `src-tauri/src/win_user.rs` (48 lines)

- `#[cfg(windows)]` the whole file (it is only used by `pipe_name()`).
- New `src-tauri/src/platform/x11.rs` (or a small `unix_user` fn) provides
  `current_user_key() -> Option<String>` returning the uid as a string.
  Prefer `libc::geteuid()`; a zero-dependency fallback is
  `std::fs::metadata("/proc/self")` uid, valid on Linux procfs.
- Semantics change: on Windows the SID is embedded in the *pipe name* to keep
  two accounts apart in a machine-wide namespace. On Linux the socket lives in
  `$XDG_RUNTIME_DIR`, which is already per-uid and `0700` — so the uid is used
  for the **peer credential check**, not the path (§4).

### 3.6 `src-tauri/src/log.rs` (28 lines)

- Line 6 `use windows::…GetLocalTime;` and line 11 `let t = unsafe { GetLocalTime() };`
  → `#[cfg(windows)]`, plus a `#[cfg(unix)]` implementation.
- Recommend one shared helper `stamp_now() -> (u16,u16,u16,u16,u16,u16)` used by
  both `log.rs:11` and `hooks.rs:200` (`stamp()`), so the two timestamp formats
  cannot drift.
- **Decision needed (§12):** `GetLocalTime` is *local* time. `SystemTime` is
  UTC. Options: (a) add `chrono` with `clock` and use `Local::now()` — one well-
  known dep, correct local time; (b) use `time` with `local-offset`; (c) ship
  UTC on Linux and document it. Recommendation: **(a) `chrono`**, because
  `settings.json.bak-YYYYMMDD-HHMMSS` backups are user-facing and a UTC
  timestamp on a local backup is a support question waiting to happen.

### 3.7 `src-tauri/src/hooks.rs` (565 lines)

Mostly portable — `merged()`, `entry_is_ours()`, the LCS diff at `:364`, and
the settings.json merge are pure Rust. Changes:

| Location | Today | Change |
|---|---|---|
| `:17`, `:200` | `use windows::…GetLocalTime`, `stamp()` | cfg branch → shared `stamp_now()` (§3.6) |
| `:61–68` `home()` | `std::env::var_os("USERPROFILE")` else `PathBuf::from(".")` | `#[cfg(unix)]`: `var_os("HOME")`, fall back to `dirs`-free `/etc/passwd` lookup or `.` |
| `:114–116` `hook_command()` | `hook_exe_path().to_string_lossy().replace('\\', "/")` then `format!("\"{exe}\" {event}")` | keep the quoting (paths may contain spaces); drop the `replace('\\', "/")` under cfg — harmless either way, but make it explicit |
| `:115` `hook_exe_path` call | resolves `…\bin\coucou-hook.exe` | resolves `…/bin/coucou-hook` (§3.8) |
| `ensure_hook_exe()` candidate paths | references `coucou-hook.exe` | cfg the filename; the `resolve("coucou-hook.exe")` resource lookup becomes `resolve("coucou-hook")` |
| tests at `:515–521` | `std::env::set_var("USERPROFILE", &tmp)` | `#[cfg(unix)]` branch sets `HOME` instead; keep the Windows branch |

`hook_command` output is written into `~/.claude/settings.json` and executed by
Claude Code through a shell, so on Linux it becomes e.g.
`"/home/you/.local/share/Coucou/bin/coucou-hook" PermissionRequest`. The
existing `MARKER` and `entry_is_ours()` logic are untouched — hook install,
preview, uninstall and the LCS diff all work as-is.

### 3.8 `src-tauri/src/settings.rs` (87 lines)

```rust
// :51 config_dir()   today: APPDATA\Coucou      → #[cfg(unix)] $XDG_CONFIG_HOME/Coucou  (default ~/.config/Coucou)
// :59 local_dir()     today: LOCALAPPDATA\Coucou → #[cfg(unix)] $XDG_STATE_HOME/Coucou   (default ~/.local/state/Coucou)
// :66 hook_exe_path() today: local_dir()/bin/coucou-hook.exe
//                            → #[cfg(unix)] local_dir()/bin/coucou-hook
```

Mirror the existing style: explicit `std::env::var_os` with a hardcoded
fallback, **no new `dirs` crate**. Keep the `Coucou` directory name (not the
identifier) so it reads the same as on Windows. Document the choice in the
comment as the Windows file does today.

Alternative under discussion: use Tauri's `app.path().app_config_dir()` which
returns `~/.config/fr.louisraille.coucou`. Rejected — it would introduce a
second config location for no benefit and diverge from the Windows naming.

`Settings` struct, `load()`, `save()`, `default_model()` — unchanged.

### 3.9 `src-tauri/src/files.rs`

No code change. `inbox_dir()` (`:22`) is `settings::local_dir().join("inbox")`,
so it inherits XDG automatically. Comments at `:1` and `:3` mentioning
`%LOCALAPPDATA%` get a `#[cfg]`-neutral reword. The 7-day sweep and both tests
are portable.

### 3.10 `src-tauri/src/secrets.rs`

No code change — `keyring::Entry` API is identical. Only the Cargo feature
split (§3.2), plus a comment correcting "Windows Credential Manager" → "the
system keyring". If no Secret Service daemon is running,
`Entry::get_password()` returns an error and `get()` already returns `None`
(`.ok().filter(...)`), so `present()` degrades to `false` and the chat view
shows its existing "no key" state. That is acceptable, but §6 must make the
message accurate.

### 3.11 `src-tauri/src/pipe.rs` (227 lines) — the relay

Full redesign of the transport, zero change to the protocol. See §4.

### 3.12 `src-tauri/src/tray.rs`

No change expected. Tauri's `tray-icon` feature (already enabled) supports
Linux via libappindicator/GTKStatusIcon. Build dependency and GNOME visibility
caveats in §7 and §12.

### 3.13 `hook/` crate (the `coucou-hook` relay)

`hook/Cargo.toml` **already** uses target-gated dependencies — the pattern the
rest of the repo should follow.

| File | Change |
|---|---|
| `hook/Cargo.toml` | add `[target.'cfg(target_os = "linux")'.dependencies]` if a helper crate is wanted; otherwise no change |
| `hook/src/main.rs:40` | `mod win;` → `#[cfg(windows)] mod win;` + `#[cfg(all(unix, not(target_os = "macos")))] mod unix;` |
| `hook/src/main.rs:45` | `pipe_path()` → `#[cfg(unix)]` returns the Unix socket path (§4); must match `pipe_name()` exactly, as today |
| `hook/src/main.rs:53` | `connect()` → `#[cfg(unix)] UnixStream::connect(path)`; map `ENOENT` and `ECONNREFUSED` to "Coucou is not running" → exit 0 with empty stdout, exactly as the `ERROR_PIPE_BUSY`/missing-pipe branch does today |
| `hook/src/main.rs:74` `main()` | deadline structure, `talk()` (`:202`), `decision_json()` (`:103`), `read_event()` (`:117`), `truncate_strings()` — **unchanged** |
| budgets | `CONNECT_TIMEOUT` 300 ms, `FIRE_AND_FORGET_BUDGET` 2 s, `DECISION_BUDGET` 110 s — **unchanged** |
| `hook/src/win.rs` | `#[cfg(windows)]`; `current_user_sid()`, `pipe_server_is_same_user()` |
| `hook/src/unix.rs` **new** | `current_user_key()` (uid) and `server_is_same_user(&UnixStream)` via `SO_PEERCRED` — gives server `uid` + `pid`; compare `uid` to our euid. Optionally confirm `/proc/<pid>/exe` is `coucou`, mirroring `GetNamedPipeServerProcessId` |

The "never block Claude Code" rule (`CLAUDE.md`) survives the port unchanged:
missing socket → exit 0; deadline enforced by the main thread; no answer →
empty stdout → terminal asks normally.

### 3.14 `src-tauri/tauri.conf.json`

**Unchanged** for Windows. Linux config goes in a new
`src-tauri/tauri.linux.conf.json`, merged by Tauri's config merge (verified in
the Tauri 2 config reference): `tauri build --config tauri.linux.conf.json`.

---

## 4. IPC: named pipe → Unix socket

**Protocol invariant (do not touch):** one newline-terminated JSON object per
connection; only `PermissionRequest` keeps the connection open and waits for a
bare `allow\n` / `deny\n`; everything else is fire-and-forget. Timeouts: ACK
800 ms, decision 108 s (`pipe.rs:158`), hook-side 300 ms / 2 s / 110 s.
`PermissionRequest` with no decision writes nothing → terminal takes over.

### 4.1 Socket path

```
$XDG_RUNTIME_DIR/coucou.sock          # e.g. /run/user/1000/coucou.sock
fallback: /tmp/coucou-<uid>/coucou.sock   (dir 0700)
```

`$XDG_RUNTIME_DIR` is per-user, `0700`, and cleared at logout — so a stale
socket cannot outlive the session, which is better than Windows' situation.
Path length must stay under 108 bytes (`sun_path`); both are far below.

**Both sides must compute the identical path.** Today `pipe.rs::pipe_name()`
and `hook/src/main.rs::pipe_path()` are deliberately duplicated with a comment
saying they must match. Keep that structure (a shared crate would be cleaner
but adds a workspace member for ~10 lines), and add a `#[cfg(test)]` in *both*
files asserting the exact format so a change to one without the other fails CI.

The SID-in-the-name defence becomes unnecessary: `$XDG_RUNTIME_DIR` is already
uid-scoped. The defence-in-depth equivalent is the peer check below.

### 4.2 Server — `pipe.rs`

| Today | Linux |
|---|---|
| `tokio::net::windows::named_pipe::NamedPipeServer` | `tokio::net::UnixStream` (same `AsyncRead`/`AsyncWrite`, so `handle()` body is nearly unchanged) |
| `ServerOptions::new().first_pipe_instance(true)` | `UnixListener::bind` after an unlink-if-stale dance (below) |
| `server.connect().await` | `listener.accept().await` |
| `pipe.disconnect()` | drop the stream |
| "refuse to join a pipe somebody else owns" | bind failure with `EADDRINUSE` → try `UnixStream::connect`: if it succeeds, another instance owns the socket → log and exit; if `ECONNREFUSED`, it is stale → unlink and retry bind |

`handle()` (`:98`), `wait_for_decision()` (`:158`), `send()` (`:193`),
`acknowledge()` (`:208`), `decline()` (`:213`), `answer()` (`:220`) keep their
bodies. Only the stream type and the disconnect call change.

Peer check on accept: `stream.peer_cred()` (Unix, gives `uid`/`pid`) — compare
`uid` to our euid; mismatch → drop the connection. This is *stronger* than
Windows, which only checks the server from the client side.

### 4.3 Client — `hook/src/main.rs::connect()`

```rust
#[cfg(unix)]
fn connect() -> Option<UnixStream> {
    let path = pipe_path();
    match UnixStream::connect(&path) {
        Ok(s) => { /* peer_cred() uid check; None → treat as not running */ Some(s) }
        Err(e) if e.kind() == ErrorKind::NotFound => None,   // no Coucou → exit 0
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => None, // stale → exit 0
        Err(_) => None,
    }
}
```

`ERROR_PIPE_BUSY` (231) retry loop disappears — `connect(2)` on a Unix socket
does not have that failure mode. Keep the 300 ms overall budget as a guard.

---

## 5. Paths, time, secrets, autostart — summary

| Concern | Windows | Linux |
|---|---|---|
| Config | `%APPDATA%\Coucou\settings.json` | `$XDG_CONFIG_HOME/Coucou/settings.json` |
| State, log, hook binary, inbox | `%LOCALAPPDATA%\Coucou\` | `$XDG_STATE_HOME/Coucou/` |
| Claude hooks file | `%USERPROFILE%\.claude\settings.json` | `$HOME/.claude/settings.json` |
| Hook binary | `…\bin\coucou-hook.exe` | `…/bin/coucou-hook` |
| Relay | `\\.\pipe\coucou-<sid>` | `$XDG_RUNTIME_DIR/coucou.sock` |
| Timestamps | `GetLocalTime()` | shared `stamp_now()` (chrono, local) |
| Secrets | Credential Manager (`windows-native`) | Secret Service (`keyring` `sync-secret-service`) |
| Open URL | `rundll32.exe shell32.dll,…` | `xdg-open` |
| Find tool on PATH | `%PATH%` × `%PATHEXT%` | `$PATH` + exec bit |
| Autostart | `tauri-plugin-autostart` registry | same plugin — writes `~/.config/autostart/*.desktop` |

---

## 6. Frontend deltas (small, all optional-but-recommended)

1. **`src/core/bridge.ts`** — extend `BootInfo` with `platform` and
   `pointerSource`. Two new optional fields; no other type changes.
2. **`src/island/island.ts:554–560`** — today the DOM-driven cursor path is
   gated on `!IS_TAURI` (browser dev only). Change the gate to
   `!IS_TAURI || boot.pointerSource === "dom"`, so a Wayland/degraded build can
   drive `onCursor` from `mouseenter`/`mousemove` instead of the Rust `cursor`
   event. On X11 the poll path stays and nothing changes.
3. **`src/settings/main.ts`** — the string `"Windows Credential Manager"`
   becomes `boot.platform === "windows" ? "Windows Credential Manager" :
   "system keyring"`; the `coucou-hook.exe` wording uses `boot.hookPath`
   (already available) rather than a hard-coded `.exe`.
4. **`src/island/hooks.ts:4`** comment mentioning Windows Terminal → neutral
   wording. Cosmetic.
5. **Drag/drop** (`island.ts:567`, `upload/`): the comment "Windows sends no
   cursor position with an OLE drag" is Windows-specific. wry's Linux handler
   *does* send positions, so `UploadSeq.updateCursor` can be fed straight from
   `DragDropEvent::Over.position` — verify during implementation; if it works,
   the Linux path is *better* than Windows.

Everything else — `mochi/engine.ts`, `island/fsm.ts`, `core/layout.ts`,
`views/*`, `chat.ts`, `style.css` — is untouched. Canvas 2D and DOM do not care
about the OS.

---

## 7. Config, packaging, CI

### 7.1 New `src-tauri/tauri.linux.conf.json`

```jsonc
{
  "bundle": {
    "targets": ["deb", "appimage"],
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/icon.png"],
    "resources": { "../target/release/coucou-hook": "coucou-hook" }
  }
}
```

- `nsis` → `deb` + `appimage` (Tauri validates targets per-OS; leaving `nsis`
  on Linux errors out).
- The `.ico` entries stay valid for Windows but should be dropped from the
  Linux override so the icon list is PNG-only.
- `resources` points at the extensionless binary.
- `additionalBrowserArgs` in the base config is Windows-only and ignored
  elsewhere — harmless to leave, cleaner to override out.
- Consider `"app": { "enableGTKAppId": true }` so GNOME associates the window
  and single-instance works properly under the identifier
  `fr.louisraille.coucou` (valid as a GTK application id). Verify — §12.
- The `island` window's `width: 240, height: 6` in the base config is the
  collapsed strip; `apply_geometry` resizes it at runtime, so no change needed.

### 7.2 `windows/scripts/pack.mjs`

~~Currently reads `target/release/bundle/nsis/*-setup.exe`. Add a branch:
on `process.platform === "linux"`, read `target/release/bundle/deb/*.deb` and
`bundle/appimage/*.AppImage`, copy into `windows/release/` with stable names.
Add `npm run pack:linux` (or make `pack` platform-aware).~~

**Implemented.** `pack.mjs` reads a `PLATFORMS` map keyed by `process.platform`
(`win32` → nsis, `linux` → deb + appimage), so there is one `npm run pack` entry
point rather than a second script to keep in sync. On Linux it emits both a
versioned name (`Coucou-Linux-<v>.deb`, `Coucou-Linux-<v>.AppImage`) and the
rolling name (`Coucou-Linux.deb`, `Coucou-Linux.AppImage`) into
`windows/release/`, and
`chmod 0o755`s the AppImage — it is copied, not installed, so the execute bit
does not survive otherwise. Verified: exit 0, 4 artifacts.

### 7.3 `.github/workflows/linux.yml` (new)

~~```yaml
runs-on: ubuntu-24.04        # pin; 26.04 runner availability unverified
steps:
  - apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
      libayatana-appindicator3-dev librsvg2-dev libssl-dev \
      pkg-config build-essential xdg-utils libdbus-1-dev
  - uses: dtolnay/rust-toolchain@stable
  - actions/setup-node@22 + npm ci   (cache: windows/package-lock.json)
  - cache cargo (mirror windows.yml's key pattern)
  - npm run build && npm run tauri build -- --config src-tauri/tauri.linux.conf.json
  - upload artifact (deb + AppImage)
```~~

Mirror `windows.yml`'s tag/version agreement check so `Cargo.toml`,
`tauri.conf.json` and `package.json` cannot drift.

`libayatana-appindicator3-dev` is for the tray icon (§12).

**Implemented — `.github/workflows/linux.yml`.** `ubuntu-24.04`, triggers on
`linux-v*` tags **and on pushes/PRs to `main`** (see the comment in the file:
a tag-only workflow is one nobody ever pushes, so "CI green" would stay
unchecked forever), installs the §7.4 `-dev` packages plus `squashfs-tools` (the
AppImage bundler needs `mksquashfs`) and `libdbus-1-dev` (§3.2), runs the
version-agreement tripwire on tags only, then `npm ci` and `npm run pack`. The
deb + AppImage are uploaded as artifacts on **every** run (`if: always()` +
`if-no-files-found: warn`, so a failed build reports one error instead of two),
and `PUBLISH: 'false'` — this workflow produces artifacts, it does not release.
`permissions`, Node 22 and the cargo cache key mirror `windows.yml`.

**Status: written, never executed.** It has not run on a runner yet; that is
the one §10 packaging item still open.

Note the planned `-- --config src-tauri/tauri.linux.conf.json` was **not**
needed: the CLI auto-merges `tauri.linux.conf.json` on Linux (confirmed in
`tauri-utils-2.10.0/src/config/parse.rs`), and passing it explicitly replaces
rather than merges.

**Trap that CI alone would not have caught:** `tauri-2.12.0/build.rs` computes
`dev = !custom_protocol` from `CARGO_FEATURE_CUSTOM_PROTOCOL`, so a bare
`cargo build --release -p coucou` produces a **dev** binary that boots pointing
at `http://localhost:1420` and shows an empty window
(`didFailProvisionalLoadForFrame: g-io-error-quark code=39`) with nothing
listening on that port. This repo's `src-tauri/Cargo.toml` has no `[features]`
section (upstream), so nothing declares `custom-protocol` for us; only
`npm run tauri build`, or `cargo build --features tauri/custom-protocol` by
hand, embeds the assets. Always build through the CLI for anything that has to
render. Symptom in `journalctl`: `WebProcessCache::takeProcess: did not find
http://localhost`.

### 7.4 Build dependencies on this machine (measured)

```
present : build-essential, pkg-config, libssl-dev, xdg-utils,
          libgtk-3.so.0, libwebkit2gtk-4.1.so.0, libX11.so.6, libXext.so.6
MISSING : libgtk-3-dev, libwebkit2gtk-4.1-dev, libx11-dev,
          libayatana-appindicator3-dev, librsvg2-dev, libdbus-1-dev
MISSING : rustc, cargo, rustup   (~/.cargo does not exist)
MISSING : libgtk-layer-shell.so.0  (irrelevant for Tier 1)
present : Node 20
```

So the toolchain install is: `rustup` + the `-dev` packages above.

**`libdbus-1-dev` is no longer optional** — keyring's `sync-secret-service`
(§3.2) links plain `libdbus`, so a machine without it cannot build the app at
all. It is in the CI install list (§7.3) and in the README build block. The
`vendored` feature was not needed: 1.16.2 is present and links fine.

---

## 8. Dependencies

| Crate | Gate | Why | New? |
|---|---|---|---|
| `keyring` | split features (§3.2) | Secret Service instead of mock store | already present |
| `windows` | `[target.'cfg(windows)']` | Win32 | already present, just gated |
| `x11rb` | `[target.'cfg(target_os = "linux")']` | `QueryPointer` → position **and** button mask in one call; replaces `GetCursorPos` + `GetAsyncKeyState`. Pure Rust, no X11 `-dev` packages required at build time | **new** |
| `chrono` | `[target.'cfg(unix)']` or unconditional | local-time timestamps (§3.6) | **new** |
| `tokio` `net` | none | `UnixListener` — the feature is already enabled in `Cargo.toml` | already present |
| `libc` | only if `geteuid()` is wanted without `/proc/self` | tiny, already in the tree transitively | optional |

Deliberately **not** added: `dirs` (repo style is explicit `env::var_os` with
hardcoded fallbacks), `gtk`/`gdk` direct (avoid raw GTK FFI; Tauri's API covers
Tier 1), `wayland-client` (Tier 2 only, and it cannot adopt GTK's connection
anyway).

---

## 9. Phased implementation

Each phase has an acceptance criterion; do not start the next phase until it
passes.

> **Status of the phases** (evidence in §12 and Appendix A):
>
> | Phase | Result |
> |---|---|
> | 0 — Spike | **passed.** Tauri window on screen under `GDK_BACKEND=x11`, undecorated, transparent, top-centre, `set_ignore_cursor_events` flips the X input shape live (`2x2` ↔ `1440x640`) |
> | 1 — Portable plumbing | **passed.** cfg gates, XDG paths, `tauri.linux.conf.json`, `pack.mjs`; `.deb` + `.AppImage` build; `cargo check` clean; 15 tests pass |
> | 2 — IPC | **passed.** `hook Stop` and `hook PermissionRequest id=<pid>-N` land in the app log; never-block worst case **2 ms** with a stale socket present (rc 0, empty stdout, empty stderr) |
> | 3 — Windowing/cursor | **passed.** Collapse lands exactly `480x12` with the input region covering it; hovering the collapsed strip wakes the island (real pointer, confirmed); while expanded the region is the island's own rectangle, driven by the frontend's pushes rather than by the cursor (§2.6); placement `x` exact, `y` clamped by Mutter to the work area |
> | 4 — Frontend + copy | **passed.** `platform=linux`, `pointer_source=poll` reported at boot; permission card renders |
> | 5 — Verification + docs | **passed, CI pending.** `LINUX-PORT.md` written and updated as each item was measured; §10 ticked as verified; README Linux section and badge in place; `.deb` + `.AppImage` rebuilt from the final source. `.github/workflows/linux.yml` parses but has not run — it only exercises on push/PR |

**Phase 0 — Spike (must pass before any porting).**
Install `rustup` + the §7.4 packages. Build the *unmodified* Windows tree with
`cargo check` after only the §3.2/§3.3 cfg gates, then get a minimal Tauri
window onto the screen. *Accept:* a 720×320 undecorated transparent always-on-
top window appears at top-centre of the primary monitor under
`GDK_BACKEND=x11`, does not appear in Alt-Tab, and `set_ignore_cursor_events`
makes clicks pass through. **This is the phase that de-risks §2** — if it
fails, the plan's core premise is wrong and we stop and re-plan.

**Phase 1 — Portable plumbing.** cfg gates, XDG paths, `stamp_now()`,
`find_on_path`, `open_url`, `hook_exe_path`, keyring features, `tauri.linux.conf.json`,
`pack.mjs`. *Accept:* `npm run tauri build -- --config tauri.linux.conf.json`
produces a `.deb`; the app launches; settings load/save under XDG; tray icon
appears; secrets round-trip (`set` then `present` after restart — this is the
mock-store tripwire test).

**Phase 2 — IPC.** §4: `UnixListener` server, `UnixStream` client, peer-cred
checks, stale-socket handling, both path-format tests. *Accept:* with Coucou
running, Claude Code's hooks reach the island; a `PermissionRequest` approved
from the island writes `allow` to the terminal; killing Coucou and running a
hook exits 0 with empty stdout in under 300 ms (the never-block rule).

**Phase 3 — Windowing/cursor.** §3.4 platform module, X11 backend,
`spawn_cursor_poll` rewired, `make_non_activating`/`set_activating` mapped to
`set_focusable`. *Accept:* hover on the collapsed strip wakes the island;
hovering the island does not steal focus from a terminal; clicks on transparent
margins pass through; gaze follows the cursor across the whole screen; file
drag-over widens the drop zone and `ingest_file` works.

**Phase 4 — Frontend + copy.** §6. *Accept:* settings window shows platform-
accurate wording; pointer-source fallback works when forced.

**Phase 5 — Verification (§10) + docs.** README Linux section, badges,
`LINUX-PORT.md` status update, CI green.

---

## 10. Verification checklist

`[x]` = verified live on this machine. `[ ]` = not yet exercised (needs a human
pointer, a real hook run, or a CI runner).

**Hook protocol**
- [ ] Hook install writes `~/.claude/settings.json` with correct quoting; preview diff shows only Coucou's block; uninstall restores the previous file; `.bak-<stamp>` created first
- [x] `PermissionRequest` → island card → Allow → terminal proceeds; Deny → denied — **Allow clicked and answered twice** (`ui  decide allow req=<pid>-1` → `hook id=<pid>-1 answered allow`, `{"behavior":"allow"}` on the hook's stdout). Card renders correctly (black toolbar, Mochi, amber panel, dim Deny / bright Allow pills). Deny path still unexercised
- [ ] Island paused / webview not listening → decision returns in ~800 ms, terminal takes over
- [x] Coucou closed → hook exits 0, empty stdout, < 300 ms — **2 ms**, measured from the installed `/usr/lib/Coucou/coucou-hook`, with and without a stale socket present
- [x] Non-permission events (PreToolUse, Stop, …) arrive and never block — `hook Stop` and `hook PermissionRequest id=<pid>-N` both land in the app log
- [ ] Foreign process on the socket → peer check rejects, logged

**Window**
- [~] Appears top-centre of the chosen monitor; `screen = primary|cursor` both work — **primary verified**: `x` centres exactly, `y` clamped to the work-area top (`64` X px) by Mutter, which is the compositor's own policy, not ours. `cursor` not exercised
- [x] Collapsed = 240×6 wake strip — X reports **480×12 physical** (= 240×6 logical at scale 2) at `+2832+64`, i.e. exactly centred
- [x] hovering it wakes — **verified with a real pointer.** At `03:06:49` the island collapsed to `480x12` with `input shape: full window (wake strip)`; 8 s later `03:06:57 island shown: full panel`. Nothing else could have triggered it — the cursor poll is parked while collapsed and no hook fired in between — so it was the DOM `mouseenter` on `#wake-strip`. Confirmed deliberate by the user
- [ ] no Alt-Tab entry; no taskbar entry — `skipTaskbar`/`alwaysOnTop` set, not visually confirmed
- [x] Transparent margins pass clicks through — the input region *is* the island's own rectangle, set by SHAPE from the frontend's `set_island_rect` pushes: the probe reads `(80,0) 1280x320` with a card up (= `640x160` logical at `(40,0)`), `289x32` when compact, and the **whole** window while collapsed. Margins are outside the region, so clicks there reach the window underneath
- [ ] Island never steals focus; chat input *can* take focus and typing works, then releases it — `set_focusable` mapping implemented, not observed
- [ ] Monitor hotplug / scale change while hidden → island reappears in the right place (`current_screen_key`)

**Interaction**
- [ ] Gaze follows the cursor across the full screen — the poll path it depends on is proven working (it streams `cursor` events and logs one line per transition of the cursor into/out of the island rect), gaze itself not eyeballed
- [ ] Bot hover (love), view switching, pinned mode, Escape-to-collapse
- [ ] File drag from a file manager → drop zone highlights → `ingest_file` copies to inbox; inbox sweep at 7 days
- [ ] Sounds play on a hook event **without** prior user gesture (§12)

**Secrets / settings**
- [x] Store key, restart, `present` still true (the mock-store tripwire) — **passed cross-process**: `set` in one process, `get` in a *fresh* process returns the value; a mock store answers `NoEntry`. `org.freedesktop.secrets` present in the release binary
- [ ] No Secret Service daemon → graceful "no key" state, no panic — `secrets.rs` maps every keyring error to `Ok(false)`/`Err(String)`; no daemon-side test run yet
- [ ] Settings persist under `$XDG_CONFIG_HOME/Coucou/settings.json` — `save()` does `create_dir_all` on the parent; `~/.config/Coucou/` is only created on the first save, which needs a Settings-window change to trigger

**Packaging**
- [x] `.deb` and `.AppImage` install and run — `npm run tauri build` → 5.10 MiB `.deb` + 87.13 MiB `.AppImage`, both `chmod 755` where it matters; the extracted `.deb` payload boots and renders (`front end booted`) with nothing on port 1420
- [x] hook binary lands in the package and is executable — **`/usr/lib/Coucou/coucou-hook`**, not `bin/`; `/usr/bin/coucou` is the app. `Depends: libayatana-appindicator3-1, libwebkit2gtk-4.1-0, libgtk-3-0`
- [ ] Autostart entry appears in `~/.config/autostart/`
- [ ] CI green on a clean runner

---

## 11. Known degradations and accepted limitations

1. **Native Wayland (Tier 2) is not parity.** Gaze only tracks the pointer while
   it is over the island; window placement is compositor-chosen; global drag
   detection unavailable. Documented, not promised.
2. **No layer-shell.** GNOME does not implement `wlr-layer-shell`, and
   `libgtk-layer-shell.so.0` is not installed. A true edge-pinned panel on
   GNOME Wayland would need a GNOME Shell extension — out of scope.
3. **XWayland pointer staleness.** While the pointer is over a *native* Wayland
   window, XWayland may report a stale position. Affects gaze and the
   approach-to-strip prediction; does **not** affect anything once the pointer
   is over the island (it is a real X surface). §2.5's event-driven wake path
   removes this from the critical path. Verify in Phase 0/3.
4. **Tray on GNOME** requires the AppIndicator extension; Ubuntu ships it, other
   GNOME distros may not. Same caveat the Windows build has with some
   taskbars.
5. **Fractional scaling** under XWayland can render the webview slightly soft.
   `apply_geometry` already re-asserts physical size after a scale change.
6. **The island is resizable on Linux.** Windows can resize a `resizable: false`
   window behind the user's back (`SetWindowPos` ignores `WS_THICKFRAME`), so
   upstream never has to offer a grip. GTK will not: `resize` is a no-op on a
   non-resizable window, and locking it again reverts the window to its clamped
   default size (§12 R11). So the window is switched to resizable for every
   placement and left there, which means a user *can* drag the island's edge
   while the pointer is over the pill — the next placement fixes the size, and
   the pill is 6 px from the screen edge, so it is hard to do by accident.
   Documented, not prevented.

---

## 12. Risks and open questions

| # | Question | Why it matters | How to settle it |
|---|---|---|---|
| R1 | Does `set_ignore_cursor_events` behave correctly on X11? (tao → `input_shape_combine_region` → X Shape) | Click-through is load-bearing | **RESOLVED — yes, verified live** (`2x2` ↔ `1440x640`). **But it is no longer how the island works.** Deriving the flag from the cursor deadlocks under XWayland — see §2.6 — so the region is now set directly by SHAPE from `gate.rect`. The primitive is proven; the app drives it from the rectangle instead |
| R2 | Does `GDK_BACKEND=x11` + `apply_geometry` place the window at true top-centre? | The island is glued to the top edge | **RESOLVED — x exact, y clamped by the work area** (see below) |
| R3 | Exact `keyring` 3.6.3 Linux feature name (`linux-native` vs `sync-secret-service` vs `linux-native-sync-persistent`), and does it need `libdbus-1-dev` or a `vendored` feature? | Wrong choice = silent mock store, API keys vanish | **RESOLVED — `sync-secret-service`.** Two traps here. (1) "Compiles" is not the test: `keyring` 3.6.3's default-store cascade in `src/lib.rs` means `linux-native` alone selects the **kernel** keyring — `keyutils.rs` uses `KeyRingIdentifier::Session`, so an API key would be wiped at logout. Only a secret-service feature selects `org.freedesktop.secrets`. (2) `libdbus-1-dev` **is** required (plain `libdbus`, not `zbus`); present here (1.16.2), and the `vendored` feature is not needed. Round-trip proven **across processes**: `set` in one, `get` in a fresh one returns the value — a mock store answers `NoEntry`, so this is the §10 tripwire, passed. Binary contains `org.freedesktop.secrets` |
| R4 | Does GNOME Shell honour `set_focusable(false)` / GDK accept-focus so clicks do not steal terminal focus? Wayland has no "don't activate" request. | WS_EX_NOACTIVATE parity | Phase 0/3; if it fails, mitigate by re-focusing the previous window where possible and document |
| R5 | Autoplay of the 28 WAVs in WebKitGTK without a user gesture | Hook approval sounds are the product's heartbeat | **RESOLVED — and the answer was two different bugs.** (1) WebKitGTK's Web Audio context starts `suspended`, and `resume()` outside a gesture never settles (probed: state stays `suspended`, the promise stays pending), while one real click is **sticky** — after it, a `resume()` from a plain timer goes `running` (peak 0.59 measured). wry already builds the GTK webview with `WebsitePolicies.autoplay = Allow` whenever its `autoplay` attribute is set, and Tauri never overrides wry's default of `true`, so no gesture is needed in practice; sticky activation is the fallback. (2) What actually shipped was silent for a different reason: the AppImage copies GStreamer's core libraries but **none of the plugins**, and its launcher sets `GST_PLUGIN_SYSTEM_PATH_1_0` to `$APPDIR/usr/lib/gstreamer-1.0` — a directory that does not exist — which *replaces* the compiled-in path. With no `appsrc`/`decodebin`/audiosink, WebKit never starts the context and `decodeAudioData` never settles, so the frontend saw a hang, not an error. `platform::repair_appimage_gstreamer()` adds the system plugin directories through the additive `GST_PLUGIN_PATH_1_0` before WebKit initializes. Verified on the packaged AppImage: 28/28 buffers, context running, and an uncorked `webaudio` stream in `pactl` for the length of a hook-triggered sound. The `.deb` uses the system libraries and was never affected |
| R6 | Tauri Linux config merge + `enableGTKAppId` behaviour | Packaging correctness | **MERGE RESOLVED** — the CLI auto-merges `tauri.linux.conf.json` on Linux (`tauri-utils-2.10.0/src/config/parse.rs`), so no `--config` flag. `enableGTKAppId` left off deliberately: the single-instance/app-id implications are not investigated |
| R7 | Whether wry's GTK drag handler reaches us while the input region is non-full | File drop could be flaky | Test in Phase 3; mitigation is widening the region on `DragEnter` |
| R8 | `chrono` vs `time` for local timestamps | Backup filenames | §3.6 — recommendation is `chrono` |
| R9 | Why this machine's GNOME session advertises **no** `wl_output` and reports empty DisplayConfig | Determines whether Tier 2 is even testable here | Investigate separately; not blocking Tier 1 |
| R10 | Does `x11rb` see a real `:0` on stock Ubuntu without X11 sessions? | XWayland must be present | Confirmed present here (`/tmp/.X11-unix/X0`, Xwayland PID 5360); verify on a clean install |
| R11 | Will GTK actually *apply* a shrink? `gtk_window_resize` is a **silent no-op** on a `resizable: false` window, and re-locking it afterwards reverts the window to its default size (240×6, which GTK itself clamps to 200 high) | The island never becomes a wake strip, so a 480×400 column eats clicks down the middle of the screen | **RESOLVED — measured.** `set_resizable(true)` before every placement and **never** `false` afterwards: X then reports exactly `480x12`. Windows' `SetWindowPos` resizes regardless of `WS_THICKFRAME`, so the call is unix-gated. **Residual:** `tauri.conf.json` still says `resizable: false`, so on Linux the island *can* be drag-resized while the pointer is over the pill; the next placement re-asserts the size. Accepted rather than pinning `min == max` through `set_geometry_hints`, whose semantics I could not pin down |
| R12 | Does `cargo build --release -p coucou` produce a runnable app? | A blank window reads as a frontend regression, not a build mistake | **RESOLVED — no.** `tauri-2.12.0/build.rs` sets `dev = !custom_protocol` from `CARGO_FEATURE_CUSTOM_PROTOCOL`, and this repo's `src-tauri/Cargo.toml` has no `[features]` to declare it, so a bare build loads `http://localhost:1420` and renders nothing (`g-io-error-quark code=39`). Use `npm run tauri build`, or `--features tauri/custom-protocol`. See §7.3 |
| R13 | Can the hover-wake path be **tested automatically**? | §9 Phase 3's acceptance criterion is "hovering the collapsed strip wakes the island" | **CLOSED — the automation is impossible, the behaviour is verified.** Under this XWayland `XWarpPointer` updates `QueryPointer` (so the app's own poll sees it) but delivers **no** event to X clients: a GTK window received 0 enter/motion events after the pointer was verifiably moved onto its centre. XTEST fake motion is accepted and then ignored (the pointer does not move). So a real mouse is required, and one was used: `03:06:49 island hidden` → `input shape: full window (wake strip)` → `03:06:57 island shown`, with the poll parked and no hook firing in between, confirmed deliberate by the user. The static reading agrees — `wakeStrip.mouseenter` registered unconditionally at `island.ts:529`, `#wake-strip` filling the 240×6 viewport exactly at `style.css:56`. **Automating this needs a real pointer or a compositor that forwards synthetic motion; not worth building for one acceptance criterion** |

---

## Appendix A — Evidence log

Sourced so each claim can be re-checked.

**Read from source:**
- `tauri` 2.12.0 (workspace lock), `keyring` 3.6.3
- tao `platform_impl/linux/util.rs` — `cursor_position(is_wayland)` returns `(0,0)` on Wayland
- tao `platform_impl/linux/event_loop.rs:452` — `CursorIgnoreEvents` → `input_shape_combine_region`
- tao `platform_impl/linux/window.rs` — `CursorIgnoreEvents`, `util::cursor_position`, `set_skip_taskbar`
- wry `webkitgtk/drag_drop.rs` — `DragDropEvent::{Enter,Over,Drop}` all carry `position`
- Tauri 2 config reference — `tauri.linux.conf.json` merge; `skipTaskbar` documented for Windows *and* Linux; `focus`, `focusable`, `transparent`, `alwaysOnTop`
- docs.rs `tauri::Window` / `WebviewWindow` — expose `cursor_position`, `set_ignore_cursor_events`, `set_focusable`, `set_always_on_top`, `set_skip_taskbar`
- `keyring` 3.6.3 docs — no platform feature ⇒ mock credential store
- `keyring` 3.6.3 `src/lib.rs` — the default-store cascade: `linux-native` alone ⇒
  `keyutils`; any `*-secret-service` ⇒ `secret_service`; `linux-native-*-persistent`
  ⇒ `keyutils_persistent`; nothing ⇒ `mock`
- `keyring` 3.6.3 `src/keyutils.rs:260` — `KeyRing::from_special_id(KeyRingIdentifier::Session)`
  ⇒ keys die with the login session
- `tauri-2.12.0/build.rs` — `dev = !custom_protocol` from `CARGO_FEATURE_CUSTOM_PROTOCOL`
- `wry-0.57.0/src/webkitgtk/mod.rs:708` — `webview.set_size_request(...)`, checked while hunting the height clamp (not the cause)
- `tao-0.37.1/src/platform_impl/linux/util.rs::set_size_constraints` — Tauri's `set_min_size`/`set_max_size` → `set_geometry_hints`
- `tao-0.37.1/src/platform_impl/linux/event_loop.rs:302` — `WindowRequest::Size((w,h)) => window.resize(w, h)`

**Measured on this machine** (PyGObject/GTK 3.24 throwaway probes since
deleted; plus `wayland-info`, `gdbus`, `xrandr`, raw `XQueryPointer` via ctypes):
- X11 backend: 1 monitor `0,0 6144x3456`; pointer `(4122,1159)`; `w.move(2712,0)` honoured
- Wayland backend: 0 monitors after 4 s; pointer `(0,0)`; window position `(0,0)`
- `wayland-info`: 42 globals, **no `wl_output`**
- `Mutter.DisplayConfig.GetCurrentState`: empty monitor and logical-monitor arrays
- `xrandr`: screen `6144x3456`, no outputs listed
- XWayland `:0` rootless running, `SHAPE` extension present
- Installed: GTK 3.24.52 runtime, WebKitGTK 4.1 runtime, `libX11`, `libXext`, Node 20, `gcc`, PyGObject
- Missing at planning time (all since installed by hand): `rustup`/`cargo`,
  `libgtk-3-dev`, `libwebkit2gtk-4.1-dev`, `libx11-dev`,
  `libayatana-appindicator3-dev`, `libdbus-1-dev` — plus rustup 1.98.1
- Running: `gnome-keyring-daemon --components=pkcs11,secrets` + session D-Bus ⇒ Secret Service available
- Desktop: `ubuntu:GNOME`, GNOME Shell 50.1, `XDG_SESSION_TYPE=wayland`

**Measured during implementation.** The app-log lines are reproducible from
`windows/`. The input-shape readings come from a scratch `x11rb` client —
geometry via `query_tree`/`get_geometry`, input region via `ShapeGetRectangles`
on the `Input` destination — written for this port and not kept; it is a
one-screen program and the numbers below are what it printed:

*Input region / click-through (R1, §2.6)*
- **The primitive works, and the app no longer drives it from the cursor.**
  `set_ignore_cursor_events` flips the X input shape live (`2x2` ↔ `1440x640`,
  tao → `input_shape_combine_region` → X Shape). That was proven first, and it
  is what showed the mechanism was sound while the *decision* was not.
- The failure that motivated §2.6: with a card armed the shape sat at
  `(0,0) 2x2` for **108 s straight** while `QueryPointer` was frozen at
  `(4055,1048)` — outside the `640x160` rect — so `accept` could never become
  true. No `click-through off` line in the whole log, every click passed
  through, `decide` never fired.
- The flip did work whenever the pointer *was* tracked: one line at
  `02:33:36 click-through off … cursor=(668,164) rect=(40,0 640x160)`,
  immediately followed by `cursor=(678,179)` → back on. Correct, just
  unreachable while the read stalls.
- After the fix, the probe reads the input shape as `1 rect(s): (80,0) 1280x320`
  with a card up (= `640x160` logical at `(40,0)`, exactly
  `islandSize("expanded","approval")`), `(431,0) 578x65` when compact
  (= `289x32` logical), and it tracks the springs frame by frame — the log shows
  `636x149 → 629x146 → 616x142 → … → 288x32`.
- Collapse → `input shape: full window (wake strip)`; the probe then reads
  `(0,0) 1440x640` on a `480x12` window, which X clips to the window, so the
  strip really does capture the pointer. Wake → `184x0` then the springs grow it
  back to `288x32`.
- App log now carries `input shape: 640x160 at (40,0)` on every change and
  `cursor inside island — cursor=(x,y) rect=(…)` / `cursor outside island …`
  once per transition: the cursor read is reported but no longer acted on.

*Permission card (§10)*
- `ui  decide allow req=<pid>-1` → `hook id=<pid>-1 answered allow`, hook stdout
  `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}`
- Only `views.ts:313-314` (the Deny/Allow buttons) call `decide()` — there is no
  timer or auto-approve path, so those two lines are human clicks
- The same `PermissionRequest` under the old build timed out every time
  (`hook id=… timed out — terminal takes over`, `DECISION_BUDGET` = 108 s)

*Collapse → wake cycle*
- `03:06:48` springs shrink the region `287x32 → … → 184x0 at (268,0)`
- `03:06:49 island hidden: shrink to the wake strip` → `placed the island:
  480x12 at (2832,0)` → `input shape: full window (wake strip)`
- `03:06:57 island shown: full panel` → `placed the island: 1440x640 at
  (2352,0)` → `input shape: 184x0 at (268,0)` → springs grow it back to `288x32`
- Nothing else fired in the 8 s between hidden and shown (no hook, no cursor
  poll — it is parked while collapsed), so the wake came from the DOM
  `mouseenter` on `#wake-strip`, i.e. a real pointer entering the strip

*Placement (R2)*
- `_NET_WORKAREA = 0,64,6144,3258`; island `1440x640` at `+2352+64` =
  `(6144-1440)/2 = 2352` exactly; the `64` is Mutter's work-area top, not an error
- Collapsed: `480x12` at `+2832+64`, `(6144-480)/2 = 2832` exactly

*Collapse geometry (R11)*
- Sequence, sampled every 100 ms through the idle collapse:
  `1440x640` → (nothing intermediate) → `480x400`, while Tauri's own
  `inner_size()` reported `480x12 (scale 2), asked for 480x12` — i.e. the resize
  landed and then *something* grew it back
- With `set_resizable(false)` suppressed: X reports **`480x12`**. Q.E.D.
- PyGObject controls (all at scale 2): plain window, `set_resizable(true)` then
  `resize(240,6)` → `(240,6)` on both decorated and undecorated; with a
  `WebKit2.WebView` child loading `about:blank` *and* our own `dist/index.html`
  → `(240,6)`. So neither GTK decorations nor WebKit nor the page clamp it —
  only the lock does

*Never-block / IPC*
- Installed `/usr/lib/Coucou/coucou-hook Stop` ×3: `rc=0`, **2 ms**, empty
  stdout and stderr; `PermissionRequest` likewise
- `hook PermissionRequest id=<pid>-N` and `hook Stop` both appear in
  `~/.local/state/Coucou/coucou.log`

*Packaging*
- `npm run tauri build` → `Coucou-Linux-0.1.1.deb` 5.10 MiB,
  `Coucou-Linux-0.1.1.AppImage` 87.13 MiB; `node scripts/pack.mjs` → exit 0,
  4 artifacts (versioned + rolling, AppImage `chmod 755`)
- `dpkg-deb -x` payload: `/usr/bin/coucou` (8.6 MB), `/usr/lib/Coucou/coucou-hook`
  (382 KB, executable); `Depends: libayatana-appindicator3-1, libwebkit2gtk-4.1-0, libgtk-3-0`
- The extracted payload boots standalone: `front end booted — platform=linux
  pointer_source=poll` with nothing listening on 1420 ⇒ not a dev build

*Secrets (R3)*
- `cargo tree -e features -p keyring` with the first attempt (`linux-native`):
  only `linux-keyutils` active ⇒ kernel keyring, no D-Bus at all
- The same with `sync-secret-service`: `dbus-secret-service 4.1.0` →
  `dbus 0.9.12` → `libdbus-sys` ⇒ a real `org.freedesktop.secrets` client
- Cross-process round trip (temporary `examples/keyring_probe.rs`, deleted
  afterwards): `del → Err(NoEntry)`, `get → Err(NoEntry)`, `set → Ok(())`,
  **`get → Ok("round-trip-value-42")` in a fresh process**, `del → Ok(())`,
  `get → Err(NoEntry)`. A mock store answers `NoEntry` in that fresh process,
  so this is the §10 tripwire by construction
- Release binary now contains `org.freedesktop.secrets`; the `.deb` grew
  5.10 → 5.20 MiB with the D-Bus client linked in

*Tests*
- `cargo check -p coucou -p coucou-hook` clean; `cargo test` 15 passing (12 + 3)

**Not verified — flagged in §12 rather than asserted:**
- Whether GTK3's Wayland backend implements input shape at all (the probe failed
  with pycairo's `'could not find foreign type Region'`, a Python binding gap,
  not a GTK verdict)
- `accept_focus`/`set_focusable` mapping inside tao's Linux window path
- Local-time timestamp library choice
- The Deny click, autostart, and CI. (Allow is now verified — two live clicks;
  hover-wake is verified with a real pointer, R13 closed; the secret round-trip
  is verified cross-process — see §10.)
