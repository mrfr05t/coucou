// Copies the installer Tauri buries in target/release/bundle/ into
// windows/release/, with the name it ships under. Used by `npm run pack` and by
// the release workflow, so both produce exactly the same file names.

import { readFileSync, mkdirSync, copyFileSync, readdirSync, statSync, chmodSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = join(root, "release");

const { version } = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8"));

// One entry per artifact a platform ships. Tauri sorts bundles into a directory
// per format, so a platform that ships two formats (Linux: deb and AppImage)
// has two entries — there is no single "the installer" to look for.
const PLATFORMS = {
  win32: [
    {
      dir: "nsis",
      match: (f) => f.endsWith("-setup.exe"),
      name: (v) => `Coucou-Windows-${v}-setup.exe`,
      rolling: "Coucou-Windows-setup.exe",
    },
  ],
  linux: [
    {
      dir: "deb",
      match: (f) => f.endsWith(".deb"),
      name: (v) => `Coucou-Linux-${v}.deb`,
      rolling: "Coucou-Linux.deb",
    },
    {
      dir: "appimage",
      match: (f) => f.endsWith(".AppImage"),
      name: (v) => `Coucou-Linux-${v}.AppImage`,
      rolling: "Coucou-Linux.AppImage",
      // An AppImage that cannot execute is a dead download.
      mode: 0o755,
    },
  ],
};

const kinds = PLATFORMS[process.platform];
if (!kinds) {
  // The macOS app is built by NotchBuddy's Xcode project, never from here.
  console.error(`pack.mjs does not package ${process.platform} — the macOS app has its own build.`);
  process.exit(1);
}

const missing = [];
const packed = [];

for (const kind of kinds) {
  const bundleDir = join(root, "target", "release", "bundle", kind.dir);
  let files = [];
  try {
    files = readdirSync(bundleDir).filter(kind.match);
  } catch {
    missing.push(bundleDir);
    continue;
  }
  if (files.length === 0) {
    missing.push(bundleDir);
    continue;
  }

  // Newest wins, in case an older build is still lying around.
  const built = files
    .map((f) => join(bundleDir, f))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];

  mkdirSync(outDir, { recursive: true });
  for (const target of [join(outDir, kind.name(version)), join(outDir, kind.rolling)]) {
    copyFileSync(built, target);
    if (kind.mode) chmodSync(target, kind.mode);
    packed.push({ path: target, size: statSync(target).size });
  }
}

if (missing.length > 0) {
  for (const dir of missing) console.error(`No artifact in ${dir} — run \`npm run tauri build\` first.`);
  process.exit(1);
}

console.log("\n  Installer ready\n");
for (const { path, size } of packed) {
  console.log(`  ${(size / 1024 / 1024).toFixed(2).padStart(7)} MB  ${path}`);
}
console.log("");
