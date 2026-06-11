#!/usr/bin/env node
var __create = Object.create;
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __getProtoOf = Object.getPrototypeOf;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __copyProps = (to, from, except, desc) => {
  if (from && typeof from === "object" || typeof from === "function") {
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
  }
  return to;
};
var __toESM = (mod, isNodeMode, target) => (target = mod != null ? __create(__getProtoOf(mod)) : {}, __copyProps(
  // If the importer is in node compatibility mode or this is not an ESM
  // file that has been converted to a CommonJS file using a Babel-
  // compatible transform (i.e. "__esModule" has not been set), then set
  // "default" to the CommonJS "module.exports" for node compatibility.
  isNodeMode || !mod || !mod.__esModule ? __defProp(target, "default", { value: mod, enumerable: true }) : target,
  mod
));

// src/cli.js
var import_node_child_process = require("node:child_process");
var import_node_child_process2 = require("node:child_process");
var import_node_fs4 = __toESM(require("node:fs"), 1);
var import_node_net = __toESM(require("node:net"), 1);
var import_node_os4 = __toESM(require("node:os"), 1);
var import_node_path3 = __toESM(require("node:path"), 1);

// src/api.js
var import_node_http = __toESM(require("node:http"), 1);
var import_node_fs3 = __toESM(require("node:fs"), 1);

// src/config.js
var import_node_crypto = __toESM(require("node:crypto"), 1);
var import_node_fs2 = __toESM(require("node:fs"), 1);
var import_node_os2 = __toESM(require("node:os"), 1);
var import_node_path2 = __toESM(require("node:path"), 1);

// src/paths.js
var import_node_fs = __toESM(require("node:fs"), 1);
var import_node_os = __toESM(require("node:os"), 1);
var import_node_path = __toESM(require("node:path"), 1);
function projectRoot() {
  return import_node_path.default.resolve(import_node_path.default.dirname(process.execPath), "..");
}
function homeDir() {
  return process.env.CAM_HOME || import_node_path.default.join(import_node_os.default.homedir(), ".codex-agent-manager");
}
function paths() {
  const root = homeDir();
  return {
    root,
    config: import_node_path.default.join(root, "config.json"),
    registry: import_node_path.default.join(root, "agents.json"),
    mailbox: import_node_path.default.join(root, "mailbox.jsonl"),
    events: import_node_path.default.join(root, "events.jsonl"),
    daemon: import_node_path.default.join(root, "daemon.json"),
    pid: import_node_path.default.join(root, "daemon.pid"),
    tunnels: import_node_path.default.join(root, "tunnels.json"),
    secretsDir: import_node_path.default.join(root, "secrets"),
    localToken: import_node_path.default.join(root, "secrets", "local-api-token"),
    logsDir: import_node_path.default.join(root, "logs"),
    daemonLog: import_node_path.default.join(root, "logs", "daemon.log")
  };
}
function ensureDirs() {
  const p = paths();
  for (const dir of [p.root, p.secretsDir, p.logsDir]) {
    import_node_fs.default.mkdirSync(dir, { recursive: true });
  }
  return p;
}
function readJson(file, fallback) {
  try {
    return JSON.parse(import_node_fs.default.readFileSync(file, "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") return fallback;
    throw error;
  }
}
function writeJsonAtomic(file, value) {
  import_node_fs.default.mkdirSync(import_node_path.default.dirname(file), { recursive: true });
  const tmp = `${file}.${process.pid}.${Date.now()}.tmp`;
  import_node_fs.default.writeFileSync(tmp, `${JSON.stringify(value, null, 2)}
`, "utf8");
  import_node_fs.default.renameSync(tmp, file);
}
function readJsonl(file) {
  try {
    return import_node_fs.default.readFileSync(file, "utf8").split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
}

// src/config.js
var DEFAULT_PORT = 37631;
function defaultCodexPath() {
  if (process.env.CAM_CODEX_EXE) return process.env.CAM_CODEX_EXE;
  if (process.platform === "win32") {
    const candidate = import_node_path2.default.join(import_node_os2.default.homedir(), "AppData", "Local", "OpenAI", "Codex", "bin", "codex.exe");
    if (import_node_fs2.default.existsSync(candidate)) return candidate;
  }
  return "codex";
}
function defaultNodeName() {
  return process.env.CAM_NODE_NAME || import_node_os2.default.hostname();
}
function initConfig({ force = false } = {}) {
  const p = ensureDirs();
  const existing = readJson(p.config, null);
  if (existing && !force) return existing;
  const config = {
    version: 1,
    nodeName: defaultNodeName(),
    bindHost: "127.0.0.1",
    port: Number(process.env.CAM_PORT || DEFAULT_PORT),
    codexPath: defaultCodexPath(),
    codexHome: process.env.CODEX_HOME || import_node_path2.default.join(import_node_os2.default.homedir(), ".codex"),
    createdAt: (/* @__PURE__ */ new Date()).toISOString()
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
      updatedAt: (/* @__PURE__ */ new Date()).toISOString()
    });
  }
  return config;
}
function loadConfig() {
  const p = ensureDirs();
  const config = readJson(p.config, null) || initConfig();
  ensureLocalToken();
  return config;
}
function ensureLocalToken() {
  const p = ensureDirs();
  if (!import_node_fs2.default.existsSync(p.localToken)) {
    import_node_fs2.default.writeFileSync(p.localToken, import_node_crypto.default.randomBytes(32).toString("base64url"), { mode: 384 });
  }
  return import_node_fs2.default.readFileSync(p.localToken, "utf8").trim();
}
function localApiBase(config = loadConfig()) {
  return `http://${config.bindHost || "127.0.0.1"}:${config.port || DEFAULT_PORT}`;
}
function allPaths() {
  return paths();
}

// src/api.js
function readLocalToken() {
  return import_node_fs3.default.readFileSync(paths().localToken, "utf8").trim();
}
async function apiRequest(method, path4, body = void 0) {
  const config = loadConfig();
  const base = new URL(localApiBase(config));
  const payload = body === void 0 ? null : JSON.stringify(body);
  const options = {
    hostname: base.hostname,
    port: base.port,
    method,
    path: path4,
    headers: {
      authorization: `Bearer ${readLocalToken()}`
    }
  };
  if (payload) {
    options.headers["content-type"] = "application/json";
    options.headers["content-length"] = Buffer.byteLength(payload);
  }
  return new Promise((resolve, reject) => {
    const req = import_node_http.default.request(options, (res) => {
      let data = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => {
        data += chunk;
      });
      res.on("end", () => {
        let parsed = data;
        try {
          parsed = data ? JSON.parse(data) : null;
        } catch {
        }
        if (res.statusCode >= 400) {
          const error = new Error(parsed?.error || `HTTP ${res.statusCode}`);
          error.response = parsed;
          reject(error);
        } else {
          resolve(parsed);
        }
      });
    });
    req.on("error", reject);
    if (payload) req.write(payload);
    req.end();
  });
}

// src/registry.js
var import_node_os3 = __toESM(require("node:os"), 1);
function loadRegistry(config) {
  const p = paths();
  const registry = readJson(p.registry, {
    version: 1,
    nodeName: config?.nodeName || import_node_os3.default.hostname(),
    agents: {},
    peers: {},
    updatedAt: (/* @__PURE__ */ new Date()).toISOString()
  });
  registry.agents ||= {};
  registry.peers ||= {};
  return registry;
}
function listAgents(config) {
  return Object.values(loadRegistry(config).agents).sort((a, b) => a.name.localeCompare(b.name));
}
function readMailbox(agentName = null) {
  const rows = readJsonl(paths().mailbox);
  return agentName ? rows.filter((row) => row.targetAgent === agentName) : rows;
}

// src/cli.js
function usage() {
  return `Usage:
  cam init
  cam doctor
  cam daemon start|stop|status
  cam node enroll <name> --ssh <user@host> --key <path> --remote-root <path>
  cam node list
  cam agent create <name> --cwd <path> [--thread-id <id>] [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]
  cam agent resume <name>
  cam agent set-model <name> [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]
  cam agent list
  cam agent status <name>
  cam agent read <name> [--latest]
  cam send <agent-name> <message> [--from <agent-name>]
  cam tunnel command <node> [--local-port <port>] [--remote-port <port>]
  cam tunnel open <node> [--local-port <port>] [--remote-port <port>] [--background]
  cam tunnel status <port>
  cam tunnel stop <pid>
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
  if (value === void 0 || value === null) return null;
  const text = String(value).trim().toLowerCase().replace(/[\s_]+/g, "-");
  const aliases = /* @__PURE__ */ new Map([
    ["minimal", "minimal"],
    ["low", "low"],
    ["medium", "medium"],
    ["high", "high"],
    ["xhigh", "xhigh"],
    ["x-high", "xhigh"],
    ["extra-high", "xhigh"]
  ]);
  const normalized = aliases.get(text);
  if (!normalized) throw new Error(`invalid effort '${value}'; expected minimal|low|medium|high|xhigh`);
  return normalized;
}
function normalizeServiceTier(opts) {
  if (opts.speed && opts.serviceTier) throw new Error("use either --speed or --service-tier, not both");
  if (opts.speed === void 0 && opts.serviceTier === void 0) return { provided: false, value: null };
  if (opts.speed !== void 0) {
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
function tryCommand(cmd, args, timeoutMs = 8e3) {
  return new Promise((resolve) => {
    let child;
    try {
      child = (0, import_node_child_process.spawn)(cmd, args, { windowsHide: true, shell: false });
    } catch (e) {
      return resolve({ ok: false, output: e.message });
    }
    let out = "";
    let err = "";
    const timer = setTimeout(() => {
      try {
        child.kill();
      } catch (_) {
      }
      resolve({ ok: false, output: "timed out" });
    }, timeoutMs);
    child.stdout?.on("data", (d) => {
      out += d;
    });
    child.stderr?.on("data", (d) => {
      err += d;
    });
    child.on("error", (e) => {
      clearTimeout(timer);
      resolve({ ok: false, output: e.message });
    });
    child.on("exit", (code) => {
      clearTimeout(timer);
      resolve({ ok: code === 0, output: (out || err).trim() });
    });
  });
}
function checkCodexAppServer(codexPath) {
  return new Promise((resolve) => {
    let child;
    try {
      child = (0, import_node_child_process.spawn)(codexPath, ["app-server", "--listen", "stdio://"], {
        stdio: ["pipe", "pipe", "pipe"],
        windowsHide: true
      });
    } catch (e) {
      return resolve({ ok: false, detail: e.message });
    }
    let buffer = "";
    const timer = setTimeout(() => {
      try {
        child.kill();
      } catch (_) {
      }
      resolve({ ok: false, detail: "timed out waiting for app-server response" });
    }, 12e3);
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      buffer += chunk;
      const idx = buffer.indexOf("\n");
      if (idx < 0) return;
      clearTimeout(timer);
      try {
        child.kill();
      } catch (_) {
      }
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
    child.on("error", (e) => {
      clearTimeout(timer);
      resolve({ ok: false, detail: e.message });
    });
    child.on("exit", (code) => {
      clearTimeout(timer);
      if (code !== null) resolve({ ok: false, detail: `exited with code ${code} before responding` });
    });
    try {
      child.stdin.write(JSON.stringify({
        id: 1,
        method: "initialize",
        params: { clientInfo: { name: "cam-doctor", version: "0.1.0" }, capabilities: { experimentalApi: true } }
      }) + "\n");
    } catch (e) {
      clearTimeout(timer);
      try {
        child.kill();
      } catch (_) {
      }
      resolve({ ok: false, detail: `stdin write failed: ${e.message}` });
    }
  });
}
async function commandDoctor() {
  const config = loadConfig();
  const p = allPaths();
  const codexPath = config.codexPath || defaultCodexPath();
  const localAppData = process.env.LOCALAPPDATA || import_node_path3.default.join(import_node_os4.default.homedir(), "AppData", "Local");
  function row(ok, label, detail) {
    const msg = `${ok ? "OK " : "BAD"} ${label}${detail ? `: ${detail}` : ""}`;
    console.log(msg);
  }
  function header(title) {
    console.log(`
[${title}]`);
  }
  header("CODEX ECOSYSTEM");
  const codexAppDir = import_node_path3.default.join(localAppData, "OpenAI", "Codex");
  const codexAppExe = import_node_path3.default.join(codexAppDir, "Codex.exe");
  const codexBinExe = import_node_path3.default.join(codexAppDir, "bin", "codex.exe");
  row(import_node_fs4.default.existsSync(codexAppDir), "Codex Desktop App installed", codexAppDir);
  row(import_node_fs4.default.existsSync(codexAppExe), "Codex Desktop App exe", codexAppExe);
  row(
    import_node_fs4.default.existsSync(codexBinExe) || codexPath === "codex",
    "codex.exe CLI binary",
    codexPath
  );
  const ver = await tryCommand(codexPath, ["--version"]);
  row(ver.ok, "codex --version", ver.output || "not found");
  const whoami = await tryCommand(codexPath, ["whoami"], 1e4);
  row(whoami.ok, "Codex auth (whoami)", whoami.ok ? whoami.output || "logged in" : "NOT logged in \u2014 run: codex login");
  const appSrv = await checkCodexAppServer(codexPath);
  row(appSrv.ok, "Codex App Server (stdio probe)", appSrv.detail);
  header("CAM DAEMON");
  row(import_node_fs4.default.existsSync(p.root), "CAM home dir", p.root);
  row(import_node_fs4.default.existsSync(p.localToken), "CAM API token", p.localToken);
  row(import_node_fs4.default.existsSync(p.registry), "CAM agent registry", p.registry);
  try {
    const health = await apiRequest("GET", "/health");
    row(true, "CAM daemon running", `port ${config.port || DEFAULT_PORT}, node=${health.nodeName}, started=${health.startedAt}`);
  } catch (error) {
    row(false, "CAM daemon running", `${error.message} \u2014 run: cam daemon start`);
  }
  header("ANTIGRAVITY");
  const agyAppDir = import_node_path3.default.join(localAppData, "Programs", "Antigravity");
  const agyAppExe = import_node_path3.default.join(agyAppDir, "Antigravity.exe");
  const agyLangSrv = import_node_path3.default.join(agyAppDir, "resources", "bin", "language_server.exe");
  row(import_node_fs4.default.existsSync(agyAppDir), "Antigravity Desktop App installed", agyAppDir);
  row(import_node_fs4.default.existsSync(agyAppExe), "Antigravity Desktop App exe", agyAppExe);
  row(import_node_fs4.default.existsSync(agyLangSrv), "Antigravity Language Server (agy)", agyLangSrv);
  const agyVer = await tryCommand("agy", ["--version"], 5e3);
  row(agyVer.ok, "agy CLI in PATH", agyVer.ok ? agyVer.output : "NOT found \u2014 install Antigravity Desktop App");
  const agyStatus = await tryCommand("agy", ["status"], 8e3);
  const agyLoggedIn = agyStatus.ok && !agyStatus.output.toLowerCase().includes("unauthenticated") && !agyStatus.output.toLowerCase().includes("login required") && !agyStatus.output.toLowerCase().includes("not logged");
  row(
    agyLoggedIn,
    "Antigravity auth (agy status)",
    agyStatus.ok ? agyLoggedIn ? agyStatus.output.split("\n")[0] : "NOT logged in \u2014 run: agy login" : "agy CLI not available"
  );
}
async function commandDaemon(args) {
  const action = args[0];
  if (action === "start") {
    initConfig();
    const node = process.env.CAM_NODE_EXE || process.execPath;
    const daemonScript = import_node_path3.default.resolve(import_node_path3.default.dirname(process.execPath), "daemon-entry.js");
    const out = import_node_fs4.default.openSync(paths().daemonLog, "a");
    const child = (0, import_node_child_process.spawn)(node, [daemonScript], {
      detached: true,
      stdio: ["ignore", out, out],
      windowsHide: true,
      env: process.env
    });
    child.unref();
    console.log(`started daemon pid=${child.pid}`);
    return;
  }
  if (action === "stop") {
    await apiRequest("POST", "/shutdown", {});
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
async function commandTunnel(args) {
  const action = args[0];
  if (action === "command" || action === "open") {
    const opts = parseOptions(args.slice(1));
    const peerName = opts._[0];
    if (!peerName) throw new Error(`usage: cam tunnel ${action} <node> [--local-port <port>] [--remote-port <port>]`);
    const peer = readJson(paths().registry, { peers: {} }).peers?.[peerName];
    if (!peer) throw new Error(`unknown peer node: ${peerName}`);
    if (peer.transport !== "ssh") throw new Error(`peer ${peerName} is not an SSH peer`);
    const localPort = Number(opts.localPort || nextTunnelPort());
    const remotePort = Number(opts.remotePort || DEFAULT_PORT);
    const ssh = tunnelSshArgs(peer, localPort, remotePort);
    if (action === "command") {
      console.log(renderCommand("ssh", ssh));
      return;
    }
    if (opts.background) {
      const out = import_node_fs4.default.openSync(paths().daemonLog, "a");
      const child = (0, import_node_child_process.spawn)("ssh", ssh, {
        detached: true,
        stdio: ["ignore", out, out],
        windowsHide: true
      });
      child.unref();
      recordTunnel({ pid: child.pid, peer: peerName, localPort, remotePort, startedAt: (/* @__PURE__ */ new Date()).toISOString() });
      console.log(`started tunnel pid=${child.pid} 127.0.0.1:${localPort} -> ${peerName}:127.0.0.1:${remotePort}`);
      return;
    }
    console.log(`opening tunnel 127.0.0.1:${localPort} -> ${peerName}:127.0.0.1:${remotePort}; press Ctrl+C to close`);
    await new Promise((resolve, reject) => {
      const child = (0, import_node_child_process.spawn)("ssh", ssh, { stdio: "inherit", windowsHide: true });
      child.on("error", reject);
      child.on("exit", (code) => {
        if (code === 0) resolve();
        else reject(new Error(`ssh tunnel exited with ${code}`));
      });
    });
    return;
  }
  if (action === "status") {
    const port = Number(args[1]);
    if (!port) throw new Error("usage: cam tunnel status <port>");
    const ok = await portOpen("127.0.0.1", port, 2e3);
    console.log(`${ok ? "OK " : "BAD"} 127.0.0.1:${port}`);
    return;
  }
  if (action === "stop") {
    const pid = Number(args[1]);
    if (!pid) throw new Error("usage: cam tunnel stop <pid>");
    stopPid(pid);
    console.log(`stopped tunnel pid=${pid}`);
    return;
  }
  throw new Error("expected tunnel command|open|status|stop");
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
      serviceTier: normalizeServiceTier(opts).value
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
      console.log(`${agent.name}	${agent.status}	${agent.node}	${agent.threadId || "-"}	${model}	${provider}	${effort}	${serviceTier}	${agent.cwd}`);
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
    const hasEffort = opts.effort !== void 0;
    const serviceTier = normalizeServiceTier(opts);
    if (!opts.model && !opts.modelProvider && !hasEffort && !serviceTier.provided) {
      throw new Error("usage: cam agent set-model <name> [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]");
    }
    const payload = { name };
    if (opts.model !== void 0) payload.model = opts.model || null;
    if (opts.modelProvider !== void 0) payload.modelProvider = opts.modelProvider || null;
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
    const result = await apiRequest("GET", `/agents/read?name=${encodeURIComponent(name)}&includeTurns=true`);
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
    return content.map((part) => {
      if (typeof part === "string") return part;
      return part?.text || part?.content || "";
    }).filter(Boolean).join("\n");
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
    latestAgentMessage
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
    sourceNode: opts.sourceNode || import_node_os4.default.hostname()
  };
  try {
    const result = await apiRequest("POST", "/send", payload);
    console.log(JSON.stringify(result.message, null, 2));
    return;
  } catch (error) {
    if (!/unknown agent/.test(error.message)) throw error;
    const routed = sendViaPeer(payload);
    if (routed) {
      console.log(routed.trim());
      return;
    }
    throw error;
  }
}
async function commandInbox(args) {
  const agent = args[0];
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
    const registry = JSON.parse(import_node_fs4.default.readFileSync(paths().registry, "utf8"));
    console.log(JSON.stringify(registry.peers || {}, null, 2));
    return;
  }
  if (action === "enroll") {
    const opts = parseOptions(args.slice(1));
    const name = opts._[0];
    if (!name) throw new Error("node name is required");
    if (!opts.ssh) throw new Error("--ssh <user@host> is required");
    if (!opts.remoteRoot) throw new Error("--remote-root <path> is required");
    const peer = {
      name,
      transport: "ssh",
      ssh: opts.ssh,
      key: opts.key || null,
      remoteRoot: opts.remoteRoot,
      agents: [],
      enrolledAt: (/* @__PURE__ */ new Date()).toISOString()
    };
    const listed = listPeerAgents(peer);
    peer.agents = listed.agents;
    peer.lastCheckedAt = (/* @__PURE__ */ new Date()).toISOString();
    const registry = readJson(paths().registry, { version: 1, nodeName: loadConfig().nodeName, agents: {}, peers: {} });
    registry.peers ||= {};
    registry.peers[name] = peer;
    writeJsonAtomic(paths().registry, registry);
    console.log(JSON.stringify(peer, null, 2));
    return;
  }
  throw new Error("expected node enroll|list");
}
function shellQuote(value) {
  return `'${String(value).replace(/'/g, `'\\''`)}'`;
}
function sshArgs(peer, command) {
  const args = ["-o", "StrictHostKeyChecking=no"];
  if (peer.key) args.push("-i", peer.key);
  args.push(peer.ssh, command);
  return args;
}
function tunnelSshArgs(peer, localPort, remotePort) {
  const args = ["-o", "StrictHostKeyChecking=no", "-N", "-L", `127.0.0.1:${localPort}:127.0.0.1:${remotePort}`];
  if (peer.key) args.push("-i", peer.key);
  args.push(peer.ssh);
  return args;
}
function renderCommand(command, args) {
  return [command, ...args.map(commandQuote)].join(" ");
}
function commandQuote(value) {
  const text = String(value);
  return /[\s"]/u.test(text) ? `"${text.replace(/"/g, '\\"')}"` : text;
}
function nextTunnelPort() {
  const registry = readJson(paths().registry, { peers: {} });
  const count = Object.keys(registry.peers || {}).length;
  return DEFAULT_PORT + count + 1;
}
function recordTunnel(tunnel) {
  const p = paths();
  const state = readJson(p.tunnels, { tunnels: [] });
  state.tunnels ||= [];
  state.tunnels.push(tunnel);
  writeJsonAtomic(p.tunnels, state);
}
function portOpen(host, port, timeoutMs) {
  return new Promise((resolve) => {
    const socket = import_node_net.default.createConnection({ host, port });
    const done = (ok) => {
      socket.removeAllListeners();
      socket.destroy();
      resolve(ok);
    };
    socket.setTimeout(timeoutMs);
    socket.once("connect", () => done(true));
    socket.once("timeout", () => done(false));
    socket.once("error", () => done(false));
  });
}
function stopPid(pid) {
  if (process.platform === "win32") {
    const result = (0, import_node_child_process2.spawnSync)("taskkill.exe", ["/PID", String(pid), "/F"], { encoding: "utf8" });
    if (result.status !== 0) throw new Error((result.stderr || result.stdout).trim());
    return;
  }
  try {
    process.kill(pid, "SIGTERM");
  } catch (error) {
    throw new Error(`could not stop pid ${pid}: ${error.message}`);
  }
}
function listPeerAgents(peer) {
  const command = `node ${shellQuote(`${peer.remoteRoot}/bin/cam.js`)} agent list`;
  const result = (0, import_node_child_process2.spawnSync)("ssh", sshArgs(peer, command), { encoding: "utf8", timeout: 3e4 });
  if (result.status !== 0) {
    return { ok: false, agents: [], error: (result.stderr || result.stdout).trim() };
  }
  const agents = result.stdout.split(/\r?\n/).filter(Boolean).map((line) => line.split(/\t/)[0]).filter(Boolean);
  return { ok: true, agents };
}
function sendViaPeer(payload) {
  const registry = readJson(paths().registry, { peers: {} });
  for (const peer of Object.values(registry.peers || {})) {
    if (!peer.agents?.includes(payload.targetAgent)) continue;
    const command = [
      "node",
      shellQuote(`${peer.remoteRoot}/bin/cam.js`),
      "send",
      shellQuote(payload.targetAgent),
      shellQuote(payload.message),
      "--from",
      shellQuote(payload.sourceAgent),
      "--source-node",
      shellQuote(payload.sourceNode)
    ].join(" ");
    const result = (0, import_node_child_process2.spawnSync)("ssh", sshArgs(peer, command), { encoding: "utf8", timeout: 12e4 });
    if (result.status !== 0) {
      throw new Error(`peer send failed via ${peer.name}: ${(result.stderr || result.stdout).trim()}`);
    }
    return result.stdout;
  }
  return null;
}
async function commandService(cmd, args) {
  initConfig();
  const opts = parseOptions(args);
  const name = opts.name || "CodexAgentManager";
  if (process.platform === "win32") {
    return cmd === "install-service" ? installWindowsTask(name) : uninstallWindowsTask(name);
  }
  return cmd === "install-service" ? installSystemdUserService(name) : uninstallSystemdUserService(name);
}
function daemonScriptPath() {
  return import_node_path3.default.resolve(import_node_path3.default.dirname(process.execPath), "daemon-entry.js");
}
function daemonNodePath() {
  return process.env.CAM_NODE_EXE || process.execPath;
}
function installWindowsTask(name) {
  const taskCommand = `"${daemonNodePath()}" "${daemonScriptPath()}"`;
  const create = (0, import_node_child_process2.spawnSync)("schtasks.exe", [
    "/Create",
    "/F",
    "/TN",
    name,
    "/SC",
    "ONLOGON",
    "/RL",
    "LIMITED",
    "/TR",
    taskCommand
  ], { encoding: "utf8" });
  if (create.status !== 0) {
    installWindowsStartupFallback(name);
    console.log(`installed Startup folder fallback because scheduled task creation failed: ${(create.stderr || create.stdout).trim()}`);
    return;
  }
  console.log((create.stdout || "").trim());
  console.log(`installed Windows logon task ${name}; use 'cam daemon start' for the current session`);
}
function uninstallWindowsTask(name) {
  const result = (0, import_node_child_process2.spawnSync)("schtasks.exe", ["/Delete", "/TN", name, "/F"], { encoding: "utf8" });
  uninstallWindowsStartupFallback(name);
  if (result.status === 0) console.log((result.stdout || "").trim());
  else console.log(`scheduled task was not removed or did not exist: ${(result.stderr || result.stdout).trim()}`);
}
function installWindowsStartupFallback(name) {
  const startupDir = import_node_path3.default.join(process.env.APPDATA || import_node_path3.default.join(import_node_os4.default.homedir(), "AppData", "Roaming"), "Microsoft", "Windows", "Start Menu", "Programs", "Startup");
  import_node_fs4.default.mkdirSync(startupDir, { recursive: true });
  const file = import_node_path3.default.join(startupDir, `${name}.cmd`);
  const lines = [
    "@echo off",
    `set CAM_HOME=${paths().root}`,
    `set CODEX_HOME=${loadConfig().codexHome}`,
    `start "" /min "${daemonNodePath()}" "${daemonScriptPath()}"`,
    ""
  ];
  import_node_fs4.default.writeFileSync(file, lines.join("\r\n"), "utf8");
}
function uninstallWindowsStartupFallback(name) {
  const startupDir = import_node_path3.default.join(process.env.APPDATA || import_node_path3.default.join(import_node_os4.default.homedir(), "AppData", "Roaming"), "Microsoft", "Windows", "Start Menu", "Programs", "Startup");
  const file = import_node_path3.default.join(startupDir, `${name}.cmd`);
  if (import_node_fs4.default.existsSync(file)) import_node_fs4.default.rmSync(file);
}
function installSystemdUserService(name) {
  const unitName = systemdUnitName(name);
  const unitDir = import_node_path3.default.join(import_node_os4.default.homedir(), ".config", "systemd", "user");
  import_node_fs4.default.mkdirSync(unitDir, { recursive: true });
  const unitPath = import_node_path3.default.join(unitDir, unitName);
  const env = [
    `CAM_HOME=${paths().root}`,
    `CODEX_HOME=${loadConfig().codexHome}`
  ];
  const unit = [
    "[Unit]",
    "Description=Codex Agent Manager",
    "",
    "[Service]",
    "Type=simple",
    `WorkingDirectory=${projectRoot()}`,
    ...env.map((item) => `Environment=${systemdEscape(item)}`),
    `ExecStart=${systemdEscape(daemonNodePath())} ${systemdEscape(daemonScriptPath())}`,
    "Restart=always",
    "RestartSec=5",
    "",
    "[Install]",
    "WantedBy=default.target",
    ""
  ].join("\n");
  import_node_fs4.default.writeFileSync(unitPath, unit, "utf8");
  const reload = (0, import_node_child_process2.spawnSync)("systemctl", ["--user", "daemon-reload"], { encoding: "utf8" });
  const enable = reload.status === 0 ? (0, import_node_child_process2.spawnSync)("systemctl", ["--user", "enable", unitName], { encoding: "utf8" }) : reload;
  if (enable.status === 0) {
    console.log(`installed ${unitPath}; use 'cam daemon start' for the current session`);
    return;
  }
  installCronFallback(name);
  console.log(`installed cron @reboot fallback because systemd user service was unavailable: ${(enable.stderr || enable.stdout).trim()}`);
}
function uninstallSystemdUserService(name) {
  const unitName = systemdUnitName(name);
  (0, import_node_child_process2.spawnSync)("systemctl", ["--user", "disable", "--now", unitName], { encoding: "utf8" });
  const unitPath = import_node_path3.default.join(import_node_os4.default.homedir(), ".config", "systemd", "user", unitName);
  if (import_node_fs4.default.existsSync(unitPath)) import_node_fs4.default.rmSync(unitPath);
  (0, import_node_child_process2.spawnSync)("systemctl", ["--user", "daemon-reload"], { encoding: "utf8" });
  uninstallCronFallback(name);
  console.log(`removed ${unitPath}`);
}
function installCronFallback(name) {
  const marker = `# ${name}`;
  const scriptPath = import_node_path3.default.join(paths().root, `${name}.start.sh`);
  const script = [
    "#!/usr/bin/env sh",
    `export CAM_HOME=${shellWord(paths().root)}`,
    `export CODEX_HOME=${shellWord(loadConfig().codexHome)}`,
    `cd ${shellWord(projectRoot())} || exit 1`,
    `${shellWord(daemonNodePath())} ${shellWord(import_node_path3.default.join(projectRoot(), "bin", "cam.js"))} daemon start >/dev/null 2>&1`,
    ""
  ].join("\n");
  import_node_fs4.default.writeFileSync(scriptPath, script, { mode: 448 });
  const existing = (0, import_node_child_process2.spawnSync)("crontab", ["-l"], { encoding: "utf8" });
  const lines = existing.status === 0 ? existing.stdout.split(/\r?\n/).filter(Boolean) : [];
  const filtered = lines.filter((line) => !line.includes(marker));
  filtered.push(`@reboot ${shellWord(scriptPath)} ${marker}`);
  const update = (0, import_node_child_process2.spawnSync)("crontab", ["-"], { input: `${filtered.join("\n")}
`, encoding: "utf8" });
  if (update.status !== 0) throw new Error((update.stderr || update.stdout).trim());
}
function uninstallCronFallback(name) {
  const marker = `# ${name}`;
  const existing = (0, import_node_child_process2.spawnSync)("crontab", ["-l"], { encoding: "utf8" });
  if (existing.status !== 0) return;
  const filtered = existing.stdout.split(/\r?\n/).filter((line) => line && !line.includes(marker));
  (0, import_node_child_process2.spawnSync)("crontab", ["-"], { input: `${filtered.join("\n")}
`, encoding: "utf8" });
}
function shellWord(value) {
  return `'${String(value).replace(/'/g, `'\\''`)}'`;
}
function systemdUnitName(name) {
  return `${String(name).replace(/[^A-Za-z0-9_.@-]/g, "-")}.service`;
}
function systemdEscape(value) {
  const text = String(value);
  return /[\s"]/u.test(text) ? `"${text.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"` : text;
}
async function main(args) {
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
  if (cmd === "doctor") return commandDoctor();
  if (cmd === "daemon") return commandDaemon(rest);
  if (cmd === "agent") return commandAgent(rest);
  if (cmd === "send") return commandSend(rest);
  if (cmd === "tunnel") return commandTunnel(rest);
  if (cmd === "inbox") return commandInbox(rest);
  if (cmd === "logs") return commandLogs();
  if (cmd === "node") return commandNode(rest);
  if (cmd === "install-service" || cmd === "uninstall-service") return commandService(cmd, rest);
  throw new Error(`unknown command: ${cmd}
${usage()}`);
}

// bin/cam.js
main(process.argv.slice(2)).catch((error) => {
  console.error(error?.stack || error?.message || String(error));
  process.exitCode = 1;
});
