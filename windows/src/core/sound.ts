// SoundEngine — port of SoundEngine.swift.
// The 28 WAVs are the macOS app's own files (see SOUNDS_DIR in vite.config.ts);
// they are served at /sounds/<name>.wav. Default volume 0.12, slider range 0–0.2,
// exactly like the Mac player, and several sounds may overlap.
//
// Failures here are invisible by design — a missing sound must never break the
// island — so anything that goes wrong is written to the shared log instead:
// "no AudioContext", "buffers 0/28" and "context suspended" are three different
// bugs, and without a line for each they all look the same from outside.

import { Bridge, IS_TAURI } from "./bridge";

function note(message: string) {
  if (IS_TAURI) void Bridge.log(`sound ${message}`);
}

export const SOUND_NAMES = [
  "peek", "open", "close", "hover", "blip", "slap", "annoyed", "dizzy", "greet",
  "work", "finish", "error", "approval", "question", "approve", "gulp", "tick",
  "send", "love", "pop", "proud", "wink", "yawn", "attach", "think", "search",
  "rate", "sleep",
] as const;

export type SoundName = (typeof SOUND_NAMES)[number];

class SoundEngine {
  enabled = true;
  volume = 0.12;

  private ctx: AudioContext | null = null;
  private master: GainNode | null = null;
  private buffers = new Map<string, AudioBuffer>();
  private loading: Promise<void> | null = null;
  private idleTimer: number | null = null;
  private skips = 0;

  /** Creates the context and decodes every WAV. Safe to call more than once. */
  preload(): Promise<void> {
    if (this.loading) return this.loading;
    this.loading = (async () => {
      const Ctor = window.AudioContext ?? (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
      if (!Ctor) {
        note("this webview has no AudioContext constructor");
        return;
      }
      let ctx: AudioContext;
      try {
        ctx = new Ctor();
      } catch (err) {
        note(`AudioContext refused to start: ${err}`);
        return;
      }
      this.ctx = ctx;
      note(`context created (${ctx.state})`);
      const master = ctx.createGain();
      master.gain.value = this.volume;
      master.connect(ctx.destination);
      this.master = master;
      const failed: string[] = [];
      await Promise.all(
        SOUND_NAMES.map(async (name) => {
          try {
            this.buffers.set(name, await this.loadWav(name, ctx));
          } catch (err) {
            /* a missing sound must never break the island */
            failed.push(`${name}: ${err}`);
          }
        }),
      );
      note(
        `buffers ${this.buffers.size}/${SOUND_NAMES.length}, context ${ctx.state}` +
          (failed.length
            ? ` — failed: ${failed.slice(0, 3).join(", ")}${failed.length > 3 ? ` +${failed.length - 3} more` : ""}`
            : ""),
      );
    })();
    return this.loading;
  }

  /** WebView2 can hand us a suspended context; call after any user input. */
  resume() {
    if (this.idleTimer != null) {
      window.clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
    const ctx = this.ctx;
    if (!ctx) return;
    // A refusal here is the autoplay policy talking: WebKit only honours this
    // from a real gesture, so the reason is worth having in the log.
    void ctx.resume().catch((err) => note(`resume rejected: ${err}`));
  }

  /**
   * Called when the island goes quiet. A running AudioContext keeps an audio
   * thread and its render quantum alive even with nothing playing, which shows
   * up as a steady trickle of CPU on a machine that is supposed to be idle.
   *
   * The delay covers the tail of whatever just played — suspending mid-sound
   * would clip it — and `play()` resumes the context on its own.
   */
  idle() {
    if (!this.ctx || this.ctx.state !== "running" || this.idleTimer != null) return;
    this.idleTimer = window.setTimeout(() => {
      this.idleTimer = null;
      void this.ctx?.suspend();
    }, 1500);
  }

  setVolume(v: number) {
    this.volume = Math.max(0, Math.min(0.2, v));
    if (this.master) this.master.gain.value = this.volume;
  }

  setEnabled(on: boolean) {
    this.enabled = on;
  }

  play(name: SoundName | string) {
    if (!this.enabled) return;
    const ctx = this.ctx;
    const master = this.master;
    const buf = this.buffers.get(name);
    if (!ctx || !master || !buf) {
      this.reportSkip(name, !ctx ? "no AudioContext" : !master ? "no mixer" : `no buffer for "${name}"`);
      return;
    }
    if (this.idleTimer != null) {
      window.clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
    if (ctx.state === "suspended") void ctx.resume().catch((err) => note(`resume rejected: ${err}`));
    if (ctx.state !== "running") this.reportSkip(name, `context ${ctx.state}`);
    const src = ctx.createBufferSource();
    src.buffer = buf;
    src.connect(master);
    src.start();
  }

  /**
   * The WAVs are served at /sounds/<name>.wav — by the dev server in a browser,
   * and by Tauri's asset protocol in the app.
   */
  private async loadWav(name: string, ctx: AudioContext): Promise<AudioBuffer> {
    const res = await fetch(`/sounds/${name}.wav`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return ctx.decodeAudioData(await res.arrayBuffer());
  }

  /** First few failures only — after that the line would just be noise. */
  private reportSkip(name: string, why: string) {
    if (this.skips++ >= 4) return;
    note(`can't play "${name}": ${why}`);
  }
}

export const Sound = new SoundEngine();
