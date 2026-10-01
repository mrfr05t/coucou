# Changelog

Notable changes, newest first.

Everything before this fork's first commit (the macOS and Windows releases up
to 0.1.1) is in the git history — upstream logged each release there rather
than in a file, so those are not re-listed here. When you need the detail
behind an entry, the commit it links to carries the reasoning, the alternatives
that were rejected and the measurements that settled it.

## [Unreleased]

The app version is still **0.1.1**; these land in the next release.

### Added — Linux

The Windows Tauri 2 app, ported. macOS (`NotchBuddy/`) is untouched. Full plan,
file-by-file change list, verification checklist and evidence log in
[`docs/LINUX-PORT.md`](docs/LINUX-PORT.md). — `6781b42`

- **Runs on Linux** — X11/XWayland first and fully verified there; native
  Wayland runs degraded (no global cursor query, so no hover-wake from off the
  island) — [`docs/LINUX-PORT.md` §11](docs/LINUX-PORT.md).
- **OS code behind `platform/`** — `win32.rs` and `x11.rs` behind one interface,
  so no `cfg` spreads through the app.
- **IPC: named pipe → Unix socket** — `$XDG_RUNTIME_DIR/coucou.sock`, peer
  credentials checked, mode 0600. Claude Code's hooks never block either way:
  no socket means exit 0 with empty output in well under 300 ms.
- **Linux integrations** — keys in the Secret Service, settings in
  `$XDG_CONFIG_HOME/Coucou`, state in `$XDG_STATE_HOME/Coucou`, autostart as a
  `.desktop` entry.
- **Packaging and CI** — AppImage and `.deb` from `npm run pack`
  (87 MB / 5 MB), bundle config split into `tauri.windows.conf.json` /
  `tauri.linux.conf.json`, and `.github/workflows/linux.yml` building on
  `linux-v*` tags plus every push and PR to `main`.
- Three fixes found by measuring rather than reasoning:
  - **window never shrank to the wake strip** — GTK silently ignores a resize on
    a `resizable: false` window, so every placement now sets `resizable(true)`
    first and never undoes it (X reports exactly `480x12` at scale 1).
  - **click-through could be applied mid-transition** — now guarded by
    `!gate.collapsed`, so the island can't swallow or lose a click while
    animating.
  - **the input region is rect-driven on X11** — a SHAPE region derived from the
    rectangle the front end already pushes, instead of a flag computed from the
    cursor, which on XWayland cannot advance while the flag says pass-through.

### Added

- **Chat provider setting** — Claude (Anthropic) stays the default; Settings →
  Chat can switch to any OpenAI-compatible API: api.openai.com, OpenRouter,
  Ollama, LM Studio, llama.cpp, or a server on `localhost`. Bare origins gain
  `/v1`. — `202ebb2`
  - Uses the **Responses API** and falls back to **Chat Completions** on
    404/405/501, remembering what each base URL accepts rather than asking
    again every turn.
  - An API key is **not** required — a local server like Ollama needs none. Keys
    that are set go to the OS keychain, never to disk.
  - The conversation survives a provider switch: history is kept in a neutral
    shape and re-rendered per route, so neither side's choice is lost.
  - Attachments reach the model as base64 data URIs; a PDF on a route that
    cannot read PDFs says so rather than silently vanishing.
- **Interface size slider** — Settings → General, 100 %–200 %. Grows the text,
  the buttons and the island itself, and the window grows with them so nothing
  is ever cropped. Applied live, persisted, and clamped so a hand-edited
  `settings.json` cannot push it out of range. — `b29320e`
  - One number drives everything: CSS `zoom` on the document, the rectangle the
    island pushes to Rust, the Rust window size, and the settings window's
    frame.
  - Canvas backing stores fold the scale into their device pixel ratio, so
    Mochi stays sharp instead of being magnified.

### Fixed

- **Chat broke on its second turn against llama.cpp** — every message after the
  first came back `400 Cannot determine type of 'item'`. We sent `input` items
  as `{role, content}`; OpenAI tolerates that, llama.cpp does not. Every item
  now carries `type: "message"`. — `72c0b9d`
- **An attachment a server refuses no longer kills the chat** — llama.cpp answers
  `400 'input_file' is not supported` and images with `500 image input is not
  supported … mmproj`. The request is retried once without the media, and the
  model is told `[shot.png was attached, but this endpoint cannot read images]`
  instead of the turn being lost.
- **The AppImage was silent** — every sound effect, and only in the packaged
  build: the same binary run straight from `target/release` played all 28. An
  AppImage bundles GStreamer's core libraries but none of its plugins, and its
  launcher sets `GST_PLUGIN_SYSTEM_PATH_1_0` to `$APPDIR/usr/lib/gstreamer-1.0`
  — a directory that does not exist. Because that variable *replaces* the path
  GStreamer was compiled with rather than adding to it, GStreamer registered no
  dynamic plugins at all: no `appsrc`, no `decodebin`, no audio sink. WebKit's
  version of "no audio sink" is an `AudioContext` that never starts plus a
  `decodeAudioData` promise that never settles, so the engine hung on the first
  file and the island was mute. The system plugin directories now go in through
  the additive `GST_PLUGIN_PATH_1_0`, before Tauri builds anything. The `.deb`
  uses the system libraries and was never affected.
- **Sound failures stop being invisible** — the engine writes `context created
  (suspended)`, `buffers 28/28, context running` and the first few sounds it
  could not play to the shared log. That is what made the above findable; the
  old code swallowed all three states as the same silence.

### Tests

- **37 tests** (34 in the app, 3 in the hook), up from 18 before this fork.
  New coverage includes three end-to-end tests that run a mock HTTP server and
  exercise the route ladder, the items llama.cpp needs, and the clamping of the
  UI scale — plus the port's geometry and window arithmetic.

### Documentation

- `docs/LINUX-PORT.md` — plan, verification checklist, risks (R1–R13) and an
  evidence log, kept up to date as each was measured.
- `README.md` — Features and Setup now describe the Linux build, the chat
  provider and the interface size slider.
