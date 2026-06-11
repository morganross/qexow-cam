import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export function projectRoot() {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
}

export function homeDir() {
  return process.env.CAM_HOME || path.join(os.homedir(), ".codex-agent-manager");
}

export function paths() {
  const root = homeDir();
  return {
    root,
    config: path.join(root, "config.json"),
    registry: path.join(root, "agents.json"),
    mailbox: path.join(root, "mailbox.jsonl"),
    events: path.join(root, "events.jsonl"),
    daemon: path.join(root, "daemon.json"),
    pid: path.join(root, "daemon.pid"),
    tunnels: path.join(root, "tunnels.json"),
    secretsDir: path.join(root, "secrets"),
    localToken: path.join(root, "secrets", "local-api-token"),
    logsDir: path.join(root, "logs"),
    daemonLog: path.join(root, "logs", "daemon.log"),
  };
}

export function ensureDirs() {
  const p = paths();
  for (const dir of [p.root, p.secretsDir, p.logsDir]) {
    fs.mkdirSync(dir, { recursive: true });
  }
  return p;
}

export function readJson(file, fallback) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") return fallback;
    throw error;
  }
}

export function writeJsonAtomic(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const tmp = `${file}.${process.pid}.${Date.now()}.tmp`;
  fs.writeFileSync(tmp, `${JSON.stringify(value, null, 2)}\n`, "utf8");
  fs.renameSync(tmp, file);
}

export function appendJsonl(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.appendFileSync(file, `${JSON.stringify(value)}\n`, "utf8");
}

export function readJsonl(file) {
  try {
    return fs.readFileSync(file, "utf8")
      .split(/\r?\n/)
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
}
