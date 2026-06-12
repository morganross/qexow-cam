#!/usr/bin/env node
/**
 * Build script for cam.exe — a Node.js Single Executable Application (SEA).
 *
 * Output: dist/cam.exe  — the ONE executable the user installs.
 *         dist/tray_windows_release.exe — systray helper, installed alongside cam.exe.
 *
 * Steps:
 *  1. Bundle all ES Module source into a single CJS bundle via esbuild
 *  2. Generate a SEA config JSON
 *  3. Run `node --experimental-sea-config` to produce a blob
 *  4. Copy node.exe → dist/cam.exe
 *  5. Inject the blob into dist/cam.exe using postject
 *  6. Copy systray2 helper binary to dist/
 */

import { execSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");
const DIST = path.join(ROOT, "dist");

fs.mkdirSync(DIST, { recursive: true });

function run(cmd, opts = {}) {
  console.log(`> ${cmd}`);
  const result = spawnSync(cmd, { shell: true, stdio: "inherit", cwd: ROOT, ...opts });
  if (result.status !== 0) {
    console.error(`Command failed: ${cmd}`);
    process.exit(result.status || 1);
  }
}

// ── Step 0: Pre-compile remote query scripts into JS strings ─────────────────
console.log("\n[BUILD] Step 0: Compiling remote query scripts...");
const pyScript = fs.readFileSync(path.join(ROOT, "src", "remote_query_threads.py"), "utf8");
const jsScript = fs.readFileSync(path.join(ROOT, "src", "remote_query_threads.js"), "utf8");
const remoteScriptsJs = `// Auto-generated during build. Do not edit.
export const pyRemoteScript = ${JSON.stringify(pyScript)};
export const jsRemoteScript = ${JSON.stringify(jsScript)};
`;
fs.writeFileSync(path.join(ROOT, "src", "remote_scripts.js"), remoteScriptsJs);

// ── Step 1: Bundle with esbuild into a single CJS file ──────────────────────
console.log("\n[BUILD] Step 1: Bundling with esbuild...");
run(`npx esbuild bin/cam.js --bundle --platform=node --format=cjs --outfile=dist/cam-bundle.cjs --external:fsevents`);

// ── Step 2: Write SEA config ─────────────────────────────────────────────────
console.log("\n[BUILD] Step 2: Writing SEA config...");
const seaConfig = {
  main: path.join(DIST, "cam-bundle.cjs"),
  output: path.join(DIST, "cam-sea.blob"),
  disableExperimentalSEAWarning: true,
};
fs.writeFileSync(path.join(DIST, "sea-config.json"), JSON.stringify(seaConfig, null, 2));

// ── Step 3: Generate SEA blob ────────────────────────────────────────────────
console.log("\n[BUILD] Step 3: Generating SEA blob...");
run(`node --experimental-sea-config dist/sea-config.json`);

// ── Step 4: Copy node.exe → dist/cam.exe ─────────────────────────────────────
console.log("\n[BUILD] Step 4: Copying node.exe -> dist/cam.exe...");
const nodeExe = process.execPath;
const camExe = path.join(DIST, "cam.exe");
fs.copyFileSync(nodeExe, camExe);

// ── Step 5: Inject blob into cam.exe via postject ─────────────────────────────
console.log("\n[BUILD] Step 5: Injecting blob into cam.exe via postject...");
run(`npx postject dist/cam.exe NODE_SEA_BLOB dist/cam-sea.blob --sentinel-fuse NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2 --overwrite`);

// ── Step 6: Copy systray2 helper binary ──────────────────────────────────────
console.log("\n[BUILD] Step 6: Copying systray2 helper binary...");
const systrayBin = path.join(ROOT, "node_modules", "systray2", "traybin", "tray_windows_release.exe");
const systrayDest = path.join(DIST, "tray_windows_release.exe");
if (fs.existsSync(systrayBin)) {
  fs.copyFileSync(systrayBin, systrayDest);
  console.log(`[BUILD] Copied tray_windows_release.exe to dist/`);
} else {
  console.warn("[BUILD] WARNING: tray_windows_release.exe not found in node_modules/systray2/traybin");
}

// ── Step 7: Clean up intermediate files ──────────────────────────────────────
console.log("\n[BUILD] Step 7: Cleaning up intermediate dist files...");
const keep = new Set(["cam.exe", "tray_windows_release.exe"]);
for (const file of fs.readdirSync(DIST)) {
  if (!keep.has(file)) {
    try {
      fs.rmSync(path.join(DIST, file), { recursive: true, force: true });
    } catch (e) {
      console.warn(`[BUILD] Could not delete ${file}: ${e.message}`);
    }
  }
}

console.log("\n[BUILD] ✅ Build complete! Outputs:");
console.log(`  dist/cam.exe                   — main application (Node.js SEA)`);
console.log(`  dist/tray_windows_release.exe  — system tray helper`);
