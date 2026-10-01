<div align="center">

<img src="NotchBuddy/Assets.xcassets/AppIcon.appiconset/icon_256x256.png" width="96" alt="Coucou icon">

# Coucou

**A tiny friend that lives in your MacBook's notch — or at the top of your screen on Windows and Linux — and keeps an eye on your Claude Code sessions.**

Approve permissions, watch your agents work, drop a file, chat with Claude — all without leaving what you're doing.

![macOS 15+](https://img.shields.io/badge/macOS-15%2B-black?logo=apple)
![Windows 10/11](https://img.shields.io/badge/Windows-10%2F11-0078D4?logo=windows&logoColor=white)
![Linux (X11 / XWayland)](https://img.shields.io/badge/Linux-X11%20%2F%20XWayland-FCC624?logo=linux&logoColor=black)
![Swift 6](https://img.shields.io/badge/Swift-6-F05138?logo=swift&logoColor=white)
![SwiftUI](https://img.shields.io/badge/SwiftUI-native-0A84FF)
![Tauri 2](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=black)
![License: MIT](https://img.shields.io/badge/license-MIT-green)
![GitHub stars](https://img.shields.io/github/stars/Louis-CFM/coucou?style=social)

<img src="docs/media/demo.gif" width="760" alt="Coucou in action">

</div>

---

## Why

Some studios showed off gorgeous notch companions… and never let anyone use them.
**Coucou is the open version.** Every line of code, every animation, every sound — free to use, read, fork and remix.

Meet **Mochi**: a soft little squircle with big eyes that pops out of your notch, waves hello, follows your cursor with its eyes, gets annoyed when you poke it (and dizzy if you insist), and tells you the moment Claude Code needs you.

## Features

- 🤖 **Claude Code, live** — see every session in your notch: what it reads, edits and runs, step by step. Finished? Mochi does a happy little jump.
- ✅ **Approve from the notch** — Claude Code permission requests show up with **Allow / Deny**. One click, back to work.
- 🧑‍💻 **Jump to the right terminal** — open the exact terminal window of a session *(macOS)*.
- 💬 **Ask Claude anything** — built-in chat, straight from the notch. Claude by default, or any OpenAI-compatible API (api.openai.com, OpenRouter, Ollama, LM Studio) — one dropdown in Settings.
- 📎 **Drop a file on the notch** — Mochi turns into a box and swallows it, then ask a question about it or send it by email *(email: macOS, Mail.app)*.
- 🪟 **Drag Mochi onto any window** — attach that window as context for Claude *(macOS)*.
- 🔌 **Integrations** — Stripe payments, n8n workflows, GitHub, Vercel deployments, Resend emails, Notion, Cal.com. Each one gets its own little colored Mochi.
- 🎭 **A real character** — idle breathing, blinks, eyes on a sphere that follow your mouse, emotes, 28 handcrafted sounds, a greeting on launch.
- 🫥 **Invisible when idle** — hides away when nothing is running, peeks out when you hover the notch (the top edge of the screen on Windows and Linux).
- 📐 **Fits your screen** — one slider in Settings grows the whole interface — text, buttons, the island itself — from 100 % to 200 %, and the window grows with it so nothing is ever cropped.
- 🔒 **Private by design** — no telemetry, no account. Keys live in your macOS Keychain, Windows Credential Manager or Linux Secret Service. The app only talks to the services you plug in.

<table>
<tr>
<td><img src="docs/media/claude-code.png" alt="Claude Code session"></td>
<td><img src="docs/media/stripe.png" alt="Stripe payments"></td>
</tr>
<tr>
<td><img src="docs/media/chat.png" alt="Chat with Claude"></td>
<td><img src="docs/media/dizzy.png" alt="Too many hits"></td>
</tr>
</table>

## Install

### Download for macOS

1. Grab the latest `Coucou.zip` from [Releases](https://github.com/Louis-CFM/coucou/releases).
2. Unzip and move **Coucou.app** to `/Applications`.
3. Launch. This build isn't notarized by Apple yet, so the first time macOS says it can't verify the developer: open **System Settings → Privacy & Security**, scroll down and click **Open Anyway** (only once).

### Windows

The Windows installer is **temporarily unavailable**. Microsoft Defender wrongly
flags the unsigned installer as malware; a false-positive report is under review
at Microsoft and the installer will come back once it is cleared and signed.
Until then you can [build it from source](#build-from-source).

There is no notch on a PC, so the island slides out of the top edge of the screen
instead of hiding inside one. See [`windows/README.md`](windows/README.md) for the
rest of the differences.

### Linux

Same Tauri app as Windows, running on X11/XWayland. Two ways in:

```bash
# 1. distro package — from a release artifact
sudo apt install ./Coucou-Linux.deb
./Coucou-Linux.AppImage          # the AppImage needs no install, just run it
```

```bash
# 2. from source — see below
```

Notes, because the desktop is a different animal:

- **It needs X11.** The island reads the global cursor position and toggles
  click-through with the X Shape extension, neither of which exists on bare
  Wayland. GNOME, KDE and friends all run XWayland by default, so it just works
  there; a pure-Wayland session is a known degradation, not a goal yet.
- **No notch, no layer-shell.** GNOME does not implement `wlr-layer-shell`, so
  the island is a normal always-on-top window pinned to the top edge — exactly
  like on Windows.
- Keys go to the **Secret Service** (gnome-keyring / KWallet, through `keyring`'s
  `sync-secret-service` feature), so they live on disk in your keyring app and
  survive reboots — the same deal as Keychain on macOS. With no daemon running,
  Coucou degrades to "no key saved" rather than silently losing it.
- The whole port, including what was measured and what is still untested, is
  written up in [`docs/LINUX-PORT.md`](docs/LINUX-PORT.md).

### Build from source

**macOS** — requirements: macOS 15+, Xcode 16+, [XcodeGen](https://github.com/yonaskolb/XcodeGen).

```bash
brew install xcodegen
git clone https://github.com/Louis-CFM/coucou.git
cd coucou/NotchBuddy
xcodegen
open NotchBuddy.xcodeproj   # then ⌘R
```

**Windows** — requirements: [Rust](https://rustup.rs), Node 20+, MSVC build tools.

```powershell
git clone https://github.com/Louis-CFM/coucou.git
cd coucou/windows
npm install
npm run pack                # installer lands in windows/release/
```

**Linux** — requirements: [Rust](https://rustup.rs), Node 20+, and the GTK/WebKit
headers:

```bash
sudo apt install build-essential pkg-config libssl-dev libdbus-1-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev libx11-dev squashfs-tools
git clone https://github.com/Louis-CFM/coucou.git
cd coucou/windows
npm install
npm run pack                # Coucou-Linux.deb + .AppImage land in windows/release/
```

Two traps worth knowing about:

- Build through `npm run pack` / `npm run tauri build`, **not** a bare
  `cargo build --release`. Tauri's `build.rs` only embeds the front end when the
  `custom-protocol` feature is on, which only the CLI passes — a bare release
  build boots pointing at `http://localhost:1420` and shows an empty window.
- `npm run dev` serves the front end on `127.0.0.1:1420` and needs Rust built
  the same way; the `predev` hook handles `coucou-hook` for you.

## Setup

Click the Coucou icon in the menu bar (macOS) or in the system tray (Windows, Linux) → **Settings…**

| What | Why | Where the key goes |
|---|---|---|
| **Claude Code hooks** | live sessions and approvals | **Install hooks** — Coucou backs up `~/.claude/settings.json`, merges its hooks and shows you the diff before writing anything |
| **Chat provider** | chat and questions about files | **Anthropic API key** by default. Settings has a dropdown for any OpenAI-compatible API — its key goes to Keychain / Windows Credential Manager / Secret Service too, except a local server like Ollama, which needs none |
| Stripe, n8n, GitHub, Vercel, Resend, Notion, Cal.com | the integration pills | Keychain / Windows Credential Manager / Secret Service, all optional |

If Coucou isn't running, the hook exits immediately: **Claude Code is never blocked.**

## Things to try

| Do this | Mochi does that |
|---|---|
| Hover the notch (top edge on Windows and Linux) | peeks out and says hi 👋 |
| Click it | opens |
| Hover Mochi | blinks, eyes grow |
| Click Mochi | squish + annoyed |
| Click 3 times fast | 😵‍💫 dizzy for a few seconds |
| Drag a file onto the island | turns into a box and swallows it |
| Drag Mochi onto a window *(macOS)* | attaches it as context |

## How it works

**macOS**

- **Island**: a borderless `NSPanel` hugging the notch, driven by a small state machine (`hidden → petit → home`).
- **Character**: drawn in SwiftUI `Canvas` + `TimelineView` at 60 fps — squircle body, eyes projected on a sphere, spring animations. No Rive, no Lottie, no images.
- **Claude Code**: a tiny `nb-hook` script receives hook events and forwards them over a Unix socket to the app. For approvals it waits for your click, then answers the hook.
- **Integrations**: lightweight pollers, paused when nothing is watching.
- **Sounds**: 28 short WAVs played through preloaded `AVAudioPlayer`s.

The macOS app is native Swift 6 / SwiftUI / AppKit with **zero third-party dependencies**.

**Windows**

- A [Tauri 2](https://tauri.app) app (Rust + TypeScript): the island is a transparent, always-on-top window that never steals focus, Mochi is drawn in Canvas 2D with the same shapes, timings and sounds as on the Mac.
- Claude Code hooks go through a tiny `coucou-hook.exe` and a named pipe; keys live in Windows Credential Manager.
- Details and differences in [`windows/README.md`](windows/README.md).

**Linux**

- The *same* Tauri 2 app, with the Win32 layer swapped for X11 (`x11rb`) behind a
  small `platform` module — so it runs on X11/XWayland, where Mutter, KWin and
  GNOME's default session all put it.
- The island window is undecorated, always-on-top and click-through: an X Shape
  input region of 2×2 px over transparent pixels, widening to the whole window
  only where Mochi actually is. When it hides it shrinks to a 240×6 **wake
  strip** at the top edge — the only thing you can hit — which wakes it again on
  hover.
- Hooks arrive over a Unix socket (`$XDG_RUNTIME_DIR/coucou.sock`, mode `0600`,
  peer-credentials checked, stale sockets removed on boot) instead of a named
  pipe; keys go to the Secret Service.
- The port, its measurements and its open items are written up in
  [`docs/LINUX-PORT.md`](docs/LINUX-PORT.md).

## Contributing

Issues and PRs are very welcome — new integrations, new emotes, new sounds, bug fixes. See [CONTRIBUTING.md](CONTRIBUTING.md).

What has changed so far is listed in [CHANGELOG.md](CHANGELOG.md); the reasoning behind each entry is in the commit that links to it.

## Credits

Built by [Louis Raillé](https://louisraille.fr) with Claude Code.
Inspired by the notch-companion concepts shared by design studios — this project is independent and not affiliated with any of them.

## License

- **Code:** [MIT](LICENSE) — use it, fork it, learn from it, just keep the copyright notice.
- **Name, Mochi character, icon, sounds and media:** © Louis Raillé, all rights reserved — see [LICENSE-ASSETS.md](LICENSE-ASSETS.md). Shipping your own fork? Give it your own name and character.

<div align="center">

**If Mochi made you smile, a ⭐ helps a lot.**

[Website](https://louis-cfm.github.io/coucou/) · [Privacy](https://louis-cfm.github.io/coucou/privacy.html) · [Terms](https://louis-cfm.github.io/coucou/terms.html) · [Support](https://louis-cfm.github.io/coucou/support.html)

</div>
