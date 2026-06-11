import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import { AppServerClient, textInput } from "./app-server.js";
import { ensureLocalToken, loadConfig } from "./config.js";
import {
  appendEvent,
  appendMailbox,
  getAgent,
  listAgents,
  markMailboxSurfaced,
  pendingMailbox,
  readMailbox,
  setAgent,
  upsertAgent,
} from "./registry.js";
import { appendJsonl, paths, writeJsonAtomic } from "./paths.js";

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
          lastError: payload.error?.message || "app-server error",
        });
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
          const agent = getAgent(this.config, "antigravity") || upsertAgent(this.config, { name: "antigravity" });
          return json(res, 200, {
            ok: true,
            agent,
            thread: { id: agent.threadId || "antigravity-session-uuid", status: { type: agent.status || "idle" }, turns: [] }
          });
        }
        const agent = await this.#ensureThread(name);
        let thread;
        try {
          thread = await this.appServer.request("thread/read", {
            threadId: agent.threadId,
            includeTurns,
          }, 60000);
        } catch (error) {
          if (!includeTurns || !/not materialized yet|includeTurns is unavailable/.test(error.message)) throw error;
          thread = await this.appServer.request("thread/read", {
            threadId: agent.threadId,
            includeTurns: false,
          }, 60000);
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

  async #sendMessage(body) {
    if (!body?.targetAgent) throw new Error("targetAgent is required");
    if (!body?.message) throw new Error("message is required");
    if (body.targetAgent === "antigravity") {
      const target = getAgent(this.config, "antigravity") || upsertAgent(this.config, {
        name: "antigravity",
        cwd: body.cwd || paths().root,
        status: "idle",
      });
      const message = {
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
      setAgent(this.config, "antigravity", { status: "active", lastDelivery: message });
      appendEvent("message.delivered", message);
      this.log("message.delivered.external", { messageId: message.messageId, target: "antigravity" });
      return { delivered: true, queued: false, message };
    }

    if (body.sourceAgent === "antigravity") {
      setAgent(this.config, "antigravity", { status: "idle" });
    }

    const target = await this.#ensureThread(body.targetAgent);
    const message = {
      messageId: crypto.randomUUID(),
      correlationId: body.correlationId || null,
      sourceAgent: body.sourceAgent || "operator",
      targetAgent: body.targetAgent,
      sourceNode: body.sourceNode || this.config.nodeName,
      targetNode: target.node,
      timestamp: new Date().toISOString(),
      body: body.message,
      delivery: "pending",
    };

    const pending = pendingMailbox(body.targetAgent);
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
}

export async function runDaemon() {
  const daemon = new AgentManagerDaemon();
  await daemon.start();
  process.on("SIGINT", () => daemon.stop().then(() => process.exit(0)));
  process.on("SIGTERM", () => daemon.stop().then(() => process.exit(0)));
}
