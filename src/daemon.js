import "./security.js";
import { enforceSpawnBlocks } from "./security.js";
enforceSpawnBlocks();

if (process.argv.includes("--headless")) {
  process.env.CAM_HEADLESS = "1";
}

import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import net from "node:net";
import { spawn } from "node:child_process";
import { AppServerClient, textInput } from "./app-server.js";
import { ensureLocalToken, loadConfig } from "./config.js";
import { discoverThreads } from "./thread-discovery.js";
import {
  appendEvent,
  appendMailbox,
  appendTestEvent,
  canonicalizeTrustedInventoryAgents,
  getAgent,
  getPeer,
  listPeers,
  listAgents,
  loadRegistry,
  saveRegistry,
  saveLocalDiscoveries,
  saveRemoteDiscoverySnapshot,
  markMailboxSurfaced,
  pendingMailbox,
  readMailbox,
  setAgent,
  upsertPeer,
  upsertAgent,
} from "./registry.js";
import { classifyThreadDiscovery, discoveryCounts } from "./discovery-policy.js";
import { paths, writeJsonAtomic, readJson } from "./paths.js";
import { logEvent, enforceRetention } from "./logger.js";
import { bootstrapAntigravity } from "./antigravity.js";
import { refreshLocalRegistryFromThreads } from "./discovery-refresh.js";
import {
  buildNodeDiscoveryPrompt,
  NODE_DISCOVERY_MARKER,
  NODE_DISCOVERY_REPLY_TYPE,
  NODE_DISCOVERY_REQUEST_TYPE,
  parseNodeDiscoveryEvidence,
} from "./node-discovery.js";
import { discoverPeerFactsFromMarkdown, discoverSshKeyPathsFromMarkdown } from "./peer-doc-discovery.js";

const CAM_TEST_MAILBOX_AGENT = "CAM test, Kexau CAM test suite mailbox";
const MAILBOX_ONLY_THREAD_SOURCES = new Set(["mailbox", "gui-only"]);
const CAM_VERSION = "2.1.52";
const STRICT_THREAD_NOT_FOUND = /thread not found/i;
const GUI_TEST_MESSAGE_TYPE = "cam-gui-test";
const GUI_TEST_REPLY_MESSAGE_TYPE = "cam-gui-test-reply";
const REMOTE_SYNC_INTERVAL_MS = 5 * 60 * 1000;
const REMOTE_INVENTORY_TIMEOUT_MS = 120000;
const DEFAULT_REMOTE_ROOTS = [
  "/opt/qexow-cam",
  "/home/ubuntu/codex-agent-manager",
  "/root/codex-agent-manager",
];

export function showWindowsAlert(title, message, iconType = "error") {
  // Completely disabled by Security Block 5: Eradication of External Scripts
  // We do not use VBScript or mshta.exe ever.
  return;
}

export function showYesNoDialog(title, message) {
  // Silent autoconfirm to avoid spawning flashing PowerShell windows
  return true;
}

export function isPortInUse(port, host) {
  return new Promise((resolve) => {
    const socket = new net.Socket();
    socket.setTimeout(1000);
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("timeout", () => {
      socket.destroy();
      resolve(false);
    });
    socket.once("error", () => {
      resolve(false);
    });
    socket.connect(port, host);
  });
}

export function gracefulShutdown(port, host) {
  return new Promise((resolve) => {
    const req = http.request({
      hostname: host,
      port: port,
      path: "/shutdown",
      method: "POST",
      timeout: 1000,
    }, (res) => {
      resolve(res.statusCode === 200);
    });
    req.on("error", () => {
      resolve(false);
    });
    req.on("timeout", () => {
      req.destroy();
      resolve(false);
    });
    req.end();
  });
}

export async function killProcessOnPort(port, host) {
  // Try graceful shutdown first
  await gracefulShutdown(port, host);
  
  // Wait up to 1.5s for the port to be released
  for (let i = 0; i < 15; i++) {
    if (!(await isPortInUse(port, host))) {
      return true;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  
  return !(await isPortInUse(port, host));
}

function sq(val) {
  if (val === null || val === undefined) return "''";
  return "'" + String(val).replace(/'/g, "'\\''") + "'";
}

function json(res, status, body) {
  const payload = JSON.stringify(body, null, 2);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(payload),
  });
  res.end(payload);
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    let body = "";
    req.setEncoding("utf8");
    req.on("data", (chunk) => {
      body += chunk;
      if (body.length > 2_000_000) reject(new Error("request body too large"));
    });
    req.on("end", () => {
      if (!body) return resolve({});
      try {
        resolve(JSON.parse(body));
      } catch (error) {
        reject(new Error(`invalid JSON body: ${error.message}`));
      }
    });
    req.on("error", reject);
  });
}

function normalizeEffort(value) {
  if (value === undefined || value === null) return null;
  const text = String(value).trim().toLowerCase().replace(/[\s_]+/g, "-");
  const aliases = new Map([
    ["minimal", "minimal"],
    ["low", "low"],
    ["medium", "medium"],
    ["high", "high"],
    ["xhigh", "xhigh"],
    ["x-high", "xhigh"],
    ["extra-high", "xhigh"],
  ]);
  const normalized = aliases.get(text);
  if (!normalized) throw new Error(`invalid effort '${value}'; expected minimal|low|medium|high|xhigh`);
  return normalized;
}

function normalizeServiceTier(value) {
  if (value === undefined || value === null) return null;
  const tier = String(value).trim();
  if (!tier) return null;
  if (tier.toLowerCase() === "standard") {
    throw new Error("service tier 'standard' is not an app-server tier; omit serviceTier for standard speed");
  }
  if (tier.toLowerCase() === "default") {
    throw new Error("service tier 'default' is invalid; omit serviceTier for standard speed");
  }
  return tier;
}

export class AgentManagerDaemon {
  constructor(config = loadConfig()) {
    this.config = config;
    this.token = ensureLocalToken();
    this.appServer = new AppServerClient({
      codexPath: config.codexPath,
      log: (type, payload) => this.log(type, payload),
    });
    this.startedAt = new Date().toISOString();
    this.server = null;
    this.threadToAgent = new Map();
    this.ensuringThreads = new Map();
    this.mailboxListeners = [];
    this.threadSyncInterval = null;
    this.peerSyncInterval = null;
    this.skippedThreadReasons = new Map();
    this.peerProbeAttempts = new Map();
    this.peerSyncInflight = new Map();
    this.peerDiscoveryPassPromise = null;
    this.docKeyPaths = [];
  }

  queueMessage(message) {
    appendMailbox(message);
    const listeners = [...this.mailboxListeners];
    for (const listener of listeners) {
      try {
        listener(message);
      } catch (err) {
        this.log("mailbox.listener.error", { error: err.message });
      }
    }
  }

  log(type, payload = {}) {
    logEvent(type, payload);
    // VBScript message box popups are disabled to prevent recursive process storms.
    /*
    if (type.includes("error") || type.includes("failed")) {
      const msg = payload.error || payload.message || JSON.stringify(payload);
      showWindowsAlert(`CAM Daemon Error [${type}]`, msg, "error");
    } else if (type.includes("warn") || type.includes("warning")) {
      const msg = payload.warn || payload.warning || payload.message || JSON.stringify(payload);
      showWindowsAlert(`CAM Daemon Warning [${type}]`, msg, "warning");
    }
    */
  }

  async start() {
    try {
      enforceRetention();
    this.log("daemon.startup.initiating", { port: this.config.port, bindHost: this.config.bindHost, nodeName: this.config.nodeName });
    const port = this.config.port;
    const host = this.config.bindHost || "127.0.0.1";

    if (await isPortInUse(port, host)) {
      const title = "CAM Port Conflict";
      const message = "Port in use. Do you want to close existing CAM?";
      if (showYesNoDialog(title, message)) {
        this.log("daemon.port_conflict.resolving", { port });
        const killed = await killProcessOnPort(port, host);
        if (!killed) {
          throw new Error(`Port ${port} is in use and could not be freed.`);
        }
        this.log("daemon.port_conflict.resolved", { port });
      } else {
        throw new Error(`Port ${port} is already in use. Startup aborted by user.`);
      }
    }

    // Bind HTTP server immediately to satisfy health checks during slow initial bootstrap
    this.server = http.createServer((req, res) => this.#handle(req, res));
    await new Promise((resolve, reject) => {
      this.server.once("error", reject);
      this.server.listen(this.config.port, this.config.bindHost, resolve);
    });
    writeJsonAtomic(paths().daemon, {
      pid: process.pid,
      nodeName: this.config.nodeName,
      url: `http://${this.config.bindHost}:${this.config.port}`,
      startedAt: this.startedAt,
      codexPath: this.config.codexPath,
    });
    fs.writeFileSync(paths().pid, String(process.pid));
    this.log("daemon.started", { pid: process.pid, url: `http://${this.config.bindHost}:${this.config.port}` });

    // Initialize Native Antigravity Integration after binding port
    bootstrapAntigravity((type, payload) => this.log(type, payload));

    await this.appServer.start();
    this.#ensureBuiltinMailboxAgents();
    this.#restorePeerTransportFromBackups();
    void this.#runPeerDiscoveryPass("startup");
    for (const agent of listAgents(this.config)) {
      if (agent.threadId) this.threadToAgent.set(agent.threadId, agent.name);
    }
    await this.syncActiveThreads();
    this.threadSyncInterval = setInterval(() => {
      void this.syncActiveThreads();
    }, 30000);
    this.peerSyncInterval = setInterval(() => {
      void this.#runPeerDiscoveryPass("interval");
    }, REMOTE_SYNC_INTERVAL_MS);
    this.appServer.on("turn/started", ({ threadId, turn }) => {
      const name = this.threadToAgent.get(threadId);
      if (name) setAgent(this.config, name, { status: "active", activeTurnId: turn.id });
    });
    this.appServer.on("turn/completed", ({ threadId, turn }) => {
      const name = this.threadToAgent.get(threadId);
      if (name) {
        setAgent(this.config, name, { status: "idle", activeTurnId: null, lastTurnId: turn.id, lastError: null });
        this.#checkInboxListeners();
        void this.#processNextQueuedMessage(name);
      }
    });
    this.appServer.on("thread/status/changed", ({ threadId, status }) => {
      const name = this.threadToAgent.get(threadId);
      if (name) {
        const current = getAgent(this.config, name);
        const statusType = status?.type || "unknown";
        const changes = { status: current?.lastError && statusType === "idle" ? "error" : statusType };
        if (statusType !== "active") changes.activeTurnId = null;
        setAgent(this.config, name, changes);
        if (statusType !== "active") {
          this.#checkInboxListeners();
        }
        if (statusType === "idle") {
          void this.#processNextQueuedMessage(name);
        }
      }
    });
    this.appServer.on("app-server/error", (payload = {}) => {
      this.log("app-server.error", payload);
      const name = this.threadToAgent.get(payload.threadId);
      if (name) {
        setAgent(this.config, name, {
          status: "error",
          activeTurnId: null,
          lastError: payload.error?.message || "app-server error",
        });
        this.#checkInboxListeners();
        void this.#processNextQueuedMessage(name);
      }
    });

    } catch (error) {
      this.log("daemon.startup.failed_zombie", { error: error.message });
      console.error("Daemon startup failed. Entering Block 8 Zombified Standby Mode to prevent terminal storms.", error);
      // Enter zombified standby loop
      setInterval(() => {}, 60000);
    }
  }

  async stop() {
    this.log("daemon.shutdown.initiating", { reason: "requested" });
    if (this.threadSyncInterval) {
      clearInterval(this.threadSyncInterval);
      this.threadSyncInterval = null;
    }
    if (this.peerSyncInterval) {
      clearInterval(this.peerSyncInterval);
      this.peerSyncInterval = null;
    }
    await new Promise((resolve) => this.server?.close(resolve));
    this.appServer.stop();
    this.log("daemon.shutdown.complete");
  }

  #serveStatusUI(req, res) {
    const config = this.config;
    const agents = listAgents(config);
    const uptime = Math.floor((Date.now() - new Date(this.startedAt).getTime()) / 1000);
    const uptimeStr = uptime < 60 ? `${uptime}s` : uptime < 3600 ? `${Math.floor(uptime/60)}m ${uptime%60}s` : `${Math.floor(uptime/3600)}h ${Math.floor((uptime%3600)/60)}m`;
    
    // Read last 50 lines of daemon log
    let logLines = [];
    try {
      const logFile = paths().daemonLog;
      if (fs.existsSync(logFile)) {
        const raw = fs.readFileSync(logFile, "utf8");
        logLines = raw.split(/\r?\n/).filter(Boolean).slice(-50);
      }
    } catch (_) {}

    const agentRows = agents.map(a => `
      <tr>
        <td>${escHtml(a.name)}</td>
        <td>${escHtml(a.status || "")}</td>
        <td class="mono">${escHtml(a.threadId || "—")}</td>
        <td>${escHtml(a.modelProvider || "")}</td>
        <td>${escHtml(a.model || "")}</td>
      </tr>`).join("") || `<tr><td colspan="5" class="empty">No agents registered.</td></tr>`;

    const logHtml = logLines.map(l => `<div class="log-line">${escHtml(l)}</div>`).join("") || `<div class="log-line empty">No logs yet.</div>`;

    function escHtml(s) {
      return String(s ?? "").replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;").replace(/"/g,"&quot;");
    }

    const html = `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<meta http-equiv="refresh" content="10">
<title>Qexow CAM Status</title>
<style>
  @import url('https://fonts.googleapis.com/css2?family=Inter:wght@300;400;600;700&display=swap');
  *{box-sizing:border-box;margin:0;padding:0}
  body{font-family:'Inter',sans-serif;background:#0d0d1a;color:#e0e0f0;min-height:100vh}
  header{background:linear-gradient(135deg,#0a0a18 0%,#12122a 100%);border-bottom:1px solid #1e1e3a;padding:18px 28px;display:flex;align-items:center;gap:16px}
  header .logo{width:36px;height:36px;background:radial-gradient(circle,#00c853 30%,#0d47a1 100%);border-radius:50%;box-shadow:0 0 12px #00c85355}
  header h1{font-size:1.25rem;font-weight:700;color:#fff;letter-spacing:.5px}
  header .sub{font-size:.8rem;color:#7878a0;margin-top:2px}
  .badge{display:inline-block;padding:2px 10px;border-radius:12px;font-size:.72rem;font-weight:600;margin-left:10px}
  .badge.up{background:#00c85320;color:#00c853;border:1px solid #00c85340}
  .badge.down{background:#f4433620;color:#f44336;border:1px solid #f4433640}
  main{padding:24px 28px;display:grid;gap:20px}
  .card{background:#12122a;border:1px solid #1e1e3a;border-radius:12px;overflow:hidden}
  .card-header{padding:14px 18px;background:#0e0e24;border-bottom:1px solid #1e1e3a;font-size:.82rem;font-weight:600;color:#9090c0;letter-spacing:1px;text-transform:uppercase}
  .stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:1px;background:#1e1e3a}
  .stat{background:#12122a;padding:18px 20px}
  .stat .label{font-size:.72rem;color:#7878a0;text-transform:uppercase;letter-spacing:.5px;margin-bottom:6px}
  .stat .value{font-size:1.4rem;font-weight:700;color:#fff}
  .stat .value.green{color:#00c853}
  .stat .value.red{color:#f44336}
  table{width:100%;border-collapse:collapse;font-size:.84rem}
  th{padding:10px 14px;text-align:left;color:#7878a0;font-weight:600;font-size:.72rem;text-transform:uppercase;border-bottom:1px solid #1e1e3a}
  td{padding:10px 14px;border-bottom:1px solid #0e0e24;vertical-align:middle}
  tr:last-child td{border-bottom:none}
  td.mono{font-family:monospace;font-size:.78rem;color:#a0a0c8}
  td.empty{color:#5050a0;font-style:italic;text-align:center;padding:20px}
  .log-area{padding:14px 18px;max-height:260px;overflow-y:auto;background:#080810}
  .log-line{font-family:monospace;font-size:.75rem;color:#80e0a0;line-height:1.5;white-space:pre-wrap;word-break:break-all}
  .log-line.empty{color:#5050a0;font-style:italic}
  .refresh-note{text-align:center;font-size:.7rem;color:#5050a0;padding:12px;border-top:1px solid #1e1e3a}
  a{color:#4c9eff;text-decoration:none}
</style>
</head>
<body>
<header>
  <div class="logo"></div>
  <div>
    <h1>Qexow CAM Dashboard <span class="badge up">LIVE</span></h1>
    <div class="sub">Agent Management System · port ${escHtml(String(config.port))} · node <strong>${escHtml(config.nodeName)}</strong></div>
  </div>
</header>
<main>
  <div class="card">
    <div class="stats">
      <div class="stat"><div class="label">Daemon Status</div><div class="value green">● Running</div></div>
      <div class="stat"><div class="label">Uptime</div><div class="value">${escHtml(uptimeStr)}</div></div>
      <div class="stat"><div class="label">Agents</div><div class="value">${agents.length}</div></div>
      <div class="stat"><div class="label">Started At</div><div class="value" style="font-size:.9rem">${escHtml(this.startedAt)}</div></div>
    </div>
  </div>

  <div class="card">
    <div class="card-header">Registered Agents</div>
    <table>
      <thead><tr><th>Name</th><th>Status</th><th>Thread / Session ID</th><th>Provider</th><th>Model</th></tr></thead>
      <tbody>${agentRows}</tbody>
    </table>
  </div>

  <div class="card">
    <div class="card-header">Live Daemon Log (last 50 lines)</div>
    <div class="log-area">${logHtml}</div>
  </div>

  <div class="refresh-note">Auto-refreshes every 10 seconds · <a href="/status-ui">Refresh now</a></div>
</main>
</body>
</html>`;

    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-cache" });
    res.end(html);
  }

  async #handle(req, res) {

    const start = Date.now();
    res.on("finish", () => {
      this.log("http.request.complete", {
        method: req.method,
        url: req.url,
        statusCode: res.statusCode,
        durationMs: Date.now() - start
      });
    });

    try {
      if (req.url === "/health" && req.method === "GET") {
        return json(res, 200, {
          ok: true,
          version: CAM_VERSION,
          nodeName: this.config.nodeName,
          startedAt: this.startedAt,
          appServerInitialized: this.appServer.initialized,
        });
      }

      // Status UI page — served without auth so the browser can load it after tray click
      if (req.url === "/status-ui" && req.method === "GET") {
        if (process.env.CAM_HEADLESS === "1") {
          return json(res, 403, { ok: false, error: "Status UI is disabled in headless mode." });
        }
        return this.#serveStatusUI(req, res);
      }

      if (!this.#authorized(req)) return json(res, 401, { ok: false, error: "unauthorized" });

      const url = new URL(req.url, `http://${req.headers.host || "localhost"}`);
      if (url.pathname === "/agents" && req.method === "GET") {
        return json(res, 200, { ok: true, agents: listAgents(this.config) });
      }
      if (url.pathname === "/peers" && req.method === "GET") {
        return json(res, 200, this.#peerDiagnosticsPayload());
      }
      if (url.pathname === "/agents/create" && req.method === "POST") {
        const body = await readBody(req);
        const agent = upsertAgent(this.config, {
          name: body.name,
          cwd: body.cwd,
          threadId: body.threadId || null,
          model: body.model ?? null,
          modelProvider: body.modelProvider ?? null,
          effort: normalizeEffort(body.effort),
          serviceTier: normalizeServiceTier(body.serviceTier),
          threadSource: body.threadSource || "codex",
          status: body.threadId ? "registered" : "unbound",
        });
        if (agent.threadId) this.threadToAgent.set(agent.threadId, agent.name);
        appendEvent("agent.created", { agent });
        return json(res, 200, { ok: true, agent });
      }
      if (url.pathname === "/agents/resume" && req.method === "POST") {
        const body = await readBody(req);
        const name = body.name;
        const agent = getAgent(this.config, name);
        const ensuredAgent = await this.#ensureThread(body.name);
        return json(res, 200, { ok: true, agent: ensuredAgent });
      }
      if (url.pathname === "/agents/set-model" && req.method === "POST") {
        const body = await readBody(req);
        if (body.recreate) {
          throw new Error("recreate is forbidden; model changes must preserve the existing chat/session/agent mapping");
        }
        const name = body.name;
        const agent = getAgent(this.config, name);
        if (!agent) throw new Error(`unknown agent: ${body?.name}`);

        const changes = {};
        if ("model" in body) changes.model = body.model ?? null;
        if ("modelProvider" in body) changes.modelProvider = body.modelProvider ?? null;
        if ("effort" in body) changes.effort = normalizeEffort(body.effort);
        if ("serviceTier" in body) changes.serviceTier = normalizeServiceTier(body.serviceTier);
        const nextAgent = setAgent(this.config, body.name, changes);
        return json(res, 200, { ok: true, agent: nextAgent });
      }
      if (url.pathname === "/agents/set-status" && req.method === "POST") {
        const body = await readBody(req);
        const agent = setAgent(this.config, body.name, { status: body.status });
        this.#checkInboxListeners();
        return json(res, 200, { ok: true, agent });
      }
      if (url.pathname === "/agents/read" && req.method === "GET") {
        const name = url.searchParams.get("name");
        const includeTurns = url.searchParams.get("includeTurns") !== "false";
        const targetAgentObj = getAgent(this.config, name);
        if (targetAgentObj && targetAgentObj.threadSource === "antigravity") {
          const targetAgentUuid = targetAgentObj.threadId;
          if (!targetAgentUuid) {
            throw new Error(`Target Antigravity agent ${name} is missing a conversation UUID.`);
          }
          let extractedTurns = [];
          if (includeTurns) {
            const turnsParam = url.searchParams.get("turns");
            const requestedTurns = turnsParam ? parseInt(turnsParam, 10) : 0;
            if (requestedTurns > 0) {
              const brainDir = path.join(os.homedir(), ".gemini", "antigravity", "brain");
              const transcriptPath = path.join(brainDir, targetAgentUuid, ".system_generated", "logs", "transcript_full.jsonl");
              if (fs.existsSync(transcriptPath)) {
                try {
                  const content = fs.readFileSync(transcriptPath, "utf8");
                  const lines = content.split(/\r?\n/).filter(Boolean);
                  
                  let allTurns = [];
                  let currentTurn = { input: "", thought: "", tool_calls: "", output: "" };
                  
                  const truncate = (str, limit = 5000) => str.length > limit ? str.substring(0, limit) + "... (truncated)" : str;

                  for (const line of lines) {
                    try {
                      const step = JSON.parse(line);
                      if (step.type === "USER_INPUT") {
                        if (currentTurn.input || currentTurn.thought) allTurns.push(currentTurn);
                        currentTurn = { input: truncate(step.content || ""), thought: "", tool_calls: "", output: "" };
                      } else if (step.type === "PLANNER_RESPONSE") {
                        if (step.thinking) currentTurn.thought += step.thinking + "\n";
                        if (step.tool_calls && step.tool_calls.length > 0) {
                          currentTurn.tool_calls += JSON.stringify(step.tool_calls, null, 2) + "\n";
                        }
                      } else if (step.status === "DONE" && step.content) {
                        currentTurn.output += `[${step.type}] ${truncate(step.content)}\n`;
                      }
                    } catch (e) {}
                  }
                  if (currentTurn.input || currentTurn.thought) allTurns.push(currentTurn);
                  
                  const selectedTurns = allTurns.slice(-requestedTurns);
                  extractedTurns = selectedTurns.map(t => {
                    let formatted = "";
                    if (t.input) formatted += `--- TRIGGER ---\n${t.input.trim()}\n\n`;
                    if (t.thought) formatted += `--- THOUGHT ---\n${t.thought.trim()}\n\n`;
                    if (t.tool_calls) formatted += `--- COMMANDS ---\n${t.tool_calls.trim()}\n\n`;
                    if (t.output) formatted += `--- OUTPUT ---\n${t.output.trim()}`;
                    return { content: formatted.trim() };
                  });
                } catch (e) {
                  this.log("agent.read.transcript_error", { error: e.message });
                }
              }
            }
          }

          return json(res, 200, {
            ok: true,
            agent: targetAgentObj,
            thread: { id: targetAgentUuid, status: { type: targetAgentObj.status || "idle" }, turns: extractedTurns }
          });
        }
        const agent = getAgent(this.config, name);
        const ensuredAgent = await this.#ensureThread(name);
        let thread;
        try {
          thread = await this.appServer.request("thread/read", {
            threadId: ensuredAgent.threadId,
            includeTurns,
          }, 60000);
        } catch (error) {
          if (!includeTurns || !/not materialized yet|includeTurns is unavailable/.test(error.message)) throw error;
          thread = await this.appServer.request("thread/read", {
            threadId: ensuredAgent.threadId,
            includeTurns: false,
          }, 60000);
          thread.readWarning = error.message;
        }
        return json(res, 200, { ok: true, agent: ensuredAgent, thread });
      }
      if (url.pathname === "/send" && req.method === "POST") {
        const body = await readBody(req);
        const result = await this.#sendMessage(body);
        return json(res, 200, { ok: result.ok !== false, ...result });
      }
      if (url.pathname === "/tests/pass" && req.method === "POST") {
        const body = await readBody(req);
        if (!body?.correlationId) throw new Error("correlationId is required");
        appendTestEvent(body.correlationId, "passed", {
          agentName: body.agentName || null,
          semanticCheck: body.semanticCheck || null,
          passedAt: new Date().toISOString(),
        });
        return json(res, 200, { ok: true });
      }
      if (url.pathname === "/inbox" && req.method === "GET") {
        const agent = url.searchParams.get("agent");
        const wait = Number(url.searchParams.get("wait") || 0);

        if (agent) {
          await this.#harvestRemoteMailbox(agent);
        }
        const messages = readMailbox(agent);
        if (messages.length > 0 || wait <= 0) {
          return json(res, 200, { ok: true, messages });
        }

        // Smart Long Polling: check if any other agent is active
        const otherActive = listAgents(this.config).some(a => a.name !== agent && a.status === "active");
        if (!otherActive) {
          return json(res, 200, { ok: true, messages: [] });
        }

        // Hold the response! (Long Polling)
        return new Promise((resolve) => {
          let resolved = false;
          let polling = false;
          let pollTimer = null;

          const listener = (msg) => {
            if (resolved) return;
            if (msg === null) {
              resolved = true;
              clearInterval(pollTimer);
              clearTimeout(timer);
              const idx = this.mailboxListeners.indexOf(listener);
              if (idx >= 0) this.mailboxListeners.splice(idx, 1);
              json(res, 200, { ok: true, messages: readMailbox(agent) });
              resolve();
              return;
            }
            if (!agent || msg.targetAgent === agent) {
              resolved = true;
              clearInterval(pollTimer);
              clearTimeout(timer);
              const idx = this.mailboxListeners.indexOf(listener);
              if (idx >= 0) this.mailboxListeners.splice(idx, 1);
              json(res, 200, { ok: true, messages: readMailbox(agent) });
              resolve();
            }
          };

          this.mailboxListeners.push(listener);

          pollTimer = setInterval(async () => {
            if (resolved || polling || !agent) return;
            polling = true;
            try {
              const imported = await this.#harvestRemoteMailbox(agent);
              if (resolved || !imported.length) return;
              resolved = true;
              clearInterval(pollTimer);
              clearTimeout(timer);
              const idx = this.mailboxListeners.indexOf(listener);
              if (idx >= 0) this.mailboxListeners.splice(idx, 1);
              json(res, 200, { ok: true, messages: readMailbox(agent) });
              resolve();
            } finally {
              polling = false;
            }
          }, 2000);

          const timer = setTimeout(() => {
            if (resolved) return;
            resolved = true;
            clearInterval(pollTimer);
            const idx = this.mailboxListeners.indexOf(listener);
            if (idx >= 0) this.mailboxListeners.splice(idx, 1);
            json(res, 200, { ok: true, messages: [] });
            resolve();
          }, Math.min(wait, 30) * 1000);
        });
      }
      if (url.pathname === "/logs" && req.method === "GET") {
        const rows = fs.existsSync(paths().daemonLog)
          ? fs.readFileSync(paths().daemonLog, "utf8").split(/\r?\n/).filter(Boolean).slice(-200).map((line) => JSON.parse(line))
          : [];
        return json(res, 200, { ok: true, logs: rows });
      }
      if (url.pathname === "/nodes/discover" && req.method === "POST") {
        const body = await readBody(req);
        const result = await this.#discoverPeer(body);
        return json(res, 200, { ok: result.ok !== false, ...result });
      }
      if (url.pathname === "/nodes/sync" && req.method === "POST") {
        const body = await readBody(req);
        const result = await this.syncPeer(body?.peerName || null);
        return json(res, 200, { ok: result.ok !== false, ...result });
      }
      if (url.pathname === "/shutdown" && req.method === "POST") {
        json(res, 200, { ok: true });
        setTimeout(() => this.stop().then(() => process.exit(0)), 50);
        return;
      }

      return json(res, 404, { ok: false, error: "not found" });
    } catch (error) {
      this.log("request.error", { url: req.url, error: error.stack || error.message });
      return json(res, 500, { ok: false, error: error.message });
    }
  }

  #authorized(req) {
    const header = req.headers.authorization || "";
    return header === `Bearer ${this.token}`;
  }

  async #ensureThread(name, options = {}) {
    if (this.ensuringThreads.has(name)) {
      return this.ensuringThreads.get(name);
    }
    const promise = (async () => {
      let agent = getAgent(this.config, name);
      if (!agent) throw new Error(`unknown agent: ${name}`);
      if (agent.threadSource === "antigravity") {
        if (!agent.threadId) {
          throw new Error(`Antigravity agent ${name} is missing a threadId/conversation UUID.`);
        }
        return agent;
      }
      if (agent.threadId && agent.activeTurnId && agent.status === "active") {
        this.threadToAgent.set(agent.threadId, agent.name);
        return agent;
      }
      if (agent.threadId) {
        try {
          const resumed = await this.appServer.request("thread/resume", {
            threadId: agent.threadId,
            cwd: agent.cwd,
            excludeTurns: true,
            persistExtendedHistory: false,
          }, 60000);
          this.threadToAgent.set(agent.threadId, agent.name);
          const statusType = resumed.thread?.status?.type || "idle";
          const changes = { status: statusType };
          if (statusType !== "active") changes.activeTurnId = null;
          agent = setAgent(this.config, name, changes);
          return agent;
        } catch (error) {
          this.log("thread.resume.failed", { agent: name, threadId: agent.threadId, error: error.message });
          if (options.strict && STRICT_THREAD_NOT_FOUND.test(error.message)) {
            setAgent(this.config, name, { status: "stale", activeTurnId: null, lastError: error.message });
            throw error;
          }
        }
      }
      const created = await this.#startThread(agent);
      const threadId = created.thread.id;
      this.threadToAgent.set(threadId, name);
      return setAgent(this.config, name, { threadId, status: created.thread.status?.type || "idle" });
    })();

    this.ensuringThreads.set(name, promise);
    try {
      return await promise;
    } finally {
      this.ensuringThreads.delete(name);
    }
  }

  async #warmKnownAgents() {
    const agents = listAgents(this.config).filter((agent) => agent.threadId);
    if (!agents.length) return;

    this.log("daemon.warm.start", { count: agents.length });
    for (const agent of agents) {
      try {
        const resumed = await this.appServer.request("thread/resume", {
          threadId: agent.threadId,
          cwd: agent.cwd,
          excludeTurns: true,
          persistExtendedHistory: false,
        }, 60000);
        this.threadToAgent.set(agent.threadId, agent.name);
        const statusType = resumed.thread?.status?.type || "idle";
        const changes = { status: statusType };
        if (statusType !== "active") changes.activeTurnId = null;
        setAgent(this.config, agent.name, changes);
        this.log("daemon.agent.warmed", {
          name: agent.name,
          threadId: agent.threadId,
          statusType,
        });
      } catch (error) {
        this.log("daemon.agent.warm.failed", {
          name: agent.name,
          threadId: agent.threadId,
          error: error.message,
        });
      }
    }
    this.log("daemon.warm.complete", { count: agents.length });
  }

  syncActiveThreads() {
    try {
      this.#refreshDocPeerFacts();
      void this.#probeDocDiscoveredPeers();
      const threads = discoverThreads();
      refreshLocalRegistryFromThreads({
        config: this.config,
        threads,
        threadToAgent: this.threadToAgent,
        skippedThreadReasons: this.skippedThreadReasons,
        log: (type, payload) => this.log(type, payload),
      });
      this.log("sync.threads.complete", { count: threads.length, source: "native-thread-discovery" });
    } catch (error) {
      this.log("sync.threads.failed", { error: error.message, source: "native-thread-discovery" });
    }
    return Promise.resolve();
  }

  #applyThreadSync(threads) {
    try {
      const registry = listAgents(this.config);
      const existingThreadMap = new Map();
      for (const agent of registry) {
        if (agent.threadId && !String(agent.route || "").startsWith("peer:")) {
          existingThreadMap.set(agent.threadId, agent);
        }
      }

      const activeThreadIds = new Set();
      const discoveryRows = [];

      const normalizeName = (text) => {
        if (!text) return "";
        return text
          .toLowerCase()
          .replace(/[^a-z0-9\s-]/g, "")
          .trim()
          .replace(/[\s_]+/g, "-")
          .replace(/-+/g, "-")
          .replace(/^-+|-+$/g, "");
      };

      for (const thread of threads) {
        const tid = thread.id;
        activeThreadIds.add(tid);

        let name = normalizeName(thread.agent_nickname);
        if (!name) {
          name = normalizeName(thread.title);
        }
        if (!name) {
          name = `agent-${tid.substring(0, 8)}`;
        }
        if (name && !name.endsWith("-agent")) {
          name = `${name}-agent`;
        }

        let cwd = thread.cwd;
        if (cwd && cwd.startsWith("\\\\?\\")) {
          cwd = cwd.substring(4);
        }

        const classification = classifyThreadDiscovery(thread, name, cwd);
        discoveryRows.push({
          id: tid,
          name,
          title: thread.title || "",
          cwd: cwd || "",
          source: thread.source || null,
          sourceKind: classification.sourceKind || null,
          threadSource: thread.thread_source || "codex",
          nodeName: thread.nodeName || this.config.nodeName,
          route: thread.route || "",
          transport: thread.transport || "",
          disposition: classification.disposition,
          reason: classification.reason,
          approved: classification.approved === true,
          rolloutPath: thread.rollout_path || null,
          updatedAt: thread.updated_at || null,
          discoveredAt: new Date().toISOString(),
        });

        if (!classification.approved) {
          const errMsg = `Thread ${tid} (${name}) is ${classification.disposition}: ${classification.reason}. Skipping active agent promotion.`;
          const previous = this.skippedThreadReasons.get(tid);
          if (previous !== errMsg) {
            this.log("sync.agent.classified_non_approved", {
              threadId: tid,
              name,
              disposition: classification.disposition,
              reason: classification.reason,
            });
            appendEvent("sync.agent.classified_non_approved", {
              threadId: tid,
              name,
              skipped: true,
              disposition: classification.disposition,
              reason: classification.reason,
            });
            this.skippedThreadReasons.set(tid, errMsg);
          }
          continue;
        }

        if (existingThreadMap.has(tid)) {
          const agent = existingThreadMap.get(tid);
          
          // If the agent name changed, rename it
          if (agent.name !== name) {
            let uniqueName = name;
            let counter = 1;
            const currentNames = new Set(listAgents(this.config).map(a => a.name));
            currentNames.delete(agent.name);
            while (currentNames.has(uniqueName)) {
              counter++;
              uniqueName = `${name}-${counter}`;
            }
            
            if (agent.name !== uniqueName) {
              try {
                const p = paths();
                const currentRegistry = readJson(p.registry, { agents: {} });
                if (currentRegistry.agents && currentRegistry.agents[agent.name]) {
                  delete currentRegistry.agents[agent.name];
                  currentRegistry.updatedAt = new Date().toISOString();
                  writeJsonAtomic(p.registry, currentRegistry);
                  this.log("sync.agent.renamed.delete-old", { oldName: agent.name, newName: uniqueName, threadId: tid });
                }
                
                // cwd has already been resolved and normalized
                
                upsertAgent(this.config, {
                  name: uniqueName,
                  node: thread.nodeName || agent.node || this.config.nodeName,
                  cwd,
                  threadId: tid,
                  status: agent.status || "idle",
                  threadSource: thread.thread_source,
                  sourceHost: thread.sourceHost,
                  hostKind: thread.hostKind,
                  transport: thread.transport,
                  route: thread.route,
                  discoveryDisposition: classification.disposition,
                  discoveryReason: classification.reason,
                  approvedForSync: true,
                });
                this.threadToAgent.set(tid, uniqueName);
                this.log("sync.agent.renamed.created-new", { oldName: agent.name, newName: uniqueName, threadId: tid });
                continue;
              } catch (e) {
                this.log("sync.agent.rename.failed", { threadId: tid, oldName: agent.name, newName: uniqueName, error: e.message });
              }
            }
          }
          
          this.threadToAgent.set(tid, agent.name);
          setAgent(this.config, agent.name, {
            node: thread.nodeName || agent.node || this.config.nodeName,
            cwd,
            threadSource: thread.thread_source,
            sourceHost: thread.sourceHost,
            hostKind: thread.hostKind,
            transport: thread.transport,
            route: thread.route,
            status: agent.status || "idle",
            discoveryDisposition: classification.disposition,
            discoveryReason: classification.reason,
            approvedForSync: true,
          });
          continue;
        }

        let uniqueName = name;
        let counter = 1;
        const currentNames = new Set(listAgents(this.config).map(a => a.name));
        while (currentNames.has(uniqueName)) {
          counter++;
          uniqueName = `${name}-${counter}`;
        }

        // cwd has already been resolved and normalized

        try {
          const agent = upsertAgent(this.config, {
            name: uniqueName,
            node: thread.nodeName || this.config.nodeName,
            cwd,
            threadId: tid,
            status: "idle",
            threadSource: thread.thread_source,
            sourceHost: thread.sourceHost,
            hostKind: thread.hostKind,
            transport: thread.transport,
            route: thread.route,
            discoveryDisposition: classification.disposition,
            discoveryReason: classification.reason,
            approvedForSync: true,
          });
          this.threadToAgent.set(tid, uniqueName);
          appendEvent("agent.created", { agent });
          this.log("sync.agent.created", { name: uniqueName, threadId: tid, cwd });
        } catch (e) {
          this.log("sync.agent.create.failed", { threadId: tid, error: e.message });
        }
      }
      this.#removeNonApprovedLocalAgents(discoveryRows);
      const localDiscoveries = saveLocalDiscoveries(this.config, discoveryRows, "native-thread-discovery");
      this.log("sync.discovery.classified", {
        counts: localDiscoveries.counts,
      });
      this.log("sync.agent.prune.skipped", { reason: "discovery is additive to avoid deleting active approved local agents", skippedThreads: this.skippedThreadReasons.size });
    } catch (e) {
      this.log("sync.apply.failed", { error: e.message });
    }
  }

  #removeNonApprovedLocalAgents(discoveryRows) {
    const rejectedThreadIds = new Set((discoveryRows || [])
      .filter((row) => row.approved !== true && row.id)
      .map((row) => row.id));
    if (!rejectedThreadIds.size) return;
    const registry = loadRegistry(this.config);
    let changed = false;
    for (const [name, agent] of Object.entries(registry.agents || {})) {
      if (!agent?.threadId || !rejectedThreadIds.has(agent.threadId)) continue;
      if (String(agent.route || "").startsWith("peer:")) continue;
      if (agent.threadSource === "mailbox") continue;
      delete registry.agents[name];
      changed = true;
      this.threadToAgent.delete(agent.threadId);
      this.log("sync.agent.demoted", {
        name,
        threadId: agent.threadId,
        reason: "local-discovery-not-approved",
      });
      appendEvent("sync.agent.demoted", {
        name,
        threadId: agent.threadId,
        reason: "local-discovery-not-approved",
      });
    }
    if (changed) saveRegistry(registry);
  }

  async #startThread(agent) {
    const base = {
      cwd: agent.cwd,
      approvalPolicy: "never",
      sandbox: "danger-full-access",
      ephemeral: false,
      threadSource: null,
      experimentalRawEvents: false,
      persistExtendedHistory: false,
    };

    const runtimeSettings = this.#runtimeSettings(agent);
    if (Object.keys(runtimeSettings).length) {
      const withModel = { ...base, ...runtimeSettings };
      try {
        return await this.appServer.request("thread/start", withModel, 60000);
      } catch (error) {
        this.log("thread.start.with-model.failed", {
          agent: agent.name,
          model: agent.model,
          modelProvider: agent.modelProvider,
          effort: agent.effort,
          serviceTier: agent.serviceTier,
          error: error.message,
        });
      }
    }
    return this.appServer.request("thread/start", base, 60000);
  }

  #ensureBuiltinMailboxAgents() {
    const agent = upsertAgent(this.config, {
      name: CAM_TEST_MAILBOX_AGENT,
      node: this.config.nodeName,
      cwd: process.cwd(),
      threadId: null,
      activeTurnId: null,
      status: "idle",
      threadSource: "mailbox",
    });
    this.log("mailbox_only_target.registered", {
      name: agent.name,
      node: agent.node,
      threadSource: agent.threadSource,
    });
  }

  async #sendMessage(body, existingMessage = null) {
    if (!existingMessage && !body?.targetAgent) throw new Error("targetAgent is required");
    if (!existingMessage && !body?.message) throw new Error("message is required");
    
    const targetAgent = existingMessage ? existingMessage.targetAgent : body.targetAgent;
    const targetAgentObj = getAgent(this.config, targetAgent);
    const strict = !existingMessage && body?.strict === true;
    const messageType = existingMessage ? existingMessage.messageType : (body?.messageType || null);

    if (targetAgentObj?.route && String(targetAgentObj.route).startsWith("peer:")) {
      return this.#sendRemoteMirrorMessage({
        body,
        existingMessage,
        targetAgent,
        targetAgentObj,
        strict,
        messageType,
      });
    }

    if ((targetAgentObj && MAILBOX_ONLY_THREAD_SOURCES.has(targetAgentObj.threadSource)) || (!targetAgentObj && (targetAgent === "operator" || targetAgent === "windows-gui"))) {
      const isGuiTestReply = messageType === GUI_TEST_REPLY_MESSAGE_TYPE && targetAgent === CAM_TEST_MAILBOX_AGENT;
      const message = existingMessage || {
        messageId: crypto.randomUUID(),
        correlationId: body?.correlationId || null,
        sourceAgent: body?.sourceAgent || "operator",
        targetAgent,
        sourceNode: this.#trustedSourceNode(body?.sourceAgent, body?.sourceNode),
        sourceRoute: this.#trustedSourceRoute(body?.sourceAgent),
        targetNode: this.config.nodeName,
        timestamp: new Date().toISOString(),
        body: body?.message,
        messageType,
        delivery: "received",
      };
      message.delivery = "received";
      message.targetNode = targetAgentObj?.node || this.config.nodeName;
      delete message.error;
      if (!existingMessage) {
        this.queueMessage(message);
        appendEvent(isGuiTestReply ? "gui_test.reply.received" : "message.received", message);
        if (isGuiTestReply) {
          appendTestEvent(message.correlationId, "reply_received", { inbound: message });
        }
        this.#ingestDiscoveryEvidence({
          targetPeerName: null,
          source: "mailbox",
          body: message.body,
          sourceAgent: message.sourceAgent,
          correlationId: message.correlationId,
          messageType: message.messageType,
        });
      }
      this.log(isGuiTestReply ? "gui_test.reply.received" : "mailbox_only_target.inbox.received", { targetAgent, messageId: message.messageId, sourceAgent: message.sourceAgent });
      return { delivered: false, queued: false, received: true, message };
    }

    if (targetAgentObj && targetAgentObj.threadSource === "antigravity") {
      if (strict) {
        const message = this.#buildFailedMessage(body, targetAgent, "strict send cannot deliver to Antigravity mailbox-only agents");
        if (message.messageType === GUI_TEST_MESSAGE_TYPE) appendTestEvent(message.correlationId, "failed", { error: message.error, outbound: message });
        return { ok: false, delivered: false, queued: false, error: message.error, message };
      }
      const message = existingMessage || {
        messageId: crypto.randomUUID(),
        correlationId: body.correlationId || null,
        sourceAgent: body.sourceAgent || "operator",
        targetAgent,
        sourceNode: this.#trustedSourceNode(body.sourceAgent, body.sourceNode),
        sourceRoute: this.#trustedSourceRoute(body.sourceAgent),
        targetNode: targetAgentObj.node,
        timestamp: new Date().toISOString(),
        body: body.message,
        messageType,
        delivery: "queued",
      };
      if (existingMessage) {
        message.delivery = "queued";
      }
      this.queueMessage(message);
      setAgent(this.config, targetAgent, { status: "idle", lastDelivery: message });
      appendEvent("message.queued", message);
      this.log("antigravity.inbox.queued", { messageId: message.messageId, target: targetAgent });
      return { delivered: false, queued: true, message };
    }

    const source = body ? getAgent(this.config, body.sourceAgent) : null;
    if (!existingMessage && body && source && source.threadSource === "antigravity") {
      setAgent(this.config, body.sourceAgent, { status: "idle" });
      this.#checkInboxListeners();
    }

    let target;
    let resolveError = null;
    try {
      target = await this.#ensureThread(targetAgent, { strict });
    } catch (ensureErr) {
      resolveError = ensureErr;
      if (strict && STRICT_THREAD_NOT_FOUND.test(ensureErr.message)) {
        target = await this.#repairStaleThreadAndEnsure(targetAgent, ensureErr);
      }
    }
    if (!target) {
      const ensureErr = resolveError || new Error(`unable to resolve target thread for ${targetAgent}`);
      if (strict) {
        const message = this.#buildFailedMessage(body, targetAgent, ensureErr.message);
        if (message.messageType === GUI_TEST_MESSAGE_TYPE) appendTestEvent(message.correlationId, "failed", { error: message.error, outbound: message });
        return { ok: false, delivered: false, queued: false, error: message.error, message };
      }
      const message = existingMessage || {
        messageId: crypto.randomUUID(),
        correlationId: body?.correlationId || null,
        sourceAgent: body?.sourceAgent || "operator",
        targetAgent: targetAgent,
        sourceNode: this.#trustedSourceNode(body?.sourceAgent, body?.sourceNode),
        sourceRoute: this.#trustedSourceRoute(body?.sourceAgent),
        targetNode: null,
        timestamp: new Date().toISOString(),
        body: body?.message,
        messageType,
        delivery: "queued",
      };
      message.delivery = "queued";
      message.error = ensureErr.message;
      if (!existingMessage) {
        this.queueMessage(message);
        appendEvent("message.queued", message);
      } else {
        // Update error in existing mailbox record
        try {
          const all = readMailbox();
          for (const row of all) {
            if (row.messageId === message.messageId) {
              row.error = ensureErr.message;
            }
          }
          fs.writeFileSync(paths().mailbox, all.map((row) => JSON.stringify(row)).join("\n") + (all.length ? "\n" : ""), "utf8");
        } catch (e) {
          this.log("mailbox.update.failed", { messageId: message.messageId, error: e.message });
        }
      }
      return { delivered: false, queued: true, message };
    }

    const message = existingMessage || {
      messageId: crypto.randomUUID(),
      correlationId: body.correlationId || null,
      sourceAgent: body.sourceAgent || "operator",
      targetAgent: targetAgent,
      sourceNode: this.#trustedSourceNode(body.sourceAgent, body.sourceNode),
      sourceRoute: this.#trustedSourceRoute(body.sourceAgent),
      targetNode: target.node,
      timestamp: new Date().toISOString(),
      body: body.message,
      messageType,
      delivery: "pending",
    };
    if (!existingMessage && message.messageType === GUI_TEST_MESSAGE_TYPE) {
      appendTestEvent(message.correlationId, "started", {
        targetAgent,
        targetNode: target.node,
        targetThreadId: target.threadId,
        outbound: message,
      });
    }

    // Filter pending mailbox to exclude this message if it's already queued
    const pending = pendingMailbox(targetAgent).filter(m => m.messageId !== message.messageId);
    const pendingText = pending.length
      ? [
          "",
          "[Queued messages surfaced by Qexow CAM]",
          ...pending.map((queued, index) => [
            `queuedMessage ${index + 1}:`,
            `messageId: ${queued.messageId}`,
            `sourceAgent: ${queued.sourceAgent}`,
            `sourceNode: ${queued.sourceNode}`,
            queued.body,
          ].join("\n")),
        ].join("\n")
      : "";

      const replyTargetAgent = message.messageType === GUI_TEST_MESSAGE_TYPE
        ? CAM_TEST_MAILBOX_AGENT
        : message.sourceAgent;
      const replyTypeInstruction = message.messageType === GUI_TEST_MESSAGE_TYPE
        ? ` Use messageType "${GUI_TEST_REPLY_MESSAGE_TYPE}".`
        : "";

      const prompt = [
      "[Qexow CAM message]",
      `messageId: ${message.messageId}`,
      `correlationId: ${message.correlationId || ""}`,
      message.messageType ? `messageType: ${message.messageType}` : null,
      `sourceAgent: ${message.sourceAgent}`,
      `sourceNode: ${message.sourceNode}`,
      `targetAgent: ${message.targetAgent}`,
      "",
      message.body,
      pendingText,
      "",
      `[To reply to this message, use the qexow-cam-messaging skill and send to targetAgent "${replyTargetAgent}" with correlationId "${message.correlationId || ""}".${replyTypeInstruction} Do not use direct CAM HTTP or older codex-agent-manager paths.]`
    ].filter(Boolean).join("\n");

    try {
      if (target.activeTurnId) {
        const steer = await this.appServer.request("turn/steer", {
          threadId: target.threadId,
          input: textInput(prompt),
          expectedTurnId: target.activeTurnId,
        }, 30000);
        message.delivery = "steered";
        message.turnId = steer.turnId;
      } else {
        const started = await this.appServer.request("turn/start", {
          threadId: target.threadId,
          input: textInput(prompt),
          cwd: target.cwd,
          approvalPolicy: "never",
          sandbox: "danger-full-access",
          ...this.#runtimeSettings(target),
        }, 60000);
        message.delivery = "started";
        message.turnId = started.turn.id;
        setAgent(this.config, target.name, { status: "active", activeTurnId: started.turn.id, lastDelivery: message });
      }
      
      const messageIdsToSurface = pending.map((queued) => queued.messageId);
      if (existingMessage) {
        messageIdsToSurface.push(existingMessage.messageId);
      }
      
      const surfaced = markMailboxSurfaced(messageIdsToSurface, message.turnId);
      for (const queued of surfaced) appendEvent("message.surfaced", queued);
      message.delivery = "delivered";
      setAgent(this.config, target.name, { lastDelivery: message });
      appendEvent("message.delivered", message);
      if (message.messageType === GUI_TEST_MESSAGE_TYPE) {
        appendTestEvent(message.correlationId, "outbound_delivered", { outbound: message });
      }
      return { delivered: true, queued: false, message };
    } catch (error) {
      if (strict) {
        if (STRICT_THREAD_NOT_FOUND.test(error.message)) {
          setAgent(this.config, targetAgent, { status: "stale", activeTurnId: null, lastError: error.message });
        }
        message.delivery = "failed";
        message.error = error.message;
        appendEvent("message.failed.strict", message);
        if (message.messageType === GUI_TEST_MESSAGE_TYPE) appendTestEvent(message.correlationId, "failed", { error: error.message, outbound: message });
        return { ok: false, delivered: false, queued: false, error: error.message, message };
      }
      message.delivery = "queued";
      message.error = error.message;
      if (!existingMessage) {
        this.queueMessage(message);
        appendEvent("message.queued", message);
      } else {
        // Update error in existing mailbox record
        try {
          const all = readMailbox();
          for (const row of all) {
            if (row.messageId === message.messageId) {
              row.error = error.message;
            }
          }
          fs.writeFileSync(paths().mailbox, all.map((row) => JSON.stringify(row)).join("\n") + (all.length ? "\n" : ""), "utf8");
        } catch (e) {
          this.log("mailbox.update.failed", { messageId: message.messageId, error: e.message });
        }
      }
      return { delivered: false, queued: true, message };
    }
  }

  #buildFailedMessage(body, targetAgent, error) {
    return {
      messageId: crypto.randomUUID(),
      correlationId: body?.correlationId || null,
      sourceAgent: body?.sourceAgent || "operator",
      targetAgent,
      sourceNode: this.#trustedSourceNode(body?.sourceAgent, body?.sourceNode),
      sourceRoute: this.#trustedSourceRoute(body?.sourceAgent),
      targetNode: null,
      timestamp: new Date().toISOString(),
      body: body?.message || "",
      messageType: body?.messageType || null,
      delivery: "failed",
      error,
    };
  }

  #trustedSourceNode(sourceAgent, fallbackNode = null) {
    const agent = sourceAgent ? getAgent(this.config, sourceAgent) : null;
    if (agent?.node) return agent.node;
    return this.config.nodeName || fallbackNode || os.hostname();
  }

  #trustedSourceRoute(sourceAgent) {
    const agent = sourceAgent ? getAgent(this.config, sourceAgent) : null;
    if (agent?.route) return agent.route;
    if (agent?.transport) return agent.transport;
    if (agent?.threadSource) return agent.threadSource;
    return "local";
  }

  async #repairStaleThreadAndEnsure(targetAgent, originalError) {
    this.log("thread.repair.start", { agent: targetAgent, error: originalError.message });
    await this.syncActiveThreads();
    const refreshed = getAgent(this.config, targetAgent);
    if (!refreshed || !refreshed.threadId) {
      this.log("thread.repair.failed", { agent: targetAgent, error: "no refreshed threadId found" });
      return null;
    }
    try {
      const resumed = await this.appServer.request("thread/resume", {
        threadId: refreshed.threadId,
        cwd: refreshed.cwd,
        excludeTurns: true,
        persistExtendedHistory: false,
      }, 60000);
      this.threadToAgent.set(refreshed.threadId, refreshed.name);
      const statusType = resumed.thread?.status?.type || "idle";
      const changes = { status: statusType, lastError: null };
      if (statusType !== "active") changes.activeTurnId = null;
      this.log("thread.repair.ok", { agent: targetAgent, threadId: refreshed.threadId, statusType });
      return setAgent(this.config, targetAgent, changes);
    } catch (error) {
      setAgent(this.config, targetAgent, { status: "stale", activeTurnId: null, lastError: error.message });
      this.log("thread.repair.failed", { agent: targetAgent, threadId: refreshed.threadId, error: error.message });
      return null;
    }
  }

  async #processNextQueuedMessage(name) {
    try {
      const pending = pendingMailbox(name);
      if (!pending.length) return;
      const oldest = pending[0];
      
      this.log("mailbox.dequeue", { agent: name, messageId: oldest.messageId });
      await this.#sendMessage(null, oldest);
    } catch (err) {
      this.log("mailbox.dequeue.error", { agent: name, error: err.message });
    }
  }

  async #sendRemoteMirrorMessage({ body, existingMessage, targetAgent, targetAgentObj, strict, messageType }) {
    const peerName = String(targetAgentObj.route || "").slice("peer:".length);
    const peer = getPeer(this.config, peerName);
    if (!peer) {
      const failed = this.#buildFailedMessage(body, targetAgent, `unknown remote peer: ${peerName}`);
      return { ok: false, delivered: false, queued: false, error: failed.error, message: failed };
    }
    const remoteRoot = await this.resolveRemoteRoot(peerName);
    const message = existingMessage || {
      messageId: crypto.randomUUID(),
      correlationId: body?.correlationId || null,
      sourceAgent: body?.sourceAgent || "operator",
      targetAgent,
      sourceNode: this.#trustedSourceNode(body?.sourceAgent, body?.sourceNode),
      sourceRoute: this.#trustedSourceRoute(body?.sourceAgent),
      targetNode: targetAgentObj.remoteNodeName || peerName,
      timestamp: new Date().toISOString(),
      body: body?.message || "",
      messageType,
      delivery: "pending",
    };
    const result = await this.#sshRunCamSend(peer, remoteRoot, {
      targetAgent: targetAgentObj.remoteAgentName || targetAgent,
      message: message.body,
      sourceAgent: message.sourceAgent,
      sourceNode: message.sourceNode,
      correlationId: message.correlationId,
      messageType: message.messageType,
      strict,
    });
    if (!result.ok) {
      message.delivery = strict ? "failed" : "queued";
      message.error = result.error;
      appendEvent(strict ? "message.failed.remote" : "message.queued.remote", {
        peerName,
        targetAgent,
        remoteAgentName: targetAgentObj.remoteAgentName || targetAgent,
        error: result.error,
      });
      if (strict) {
        return { ok: false, delivered: false, queued: false, error: result.error, message };
      }
      this.queueMessage(message);
      return { delivered: false, queued: true, message };
    }
    message.delivery = "delivered";
    message.remoteDelivery = "ssh-remote-send";
    message.remotePeerName = peerName;
    message.remoteAgentName = targetAgentObj.remoteAgentName || targetAgent;
    appendEvent("message.delivered.remote", {
      peerName,
      targetAgent,
      remoteAgentName: targetAgentObj.remoteAgentName || targetAgent,
      messageId: message.messageId,
      correlationId: message.correlationId || null,
    });
    return { delivered: true, queued: false, message };
  }

  #runtimeSettings(agent) {
    const settings = {};
    if (agent.model) settings.model = agent.model;
    if (agent.modelProvider) settings.modelProvider = agent.modelProvider;
    if (agent.effort) settings.effort = agent.effort;
    if (agent.serviceTier) settings.serviceTier = agent.serviceTier;
    return settings;
  }

  #checkInboxListeners() {
    const activeAgents = listAgents(this.config).filter(a => a.status === "active");
    if (activeAgents.length === 0) {
      const listeners = [...this.mailboxListeners];
      for (const listener of listeners) {
        try {
          listener(null);
        } catch (err) {
          this.log("mailbox.listener.error", { error: err.message });
        }
      }
    }
  }

  async resolveRemoteRoot(peerName, peerOverride = null) {
    const peer = peerOverride || getPeer(this.config, peerName);
    if (!peer) return null;
    if (peer.remoteRoot && peer.remoteRoot !== "auto") return peer.remoteRoot;
    if (peer.remoteSync?.remoteRoot) return peer.remoteSync.remoteRoot;
    const username = this.#sshUsername(peer);
    if (!username) return null;
    const candidates = [
      ...DEFAULT_REMOTE_ROOTS.map((root) => root.replace("/ubuntu/", `/${username}/`)),
    ];
    for (const candidate of [...new Set(candidates.filter(Boolean))]) {
      this.log("peer.remote_root.probe", { peerName, candidate });
      const result = await this.#probeRemoteCommand(peer, `test -f ${sq(path.posix.join(candidate, "bin", "cam.js"))}`);
      if (result.ok) {
        upsertPeer(this.config, peerName, {
          ...peer,
          remoteRoot: candidate,
        });
        this.log("peer.remote_root.verified", { peerName, remoteRoot: candidate });
        return candidate;
      }
    }
    return null;
  }

  async syncPeer(peerName = null) {
    if (peerName) {
      const peer = getPeer(this.config, peerName);
      if (!peer) {
        return { ok: false, error: `unknown peer: ${peerName}` };
      }
      return this.#syncSinglePeer(peer);
    }
    return this.#syncKnownPeers();
  }

  async #syncKnownPeers() {
    const peers = listPeers(this.config).filter((peer) => peer?.transport === "ssh" && String(peer?.ssh || "").includes("@") && peer?.key);
    const results = [];
    for (const peer of peers) {
      results.push(await this.#syncSinglePeer(peer));
    }
    return {
      ok: results.every((row) => row.ok !== false),
      peers: results,
    };
  }

  async #syncSinglePeer(peer) {
    const peerName = peer.name;
    if (this.peerSyncInflight.has(peerName)) {
      this.log("peer.sync.coalesced", { peerName });
      return this.peerSyncInflight.get(peerName);
    }
    const syncPromise = this.#syncSinglePeerInner(peer);
    this.peerSyncInflight.set(peerName, syncPromise);
    try {
      return await syncPromise;
    } finally {
      if (this.peerSyncInflight.get(peerName) === syncPromise) {
        this.peerSyncInflight.delete(peerName);
      }
    }
  }

  async #syncSinglePeerInner(peer) {
    const peerName = peer.name;
    const previousRemoteSync = getPeer(this.config, peerName)?.remoteSync || {};
    if (this.#isSelfPeer(peer)) {
      this.#pruneMirroredAgentsForPeer(peerName, []);
      upsertPeer(this.config, peerName, {
        ...getPeer(this.config, peerName),
        remoteSync: {
          ...(getPeer(this.config, peerName)?.remoteSync || {}),
          lastAttemptAt: new Date().toISOString(),
          lastStatus: "skipped-self",
          lastError: null,
          syncedAt: new Date().toISOString(),
          mirroredAgents: [],
        },
      });
      this.log("peer.sync.skipped_self", {
        peerName,
        ssh: peer?.ssh || null,
      });
      appendEvent("peer.sync.skipped_self", {
        peerName,
        ssh: peer?.ssh || null,
      });
      return { ok: true, peerName, skipped: "self" };
    }
    const remoteRoot = await this.resolveRemoteRoot(peerName, peer);
    if (!remoteRoot) {
      const error = "unable to locate remote CAM manager root";
      upsertPeer(this.config, peerName, {
        ...getPeer(this.config, peerName),
        remoteSync: {
          ...(getPeer(this.config, peerName)?.remoteSync || {}),
          lastAttemptAt: new Date().toISOString(),
          lastStatus: "failed",
          lastError: error,
        },
      });
      this.log("peer.sync.failed", { peerName, error });
      return { ok: false, peerName, error };
    }
    const remoteInventoryResult = await this.#fetchRemoteCamInventory(peer, remoteRoot);
    if (!remoteInventoryResult.ok) {
      upsertPeer(this.config, peerName, {
        ...getPeer(this.config, peerName),
        remoteSync: {
          ...(getPeer(this.config, peerName)?.remoteSync || {}),
          lastAttemptAt: new Date().toISOString(),
          remoteRoot: remoteRoot || null,
          lastStatus: "failed",
          lastError: remoteInventoryResult.error,
        },
      });
      this.log("peer.sync.failed", { peerName, error: remoteInventoryResult.error });
      return { ok: false, peerName, error: remoteInventoryResult.error };
    }
    const remoteInventory = remoteInventoryResult.inventory;
    const result = this.#ingestRemotePeerRegistry(peer, remoteInventory, remoteRoot, previousRemoteSync);
    upsertPeer(this.config, peerName, {
      ...getPeer(this.config, peerName),
      remoteSync: {
        ...(getPeer(this.config, peerName)?.remoteSync || {}),
        lastAttemptAt: new Date().toISOString(),
        lastStatus: "ok",
        lastError: null,
        syncedAt: new Date().toISOString(),
        remoteRoot: remoteRoot || null,
        remoteInventorySource: remoteInventoryResult.source,
        remoteInventorySchema: remoteInventory?.inventorySchema || 1,
        remoteInventoryDegraded: remoteInventoryResult.degraded === true || result.inventoryPreserved === true,
        remoteDiscoveryCounts: result.discoveryCounts,
        remoteRejectedDiscoveries: result.rejectedDiscoveries.length,
        remoteApprovedDiscoveries: result.approvedDiscoveries.length,
        remoteNodeName: remoteInventory?.nodeName || null,
        mirroredAgents: result.mirroredAgents,
      },
    });
    this.log("peer.sync.complete", {
      peerName,
      mirroredAgents: result.mirroredAgents.length,
      remoteDiscoveryCounts: result.discoveryCounts,
      remoteInventoryDegraded: remoteInventoryResult.degraded === true || result.inventoryPreserved === true,
      remoteRoot: remoteRoot || null,
      remoteNodeName: remoteInventory?.nodeName || null,
      inventoryPreserved: result.inventoryPreserved === true,
    });
    appendEvent("peer.sync.complete", {
      peerName,
      mirroredAgents: result.mirroredAgents.length,
      remoteDiscoveryCounts: result.discoveryCounts,
      remoteInventoryDegraded: remoteInventoryResult.degraded === true || result.inventoryPreserved === true,
      remoteRoot: remoteRoot || null,
      remoteNodeName: remoteInventory?.nodeName || null,
      inventoryPreserved: result.inventoryPreserved === true,
    });
    return {
      ok: true,
      peerName,
      remoteRoot: remoteRoot || null,
      remoteInventorySource: remoteInventoryResult.source,
      remoteNodeName: remoteInventory?.nodeName || null,
      mirroredAgents: result.mirroredAgents,
    };
  }

  #ingestRemotePeerRegistry(peer, remoteRegistry, remoteRoot, previousRemoteSync = {}) {
    const remoteAgents = canonicalizeTrustedInventoryAgents(Object.values(remoteRegistry?.agents || {}));
    const rawDiscoveries = remoteRegistry?.discoveries?.local?.rows || [];
    const counts = remoteRegistry?.counts?.localDiscoveries || remoteRegistry?.discoveries?.local?.counts || discoveryCounts(rawDiscoveries);
    const approvedDiscoveries = rawDiscoveries.filter((row) => row?.approved === true || row?.disposition === "approved");
    const rejectedDiscoveries = rawDiscoveries.filter((row) => !(row?.approved === true || row?.disposition === "approved"));
    const previouslyMirrored = Array.isArray(previousRemoteSync?.mirroredAgents) ? previousRemoteSync.mirroredAgents : [];
    const preserveExistingMirrors =
      previouslyMirrored.length > 0 &&
      remoteAgents.length === 0 &&
      approvedDiscoveries.length === 0 &&
      Number(counts?.total || 0) === 0;
    saveRemoteDiscoverySnapshot(this.config, peer.name, {
      source: remoteRegistry?.exportPolicy || "legacy-or-unknown",
      inventorySchema: remoteRegistry?.inventorySchema || 1,
      remoteNodeName: remoteRegistry?.nodeName || peer.name,
      remoteRoot: remoteRoot || null,
      counts,
      rawDiscoveries,
      approvedDiscoveries,
      rejectedDiscoveries,
      approvedAgents: remoteAgents,
    });
    if (preserveExistingMirrors) {
      this.log("peer.sync.empty_inventory_preserved", {
        peerName: peer.name,
        remoteRoot: remoteRoot || null,
        preservedMirrors: previouslyMirrored.length,
        reason: "remote-export-returned-zero-agents-and-zero-discoveries-after-prior-success",
      });
      appendEvent("peer.sync.empty_inventory_preserved", {
        peerName: peer.name,
        remoteRoot: remoteRoot || null,
        preservedMirrors: previouslyMirrored.length,
        reason: "remote-export-returned-zero-agents-and-zero-discoveries-after-prior-success",
      });
      return {
        mirroredAgents: previouslyMirrored,
        discoveryCounts: counts,
        approvedDiscoveries,
        rejectedDiscoveries,
        inventoryPreserved: true,
      };
    }
    this.#pruneMirroredAgentsForPeer(peer.name, remoteAgents);
    const mirroredAgents = [];
    for (const remoteAgent of remoteAgents) {
      if (!remoteAgent?.name) continue;
      const mirrorName = this.#remoteMirrorName(peer.name, remoteAgent.name);
      const mirrored = upsertAgent(this.config, {
        name: mirrorName,
        node: remoteRegistry?.nodeName || peer.name,
        cwd: remoteAgent.cwd || remoteRoot || process.cwd(),
        threadId: remoteAgent.threadId ?? null,
        activeTurnId: remoteAgent.activeTurnId ?? null,
        model: remoteAgent.model ?? null,
        modelProvider: remoteAgent.modelProvider ?? null,
        effort: remoteAgent.effort ?? null,
        serviceTier: remoteAgent.serviceTier ?? null,
        status: remoteAgent.status || "remote",
        threadSource: "remote-cam",
        sourceHost: peer.name,
        hostKind: "remote",
        transport: "ssh",
        route: `peer:${peer.name}`,
        remotePeerName: peer.name,
        remoteAgentName: remoteAgent.name,
        remoteNodeName: remoteRegistry?.nodeName || peer.name,
        remoteRoot: remoteRoot || null,
        discoveredBy: "remote-cam-sync",
        discoveryDisposition: "approved",
        discoveryReason: remoteAgent.discoveryReason || "remote-approved-agent",
        approvedForSync: true,
        discoveredAt: new Date().toISOString(),
      });
      mirroredAgents.push({
        localName: mirrorName,
        remoteAgentName: remoteAgent.name,
        threadId: remoteAgent.threadId ?? null,
        status: remoteAgent.status || "remote",
      });
      this.threadToAgent.delete(remoteAgent.threadId);
      void mirrored;
    }
    return { mirroredAgents, discoveryCounts: counts, approvedDiscoveries, rejectedDiscoveries, inventoryPreserved: false };
  }

  #pruneMirroredAgentsForPeer(peerName, remoteAgents) {
    const registry = loadRegistry(this.config);
    const allowedNames = new Set(remoteAgents.map((agent) => this.#remoteMirrorName(peerName, agent.name)));
    const allowedThreadIds = new Set(remoteAgents.map((agent) => agent.threadId).filter(Boolean));
    let changed = false;
    for (const [name, agent] of Object.entries(registry.agents || {})) {
      if (String(agent?.route || "") !== `peer:${peerName}`) continue;
      const keepByName = allowedNames.has(name);
      const keepByThread = agent?.threadId ? allowedThreadIds.has(agent.threadId) : false;
      if (keepByName && (!agent?.threadId || keepByThread)) continue;
      delete registry.agents[name];
      changed = true;
      this.log("peer.sync.pruned_mirror", {
        peerName,
        name,
        threadId: agent?.threadId || null,
        reason: keepByThread ? "renamed-or-recursive-alias" : "not-in-validated-remote-export",
      });
      appendEvent("peer.sync.pruned_mirror", {
        peerName,
        name,
        threadId: agent?.threadId || null,
        reason: keepByThread ? "renamed-or-recursive-alias" : "not-in-validated-remote-export",
      });
    }
    if (changed) saveRegistry(registry);
  }

  #remoteMirrorName(peerName, remoteAgentName) {
    return `${peerName}::${remoteAgentName}`;
  }

  #isSelfPeer(peer) {
    const ssh = String(peer?.ssh || "").trim();
    const host = ssh.includes("@") ? ssh.split("@")[1] : ssh;
    const localHosts = new Set([
      "localhost",
      "127.0.0.1",
      "::1",
      String(this.config?.nodeName || "").trim().toLowerCase(),
      String(process.env.CAM_NODE_NAME || "").trim().toLowerCase(),
      String(process.env.HOSTNAME || "").trim().toLowerCase(),
      String(process.env.COMPUTERNAME || "").trim().toLowerCase(),
      String(os.hostname?.() || "").trim().toLowerCase(),
    ].filter(Boolean));
    if (host && localHosts.has(String(host).trim().toLowerCase())) return true;

    const localIps = new Set();
    try {
      const interfaces = os.networkInterfaces?.() || {};
      for (const rows of Object.values(interfaces)) {
        for (const row of rows || []) {
          if (row?.address) localIps.add(String(row.address).trim());
        }
      }
    } catch {
      // Best effort only.
    }
    const peerIps = new Set([
      host,
      ...(Array.isArray(peer?.observedPrivateIps) ? peer.observedPrivateIps : []),
      ...(Array.isArray(peer?.docDiscovery?.candidateIps) ? peer.docDiscovery.candidateIps : []),
      ...(Array.isArray(peer?.docDiscovery?.candidatePrivateIps) ? peer.docDiscovery.candidatePrivateIps : []),
    ].filter(Boolean).map((value) => String(value).trim()));
    for (const ip of peerIps) {
      if (localIps.has(ip)) return true;
    }
    return false;
  }

  #remoteCamShell(remoteRoot, argv) {
    const args = argv.map((part) => sq(part)).join(" ");
    const installed = remoteRoot && /^\/opt\/qexow-cam(?:\/|$)/.test(remoteRoot)
      ? `CAM_APP_ROOT=${sq(remoteRoot)} /usr/local/bin/cam ${args}`
      : null;
    const legacyRoot = remoteRoot || "/home/ubuntu/codex-agent-manager";
    const legacy = `cd ${sq(legacyRoot)} && node ${sq("./bin/cam.js")} ${args}`;
    if (installed) return installed;
    return [
      `if command -v cam >/dev/null 2>&1; then cam ${args}`,
      `elif [ -x /usr/local/bin/cam ]; then /usr/local/bin/cam ${args}`,
      `elif [ -d ${sq(legacyRoot)} ]; then ${legacy}`,
      "else echo __CAM_REMOTE_MISSING__; exit 45",
      "fi",
    ].join("; ");
  }

  async #sshRunCamSend(peer, remoteRoot, payload) {
    const root = remoteRoot || this.#remoteManagerRoot(peer);
    const command = this.#remoteCamShell(root, [
      "send",
      payload.targetAgent,
      payload.message,
      "--from",
      payload.sourceAgent || "operator",
      "--source-node",
      payload.sourceNode || this.config.nodeName,
      ...(payload.correlationId ? ["--correlation-id", payload.correlationId] : []),
      ...(payload.messageType ? ["--message-type", payload.messageType] : []),
      ...(payload.strict ? ["--strict"] : []),
    ]);
    this.log("peer.remote_send.command", {
      peerName: peer.name,
      command,
    });
    return this.#probeRemoteCommand(peer, command, 20000);
  }

  async #sshRunCamInventoryExport(peer, remoteRoot) {
    const root = remoteRoot || this.#remoteManagerRoot(peer);
    const command = this.#remoteCamShell(root, ["inventory", "export"]);
    this.log("peer.remote_inventory.command", {
      peerName: peer.name,
      command,
    });
    return this.#probeRemoteCommand(peer, command, REMOTE_INVENTORY_TIMEOUT_MS);
  }

  async #sshRunCamInbox(peer, remoteRoot, targetAgent) {
    const root = remoteRoot || this.#remoteManagerRoot(peer);
    const command = this.#remoteCamShell(root, ["inbox", targetAgent]);
    this.log("peer.remote_inbox.command", {
      peerName: peer.name,
      targetAgent,
      command,
    });
    return this.#probeRemoteCommand(peer, command, 20000);
  }

  #relayImportedMessageId(peerName, remoteMessageId) {
    return `relay:${peerName}:${remoteMessageId}`;
  }

  #deriveRelayedSourceAgent(remoteMessage, fallbackAgent = null) {
    const body = String(remoteMessage?.body || "");
    const equalsMatch = body.match(/\bagent=([A-Za-z0-9._:-]+)\b/i);
    if (equalsMatch?.[1]) return equalsMatch[1];
    const labelMatch = body.match(/\bAgent:\s*([A-Za-z0-9._:-]+)/i);
    if (labelMatch?.[1]) return labelMatch[1];
    return fallbackAgent || remoteMessage?.sourceAgent || "operator";
  }

  async #harvestRemoteMailbox(targetAgent) {
    if (!targetAgent) return [];
    const peers = listPeers(this.config).filter((peer) =>
      peer?.transport === "ssh" &&
      String(peer?.ssh || "").includes("@") &&
      peer?.key &&
      !this.#isSelfPeer(peer)
    );
    if (!peers.length) return [];

    const existingRows = readMailbox(targetAgent);
    const seenRelayKeys = new Set(
      existingRows
        .map((row) => row?.relayKey || (row?.relayedFromPeerName && row?.remoteMessageId ? `${row.relayedFromPeerName}:${row.remoteMessageId}` : null))
        .filter(Boolean)
    );
    const imported = [];

    for (const peer of peers) {
      try {
        const remoteRoot = await this.resolveRemoteRoot(peer.name, peer);
        if (!remoteRoot) continue;
        const inboxResult = await this.#sshRunCamInbox(peer, remoteRoot, targetAgent);
        if (!inboxResult.ok) continue;

        let remoteRows;
        try {
          remoteRows = JSON.parse(inboxResult.text);
        } catch (error) {
          this.log("peer.remote_inbox.invalid_json", {
            peerName: peer.name,
            targetAgent,
            error: error.message,
          });
          continue;
        }
        if (!Array.isArray(remoteRows)) continue;

        for (const row of remoteRows) {
          if (!row || row.targetAgent !== targetAgent) continue;
          if (String(row.delivery || "").toLowerCase() !== "received") continue;
          const remoteMessageId = String(row.messageId || "").trim();
          if (!remoteMessageId) continue;
          const relayKey = `${peer.name}:${remoteMessageId}`;
          if (seenRelayKeys.has(relayKey)) continue;

          const importedRow = {
            ...row,
            messageId: this.#relayImportedMessageId(peer.name, remoteMessageId),
            remoteMessageId,
            relayKey,
            relayedFromPeerName: peer.name,
            relayedAt: new Date().toISOString(),
            targetNode: this.config.nodeName,
            sourceNode: row.sourceNode || peer.remoteSync?.remoteNodeName || peer.name,
            sourceRoute: `peer:${peer.name}`,
            sourceAgent: this.#deriveRelayedSourceAgent(row, row.sourceAgent),
            relayObservedSourceAgent: row.sourceAgent || null,
            relayObservedTargetAgent: row.targetAgent || null,
            delivery: "received",
          };
          appendMailbox(importedRow);
          appendEvent("message.received.relayed", importedRow);
          if (
            targetAgent === CAM_TEST_MAILBOX_AGENT &&
            (
              String(importedRow.messageType || "").toLowerCase() === GUI_TEST_REPLY_MESSAGE_TYPE ||
              String(importedRow.body || "").includes("CAM_GUI_TEST_RESPONSE")
            )
          ) {
            appendTestEvent(importedRow.correlationId, "reply_received", {
              inbound: importedRow,
              relayed: true,
            });
          }
          this.#ingestDiscoveryEvidence({
            targetPeerName: null,
            source: "mailbox",
            body: importedRow.body,
            sourceAgent: importedRow.sourceAgent,
            correlationId: importedRow.correlationId,
            messageType: importedRow.messageType,
          });
          imported.push(importedRow);
          seenRelayKeys.add(relayKey);
          for (const listener of [...this.mailboxListeners]) {
            try {
              listener(importedRow);
            } catch (error) {
              this.log("mailbox.listener.error", { error: error.message });
            }
          }
        }
      } catch (error) {
        this.log("peer.remote_inbox.harvest_failed", {
          peerName: peer.name,
          targetAgent,
          error: error.message,
        });
      }
    }

    return imported;
  }

  async #fetchRemoteCamInventory(peer, remoteRoot) {
    const exported = await this.#sshRunCamInventoryExport(peer, remoteRoot);
    if (exported.ok) {
      try {
        return {
          ok: true,
          source: "cam inventory export",
          degraded: false,
          inventory: JSON.parse(exported.text),
        };
      } catch (error) {
        return {
          ok: false,
          error: `invalid remote CAM inventory JSON: ${error.message}`,
        };
      }
    }

    if (!/unknown command:\s*inventory/i.test(exported.error || "")) {
      return {
        ok: false,
        error: exported.error,
      };
    }

    this.log("peer.remote_inventory.fallback_legacy", {
      peerName: peer.name,
      reason: "remote cam lacks inventory export command",
    });

    const registryResult = await this.#sshRunRemoteRegistryExport(peer, remoteRoot);
    if (registryResult.ok) {
      try {
        const parsed = JSON.parse(registryResult.text);
        return {
          ok: true,
          source: "remote ~/.qexow-cam/agents.json",
          degraded: true,
          inventory: {
            version: 1,
            nodeName: parsed?.nodeName || peer.name,
            exportedAt: new Date().toISOString(),
            exportPolicy: "legacy-raw-registry",
            agents: parsed?.agents || {},
            discoveries: parsed?.localDiscoveries ? { local: parsed.localDiscoveries } : undefined,
            peers: parsed?.peers || {},
          },
        };
      } catch (error) {
        return {
          ok: false,
          error: `invalid remote CAM registry JSON: ${error.message}`,
        };
      }
    }

    const statusResult = await this.#sshRunCamDaemonStatus(peer, remoteRoot);
    if (!statusResult.ok) {
      return { ok: false, error: `inventory export unsupported; remote registry unavailable; daemon status failed: ${statusResult.error}` };
    }
    const agentListResult = await this.#sshRunCamAgentList(peer, remoteRoot);
    if (!agentListResult.ok) {
      return { ok: false, error: `inventory export unsupported; agent list failed: ${agentListResult.error}` };
    }

    let daemonStatus;
    try {
      daemonStatus = JSON.parse(statusResult.text);
    } catch (error) {
      return {
        ok: false,
        error: `invalid remote CAM daemon status JSON: ${error.message}`,
      };
    }

    return {
      ok: true,
      source: "cam daemon status + cam agent list",
      degraded: true,
      inventory: {
        version: 1,
        nodeName: daemonStatus.nodeName || peer.name,
        exportedAt: new Date().toISOString(),
        exportPolicy: "legacy-agent-list",
        agents: this.#parseLegacyAgentList(agentListResult.text),
        peers: {},
      },
    };
  }

  async #sshRunRemoteRegistryExport(peer, remoteRoot) {
    const root = remoteRoot || this.#remoteManagerRoot(peer);
    const command = [
      "if [ -f ~/.qexow-cam/agents.json ]; then cat ~/.qexow-cam/agents.json; else echo __CAM_REGISTRY_MISSING__; exit 44; fi",
    ].join(" ");
    this.log("peer.remote_registry.command", {
      peerName: peer.name,
      command,
    });
    const result = await this.#probeRemoteCommand(peer, command, 20000);
    if (result.ok && /__CAM_REGISTRY_MISSING__/.test(result.text || "")) {
      return { ok: false, error: "remote registry file missing" };
    }
    return result;
  }

  async #sshRunCamDaemonStatus(peer, remoteRoot) {
    const root = remoteRoot || this.#remoteManagerRoot(peer);
    const command = this.#remoteCamShell(root, ["daemon", "status"]);
    this.log("peer.remote_status.command", {
      peerName: peer.name,
      command,
    });
    return this.#probeRemoteCommand(peer, command, 20000);
  }

  async #sshRunCamAgentList(peer, remoteRoot) {
    const root = remoteRoot || this.#remoteManagerRoot(peer);
    const command = this.#remoteCamShell(root, ["agent", "list"]);
    this.log("peer.remote_agent_list.command", {
      peerName: peer.name,
      command,
    });
    return this.#probeRemoteCommand(peer, command, 20000);
  }

  #parseLegacyAgentList(text) {
    const lines = String(text || "").split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
    const agents = [];
    for (const line of lines) {
      const parts = line.split("\t");
      if (parts.length < 9) continue;
      agents.push({
        name: parts[0] || null,
        status: parts[1] || null,
        node: parts[2] || null,
        threadId: parts[3] && parts[3] !== "-" ? parts[3] : null,
        model: parts[4] && parts[4] !== "-" ? parts[4] : null,
        modelProvider: parts[5] && parts[5] !== "-" ? parts[5] : null,
        effort: parts[6] && parts[6] !== "-" ? parts[6] : null,
        serviceTier: parts[7] && parts[7] !== "-" && parts[7] !== "standard" ? parts[7] : null,
        cwd: parts.slice(8).join("\t") || null,
      });
    }
    return agents;
  }

  #remoteManagerRoot(peer) {
    if (peer?.remoteRoot && peer.remoteRoot !== "auto") return peer.remoteRoot;
    if (peer?.remoteSync?.remoteRoot) return peer.remoteSync.remoteRoot;
    const username = this.#sshUsername(peer) || "ubuntu";
    if (username === "root") return "/root/codex-agent-manager";
    return `/home/${username}/codex-agent-manager`;
  }

  #sshUsername(peer) {
    const ssh = String(peer?.ssh || "");
    if (!ssh.includes("@")) return null;
    return ssh.split("@")[0] || null;
  }

  async #probeRemoteCommand(peer, remoteCommand, timeoutMs = 10000) {
    return new Promise((resolve) => {
      const ssh = String(peer?.ssh || "");
      const key = String(peer?.key || "");
      if (!ssh || !ssh.includes("@") || !key) {
        resolve({ ok: false, error: "peer is missing ssh or key" });
        return;
      }
      const nullDevice = process.platform === "win32" ? "NUL" : "/dev/null";
      const child = spawn("ssh", [
        "-o", "BatchMode=yes",
        "-o", "StrictHostKeyChecking=no",
        "-o", `UserKnownHostsFile=${nullDevice}`,
        "-o", "ConnectTimeout=5",
        "-i", key,
        ssh,
        remoteCommand,
      ], {
        windowsHide: true,
        shell: false,
      });
      let stdout = "";
      let stderr = "";
      const timer = setTimeout(() => {
        try { child.kill(); } catch {}
        resolve({ ok: false, error: "timeout" });
      }, timeoutMs);
      child.stdout?.on("data", (chunk) => { stdout += String(chunk); });
      child.stderr?.on("data", (chunk) => { stderr += String(chunk); });
      child.on("error", (error) => {
        clearTimeout(timer);
        resolve({ ok: false, error: error.message });
      });
      child.on("exit", (code) => {
        clearTimeout(timer);
        if (code !== 0) {
          resolve({ ok: false, error: stderr.trim() || `ssh exit ${code}` });
          return;
        }
        resolve({ ok: true, text: stdout, stderr: stderr.trim() || null });
      });
    });
  }

  async #discoverPeer(body) {
    const peerName = String(body?.peerName || "").trim();
    const targetAgent = String(body?.targetAgent || "").trim();
    const waitSeconds = Math.max(5, Math.min(Number(body?.waitSeconds || 45), 120));
    if (!peerName) throw new Error("peerName is required");
    if (!targetAgent) throw new Error("targetAgent is required");

    const correlationId = crypto.randomUUID();
    const outbound = await this.#sendMessage({
      targetAgent,
      sourceAgent: CAM_TEST_MAILBOX_AGENT,
      sourceNode: this.config.nodeName,
      correlationId,
      messageType: NODE_DISCOVERY_REQUEST_TYPE,
      strict: true,
      message: buildNodeDiscoveryPrompt({ peerName }),
    });

    if (!outbound?.delivered) {
      return {
        ok: false,
        peerName,
        targetAgent,
        correlationId,
        error: outbound?.error || "discovery request was not delivered",
        outbound,
      };
    }

    const deadline = Date.now() + waitSeconds * 1000;
    let mailboxEvidence = null;
    let transcriptEvidence = null;

    while (Date.now() < deadline && (!mailboxEvidence || !transcriptEvidence)) {
      if (!mailboxEvidence) {
        mailboxEvidence = this.#findMailboxDiscoveryEvidence(peerName, targetAgent, correlationId);
      }
      if (!transcriptEvidence) {
        transcriptEvidence = await this.#readTranscriptDiscoveryEvidence(peerName, targetAgent);
      }
      if (mailboxEvidence && transcriptEvidence) break;
      await new Promise((resolve) => setTimeout(resolve, 2000));
    }

    const peer = getPeer(this.config, peerName);
    const agreement = this.#compareDiscoveryEvidence(mailboxEvidence?.parsed?.data, transcriptEvidence?.parsed?.data);

    if (!mailboxEvidence && !transcriptEvidence) {
      this.log("peer.discovery.timeout", { peerName, targetAgent, correlationId, waitSeconds });
      appendEvent("peer.discovery.timeout", { peerName, targetAgent, correlationId, waitSeconds });
      return {
        ok: false,
        peerName,
        targetAgent,
        correlationId,
        error: "no discovery evidence received before timeout",
        outbound,
      };
    }

    return {
      ok: true,
      peerName,
      targetAgent,
      correlationId,
      outbound,
      mailboxEvidence,
      transcriptEvidence,
      agreement,
      peer,
    };
  }

  #findMailboxDiscoveryEvidence(peerName, targetAgent, correlationId) {
    const rows = readMailbox(CAM_TEST_MAILBOX_AGENT).filter((row) =>
      row.correlationId === correlationId &&
      row.sourceAgent === targetAgent &&
      row.delivery === "received",
    );
    for (const row of rows.reverse()) {
      const parsed = parseNodeDiscoveryEvidence(row.body || "");
      if (!parsed.ok) continue;
      this.#ingestDiscoveryEvidence({
        targetPeerName: peerName,
        source: "mailbox",
        body: row.body,
        sourceAgent: row.sourceAgent,
        correlationId,
        messageType: row.messageType,
      });
      return { message: row, parsed };
    }
    return null;
  }

  async #readTranscriptDiscoveryEvidence(peerName, targetAgent) {
    const agent = getAgent(this.config, targetAgent);
    if (!agent?.threadId) return null;
    let thread;
    try {
      thread = await this.appServer.request("thread/read", {
        threadId: agent.threadId,
        includeTurns: true,
      }, 60000);
    } catch {
      return null;
    }
    const texts = this.#collectThreadTexts(thread);
    for (const text of texts.reverse()) {
      const parsed = parseNodeDiscoveryEvidence(text);
      if (!parsed.ok) continue;
      this.#ingestDiscoveryEvidence({
        targetPeerName: peerName,
        source: "transcript",
        body: text,
        sourceAgent: targetAgent,
        correlationId: null,
        messageType: null,
      });
      return { text, parsed };
    }
    return null;
  }

  #collectThreadTexts(threadResult) {
    const thread = threadResult?.thread || threadResult;
    const texts = [];
    for (const turn of Array.isArray(thread?.turns) ? thread.turns : []) {
      for (const item of Array.isArray(turn.items) ? turn.items : []) {
        for (const text of this.#extractDiscoveryStrings(item)) {
          texts.push(text);
        }
      }
    }
    return texts;
  }

  #extractDiscoveryStrings(value, out = []) {
    if (typeof value === "string") {
      if (value.includes(NODE_DISCOVERY_MARKER)) out.push(value);
      return out;
    }
    if (Array.isArray(value)) {
      for (const item of value) this.#extractDiscoveryStrings(item, out);
      return out;
    }
    if (value && typeof value === "object") {
      for (const nested of Object.values(value)) this.#extractDiscoveryStrings(nested, out);
    }
    return out;
  }

  #ingestDiscoveryEvidence({ targetPeerName, source, body, sourceAgent, correlationId, messageType }) {
    const parsed = parseNodeDiscoveryEvidence(body || "");
    if (!parsed.ok) return null;

    const peerName = targetPeerName || parsed.data.peerName || sourceAgent || null;
    if (!peerName) return null;

    const existing = getPeer(this.config, peerName) || {};
    const next = {
      ...existing,
      name: peerName,
      discoveryPrimary: existing.discoveryPrimary || "installer",
      observedHostname: parsed.data.hostname || existing.observedHostname || null,
      observedWhoami: parsed.data.whoami || existing.observedWhoami || null,
      observedPrivateIps: this.#mergeArrays(existing.observedPrivateIps, parsed.data.privateIps),
      observedPublicIp: parsed.data.publicIp || existing.observedPublicIp || null,
      camNodeName: parsed.data.camNodeName || existing.camNodeName || null,
      camBindHost: parsed.data.camBindHost || existing.camBindHost || null,
      camPort: parsed.data.camPort || existing.camPort || null,
      camConfigPath: parsed.data.camConfigPath || existing.camConfigPath || null,
      camRegistryPath: parsed.data.camRegistryPath || existing.camRegistryPath || null,
      camRoot: parsed.data.camRoot || existing.camRoot || null,
      camOk: parsed.data.camOk ?? existing.camOk ?? null,
      discoveryEvidence: {
        ...(existing.discoveryEvidence || {}),
        [source]: {
          parsedAt: new Date().toISOString(),
          parserMode: parsed.mode,
          sourceAgent,
          correlationId,
          messageType,
          raw: body,
          data: parsed.data,
        },
      },
      lastDiscoveryAt: new Date().toISOString(),
    };

    const conflicts = this.#peerDiscoveryConflicts(existing, parsed.data);
    const peer = upsertPeer(this.config, peerName, next);
    if (source !== "installer") {
      this.log("peer.discovery.fallback.write", {
        peerName,
        source,
        parserMode: parsed.mode,
        correlationId,
        messageType,
      });
      appendEvent("peer.discovery.fallback.write", {
        peerName,
        source,
        parserMode: parsed.mode,
        correlationId,
        messageType,
      });
    }
    if (conflicts.length) {
      this.log("peer.discovery.conflict", { peerName, source, conflicts });
      appendEvent("peer.discovery.conflict", { peerName, source, conflicts });
    }
    return peer;
  }

  #mergeArrays(left, right) {
    return [...new Set([...(Array.isArray(left) ? left : []), ...(Array.isArray(right) ? right : [])].filter(Boolean))];
  }

  #refreshDocPeerFacts() {
    try {
      const rows = discoverPeerFactsFromMarkdown({ codexHome: this.config.codexHome });
      this.docKeyPaths = discoverSshKeyPathsFromMarkdown({ codexHome: this.config.codexHome });
      const knownPeers = this.#canonicalPeerNames();
      const aliases = new Map([
        ["production-frontend", "prod-frontend"],
        ["production-backend", "prod-backend"],
        ["racknerd-vps-webmail", "racknerd-vpn-codex"],
        ["frontend-dev-frontend", "frontend"],
        ["backend-dev-backend", "backend"],
        ["copilotkit-assistant", "copilotkit"],
        ["racknerd-vps", "racknerd-vpn-codex"],
      ]);
      for (const row of rows) {
        const targetName = knownPeers.has(row.peerName) ? row.peerName : (aliases.get(row.peerName) || null);
        if (!targetName) continue;
        const existing = getPeer(this.config, targetName) || null;
        const peer = upsertPeer(this.config, targetName, {
          ...(existing || {}),
          name: targetName,
          discovered: existing?.discovered ?? false,
          docDiscovery: {
            files: row.files,
            displaySections: row.displaySections,
            candidateIps: row.candidateIps,
            candidatePrivateIps: row.candidatePrivateIps,
            candidateSshTargets: row.candidateSshTargets,
            candidateHostnames: row.candidateHostnames,
            sourceLines: row.sourceLines,
            scrapedAt: new Date().toISOString(),
          },
        });
        if (!existing || JSON.stringify(existing.docDiscovery || null) !== JSON.stringify(peer.docDiscovery || null)) {
          this.log("peer.discovery.fallback.docs", {
            peerName: targetName,
            files: row.files.length,
            candidateIps: row.candidateIps,
            candidateSshTargets: row.candidateSshTargets,
            probeEligible: this.#registryKeyPool(loadRegistry(this.config)).length > 0,
          });
          appendEvent("peer.discovery.fallback.docs", {
            peerName: targetName,
            files: row.files.length,
            candidateIps: row.candidateIps,
            candidateSshTargets: row.candidateSshTargets,
            probeEligible: this.#registryKeyPool(loadRegistry(this.config)).length > 0,
          });
        }
      }
      this.#pruneNonCanonicalPeers(knownPeers);
    } catch (error) {
      this.log("peer.discovery.docs.failed", { error: error.message });
    }
  }

  #canonicalPeerNames() {
    const names = new Set([
      "backend",
      "frontend",
      "dashboard",
      "searchbox",
      "copilotkit",
      "multi-site",
      "prod-frontend",
      "prod-backend",
      "racknerd-vpn-codex",
      "frontend-fresh",
    ]);
    const registry = loadRegistry(this.config);
    for (const [name, peer] of Object.entries(registry.peers || {})) {
      if (peer?.codexHostId || peer?.discovered === true || peer?.remoteSync?.syncedAt) names.add(name);
    }
    return names;
  }

  #pruneNonCanonicalPeers(canonicalNames) {
    const registry = loadRegistry(this.config);
    let changed = false;
    for (const [name, peer] of Object.entries(registry.peers || {})) {
      if (canonicalNames.has(name)) continue;
      delete registry.peers[name];
      changed = true;
      this.log("peer.discovery.docs.pruned", { peerName: name, reason: "non-canonical-peer" });
      appendEvent("peer.discovery.docs.pruned", { peerName: name, reason: "non-canonical-peer" });
    }
    if (changed) saveRegistry(registry);
  }

  #restorePeerTransportFromBackups() {
    try {
      const current = loadRegistry(this.config);
      const candidates = this.#backupRegistryFiles();
      if (!candidates.length) return;
      let changed = false;
      for (const file of candidates) {
        let backup;
        try {
          backup = JSON.parse(fs.readFileSync(file, "utf8"));
        } catch {
          continue;
        }
        const peers = backup?.peers || {};
        for (const [name, oldPeer] of Object.entries(peers)) {
          if (!oldPeer || typeof oldPeer !== "object") continue;
          const live = current.peers?.[name];
          if (!live) continue;
          const needsKey = !live.key && !!oldPeer.key;
          const needsSsh = (!live.ssh || !String(live.ssh).includes("@")) && !!oldPeer.ssh && String(oldPeer.ssh).includes("@");
          const needsTransport = live.transport === "codex-managed" && oldPeer.transport === "ssh";
          if (!needsKey && !needsSsh && !needsTransport) continue;
          current.peers[name] = {
            ...live,
            transport: needsTransport ? oldPeer.transport : live.transport,
            ssh: needsSsh ? oldPeer.ssh : live.ssh,
            key: needsKey ? oldPeer.key : live.key,
            recoveredTransportFromBackup: {
              file,
              recoveredAt: new Date().toISOString(),
            },
          };
          changed = true;
          this.log("peer.transport.recovered_from_backup", {
            peerName: name,
            file,
            recoveredKey: needsKey,
            recoveredSsh: needsSsh,
            recoveredTransport: needsTransport,
          });
          appendEvent("peer.transport.recovered_from_backup", {
            peerName: name,
            file,
            recoveredKey: needsKey,
            recoveredSsh: needsSsh,
            recoveredTransport: needsTransport,
          });
        }
      }
      if (changed) {
        saveRegistry(current);
      }
    } catch (error) {
      this.log("peer.transport.recovery.failed", { error: error.message });
    }
  }

  #backupRegistryFiles() {
    const root = paths().root;
    const files = [];
    const direct = fs.existsSync(root) ? fs.readdirSync(root) : [];
    for (const name of direct) {
      if (/^agents\.json\.bak-/i.test(name)) files.push(path.join(root, name));
    }
    const installBackups = path.join(root, "install-backups");
    if (fs.existsSync(installBackups)) {
      for (const dir of fs.readdirSync(installBackups)) {
        const full = path.join(installBackups, dir);
        if (!fs.statSync(full).isDirectory()) continue;
        for (const name of fs.readdirSync(full)) {
          if (/^agents\.json\.bak-/i.test(name)) files.push(path.join(full, name));
        }
      }
    }
    return files.sort((a, b) => {
      try {
        return fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs;
      } catch {
        return 0;
      }
    });
  }

  async #probeDocDiscoveredPeers() {
    const registry = loadRegistry(this.config);
    const keyPool = this.#registryKeyPool(registry);
    if (!keyPool.length) {
      this.log("peer.discovery.probe.skipped", { reason: "no_registry_keys_available" });
      return;
    }

    const peers = Object.values(registry.peers || {}).filter(Boolean);
    await Promise.all(peers.map((peer) => this.#probeSingleDocDiscoveredPeer(peer, keyPool)));
  }

  async #probeSingleDocDiscoveredPeer(peer, keyPool) {
    if (!peer || peer.key) return;
    const doc = peer.docDiscovery;
    if (!doc?.candidateIps?.length) return;
    if (this.#isSelfPeer(peer)) {
      this.log("peer.discovery.probe.skipped", {
        peerName: peer.name,
        reason: "self-peer",
      });
      appendEvent("peer.discovery.probe.skipped", {
        peerName: peer.name,
        reason: "self-peer",
      });
      return;
    }
    const usernames = this.#candidateUsernames(peer);
    if (!usernames.length) return;
    let lastFailure = null;
    for (const ip of doc.candidateIps) {
      for (const username of usernames) {
        const cacheKey = `${peer.name}|${username}@${ip}|${keyPool.map((row) => row.key).join(",")}`;
        const lastTried = this.peerProbeAttempts.get(cacheKey);
        if (lastTried && Date.now() - lastTried < 10 * 60 * 1000) continue;
        this.peerProbeAttempts.set(cacheKey, Date.now());
        const verified = await this.#tryRegistryKeysAgainstTarget(keyPool, username, ip);
        if (!verified) {
          lastFailure = `no SSH key matched ${username}@${ip}`;
          continue;
        }
        const updated = upsertPeer(this.config, peer.name, {
          ...peer,
          transport: "ssh",
          ssh: `${username}@${ip}`,
          key: verified.key,
          docProbe: {
            verifiedAt: new Date().toISOString(),
            username,
            ip,
            recoveredFromRegistryKeyOwner: verified.owner,
            lastFailure: null,
          },
        });
        this.log("peer.discovery.probe.verified", {
          peerName: peer.name,
          ssh: `${username}@${ip}`,
          keyOwner: verified.owner,
        });
        appendEvent("peer.discovery.probe.verified", {
          peerName: peer.name,
          ssh: `${username}@${ip}`,
          keyOwner: verified.owner,
        });
        try {
          await this.#syncSinglePeer(updated);
        } catch (error) {
          this.log("peer.discovery.probe.sync_failed", {
            peerName: peer.name,
            ssh: `${username}@${ip}`,
            error: error.message,
          });
        }
        break;
      }
      const refreshed = getPeer(this.config, peer.name);
      if (refreshed?.key) break;
    }
    let refreshed = getPeer(this.config, peer.name);
    if (!refreshed?.key && Array.isArray(doc?.candidateSshTargets) && doc.candidateSshTargets.length) {
      const direct = await this.#tryDocSshTargetsAndSync(peer, keyPool, doc.candidateSshTargets);
      if (direct.ok) {
        refreshed = getPeer(this.config, peer.name);
      } else if (direct.error) {
        lastFailure = direct.error;
      }
    }
    if (!refreshed?.key && lastFailure) {
      upsertPeer(this.config, peer.name, {
        ...refreshed,
        docProbe: {
          ...(refreshed?.docProbe || {}),
          lastFailure,
          attemptedAt: new Date().toISOString(),
        },
      });
      this.log("peer.discovery.probe.failed", {
        peerName: peer.name,
        error: lastFailure,
      });
    }
  }

  async #tryDocSshTargetsAndSync(peer, keyPool, candidateTargets) {
    for (const target of candidateTargets || []) {
      const normalizedTarget = String(target || "").trim();
      if (!normalizedTarget.includes("@")) continue;
      const [username, ip] = normalizedTarget.split("@");
      for (const candidate of keyPool) {
        const candidatePeer = {
          ...peer,
          transport: "ssh",
          ssh: normalizedTarget,
          key: candidate.key,
        };
        const result = await this.#syncSinglePeer(candidatePeer);
        if (!result?.ok) continue;
        upsertPeer(this.config, peer.name, {
          ...getPeer(this.config, peer.name),
          transport: "ssh",
          ssh: normalizedTarget,
          key: candidate.key,
          docProbe: {
            ...(getPeer(this.config, peer.name)?.docProbe || {}),
            verifiedAt: new Date().toISOString(),
            username,
            ip,
            recoveredFromRegistryKeyOwner: candidate.owner,
            lastFailure: null,
            directDocTarget: true,
          },
        });
        this.log("peer.discovery.doc_target.verified", {
          peerName: peer.name,
          ssh: normalizedTarget,
          keyOwner: candidate.owner,
        });
        return { ok: true, ssh: normalizedTarget, key: candidate.key };
      }
    }
    const firstTarget = (candidateTargets || []).find((row) => String(row || "").includes("@"));
    return {
      ok: false,
      error: firstTarget ? `direct doc SSH target failed for ${firstTarget}` : "no direct doc SSH target available",
    };
  }

  #registryKeyPool(registry) {
    const peers = Object.values(registry?.peers || {});
    const seen = new Set();
    const pool = [];
    for (const peer of peers) {
      const key = peer?.key ? String(peer.key) : "";
      if (!key || seen.has(key)) continue;
      seen.add(key);
      pool.push({ owner: peer.name, key, source: "registry" });
    }
    for (const row of this.docKeyPaths || []) {
      const key = String(row?.keyPath || "");
      if (!key || seen.has(key) || !fs.existsSync(key)) continue;
      seen.add(key);
      pool.push({ owner: row.file || "docs", key, source: "docs" });
    }
    return pool;
  }

  async #runPeerDiscoveryPass(source = "manual") {
    if (this.peerDiscoveryPassPromise) {
      this.log("peer.discovery.pass.coalesced", { source });
      return this.peerDiscoveryPassPromise;
    }
    this.peerDiscoveryPassPromise = (async () => {
      this.log("peer.discovery.pass.start", { source });
      try {
        this.#refreshDocPeerFacts();
        await this.#probeDocDiscoveredPeers();
        await this.#syncKnownPeers();
        this.log("peer.discovery.pass.complete", { source });
      } catch (error) {
        this.log("peer.discovery.pass.failed", { source, error: error.message });
      } finally {
        this.peerDiscoveryPassPromise = null;
      }
    })();
    return this.peerDiscoveryPassPromise;
  }

  #candidateUsernames(peer) {
    const usernames = [];
    const ssh = String(peer?.ssh || "");
    if (ssh.includes("@")) usernames.push(ssh.split("@")[0]);
    for (const target of peer?.docDiscovery?.candidateSshTargets || []) {
      if (String(target).includes("@")) usernames.push(String(target).split("@")[0]);
    }
    for (const username of peer?.docDiscovery?.candidateUsernames || []) {
      usernames.push(String(username));
    }
    return [...new Set(usernames.filter(Boolean))];
  }

  async #tryRegistryKeysAgainstTarget(keyPool, username, ip) {
    for (const candidate of keyPool) {
      const result = await this.#probeSsh(candidate.key, username, ip);
      if (result.ok) {
        return { ...candidate, ...result };
      }
    }
    return null;
  }

  async #probeSsh(keyPath, username, ip) {
    return new Promise((resolve) => {
      const nullDevice = process.platform === "win32" ? "NUL" : "/dev/null";
      const child = spawn("ssh", [
        "-o", "BatchMode=yes",
        "-o", "StrictHostKeyChecking=no",
        "-o", `UserKnownHostsFile=${nullDevice}`,
        "-o", "ConnectTimeout=5",
        "-i", keyPath,
        `${username}@${ip}`,
        "hostname; whoami; hostname -I",
      ], {
        windowsHide: true,
        shell: false,
      });

      let stdout = "";
      let stderr = "";
      const timer = setTimeout(() => {
        try { child.kill(); } catch {}
        resolve({ ok: false, error: "timeout" });
      }, 8000);

      child.stdout?.on("data", (chunk) => { stdout += String(chunk); });
      child.stderr?.on("data", (chunk) => { stderr += String(chunk); });
      child.on("error", (error) => {
        clearTimeout(timer);
        resolve({ ok: false, error: error.message });
      });
      child.on("exit", (code) => {
        clearTimeout(timer);
        if (code !== 0) {
          resolve({ ok: false, error: stderr.trim() || `ssh exit ${code}` });
          return;
        }
        const lines = stdout.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
        resolve({
          ok: true,
          hostname: lines[0] || null,
          whoami: lines[1] || null,
          privateIps: lines[2] ? lines[2].split(/\s+/).filter(Boolean) : [],
          raw: stdout.trim(),
        });
      });
    });
  }

  #peerDiagnosticsPayload() {
    const registry = loadRegistry(this.config);
    const peers = listPeers(this.config).map((peer) => this.#describePeer(registry, peer));
    const summary = {
      total: peers.length,
      codexManaged: peers.filter((peer) => peer.transport === "codex-managed").length,
      docMatched: peers.filter((peer) => peer.docFilesCount > 0).length,
      probeReady: peers.filter((peer) => peer.state === "probe-ready").length,
      probeFailed: peers.filter((peer) => peer.state === "probe-failed").length,
      missingIp: peers.filter((peer) => peer.state === "missing-ip").length,
      missingKey: peers.filter((peer) => peer.state === "missing-key").length,
      missingUsername: peers.filter((peer) => peer.state === "missing-username").length,
      verified: peers.filter((peer) => peer.state === "verified").length,
      mirrored: peers.filter((peer) => peer.state === "mirrored").length,
      mirroredDegraded: peers.filter((peer) => peer.state === "mirrored-degraded").length,
      syncFailed: peers.filter((peer) => peer.state === "sync-failed").length,
      remoteRawDiscoveries: peers.reduce((sum, peer) => sum + Number(peer.remoteRawDiscoveries || 0), 0),
      remoteApprovedDiscoveries: peers.reduce((sum, peer) => sum + Number(peer.remoteApprovedDiscoveries || 0), 0),
      remoteRejectedDiscoveries: peers.reduce((sum, peer) => sum + Number(peer.remoteRejectedDiscoveries || 0) + Number(peer.remoteQuarantinedDiscoveries || 0), 0),
      availableKeys: this.#registryKeyPool(registry).length,
      docKeyPaths: (this.docKeyPaths || []).length,
    };
    return { ok: true, peers, summary };
  }

  #describePeer(registry, peer) {
    const doc = peer?.docDiscovery || {};
    const remoteSync = peer?.remoteSync || {};
    const remoteDiscoveryCounts = remoteSync?.remoteDiscoveryCounts || {};
    const ssh = String(peer?.ssh || "");
    const candidateIps = this.#mergeArrays(
      doc.candidateIps || [],
      ssh.includes("@") ? [ssh.split("@")[1]] : [],
    );
    const candidateUsernames = this.#candidateUsernames(peer);
    const docFiles = doc.files || [];
    const keyPool = this.#registryKeyPool(registry);
    const hasAnyKey = keyPool.length > 0;
    const mirroredAgents = Array.isArray(remoteSync.mirroredAgents) ? remoteSync.mirroredAgents.length : 0;
    const verifiedSsh = peer?.transport === "ssh" && ssh.includes("@") && !!peer?.key;
    const lastProbeFailure = String(peer?.docProbe?.lastFailure || "").trim();
    const blockers = [];
    let state = "codex-alias-only";

    if (remoteSync?.syncedAt && mirroredAgents > 0) {
      state = remoteSync?.remoteInventoryDegraded ? "mirrored-degraded" : "mirrored";
    } else if (remoteSync?.lastStatus === "failed") {
      state = "sync-failed";
      blockers.push(remoteSync?.lastError || "Remote CAM sync failed.");
    } else if (remoteSync?.syncedAt && mirroredAgents === 0 && verifiedSsh) {
      state = "verified";
    } else if (verifiedSsh) {
      state = "verified";
    } else if (!docFiles.length) {
      state = "needs-doc-match";
      blockers.push("No canonical docs match yet.");
    } else if (!candidateIps.length) {
      state = "missing-ip";
      blockers.push("Docs matched this node, but no candidate IP was scraped.");
    } else if (!candidateUsernames.length) {
      state = "missing-username";
      blockers.push("Candidate IPs exist, but no SSH username was found.");
    } else if (!hasAnyKey) {
      state = "missing-key";
      blockers.push("No local SSH key path was discovered or registered.");
    } else if (lastProbeFailure) {
      state = "probe-failed";
      blockers.push(lastProbeFailure);
    } else {
      state = "probe-ready";
    }

    if (remoteSync?.syncedAt && mirroredAgents === 0 && verifiedSsh) {
      blockers.push("SSH verified, but remote CAM inventory returned zero mirrored agents.");
    }
    if (verifiedSsh && !remoteSync?.syncedAt) {
      blockers.push("SSH verified, waiting for remote CAM inventory sync.");
    }

    return {
      name: peer?.name || "",
      transport: peer?.transport || "",
      codexDisplayName: peer?.codexDisplayName || "",
      codexHostId: peer?.codexHostId || "",
      ssh: ssh,
      key: peer?.key || "",
      keySource: peer?.docProbe?.recoveredFromRegistryKeyOwner || "",
      state,
      blockers,
      blockerSummary: blockers.join(" "),
      docFilesCount: docFiles.length,
      candidateIps,
      candidatePrivateIps: doc.candidatePrivateIps || [],
      candidateSshTargets: doc.candidateSshTargets || [],
      candidateHostnames: doc.candidateHostnames || [],
      candidateUsernames,
      mirroredAgents,
      remoteRawDiscoveries: Number(remoteDiscoveryCounts.total || 0),
      remoteApprovedDiscoveries: Number(remoteDiscoveryCounts.approved || remoteSync?.remoteApprovedDiscoveries || 0),
      remoteCandidateDiscoveries: Number(remoteDiscoveryCounts.candidate || 0),
      remoteQuarantinedDiscoveries: Number(remoteDiscoveryCounts.quarantined || 0),
      remoteRejectedDiscoveries: Number(remoteDiscoveryCounts.rejected || remoteSync?.remoteRejectedDiscoveries || 0),
      remoteInventorySchema: remoteSync?.remoteInventorySchema || "",
      remoteInventoryDegraded: remoteSync?.remoteInventoryDegraded === true,
      remoteSyncStatus: remoteSync?.lastStatus || "",
      remoteSyncError: remoteSync?.lastError || "",
      lastProbeFailure: lastProbeFailure || "",
      remoteRoot: remoteSync?.remoteRoot || peer?.remoteRoot || "",
      remoteNodeName: remoteSync?.remoteNodeName || "",
      syncedAt: remoteSync?.syncedAt || "",
      discovered: peer?.discovered === true,
      sourceFiles: docFiles,
    };
  }

  #peerDiscoveryConflicts(existing, incoming) {
    const conflicts = [];
    if (existing?.observedPublicIp && incoming.publicIp && existing.observedPublicIp !== incoming.publicIp) {
      conflicts.push({ field: "observedPublicIp", existing: existing.observedPublicIp, incoming: incoming.publicIp });
    }
    if (existing?.observedHostname && incoming.hostname && existing.observedHostname !== incoming.hostname) {
      conflicts.push({ field: "observedHostname", existing: existing.observedHostname, incoming: incoming.hostname });
    }
    if (existing?.camNodeName && incoming.camNodeName && existing.camNodeName !== incoming.camNodeName) {
      conflicts.push({ field: "camNodeName", existing: existing.camNodeName, incoming: incoming.camNodeName });
    }
    return conflicts;
  }

  #compareDiscoveryEvidence(left, right) {
    if (!left || !right) return { compared: false, agreed: null, fields: [] };
    const fields = [
      ["hostname", left.hostname, right.hostname],
      ["publicIp", left.publicIp, right.publicIp],
      ["camNodeName", left.camNodeName, right.camNodeName],
    ].map(([field, a, b]) => ({ field, left: a || null, right: b || null, equal: (a || null) === (b || null) }));
    return {
      compared: true,
      agreed: fields.every((field) => field.equal || !field.left || !field.right),
      fields,
    };
  }
}


export async function runDaemon() {
  try {
    const daemon = new AgentManagerDaemon();
    await daemon.start();
    process.on("SIGINT", () => daemon.stop().then(() => process.exit(0)));
    process.on("SIGTERM", () => daemon.stop().then(() => process.exit(0)));
  } catch (err) {
    try {
      const logFile = path.join(os.homedir(), ".qexow-cam", "logs", "daemon.log");
      const entry = {
        timestamp: new Date().toISOString(),
        type: "daemon.startup.fatal_error",
        payload: { error: err.message, stack: err.stack },
      };
      fs.appendFileSync(logFile, JSON.stringify(entry) + "\n", "utf8");
    } catch (_) {}
    process.exit(1);
  }
}
