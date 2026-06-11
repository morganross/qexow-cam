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

// src/daemon.js
var import_node_crypto2 = __toESM(require("node:crypto"), 1);
var import_node_fs4 = __toESM(require("node:fs"), 1);
var import_node_http = __toESM(require("node:http"), 1);

// src/app-server.js
var import_node_child_process = require("node:child_process");
var import_node_events = require("node:events");
var AppServerClient = class extends import_node_events.EventEmitter {
  constructor({ codexPath, log = () => {
  } }) {
    super();
    this.codexPath = codexPath;
    this.log = log;
    this.child = null;
    this.nextId = 1;
    this.pending = /* @__PURE__ */ new Map();
    this.buffer = "";
    this.initialized = false;
  }
  async start() {
    if (this.child) return;
    this.child = (0, import_node_child_process.spawn)(this.codexPath, ["app-server", "--listen", "stdio://"], {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true
    });
    this.child.stdout.setEncoding("utf8");
    this.child.stdout.on("data", (chunk) => this.#onStdout(chunk));
    this.child.stderr.setEncoding("utf8");
    this.child.stderr.on("data", (chunk) => this.log("app-server.stderr", chunk.trim()));
    this.child.on("exit", (code, signal) => {
      this.log("app-server.exit", { code, signal });
      this.child = null;
      for (const [id, pending] of this.pending) {
        pending.reject(new Error(`app-server exited before response ${id}`));
      }
      this.pending.clear();
      this.initialized = false;
    });
    const result = await this.request("initialize", {
      clientInfo: { name: "codex-agent-manager", version: "0.1.0" },
      capabilities: { experimentalApi: true }
    });
    this.notify("initialized");
    this.initialized = true;
    this.log("app-server.initialized", result);
    return result;
  }
  stop() {
    if (this.child) {
      this.child.kill();
      this.child = null;
    }
  }
  request(method, params = void 0, timeoutMs = 3e4) {
    if (!this.child && method !== "initialize") {
      return Promise.reject(new Error("app-server is not running"));
    }
    const id = this.nextId++;
    const message = { id, method };
    if (params !== void 0) message.params = params;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`app-server request timed out: ${method}`));
      }, timeoutMs);
      this.pending.set(id, {
        method,
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        }
      });
      this.#write(message);
    });
  }
  notify(method, params = void 0) {
    const message = { method };
    if (params !== void 0) message.params = params;
    this.#write(message);
  }
  #write(message) {
    if (!this.child?.stdin?.writable) throw new Error("app-server stdin is not writable");
    this.child.stdin.write(`${JSON.stringify(message)}
`);
  }
  #onStdout(chunk) {
    this.buffer += chunk;
    while (true) {
      const idx = this.buffer.indexOf("\n");
      if (idx < 0) return;
      const line = this.buffer.slice(0, idx).trim();
      this.buffer = this.buffer.slice(idx + 1);
      if (!line) continue;
      let message;
      try {
        message = JSON.parse(line);
      } catch (error) {
        this.log("app-server.parse-error", { line, error: error.message });
        continue;
      }
      this.#onMessage(message);
    }
  }
  #onMessage(message) {
    if (Object.prototype.hasOwnProperty.call(message, "id") && (message.result || message.error)) {
      const pending = this.pending.get(message.id);
      if (!pending) {
        this.log("app-server.unmatched-response", message);
        return;
      }
      this.pending.delete(message.id);
      if (message.error) {
        const error = new Error(message.error.message || `app-server error for ${pending.method}`);
        error.data = message.error;
        pending.reject(error);
      } else {
        pending.resolve(message.result);
      }
      return;
    }
    if (message.method && Object.prototype.hasOwnProperty.call(message, "id")) {
      this.emit("serverRequest", message);
      this.#write({ id: message.id, error: { code: -32601, message: `CAM does not handle ${message.method}` } });
      return;
    }
    if (message.method) {
      this.emit("notification", message);
      if (message.method === "error") {
        this.emit("app-server/error", message.params);
        if (this.listenerCount("error") > 0) this.emit("error", message.params);
        return;
      }
      this.emit(message.method, message.params);
    }
  }
};
function textInput(text) {
  return [{ type: "text", text, text_elements: [] }];
}

// src/config.js
var import_node_crypto = __toESM(require("node:crypto"), 1);
var import_node_fs2 = __toESM(require("node:fs"), 1);
var import_node_os2 = __toESM(require("node:os"), 1);
var import_node_path2 = __toESM(require("node:path"), 1);

// src/paths.js
var import_node_fs = __toESM(require("node:fs"), 1);
var import_node_os = __toESM(require("node:os"), 1);
var import_node_path = __toESM(require("node:path"), 1);
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
function appendJsonl(file, value) {
  import_node_fs.default.mkdirSync(import_node_path.default.dirname(file), { recursive: true });
  import_node_fs.default.appendFileSync(file, `${JSON.stringify(value)}
`, "utf8");
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

// src/registry.js
var import_node_os3 = __toESM(require("node:os"), 1);
var import_node_fs3 = __toESM(require("node:fs"), 1);
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
function saveRegistry(registry) {
  registry.updatedAt = (/* @__PURE__ */ new Date()).toISOString();
  writeJsonAtomic(paths().registry, registry);
}
function upsertAgent(config, partial) {
  if (!partial?.name) throw new Error("agent name is required");
  const registry = loadRegistry(config);
  const now = (/* @__PURE__ */ new Date()).toISOString();
  const existing = registry.agents[partial.name] || {};
  const agent = {
    name: partial.name,
    node: partial.node || registry.nodeName || config.nodeName,
    cwd: partial.cwd || existing.cwd || process.cwd(),
    threadId: partial.threadId ?? existing.threadId ?? null,
    activeTurnId: partial.activeTurnId ?? existing.activeTurnId ?? null,
    model: partial.model !== void 0 ? partial.model : existing.model ?? null,
    modelProvider: partial.modelProvider !== void 0 ? partial.modelProvider : existing.modelProvider ?? null,
    effort: partial.effort !== void 0 ? partial.effort : existing.effort ?? null,
    serviceTier: partial.serviceTier !== void 0 ? partial.serviceTier : existing.serviceTier ?? null,
    status: partial.status || existing.status || "unbound",
    createdAt: existing.createdAt || now,
    updatedAt: now,
    lastDelivery: partial.lastDelivery ?? existing.lastDelivery ?? null
  };
  registry.agents[partial.name] = agent;
  saveRegistry(registry);
  return agent;
}
function setAgent(config, name, changes) {
  const registry = loadRegistry(config);
  const agent = registry.agents[name];
  if (!agent) throw new Error(`unknown agent: ${name}`);
  Object.assign(agent, changes, { updatedAt: (/* @__PURE__ */ new Date()).toISOString() });
  saveRegistry(registry);
  return agent;
}
function getAgent(config, name) {
  return loadRegistry(config).agents[name] || null;
}
function listAgents(config) {
  return Object.values(loadRegistry(config).agents).sort((a, b) => a.name.localeCompare(b.name));
}
function appendEvent(type, payload) {
  appendJsonl(paths().events, {
    type,
    timestamp: (/* @__PURE__ */ new Date()).toISOString(),
    ...payload
  });
}
function appendMailbox(message) {
  appendJsonl(paths().mailbox, message);
}
function readMailbox(agentName = null) {
  const rows = readJsonl(paths().mailbox);
  return agentName ? rows.filter((row) => row.targetAgent === agentName) : rows;
}
function pendingMailbox(agentName) {
  return readMailbox(agentName).filter((row) => row.delivery === "queued" && !row.surfacedAt);
}
function markMailboxSurfaced(messageIds, turnId) {
  if (!messageIds.length) return [];
  const all = readJsonl(paths().mailbox);
  const now = (/* @__PURE__ */ new Date()).toISOString();
  const touched = [];
  for (const row of all) {
    if (messageIds.includes(row.messageId) && row.delivery === "queued" && !row.surfacedAt) {
      row.surfacedAt = now;
      row.surfacedTurnId = turnId;
      row.delivery = "surfaced";
      touched.push(row);
    }
  }
  import_node_fs3.default.writeFileSync(paths().mailbox, all.map((row) => JSON.stringify(row)).join("\n") + (all.length ? "\n" : ""), "utf8");
  return touched;
}

// src/daemon.js
function json(res, status, body) {
  const payload = JSON.stringify(body, null, 2);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(payload)
  });
  res.end(payload);
}
function readBody(req) {
  return new Promise((resolve, reject) => {
    let body = "";
    req.setEncoding("utf8");
    req.on("data", (chunk) => {
      body += chunk;
      if (body.length > 2e6) reject(new Error("request body too large"));
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
function normalizeServiceTier(value) {
  if (value === void 0 || value === null) return null;
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
var AgentManagerDaemon = class {
  constructor(config = loadConfig()) {
    this.config = config;
    this.token = ensureLocalToken();
    this.appServer = new AppServerClient({
      codexPath: config.codexPath,
      log: (type, payload) => this.log(type, payload)
    });
    this.startedAt = (/* @__PURE__ */ new Date()).toISOString();
    this.server = null;
    this.threadToAgent = /* @__PURE__ */ new Map();
  }
  log(type, payload = {}) {
    appendJsonl(paths().daemonLog, {
      timestamp: (/* @__PURE__ */ new Date()).toISOString(),
      type,
      payload
    });
  }
  async start() {
    await this.appServer.start();
    for (const agent of listAgents(this.config)) {
      if (agent.threadId) this.threadToAgent.set(agent.threadId, agent.name);
    }
    this.appServer.on("turn/started", ({ threadId, turn }) => {
      const name = this.threadToAgent.get(threadId);
      if (name) setAgent(this.config, name, { status: "active", activeTurnId: turn.id });
    });
    this.appServer.on("turn/completed", ({ threadId, turn }) => {
      const name = this.threadToAgent.get(threadId);
      if (name) setAgent(this.config, name, { status: "idle", activeTurnId: null, lastTurnId: turn.id, lastError: null });
    });
    this.appServer.on("thread/status/changed", ({ threadId, status }) => {
      const name = this.threadToAgent.get(threadId);
      if (name) {
        const current = getAgent(this.config, name);
        const statusType = status?.type || "unknown";
        const changes = { status: current?.lastError && statusType === "idle" ? "error" : statusType };
        if (statusType !== "active") changes.activeTurnId = null;
        setAgent(this.config, name, changes);
      }
    });
    this.appServer.on("app-server/error", (payload = {}) => {
      this.log("app-server.error", payload);
      const name = this.threadToAgent.get(payload.threadId);
      if (name) {
        setAgent(this.config, name, {
          status: "error",
          activeTurnId: null,
          lastError: payload.error?.message || "app-server error"
        });
      }
    });
    this.server = import_node_http.default.createServer((req, res) => this.#handle(req, res));
    await new Promise((resolve, reject) => {
      this.server.once("error", reject);
      this.server.listen(this.config.port, this.config.bindHost, resolve);
    });
    writeJsonAtomic(paths().daemon, {
      pid: process.pid,
      nodeName: this.config.nodeName,
      url: `http://${this.config.bindHost}:${this.config.port}`,
      startedAt: this.startedAt,
      codexPath: this.config.codexPath
    });
    import_node_fs4.default.writeFileSync(paths().pid, String(process.pid));
    this.log("daemon.started", { pid: process.pid, url: `http://${this.config.bindHost}:${this.config.port}` });
    void this.#warmKnownAgents();
  }
  async stop() {
    await new Promise((resolve) => this.server?.close(resolve));
    this.appServer.stop();
  }
  async #handle(req, res) {
    try {
      if (req.url === "/health" && req.method === "GET") {
        return json(res, 200, {
          ok: true,
          nodeName: this.config.nodeName,
          startedAt: this.startedAt,
          appServerInitialized: this.appServer.initialized
        });
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
          status: body.threadId ? "registered" : "unbound"
        });
        if (agent.threadId) this.threadToAgent.set(agent.threadId, agent.name);
        appendEvent("agent.created", { agent });
        return json(res, 200, { ok: true, agent });
      }
      if (url.pathname === "/agents/resume" && req.method === "POST") {
        const body = await readBody(req);
        const agent = await this.#ensureThread(body.name);
        return json(res, 200, { ok: true, agent });
      }
      if (url.pathname === "/agents/set-model" && req.method === "POST") {
        const body = await readBody(req);
        if (body.recreate) {
          throw new Error("recreate is forbidden; model changes must preserve the existing chat/session/agent mapping");
        }
        const agent = getAgent(this.config, body.name);
        if (!agent) throw new Error(`unknown agent: ${body?.name}`);
        const changes = {};
        if ("model" in body) changes.model = body.model ?? null;
        if ("modelProvider" in body) changes.modelProvider = body.modelProvider ?? null;
        if ("effort" in body) changes.effort = normalizeEffort(body.effort);
        if ("serviceTier" in body) changes.serviceTier = normalizeServiceTier(body.serviceTier);
        const nextAgent = setAgent(this.config, body.name, changes);
        return json(res, 200, { ok: true, agent: nextAgent });
      }
      if (url.pathname === "/agents/read" && req.method === "GET") {
        const name = url.searchParams.get("name");
        const includeTurns = url.searchParams.get("includeTurns") !== "false";
        if (name === "antigravity") {
          const agent2 = getAgent(this.config, "antigravity") || upsertAgent(this.config, { name: "antigravity" });
          return json(res, 200, {
            ok: true,
            agent: agent2,
            thread: { id: agent2.threadId || "antigravity-session-uuid", status: { type: agent2.status || "idle" }, turns: [] }
          });
        }
        const agent = await this.#ensureThread(name);
        let thread;
        try {
          thread = await this.appServer.request("thread/read", {
            threadId: agent.threadId,
            includeTurns
          }, 6e4);
        } catch (error) {
          if (!includeTurns || !/not materialized yet|includeTurns is unavailable/.test(error.message)) throw error;
          thread = await this.appServer.request("thread/read", {
            threadId: agent.threadId,
            includeTurns: false
          }, 6e4);
          thread.readWarning = error.message;
        }
        return json(res, 200, { ok: true, agent, thread });
      }
      if (url.pathname === "/send" && req.method === "POST") {
        const body = await readBody(req);
        const result = await this.#sendMessage(body);
        return json(res, 200, { ok: true, ...result });
      }
      if (url.pathname === "/inbox" && req.method === "GET") {
        return json(res, 200, { ok: true, messages: readMailbox(url.searchParams.get("agent")) });
      }
      if (url.pathname === "/logs" && req.method === "GET") {
        const rows = import_node_fs4.default.existsSync(paths().daemonLog) ? import_node_fs4.default.readFileSync(paths().daemonLog, "utf8").split(/\r?\n/).filter(Boolean).slice(-200).map((line) => JSON.parse(line)) : [];
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
  async #ensureThread(name) {
    let agent = getAgent(this.config, name);
    if (!agent) throw new Error(`unknown agent: ${name}`);
    if (name === "antigravity") {
      if (!agent.threadId) {
        agent = setAgent(this.config, name, { threadId: "antigravity-session-uuid", status: "idle" });
      }
      return agent;
    }
    if (agent.threadId) {
      try {
        const resumed = await this.appServer.request("thread/resume", {
          threadId: agent.threadId,
          cwd: agent.cwd,
          excludeTurns: true,
          persistExtendedHistory: false
        }, 6e4);
        this.threadToAgent.set(agent.threadId, agent.name);
        const statusType = resumed.thread?.status?.type || "idle";
        const changes = { status: statusType };
        if (statusType !== "active") changes.activeTurnId = null;
        agent = setAgent(this.config, name, changes);
        return agent;
      } catch (error) {
        this.log("thread.resume.failed", { agent: name, threadId: agent.threadId, error: error.message });
      }
    }
    const created = await this.#startThread(agent);
    const threadId = created.thread.id;
    this.threadToAgent.set(threadId, name);
    return setAgent(this.config, name, { threadId, status: created.thread.status?.type || "idle" });
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
          persistExtendedHistory: false
        }, 6e4);
        this.threadToAgent.set(agent.threadId, agent.name);
        const statusType = resumed.thread?.status?.type || "idle";
        const changes = { status: statusType };
        if (statusType !== "active") changes.activeTurnId = null;
        setAgent(this.config, agent.name, changes);
        this.log("daemon.agent.warmed", {
          name: agent.name,
          threadId: agent.threadId,
          statusType
        });
      } catch (error) {
        this.log("daemon.agent.warm.failed", {
          name: agent.name,
          threadId: agent.threadId,
          error: error.message
        });
      }
    }
    this.log("daemon.warm.complete", { count: agents.length });
  }
  async #startThread(agent) {
    const base = {
      cwd: agent.cwd,
      approvalPolicy: "never",
      sandbox: "danger-full-access",
      ephemeral: false,
      threadSource: null,
      experimentalRawEvents: false,
      persistExtendedHistory: false
    };
    const runtimeSettings = this.#runtimeSettings(agent);
    if (Object.keys(runtimeSettings).length) {
      const withModel = { ...base, ...runtimeSettings };
      try {
        return await this.appServer.request("thread/start", withModel, 6e4);
      } catch (error) {
        this.log("thread.start.with-model.failed", {
          agent: agent.name,
          model: agent.model,
          modelProvider: agent.modelProvider,
          effort: agent.effort,
          serviceTier: agent.serviceTier,
          error: error.message
        });
      }
    }
    return this.appServer.request("thread/start", base, 6e4);
  }
  async #sendMessage(body) {
    if (!body?.targetAgent) throw new Error("targetAgent is required");
    if (!body?.message) throw new Error("message is required");
    if (body.targetAgent === "antigravity") {
      const target2 = getAgent(this.config, "antigravity") || upsertAgent(this.config, {
        name: "antigravity",
        cwd: body.cwd || paths().root,
        status: "idle"
      });
      const message2 = {
        messageId: import_node_crypto2.default.randomUUID(),
        correlationId: body.correlationId || null,
        sourceAgent: body.sourceAgent || "operator",
        targetAgent: "antigravity",
        sourceNode: body.sourceNode || this.config.nodeName,
        targetNode: target2.node,
        timestamp: (/* @__PURE__ */ new Date()).toISOString(),
        body: body.message,
        delivery: "delivered"
      };
      setAgent(this.config, "antigravity", { status: "active", lastDelivery: message2 });
      appendEvent("message.delivered", message2);
      this.log("message.delivered.external", { messageId: message2.messageId, target: "antigravity" });
      return { delivered: true, queued: false, message: message2 };
    }
    if (body.sourceAgent === "antigravity") {
      setAgent(this.config, "antigravity", { status: "idle" });
    }
    const target = await this.#ensureThread(body.targetAgent);
    const message = {
      messageId: import_node_crypto2.default.randomUUID(),
      correlationId: body.correlationId || null,
      sourceAgent: body.sourceAgent || "operator",
      targetAgent: body.targetAgent,
      sourceNode: body.sourceNode || this.config.nodeName,
      targetNode: target.node,
      timestamp: (/* @__PURE__ */ new Date()).toISOString(),
      body: body.message,
      delivery: "pending"
    };
    const pending = pendingMailbox(body.targetAgent);
    const pendingText = pending.length ? [
      "",
      "[Queued messages surfaced by Codex Agent Manager]",
      ...pending.map((queued, index) => [
        `queuedMessage ${index + 1}:`,
        `messageId: ${queued.messageId}`,
        `sourceAgent: ${queued.sourceAgent}`,
        `sourceNode: ${queued.sourceNode}`,
        queued.body
      ].join("\n"))
    ].join("\n") : "";
    const prompt = [
      "[Codex Agent Manager message]",
      `messageId: ${message.messageId}`,
      `sourceAgent: ${message.sourceAgent}`,
      `sourceNode: ${message.sourceNode}`,
      `targetAgent: ${message.targetAgent}`,
      "",
      message.body,
      pendingText
    ].join("\n");
    try {
      if (target.activeTurnId) {
        const steer = await this.appServer.request("turn/steer", {
          threadId: target.threadId,
          input: textInput(prompt),
          expectedTurnId: target.activeTurnId
        }, 3e4);
        message.delivery = "steered";
        message.turnId = steer.turnId;
      } else {
        const started = await this.appServer.request("turn/start", {
          threadId: target.threadId,
          input: textInput(prompt),
          cwd: target.cwd,
          approvalPolicy: "never",
          ...this.#runtimeSettings(target)
        }, 6e4);
        message.delivery = "started";
        message.turnId = started.turn.id;
        setAgent(this.config, target.name, { status: "active", activeTurnId: started.turn.id, lastDelivery: message });
      }
      const surfaced = markMailboxSurfaced(pending.map((queued) => queued.messageId), message.turnId);
      for (const queued of surfaced) appendEvent("message.surfaced", queued);
      appendEvent("message.delivered", message);
      return { delivered: true, queued: false, message };
    } catch (error) {
      message.delivery = "queued";
      message.error = error.message;
      appendMailbox(message);
      appendEvent("message.queued", message);
      return { delivered: false, queued: true, message };
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
};
async function runDaemon() {
  const daemon = new AgentManagerDaemon();
  await daemon.start();
  process.on("SIGINT", () => daemon.stop().then(() => process.exit(0)));
  process.on("SIGTERM", () => daemon.stop().then(() => process.exit(0)));
}

// src/daemon-entry.js
runDaemon().catch((error) => {
  console.error(error?.stack || error?.message || String(error));
  process.exit(1);
});
