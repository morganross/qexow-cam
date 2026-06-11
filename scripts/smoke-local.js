#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const cam = path.join(root, "bin", "cam.js");
const node = process.execPath;
const env = { ...process.env, CAM_HOME: path.join(root, ".cam", "smoke") };

function run(args, options = {}) {
  const result = spawnSync(node, [cam, ...args], {
    cwd: root,
    env,
    encoding: "utf8",
    timeout: options.timeout || 30000,
  });
  if (result.status !== 0) {
    throw new Error(`cam ${args.join(" ")} failed\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`);
  }
  return result.stdout.trim();
}

console.log(run(["init"]));
console.log(run(["doctor"]));
console.log("Smoke setup complete. Start the daemon separately before delivery tests.");
