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
import { execFile } from "node:child_process";
import { AppServerClient, textInput } from "./app-server.js";
import { ensureLocalToken, loadConfig } from "./config.js";
import {
  appendEvent,
  appendMailbox,
  appendTestEvent,
  getAgent,
  listAgents,
  loadRegistry,
  markMailboxSurfaced,
  pendingMailbox,
  readMailbox,
  saveRegistry,
  setAgent,
  upsertAgent,
} from "./registry.js";
import { paths, writeJsonAtomic, readJson } from "./paths.js";
import { logEvent, enforceRetention } from "./logger.js";
import { bootstrapAntigravity } from "./antigravity.js";

const CAM_TEST_MAILBOX_AGENT = "CAM test, Kexau CAM test suite mailbox";
const MAILBOX_ONLY_THREAD_SOURCES = new Set(["mailbox", "gui-only"]);
const CAM_VERSION = "2.1.30";
const STRICT_THREAD_NOT_FOUND = /thread not found/i;
const GUI_TEST_MESSAGE_TYPE = "cam-gui-test";
const GUI_TEST_REPLY_MESSAGE_TYPE = "cam-gui-test-reply";

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
    this.skippedThreadReasons = new Map();
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
    for (const agent of listAgents(this.config)) {
      if (agent.threadId) this.threadToAgent.set(agent.threadId, agent.name);
    }
    await this.syncActiveThreads();
    this.threadSyncInterval = setInterval(() => {
      void this.syncActiveThreads();
    }, 30000);
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
      if (url.pathname === "/inbox" && req.method === "GET") {
        const agent = url.searchParams.get("agent");
        const wait = Number(url.searchParams.get("wait") || 0);

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

          const listener = (msg) => {
            if (resolved) return;
            if (msg === null) {
              resolved = true;
              clearTimeout(timer);
              const idx = this.mailboxListeners.indexOf(listener);
              if (idx >= 0) this.mailboxListeners.splice(idx, 1);
              json(res, 200, { ok: true, messages: readMailbox(agent) });
              resolve();
              return;
            }
            if (!agent || msg.targetAgent === agent) {
              resolved = true;
              clearTimeout(timer);
              const idx = this.mailboxListeners.indexOf(listener);
              if (idx >= 0) this.mailboxListeners.splice(idx, 1);
              json(res, 200, { ok: true, messages: readMailbox(agent) });
              resolve();
            }
          };

          this.mailboxListeners.push(listener);

          const timer = setTimeout(() => {
            if (resolved) return;
            resolved = true;
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
    return new Promise((resolve) => {
      const scriptPath = this.#queryThreadsScriptPath();
      if (!scriptPath) {
        this.log("sync.threads.failed", { error: "query_threads.py not found; refusing session_index fallback" });
        resolve();
        return;
      }

      const tryPython = (cmd) => {
        execFile(cmd, [scriptPath], { env: process.env }, (error, stdout, stderr) => {
          if (error) {
            if (cmd === "python") {
              tryPython("python3");
              return;
            }
            this.log("sync.threads.failed", { error: error.message, stderr, source: "query_threads.py" });
            resolve();
            return;
          }

          try {
            const data = JSON.parse(stdout);
            if (data.error) {
              this.log("sync.threads.failed", { error: data.error, source: "query_threads.py" });
              resolve();
              return;
            }
            const threads = Array.isArray(data.threads) ? data.threads : [];
            this.#applyThreadSync(threads);
            this.log("sync.threads.complete", { count: threads.length, source: "query_threads.py", scriptPath });
          } catch (e) {
            this.log("sync.threads.parse.failed", { error: e.message, stdout, source: "query_threads.py" });
          }
          resolve();
        });
      };

      tryPython("python");
    });
  }

  #queryThreadsScriptPath() {
    const candidates = [
      path.join(process.cwd(), "src", "query_threads.py"),
      path.join(process.cwd(), "query_threads.py"),
      path.join(path.dirname(process.execPath), "query_threads.py"),
      path.join(path.dirname(process.execPath), "src", "query_threads.py"),
      path.join(os.homedir(), "OneDrive", "Documents", "New project", "codex-agent-manager", "src", "query_threads.py"),
    ];
    return candidates.find((candidate) => fs.existsSync(candidate)) || null;
  }

  #applyThreadSync(threads) {
    try {
      const registry = listAgents(this.config);
      const existingThreadMap = new Map();
      for (const agent of registry) {
        if (agent.threadId) {
          existingThreadMap.set(agent.threadId, agent);
        }
      }

      const activeThreadIds = new Set();

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
        if (!cwd || cwd === "outside-of-project") {
          const errMsg = `Thread ${tid} (${name}) is missing a valid workspace path. Skipping registry sync.`;
          const previous = this.skippedThreadReasons.get(tid);
          if (previous !== errMsg) {
            this.log("sync.agent.error", { threadId: tid, name, error: errMsg });
            appendEvent("sync.agent.error", { threadId: tid, name, error: errMsg, skipped: true, reason: "missing-workspace" });
            this.skippedThreadReasons.set(tid, errMsg);
          }
          continue;
        }
        if (cwd.startsWith("\\\\?\\")) {
          cwd = cwd.substring(4);
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
          });
          this.threadToAgent.set(tid, uniqueName);
          appendEvent("agent.created", { agent });
          this.log("sync.agent.created", { name: uniqueName, threadId: tid, cwd });
        } catch (e) {
          this.log("sync.agent.create.failed", { threadId: tid, error: e.message });
        }
      }
      this.log("sync.agent.prune.skipped", { reason: "discovery is additive to avoid deleting active local agents", skippedThreads: this.skippedThreadReasons.size });
    } catch (e) {
      this.log("sync.apply.failed", { error: e.message });
    }
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
      `[To reply to this message, use the qexow-cam-messaging skill or send via CAM HTTP to targetAgent "${message.sourceAgent}" and include correlationId "${message.correlationId || ""}". Do not use older codex-agent-manager paths.]`
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

  async resolveRemoteRoot() {
    return null;
  }

  async syncPeer() {
    return;
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
