#!/usr/bin/env node
/**
 * Build script for cam.exe using Node.js built-in SEA (Single Executable Application).
 * Replaces the broken `pkg` approach which cannot handle ES Modules / import.meta.
 *
 * Steps:
 *  1. Bundle all ES Module source into a single CJS bundle via esbuild
 *  2. Generate a SEA config JSON
 *  3. Run `node --experimental-sea-config` to produce a blob
 *  4. Copy node.exe -> cam.exe
 *  5. Inject the blob into cam.exe using postject
 *  6. Compile cam-tray.exe via the native csc.exe C# compiler
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

// ── Step 1: Bundle with esbuild into a single CJS file ──────────────────────
console.log("\n[BUILD] Step 1: Bundling with esbuild...");
run(`npx esbuild bin/cam.js --bundle --platform=node --format=cjs --outfile=dist/cam-bundle.cjs --external:fsevents`);
run(`npx esbuild src/daemon-entry.js --bundle --platform=node --format=cjs --outfile=dist/daemon-entry.js --external:fsevents`);

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

// ── Step 4: Copy node.exe to cam.exe ─────────────────────────────────────────
console.log("\n[BUILD] Step 4: Copying node.exe -> dist/cam.exe...");
const nodeExe = process.execPath;
const camExe = path.join(DIST, "cam.exe");
fs.copyFileSync(nodeExe, camExe);

// ── Step 5: Inject blob via postject ─────────────────────────────────────────
console.log("\n[BUILD] Step 5: Injecting blob into cam.exe via postject...");
run(`npx postject dist/cam.exe NODE_SEA_BLOB dist/cam-sea.blob --sentinel-fuse NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2 --overwrite`);

// ── Step 6: Compile cam-tray.exe via csc.exe ─────────────────────────────────
console.log("\n[BUILD] Step 6: Compiling cam-tray.exe via native C# compiler...");
const cscPath = "C:\\Windows\\Microsoft.NET\\Framework64\\v4.0.30319\\csc.exe";
if (fs.existsSync(cscPath)) {
  run(`"${cscPath}" /target:winexe /out:dist\\cam-tray.exe src\\tray\\CamTray.cs`);
} else {
  console.warn("[BUILD] WARNING: csc.exe not found, skipping cam-tray.exe compilation.");
}

// Copy query_threads.py to dist
fs.copyFileSync(path.join(ROOT, "src", "query_threads.py"), path.join(DIST, "query_threads.py"));

console.log("\n[BUILD] ✅ Build complete! Outputs:");
console.log(`  dist/cam.exe`);
console.log(`  dist/cam-tray.exe`);
console.log(`  dist/query_threads.py`);
