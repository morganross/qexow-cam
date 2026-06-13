import "./security.js";
import { enforceSpawnBlocks } from "./security.js";
enforceSpawnBlocks();
import fs from "node:fs";
import crypto from "node:crypto";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { apiRequest } from "./api.js";
import { allPaths, defaultCodexPath, initConfig, loadConfig } from "./config.js";
import { readMailbox, listAgents, loadRegistry, saveRegistry } from "./registry.js";
import { paths, readJson, writeJsonAtomic } from "./paths.js";
import { logEvent } from "./logger.js";

function usage() {
  return `Usage:
  cam init
  cam doctor
  cam daemon start|stop|status
  cam node enroll <name> --ssh <user@host> --key <path> --remote-root <path>
  cam node list
  cam agent create <name> --cwd <path> [--thread-id <id>] [--source <codex|antigravity>] [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]
  cam agent resume <name>
  cam agent set-model <name> [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]
  cam agent list
  cam agent status <name>
  cam agent read <name> [--latest]
  cam send <agent-name> <message> [--from <agent-name>] [--correlation-id <id>] [--message-type <type>] [--strict]
  cam inbox [agent-name]
  cam logs
  cam install-service
  cam uninstall-service`;
}

function parseOptions(args) {
  const opts = { _: [] };
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg.startsWith("--")) {
      const key = arg.slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
      opts[key] = args[i + 1] && !args[i + 1].startsWith("--") ? args[++i] : true;
    } else {
      opts._.push(arg);
    }
  }
  return opts;
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

function normalizeServiceTier(opts) {
  if (opts.speed && opts.serviceTier) throw new Error("use either --speed or --service-tier, not both");
  if (opts.speed === undefined && opts.serviceTier === undefined) return { provided: false, value: null };
  if (opts.speed !== undefined) {
    const speed = String(opts.speed).trim().toLowerCase();
    if (speed === "standard") return { provided: true, value: null };
    if (speed === "fast") return { provided: true, value: "fast" };
    throw new Error(`invalid speed '${opts.speed}'; expected standard|fast`);
  }
  const tier = String(opts.serviceTier).trim();
  if (!tier) throw new Error("--service-tier cannot be empty");
  if (tier.toLowerCase() === "default") {
    throw new Error("service tier 'default' is invalid; use --speed standard to omit the service tier override");
  }
  return { provided: true, value: tier };
}

// Helper: run a command with a timeout, return { ok, output }
function tryCommand(cmd, args, timeoutMs = 8000) {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(cmd, args, { windowsHide: true, shell: false });
    } catch (e) {
      return resolve({ ok: false, output: e.message });
    }
    let out = "";
    let err = "";
    const timer = setTimeout(() => {
      try { child.kill(); } catch (_) {}
      resolve({ ok: false, output: "timed out" });
    }, timeoutMs);
    child.stdout?.on("data", (d) => { out += d; });
    child.stderr?.on("data", (d) => { err += d; });
    child.on("error", (e) => { clearTimeout(timer); resolve({ ok: false, output: e.message }); });
    child.on("exit", (code) => {
      clearTimeout(timer);
      resolve({ ok: code === 0, output: (out || err).trim() });
    });
  });
}

// Helper: probe codex app-server over stdio
function checkCodexAppServer(codexPath) {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(codexPath, ["app-server", "--listen", "stdio://"], {
        stdio: ["pipe", "pipe", "pipe"],
        windowsHide: true,
      });
    } catch (e) {
      return resolve({ ok: false, detail: e.message });
    }
    let buffer = "";
    const timer = setTimeout(() => {
      try { child.kill(); } catch (_) {}
      resolve({ ok: false, detail: "timed out waiting for app-server response" });
    }, 12000);

    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      buffer += chunk;
      const idx = buffer.indexOf("\n");
      if (idx < 0) return;
      clearTimeout(timer);
      try { child.kill(); } catch (_) {}
      try {
        const msg = JSON.parse(buffer.slice(0, idx).trim());
        if (msg.result) {
          const info = msg.result.serverInfo ? `${msg.result.serverInfo.name} ${msg.result.serverInfo.version || ""}`.trim() : "OK";
          resolve({ ok: true, detail: info });
        } else if (msg.error) {
          resolve({ ok: false, detail: `protocol error: ${msg.error.message}` });
        } else {
          resolve({ ok: true, detail: "responds OK" });
        }
      } catch (_) {
        resolve({ ok: true, detail: "responds (non-JSON)" });
      }
    });
    child.on("error", (e) => { clearTimeout(timer); resolve({ ok: false, detail: e.message }); });
    child.on("exit", (code) => {
      clearTimeout(timer);
      if (code !== null) resolve({ ok: false, detail: `exited with code ${code} before responding` });
    });
    try {
      child.stdin.write(JSON.stringify({
        id: 1, method: "initialize",
        params: { clientInfo: { name: "cam-doctor", version: "0.1.0" }, capabilities: { experimentalApi: true } },
      }) + "\n");
    } catch (e) {
      clearTimeout(timer);
      try { child.kill(); } catch (_) {}
      resolve({ ok: false, detail: `stdin write failed: ${e.message}` });
    }
  });
}

async function commandDoctor() {
  const config = loadConfig();
  const p = allPaths();
  const codexPath = config.codexPath || defaultCodexPath();
  const localAppData = process.env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local");

  function row(ok, label, detail) {
    const msg = `${ok ? "OK " : "BAD"} ${label}${detail ? `: ${detail}` : ""}`;
    console.log(msg);
  }
  function header(title) {
    console.log(`\n[${title}]`);
  }

  // ── CODEX ECOSYSTEM ─────────────────────────────────────────────────────────
  header("CODEX ECOSYSTEM");
  const codexAppDir  = path.join(localAppData, "OpenAI", "Codex");
  const codexAppExe  = path.join(codexAppDir, "Codex.exe");
  const codexBinExe  = path.join(codexAppDir, "bin", "codex.exe");

  // Robust UWP (Microsoft Store) package detection
  const packagesDir = path.join(localAppData, "Packages");
  let uwpDirName = "";
  if (fs.existsSync(packagesDir)) {
    try {
      const dirs = fs.readdirSync(packagesDir);
      const found = dirs.find((d) => d.startsWith("OpenAI.Codex_"));
      if (found) uwpDirName = found;
    } catch (_) {}
  }

  const hasAppDir = fs.existsSync(codexAppDir);
  const hasAppExe = fs.existsSync(codexAppExe);
  const isUwpInstalled = uwpDirName !== "";

  row(hasAppDir || isUwpInstalled, "Codex Desktop App installed", isUwpInstalled ? `UWP package (${uwpDirName})` : codexAppDir);
  row(hasAppExe || isUwpInstalled, "Codex Desktop App execution", isUwpInstalled ? "UWP App Alias" : (hasAppExe ? codexAppExe : "not found"));
  row(fs.existsSync(codexBinExe) || codexPath === "codex", "codex.exe CLI binary", codexPath);

  const ver = await tryCommand(codexPath, ["--version"]);
  row(ver.ok, "codex --version", ver.output || "not found");

  const whoami = await tryCommand(codexPath, ["whoami"], 10000);
  row(whoami.ok, "Codex auth (whoami)", whoami.ok ? (whoami.output || "logged in") : "NOT logged in — run: codex login");

  const appSrv = await checkCodexAppServer(codexPath);
  row(appSrv.ok, "Codex App Server (stdio probe)", appSrv.detail);

  // ── CAM DAEMON ──────────────────────────────────────────────────────────────
  header("CAM DAEMON");
  row(fs.existsSync(p.root),       "CAM home dir",        p.root);
  row(fs.existsSync(p.localToken), "CAM API token",       p.localToken);
  row(fs.existsSync(p.registry),   "CAM agent registry",  p.registry);

  try {
    const health = await apiRequest("GET", "/health");
    row(true, "CAM config init", paths().config);
    if (!config.port) throw new Error("CAM port is not configured.");
    row(true, "CAM daemon running", `port ${config.port}, node=${health.nodeName}, started=${health.startedAt}`);
  } catch (error) {
    row(false, "CAM daemon running", `${error.message} — run: cam daemon start`);
  }
  // ── SKILLS ──────────────────────────────────────────────────────────────────
  header("QEXOW CAM SKILLS");
  const agySkillDir = path.join(os.homedir(), ".gemini", "antigravity", "skills", "qexow-cam-messaging");
  const codexSkillDir = path.join(os.homedir(), ".codex", "skills", "qexow-cam-messaging");
  const bossMdDest = path.join(p.root, "boss.md");
  
  row(fs.existsSync(agySkillDir), "Antigravity Messaging Skill", agySkillDir);
  row(fs.existsSync(codexSkillDir), "Codex Messaging Skill", codexSkillDir);
  row(fs.existsSync(bossMdDest), "Boss Agent Prompt", bossMdDest);

  // ── INSTALLATION ASSISTANCE ────────────────────────────────────────────────
  const missing = [];
  if (!hasAppDir && !isUwpInstalled) {
    missing.push({
      name: "Codex Desktop App",
      command: "Install Codex Desktop manually outside CAM."
    });
  }
  if (!fs.existsSync(codexBinExe) && codexPath !== "codex") {
    missing.push({
      name: "Codex CLI",
      command: "Install Codex CLI manually outside CAM."
    });
  }
  if (!ver.ok) {
    missing.push({
      name: "Codex CLI (runnable)",
      command: "Install or repair Codex CLI manually outside CAM."
    });
  }
  if (!whoami.ok) {
    missing.push({
      name: "Codex Authentication",
      command: "codex login"
    });
  }

  if (missing.length > 0) {
    header("INSTALLATION ASSISTANCE");
    console.log("Some components are missing or unconfigured. Install or authenticate them manually outside CAM:");
    for (const item of missing) {
      console.log(`\n* ${item.name}:`);
      console.log(`  ${item.command}`);
    }
  }
}

async function commandDaemon(args) {
  const action = args[0];
  if (action === "start") {
    const opts = parseOptions(args.slice(1));
    logEvent("cli.daemon.start.initiating");
    initConfig();
    if (opts.headless) process.env.CAM_HEADLESS = "1";
    const { runDaemon } = await import("./daemon.js");
    logEvent("cli.daemon.start.complete", { pid: process.pid, inProcess: true });
    console.log(`started daemon pid=${process.pid}${opts.headless ? " (headless)" : ""}`);
    return runDaemon();
  }
  if (action === "stop") {
    logEvent("cli.daemon.stop.initiating");
    await apiRequest("POST", "/shutdown", {});
    logEvent("cli.daemon.stop.complete");
    console.log("stopped daemon");
    return;
  }
  if (action === "status") {
    const health = await apiRequest("GET", "/health");
    console.log(JSON.stringify(health, null, 2));
    return;
  }
  throw new Error("expected daemon start|stop|status");
}


async function commandAgent(args) {
  const action = args[0];
  if (action === "create") {
    const opts = parseOptions(args.slice(1));
    const name = opts._[0];
    if (!name) throw new Error("agent name is required");
    const cwd = opts.cwd || process.cwd();
    const result = await apiRequest("POST", "/agents/create", {
      name,
      cwd,
      threadId: opts.threadId || null,
      model: opts.model || null,
      modelProvider: opts.modelProvider || null,
      effort: opts.effort ? normalizeEffort(opts.effort) : null,
      serviceTier: normalizeServiceTier(opts).value,
      threadSource: opts.source || opts.threadSource || opts["thread-source"] || "codex",
    });
    console.log(JSON.stringify(result.agent, null, 2));
    return;
  }
  if (action === "resume") {
    const name = args[1];
    if (!name) throw new Error("agent name is required");
    const result = await apiRequest("POST", "/agents/resume", { name });
    console.log(JSON.stringify(result.agent, null, 2));
    return;
  }
  if (action === "list") {
    const result = await apiRequest("GET", "/agents");
    for (const agent of result.agents) {
      const model = agent.model || "-";
      const provider = agent.modelProvider || "-";
      const effort = agent.effort || "-";
      const serviceTier = agent.serviceTier || "standard";
      console.log(`${agent.name}\t${agent.status}\t${agent.node}\t${agent.threadId || "-"}\t${model}\t${provider}\t${effort}\t${serviceTier}\t${agent.cwd}`);
    }
    return;
  }
  if (action === "status") {
    const name = args[1];
    const agent = listAgents(loadConfig()).find((item) => item.name === name);
    if (!agent) throw new Error(`unknown agent: ${name}`);
    console.log(JSON.stringify(agent, null, 2));
    return;
  }
  if (action === "set-model") {
    const opts = parseOptions(args.slice(1));
    const name = opts._[0];
    if (!name) throw new Error("agent name is required");
    if ("recreate" in opts) {
      throw new Error("--recreate is forbidden; model changes must preserve the existing chat/session/agent mapping");
    }
    const hasEffort = opts.effort !== undefined;
    const serviceTier = normalizeServiceTier(opts);
    if (!opts.model && !opts.modelProvider && !hasEffort && !serviceTier.provided) {
      throw new Error("usage: cam agent set-model <name> [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]");
    }
    const payload = { name };
    if (opts.model !== undefined) payload.model = opts.model || null;
    if (opts.modelProvider !== undefined) payload.modelProvider = opts.modelProvider || null;
    if (hasEffort) payload.effort = normalizeEffort(opts.effort);
    if (serviceTier.provided) payload.serviceTier = serviceTier.value;
    const result = await apiRequest("POST", "/agents/set-model", payload);
    console.log(JSON.stringify(result.agent, null, 2));
    return;
  }
  if (action === "read") {
    const opts = parseOptions(args.slice(1));
    const name = opts._[0];
    if (!name) throw new Error("agent name is required");
    let url = `/agents/read?name=${encodeURIComponent(name)}&includeTurns=true`;
    if (opts.turns) url += `&turns=${opts.turns}`;
    const result = await apiRequest("GET", url);
    const thread = result.thread?.thread || result.thread;
    if (opts.latest) {
      console.log(JSON.stringify(summarizeThread(thread), null, 2));
      return;
    }
    console.log(JSON.stringify(result.thread, null, 2));
    return;
  }
  throw new Error("expected agent create|resume|set-model|list|status|read");
}

function extractText(item) {
  if (!item) return "";
  if (typeof item.text === "string") return item.text;
  const content = item.content;
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((part) => {
        if (typeof part === "string") return part;
        return part?.text || part?.content || "";
      })
      .filter(Boolean)
      .join("\n");
  }
  return "";
}

function summarizeThread(thread) {
  const turns = Array.isArray(thread?.turns) ? thread.turns : [];
  let latestAgentMessage = "";
  let latestUserMessage = "";
  let latestTurnId = null;
  let latestAgentItemId = null;
  for (const turn of turns) {
    for (const item of Array.isArray(turn.items) ? turn.items : []) {
      if (item?.type === "userMessage") {
        const text = extractText(item).trim();
        if (text) latestUserMessage = text;
      }
      if (item?.type === "agentMessage") {
        const text = extractText(item).trim();
        if (text) {
          latestAgentMessage = text;
          latestTurnId = turn.id || latestTurnId;
          latestAgentItemId = item.id || latestAgentItemId;
        }
      }
    }
  }
  return {
    id: thread?.id || thread?.sessionId || null,
    name: thread?.name || null,
    status: thread?.status || null,
    path: thread?.path || null,
    cwd: thread?.cwd || null,
    latestTurnId,
    latestAgentItemId,
    latestUserMessage,
    latestAgentMessage,
  };
}

async function commandSend(args) {
  const opts = parseOptions(args);
  const targetAgent = opts._[0];
  const message = opts._.slice(1).join(" ");
  if (!targetAgent || !message) throw new Error("usage: cam send <agent-name> <message>");
  const payload = {
    targetAgent,
    message,
    sourceAgent: opts.from || "operator",
    sourceNode: opts.sourceNode || os.hostname(),
  };
  if (opts.correlationId) payload.correlationId = opts.correlationId;
  if (opts.messageType) payload.messageType = opts.messageType;
  if (opts.strict) payload.strict = true;
  try {
    const result = await apiRequest("POST", "/send", payload);
    console.log(JSON.stringify(result.message, null, 2));
    return;
  } catch (error) {
    throw error;
  }
}

async function commandInbox(args) {
  const opts = parseOptions(args);
  const agent = opts._[0];
  const wait = opts.wait;

  if (wait !== undefined) {
    let url = `/inbox?agent=${encodeURIComponent(agent || "")}`;
    if (wait && wait !== true) url += `&wait=${wait}`;
    try {
      const result = await apiRequest("GET", url);
      console.log(JSON.stringify(result.messages || result, null, 2));
      return;
    } catch (err) {
      // Fallback if daemon is not reachable
    }
  }

  const messages = agent ? readMailbox(agent) : readMailbox();
  console.log(JSON.stringify(messages, null, 2));
}

async function commandLogs() {
  const result = await apiRequest("GET", "/logs");
  for (const row of result.logs) console.log(JSON.stringify(row));
}

async function commandNode(args) {
  const action = args[0];
  if (action === "list") {
    const config = loadConfig();
    const registry = loadRegistry(config);
    console.log(JSON.stringify(registry.peers || {}, null, 2));
    return;
  }
  if (action === "enroll") {
    const opts = parseOptions(args.slice(1));
    const name = opts._[0];
    if (!name) throw new Error("node name is required");
    if (!opts.ssh) throw new Error("--ssh <user@host> is required");
    const peer = {
      name,
      transport: "ssh",
      ssh: opts.ssh,
      key: opts.key || null,
      remoteRoot: opts.remoteRoot || "auto",
      agents: [],
      enrolledAt: new Date().toISOString(),
    };
    const config = loadConfig();
    const registry = loadRegistry(config);
    registry.peers ||= {};
    registry.peers[name] = peer;
    saveRegistry(registry);
    console.log(JSON.stringify(peer, null, 2));
    return;
  }
  throw new Error("expected node enroll|list");
}

async function commandService(cmd, args) {
  logEvent("cli.service.action", { command: cmd, args });
  initConfig();
  const opts = parseOptions(args);
  const name = opts.name || "QexowCam";
  const headless = !!opts.headless;
  const serviceFile = path.join(paths().root, "service.json");
  writeJsonAtomic(serviceFile, {
    name,
    headless,
    enabled: cmd === "install-service",
    updatedAt: new Date().toISOString(),
  });
  console.log(`recorded ${cmd} in ${serviceFile}; start the daemon with 'cam daemon start'`);
}

export async function main(args) {
  const [cmd, ...rest] = args;
  if (!cmd || cmd === "--help" || cmd === "-h") {
    console.log(usage());
    return;
  }
  if (cmd === "init") {
    const config = initConfig();
    console.log(JSON.stringify({ config, paths: allPaths() }, null, 2));
    return;
  }
  if (cmd === "install-skills") {
    const { bootstrapAntigravity } = await import("./antigravity.js");
    bootstrapAntigravity((type, payload) => console.log(`[${type}] ${payload.message}`));
    return;
  }
  if (cmd === "doctor") return commandDoctor();
  if (cmd === "daemon") return commandDaemon(rest);
  if (cmd === "daemon-run") {
    const { runDaemon } = await import("./daemon.js");
    return runDaemon();
  }
  if (cmd === "tray") {
    const { runTray } = await import("./tray.js");
    return runTray();
  }
  if (cmd === "agent") return commandAgent(rest);
  if (cmd === "send") return commandSend(rest);
  if (cmd === "inbox") return commandInbox(rest);
  if (cmd === "logs") return commandLogs();
  if (cmd === "node") return commandNode(rest);
  if (cmd === "install-service" || cmd === "uninstall-service") return commandService(cmd, rest);
  throw new Error(`unknown command: ${cmd}\n${usage()}`);
}
