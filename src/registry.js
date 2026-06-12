import os from "node:os";
import fs from "node:fs";
import { appendJsonl, paths, readJson, readJsonl, writeJsonAtomic } from "./paths.js";

export function loadRegistry(config) {
  const p = paths();
  const registry = readJson(p.registry, {
    version: 1,
    nodeName: config?.nodeName || os.hostname(),
    agents: {},
    peers: {},
    updatedAt: new Date().toISOString(),
  });
  registry.agents ||= {};
  registry.peers ||= {};
  return registry;
}

export function saveRegistry(registry) {
  registry.updatedAt = new Date().toISOString();
  writeJsonAtomic(paths().registry, registry);
}

export function upsertAgent(config, partial) {
  if (!partial?.name) throw new Error("agent name is required");
  const registry = loadRegistry(config);
  const now = new Date().toISOString();
  const existing = registry.agents[partial.name] || {};
  const agent = {
    name: partial.name,
    node: partial.node || registry.nodeName || config.nodeName,
    cwd: partial.cwd || existing.cwd || process.cwd(),
    threadId: partial.threadId ?? existing.threadId ?? null,
    activeTurnId: partial.activeTurnId ?? existing.activeTurnId ?? null,
    model: partial.model !== undefined ? partial.model : (existing.model ?? null),
    modelProvider: partial.modelProvider !== undefined ? partial.modelProvider : (existing.modelProvider ?? null),
    effort: partial.effort !== undefined ? partial.effort : (existing.effort ?? null),
    serviceTier: partial.serviceTier !== undefined ? partial.serviceTier : (existing.serviceTier ?? null),
    status: partial.status || existing.status || "unbound",
    createdAt: existing.createdAt || now,
    updatedAt: now,
    lastDelivery: partial.lastDelivery ?? existing.lastDelivery ?? null,
    threadSource: partial.threadSource !== undefined ? partial.threadSource : (existing.threadSource ?? "codex"),
  };
  registry.agents[partial.name] = agent;
  saveRegistry(registry);
  return agent;
}

export function setAgent(config, name, changes) {
  const registry = loadRegistry(config);
  const agent = registry.agents[name];
  if (!agent) throw new Error(`unknown agent: ${name}`);
  Object.assign(agent, changes, { updatedAt: new Date().toISOString() });
  saveRegistry(registry);
  return agent;
}

export function getAgent(config, name) {
  return loadRegistry(config).agents[name] || null;
}

export function listAgents(config) {
  return Object.values(loadRegistry(config).agents).sort((a, b) => a.name.localeCompare(b.name));
}

export function appendEvent(type, payload) {
  appendJsonl(paths().events, {
    type,
    timestamp: new Date().toISOString(),
    ...payload,
  });
}

export function appendMailbox(message) {
  appendJsonl(paths().mailbox, message);
}

export function readMailbox(agentName = null) {
  const rows = readJsonl(paths().mailbox);
  return agentName ? rows.filter((row) => row.targetAgent === agentName) : rows;
}

export function pendingMailbox(agentName) {
  return readMailbox(agentName).filter((row) => row.delivery === "queued" && !row.surfacedAt);
}

export function markMailboxSurfaced(messageIds, turnId) {
  if (!messageIds.length) return [];
  const all = readJsonl(paths().mailbox);
  const now = new Date().toISOString();
  const touched = [];
  for (const row of all) {
    if (messageIds.includes(row.messageId) && row.delivery === "queued" && !row.surfacedAt) {
      row.surfacedAt = now;
      row.surfacedTurnId = turnId;
      row.delivery = "surfaced";
      touched.push(row);
    }
  }
  fs.writeFileSync(paths().mailbox, all.map((row) => JSON.stringify(row)).join("\n") + (all.length ? "\n" : ""), "utf8");
  return touched;
}
