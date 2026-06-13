import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { ensureDirs, paths, readJson, writeJsonAtomic } from "./paths.js";

export const DEFAULT_CAM_PORT = 37631;

export function defaultCodexPath() {
  if (process.env.CAM_CODEX_EXE) return process.env.CAM_CODEX_EXE;
  if (process.platform === "win32") {
    const candidate = path.join(os.homedir(), "AppData", "Local", "OpenAI", "Codex", "bin", "codex.exe");
    if (fs.existsSync(candidate)) return candidate;
  }
  throw new Error("Codex execution path not configured. Please set the CAM_CODEX_EXE environment variable or the codexPath in config.json.");
}

export function defaultNodeName() {
  return process.env.CAM_NODE_NAME || os.hostname();
}

export function initConfig({ force = false } = {}) {
  const p = ensureDirs();
  const existing = readJson(p.config, null);
  if (existing && !force) return existing;
  const configuredPort = process.env.CAM_PORT ? Number(process.env.CAM_PORT) : null;
  const port = configuredPort || DEFAULT_CAM_PORT;
  const config = {
    version: 1,
    nodeName: defaultNodeName(),
    bindHost: "127.0.0.1",
    port,
    codexPath: defaultCodexPath(),
    codexHome: process.env.CODEX_HOME || path.join(os.homedir(), ".codex"),
    createdAt: new Date().toISOString(),
  };
  writeJsonAtomic(p.config, config);
  ensureLocalToken();
  const registry = readJson(p.registry, null);
  if (!registry) {
    writeJsonAtomic(p.registry, {
      version: 1,
      nodeName: config.nodeName,
      agents: {},
      peers: {},
      updatedAt: new Date().toISOString(),
    });
  }
  return config;
}

export function loadConfig() {
  const p = ensureDirs();
  const config = readJson(p.config, null) || initConfig();
  ensureLocalToken();
  return config;
}

export function ensureLocalToken() {
  const p = ensureDirs();
  if (!fs.existsSync(p.localToken)) {
    fs.writeFileSync(p.localToken, crypto.randomBytes(32).toString("base64url"), { mode: 0o600 });
  }
  return fs.readFileSync(p.localToken, "utf8").trim();
}

export function localApiBase(config = loadConfig()) {
  const port = config.port || process.env.CAM_PORT || DEFAULT_CAM_PORT;
  if (!port) {
    throw new Error("Configuration Error: CAM port is not configured.");
  }
  return `http://${config.bindHost || "127.0.0.1"}:${port}`;
}

export function allPaths() {
  return paths();
}
