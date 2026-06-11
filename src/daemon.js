import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { execFile } from "node:child_process";
import { AppServerClient, textInput } from "./app-server.js";
import { ensureLocalToken, loadConfig } from "./config.js";
import {
  appendEvent,
  appendMailbox,
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
import { appendJsonl, paths, writeJsonAtomic, readJson } from "./paths.js";

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

    // Load remote query scripts
    const srcDir = typeof __dirname !== "undefined" ? __dirname : path.dirname(fileURLToPath(import.meta.url));
    this.pyRemoteScript = fs.readFileSync(path.join(srcDir, "remote_query_threads.py"), "utf8");
    this.jsRemoteScript = fs.readFileSync(path.join(srcDir, "remote_query_threads.js"), "utf8");
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
    appendJsonl(paths().daemonLog, {
      timestamp: new Date().toISOString(),
      type,
      payload,
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

    await this.syncActiveThreads();
    this.syncInterval = setInterval(() => {
      void this.syncActiveThreads();
    }, 5000);

    // Initial remote peer sync and interval
    await this.syncRemotePeers();
    this.syncRemoteInterval = setInterval(() => {
      void this.syncRemotePeers();
    }, 30000);

    void this.#warmKnownAgents();
  }

  async stop() {
    if (this.syncInterval) {
      clearInterval(this.syncInterval);
    }
    if (this.syncRemoteInterval) {
      clearInterval(this.syncRemoteInterval);
    }
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
          appServerInitialized: this.appServer.initialized,
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
        if (agent && agent.node && agent.node !== this.config.nodeName) {
          try {
            const registry = loadRegistry(this.config);
            const peer = registry.peers?.[agent.node];
            if (!peer) throw new Error(`unknown remote node: ${agent.node}`);
            const command = [
              "node",
              sq(`${peer.remoteRoot}/bin/cam.js`),
              "agent",
              "resume",
              sq(name)
            ].join(" ");
            const result = await this.runSshCommand(peer, [command]);
            if (!result.ok) throw new Error(result.stderr || `failed to resume remote agent: exit code ${result.code}`);
            const resumedAgent = JSON.parse(result.stdout);
            return json(res, 200, { ok: true, agent: resumedAgent });
          } catch (err) {
            return json(res, 500, { ok: false, error: err.message });
          }
        }
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
        if (agent && agent.node && agent.node !== this.config.nodeName) {
          try {
            const registry = loadRegistry(this.config);
            const peer = registry.peers?.[agent.node];
            if (!peer) throw new Error(`unknown remote node: ${agent.node}`);
            const args = ["node", sq(`${peer.remoteRoot}/bin/cam.js`), "agent", "set-model", sq(name)];
            if (body.model !== undefined && body.model !== null) args.push("--model", sq(body.model));
            if (body.modelProvider !== undefined && body.modelProvider !== null) args.push("--model-provider", sq(body.modelProvider));
            if (body.effort !== undefined && body.effort !== null) args.push("--effort", sq(body.effort));
            if (body.serviceTier !== undefined && body.serviceTier !== null) args.push("--service-tier", sq(body.serviceTier));
            const command = args.join(" ");
            const result = await this.runSshCommand(peer, [command]);
            if (!result.ok) throw new Error(result.stderr || `failed to set model on remote agent: exit code ${result.code}`);
            const updatedAgent = JSON.parse(result.stdout);
            return json(res, 200, { ok: true, agent: updatedAgent });
          } catch (err) {
            return json(res, 500, { ok: false, error: err.message });
          }
        }
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
        if (name === "antigravity") {
          const agent = getAgent(this.config, "antigravity") || upsertAgent(this.config, { name: "antigravity" });
          return json(res, 200, {
            ok: true,
            agent,
            thread: { id: agent.threadId || "antigravity-session-uuid", status: { type: agent.status || "idle" }, turns: [] }
          });
        }
        const agent = getAgent(this.config, name);
        if (agent && agent.node && agent.node !== this.config.nodeName) {
          try {
            const registry = loadRegistry(this.config);
            const peer = registry.peers?.[agent.node];
            if (!peer) throw new Error(`unknown remote node: ${agent.node}`);
            const args = ["node", sq(`${peer.remoteRoot}/bin/cam.js`), "agent", "read", sq(name)];
            if (!includeTurns) args.push("--latest");
            const command = args.join(" ");
            const result = await this.runSshCommand(peer, [command]);
            if (!result.ok) throw new Error(result.stderr || `failed to read remote agent: exit code ${result.code}`);
            const thread = JSON.parse(result.stdout);
            return json(res, 200, { ok: true, agent, thread });
          } catch (err) {
            return json(res, 500, { ok: false, error: err.message });
          }
        }
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
        const targetAgent = body.targetAgent;
        const target = getAgent(this.config, targetAgent);
        if (target && target.node && target.node !== this.config.nodeName) {
          try {
            const registry = loadRegistry(this.config);
            const peer = registry.peers?.[target.node];
            if (!peer) throw new Error(`unknown remote node: ${target.node}`);
            const command = [
              "node",
              sq(`${peer.remoteRoot}/bin/cam.js`),
              "send",
              sq(targetAgent),
              sq(body.message),
              "--from",
              sq(body.sourceAgent || "operator"),
              "--source-node",
              sq(body.sourceNode || this.config.nodeName)
            ].join(" ");
            const result = await this.runSshCommand(peer, [command]);
            if (!result.ok) throw new Error(result.stderr || `failed to send message via remote peer: exit code ${result.code}`);
            const message = JSON.parse(result.stdout);
            return json(res, 200, { ok: true, delivered: true, queued: false, message });
          } catch (err) {
            return json(res, 500, { ok: false, error: err.message });
          }
        }
        const result = await this.#sendMessage(body);
        return json(res, 200, { ok: true, ...result });
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

  async #ensureThread(name) {
    if (this.ensuringThreads.has(name)) {
      return this.ensuringThreads.get(name);
    }
    const promise = (async () => {
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
      const scriptPath = path.resolve(
        typeof __dirname !== "undefined" ? __dirname : path.dirname(fileURLToPath(import.meta.url)),
        "query_threads.py"
      );

      const tryPython = (cmd) => {
        execFile(cmd, [scriptPath], { env: process.env }, (error, stdout, stderr) => {
          if (error) {
            if (cmd === "python") {
              tryPython("python3");
            } else {
              this.log("sync.threads.failed", { error: error.message, stderr });
              resolve();
            }
            return;
          }

          try {
            const data = JSON.parse(stdout);
            if (data.error) {
              this.log("sync.threads.error", { error: data.error });
              resolve();
              return;
            }

            this.#applyThreadSync(data.threads);
            resolve();
          } catch (e) {
            this.log("sync.threads.parse.failed", { error: e.message, stdout });
            resolve();
          }
        });
      };

      tryPython("python");
    });
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
      const defaultWorkspace = "C:\\Users\\kjhgf\\OneDrive\\Documents\\New project";

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
                
                let cwd = thread.cwd || defaultWorkspace;
                if (cwd.startsWith("\\\\?\\")) {
                  cwd = cwd.substring(4);
                }
                
                upsertAgent(this.config, {
                  name: uniqueName,
                  cwd,
                  threadId: tid,
                  status: agent.status || "idle",
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
          continue;
        }

        let uniqueName = name;
        let counter = 1;
        const currentNames = new Set(listAgents(this.config).map(a => a.name));
        while (currentNames.has(uniqueName)) {
          counter++;
          uniqueName = `${name}-${counter}`;
        }

        let cwd = thread.cwd || defaultWorkspace;
        if (cwd.startsWith("\\\\?\\")) {
          cwd = cwd.substring(4);
        }

        try {
          const agent = upsertAgent(this.config, {
            name: uniqueName,
            cwd,
            threadId: tid,
            status: "idle",
          });
          this.threadToAgent.set(tid, uniqueName);
          appendEvent("agent.created", { agent });
          this.log("sync.agent.created", { name: uniqueName, threadId: tid, cwd });
        } catch (e) {
          this.log("sync.agent.create.failed", { threadId: tid, error: e.message });
        }
      }

      const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
      const localNode = this.config.nodeName;
      for (const agent of registry) {
        const isLocal = !agent.node || agent.node === localNode;
        if (isLocal && agent.threadId && uuidRegex.test(agent.threadId) && !activeThreadIds.has(agent.threadId)) {
          try {
            const p = paths();
            const currentRegistry = readJson(p.registry, { agents: {} });
            if (currentRegistry.agents && currentRegistry.agents[agent.name]) {
              delete currentRegistry.agents[agent.name];
              currentRegistry.updatedAt = new Date().toISOString();
              writeJsonAtomic(p.registry, currentRegistry);
              this.threadToAgent.delete(agent.threadId);
              appendEvent("agent.deleted", { name: agent.name, threadId: agent.threadId });
              this.log("sync.agent.deleted", { name: agent.name, threadId: agent.threadId });
            }
          } catch (e) {
            this.log("sync.agent.delete.failed", { name: agent.name, error: e.message });
          }
        }
      }
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

  async #sendMessage(body, existingMessage = null) {
    if (!existingMessage && !body?.targetAgent) throw new Error("targetAgent is required");
    if (!existingMessage && !body?.message) throw new Error("message is required");
    
    const targetAgent = existingMessage ? existingMessage.targetAgent : body.targetAgent;

    if (targetAgent === "antigravity") {
      const target = getAgent(this.config, "antigravity") || upsertAgent(this.config, {
        name: "antigravity",
        cwd: (existingMessage ? null : body.cwd) || paths().root,
        status: "idle",
      });
      const message = existingMessage || {
        messageId: crypto.randomUUID(),
        correlationId: body.correlationId || null,
        sourceAgent: body.sourceAgent || "operator",
        targetAgent: "antigravity",
        sourceNode: body.sourceNode || this.config.nodeName,
        targetNode: target.node,
        timestamp: new Date().toISOString(),
        body: body.message,
        delivery: "delivered",
      };
      if (existingMessage) {
        message.delivery = "delivered";
      }
      setAgent(this.config, "antigravity", { status: "active", lastDelivery: message });
      appendEvent("message.delivered", message);
      this.log("message.delivered.external", { messageId: message.messageId, target: "antigravity" });
      
      if (existingMessage) {
        markMailboxSurfaced([message.messageId], null);
      }
      return { delivered: true, queued: false, message };
    }

    if (!existingMessage && body.sourceAgent === "antigravity") {
      setAgent(this.config, "antigravity", { status: "idle" });
      this.#checkInboxListeners();
    }

    let target;
    try {
      target = await this.#ensureThread(targetAgent);
    } catch (ensureErr) {
      const message = existingMessage || {
        messageId: crypto.randomUUID(),
        correlationId: body?.correlationId || null,
        sourceAgent: body?.sourceAgent || "operator",
        targetAgent: targetAgent,
        sourceNode: body?.sourceNode || this.config.nodeName,
        targetNode: null,
        timestamp: new Date().toISOString(),
        body: body?.message,
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
      sourceNode: body.sourceNode || this.config.nodeName,
      targetNode: target.node,
      timestamp: new Date().toISOString(),
      body: body.message,
      delivery: "pending",
    };

    // Filter pending mailbox to exclude this message if it's already queued
    const pending = pendingMailbox(targetAgent).filter(m => m.messageId !== message.messageId);
    const pendingText = pending.length
      ? [
          "",
          "[Queued messages surfaced by Codex Agent Manager]",
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
      "[Codex Agent Manager message]",
      `messageId: ${message.messageId}`,
      `sourceAgent: ${message.sourceAgent}`,
      `sourceNode: ${message.sourceNode}`,
      `targetAgent: ${message.targetAgent}`,
      "",
      message.body,
      pendingText,
    ].join("\n");

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
      appendEvent("message.delivered", message);
      return { delivered: true, queued: false, message };
    } catch (error) {
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

  runSshCommand(peer, commandArgs, stdinContent = null, timeoutMs = 45000) {
    return new Promise((resolve) => {
      const sshArgs = ["-o", "StrictHostKeyChecking=no", "-o", "ConnectTimeout=5"];
      if (peer.key) sshArgs.push("-i", peer.key);
      sshArgs.push(peer.ssh, ...commandArgs);

      const child = execFile("ssh", sshArgs, { timeout: timeoutMs }, (error, stdout, stderr) => {
        if (error) {
          resolve({ ok: false, code: error.code || -1, stdout, stderr: stderr || error.message });
        } else {
          resolve({ ok: true, code: 0, stdout, stderr });
        }
      });

      if (stdinContent && child.stdin) {
        child.stdin.write(stdinContent);
        child.stdin.end();
      }
    });
  }

  async syncRemotePeers() {
    this.log("sync.remote.start");
    try {
      const registry = loadRegistry(this.config);
      const peers = Object.values(registry.peers || {});
      if (!peers.length) {
        return;
      }
      for (const peer of peers) {
        try {
          await this.syncPeer(peer);
        } catch (e) {
          this.log("sync.remote.peer.failed", { peer: peer.name, error: e.message });
        }
      }
    } catch (e) {
      this.log("sync.remote.failed", { error: e.message });
    }
  }

  async syncPeer(peer) {
    const peerName = peer.name;
    this.log("sync.peer.start", { peer: peerName });

    let remoteAgents = [];
    let successStrategy = null;

    // Stage 1: CLI list
    if (!successStrategy) {
      const command = `node ${sq(`${peer.remoteRoot}/bin/cam.js`)} agent list`;
      const result = await this.runSshCommand(peer, [command]);
      if (result.ok) {
        try {
          const lines = result.stdout.split(/\r?\n/).filter(Boolean);
          const agents = [];
          for (const line of lines) {
            const fields = line.split("\t");
            if (fields.length >= 3) {
              agents.push({
                name: fields[0],
                status: fields[1],
                node: fields[2],
                threadId: fields[3] === "-" ? null : fields[3],
                model: fields[4] === "-" ? null : fields[4],
                modelProvider: fields[5] === "-" ? null : fields[5],
                effort: fields[6] === "-" ? null : fields[6],
                serviceTier: fields[7] === "standard" ? null : fields[7],
                cwd: fields[8] || ""
              });
            }
          }
          if (agents.length > 0) {
            remoteAgents = agents;
            successStrategy = "cli";
            this.log("sync.peer.success", { peer: peerName, strategy: "cli", count: agents.length });
          }
        } catch (e) {
          this.log("sync.peer.strategy.cli.parse.error", { peer: peerName, error: e.message });
        }
      }
    }

    // Stage 2: Registry JSON file
    if (!successStrategy) {
      const command = `cat ~/.codex-agent-manager/agents.json`;
      const result = await this.runSshCommand(peer, [command]);
      if (result.ok) {
        try {
          const registry = JSON.parse(result.stdout);
          const agents = [];
          if (registry && registry.agents) {
            for (const key of Object.keys(registry.agents)) {
              const a = registry.agents[key];
              agents.push({
                name: a.name,
                status: a.status || "idle",
                node: peerName,
                threadId: a.threadId || null,
                model: a.model || null,
                modelProvider: a.modelProvider || null,
                effort: a.effort || null,
                serviceTier: a.serviceTier || null,
                cwd: a.cwd || ""
              });
            }
          }
          remoteAgents = agents;
          successStrategy = "registry";
          this.log("sync.peer.success", { peer: peerName, strategy: "registry", count: agents.length });
        } catch (e) {
          this.log("sync.peer.strategy.registry.parse.error", { peer: peerName, error: e.message });
        }
      }
    }

    // Stage 3: Python direct query
    if (!successStrategy) {
      const result = await this.runSshCommand(peer, ["python3"], this.pyRemoteScript);
      if (result.ok) {
        try {
          const data = JSON.parse(result.stdout);
          if (data.threads) {
            remoteAgents = data.threads.map(t => ({
              name: t.title,
              status: "idle",
              node: peerName,
              threadId: t.id,
              cwd: t.cwd || ""
            }));
            successStrategy = "python";
            this.log("sync.peer.success", { peer: peerName, strategy: "python", count: remoteAgents.length });
          }
        } catch (e) {
          this.log("sync.peer.strategy.python.parse.error", { peer: peerName, error: e.message });
        }
      }
    }

    // Stage 4: Node.js direct query
    if (!successStrategy) {
      const result = await this.runSshCommand(peer, ["node -e " + sq("eval(require('fs').readFileSync(0, 'utf-8'))")], this.jsRemoteScript);
      if (result.ok) {
        try {
          const data = JSON.parse(result.stdout);
          if (data.threads) {
            remoteAgents = data.threads.map(t => ({
              name: t.title,
              status: "idle",
              node: peerName,
              threadId: t.id,
              cwd: t.cwd || ""
            }));
            successStrategy = "node";
            this.log("sync.peer.success", { peer: peerName, strategy: "node", count: remoteAgents.length });
          }
        } catch (e) {
          this.log("sync.peer.strategy.node.parse.error", { peer: peerName, error: e.message });
        }
      }
    }

    if (!successStrategy) {
      this.log("sync.peer.failed.all-strategies", { peer: peerName });
      return;
    }

    // Process and upsert remote agents
    const currentRegistry = loadRegistry(this.config);
    const peerAgentNames = [];
    const activeRemoteThreadIds = new Set();

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

    for (const agent of remoteAgents) {
      if (agent.threadId) {
        activeRemoteThreadIds.add(agent.threadId);
      }

      // Check name conflict
      let baseName = normalizeName(agent.name);
      if (!baseName) {
        baseName = agent.threadId ? `agent-${agent.threadId.substring(0, 8)}` : "remote-agent";
      }

      let uniqueName = baseName;
      let existingAgent = currentRegistry.agents[uniqueName];

      // If name already exists but belongs to a different node, resolve conflict
      if (existingAgent && existingAgent.node !== peerName) {
        uniqueName = `${baseName}-${peerName}`;
        existingAgent = currentRegistry.agents[uniqueName];
      }

      let counter = 1;
      while (existingAgent && existingAgent.node !== peerName) {
        counter++;
        uniqueName = `${baseName}-${peerName}-${counter}`;
        existingAgent = currentRegistry.agents[uniqueName];
      }

      // If we are renaming the agent locally, cleanup old name
      if (existingAgent && existingAgent.threadId === agent.threadId && existingAgent.name !== uniqueName) {
        delete currentRegistry.agents[existingAgent.name];
      }

      agent.name = uniqueName;
      upsertAgent(this.config, agent);
      peerAgentNames.push(uniqueName);
    }

    // Update peer registry listing
    const freshRegistry = loadRegistry(this.config);
    if (freshRegistry.peers && freshRegistry.peers[peerName]) {
      freshRegistry.peers[peerName].agents = peerAgentNames;
      freshRegistry.peers[peerName].lastCheckedAt = new Date().toISOString();
      saveRegistry(freshRegistry);
    }

    // Clean up stale agents for this remote node
    const finalRegistry = loadRegistry(this.config);
    const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
    for (const key of Object.keys(finalRegistry.agents)) {
      const a = finalRegistry.agents[key];
      if (a.node === peerName && a.threadId && uuidRegex.test(a.threadId) && !activeRemoteThreadIds.has(a.threadId)) {
        delete finalRegistry.agents[key];
        saveRegistry(finalRegistry);
        this.threadToAgent.delete(a.threadId);
        this.log("sync.remote.agent.deleted", { name: a.name, threadId: a.threadId, peer: peerName });
      }
    }
  }
}


export async function runDaemon() {
  const daemon = new AgentManagerDaemon();
  await daemon.start();
  process.on("SIGINT", () => daemon.stop().then(() => process.exit(0)));
  process.on("SIGTERM", () => daemon.stop().then(() => process.exit(0)));
}
