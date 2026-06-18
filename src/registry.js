import os from "node:os";
import fs from "node:fs";
import path from "node:path";
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

  try {
    const codexHome = config?.codexHome || process.env.CODEX_HOME || path.join(os.homedir(), ".codex");
    const globalStatePath = path.join(codexHome, ".codex-global-state.json");
    let changed = false;
    if (fs.existsSync(globalStatePath)) {
      const state = JSON.parse(fs.readFileSync(globalStatePath, "utf8"));
      const connections = state?.["codex-managed-remote-connections"] || [];
      for (const conn of connections) {
        const alias = conn.alias || conn.displayName;
        if (!alias) continue;
        const existing = registry.peers[alias] || {};
        const normalized = {
          ...existing,
          name: alias,
          transport: "codex-managed",
          ssh: alias,
          key: null,
          remoteRoot: "auto",
          agents: existing.agents || [],
          enrolledAt: existing.enrolledAt || new Date().toISOString(),
          discovered: true,
          codexHostId: conn.hostId || existing.codexHostId || null,
          codexDisplayName: conn.displayName || existing.codexDisplayName || alias,
        };
        if (JSON.stringify(existing) !== JSON.stringify(normalized)) {
          registry.peers[alias] = normalized;
          changed = true;
        }
      }
    }
    if (changed) {
      registry.updatedAt = new Date().toISOString();
      writeJsonAtomic(p.registry, registry);
    }
  } catch (error) {
    // Fail silently on read error to prevent crashing CLI/daemon
  }

  return registry;
}

export function saveRegistry(registry) {
  registry.updatedAt = new Date().toISOString();
  writeJsonAtomic(paths().registry, registry);
}

export function normalizeName(text) {
  if (!text) return "";
  return text
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, "")
    .trim()
    .replace(/[\s_]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export function upsertAgent(config, partial) {
  if (!partial?.name) throw new Error("agent name is required");
  const registry = loadRegistry(config);
  const now = new Date().toISOString();

  // Look up existing agent flexibly to preserve metadata
  let existingKey = null;
  let existing = {};
  if (partial.threadId && registry.agents[partial.threadId]) {
    existingKey = partial.threadId;
    existing = registry.agents[partial.threadId];
  } else if (partial.name && registry.agents[partial.name]) {
    existingKey = partial.name;
    existing = registry.agents[partial.name];
  } else {
    for (const [k, a] of Object.entries(registry.agents)) {
      if ((partial.threadId && a.threadId === partial.threadId) || a.name === partial.name) {
        existingKey = k;
        existing = a;
        break;
      }
    }
  }

  const agent = {
    name: partial.name,
    node: partial.node || registry.nodeName || config?.nodeName,
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

  const nextKey = agent.threadId || agent.name;

  if (existingKey && existingKey !== nextKey) {
    delete registry.agents[existingKey];
  }

  registry.agents[nextKey] = agent;
  saveRegistry(registry);
  return agent;
}

export function setAgent(config, query, changes) {
  const registry = loadRegistry(config);
  let agentKey = null;
  let agent = registry.agents[query];
  if (agent) {
    agentKey = query;
  } else {
    for (const [k, a] of Object.entries(registry.agents)) {
      if (a.threadId === query || a.name === query || normalizeName(a.name) === normalizeName(query)) {
        agentKey = k;
        agent = a;
        break;
      }
    }
  }
  if (!agent) throw new Error(`unknown agent: ${query}`);

  Object.assign(agent, changes, { updatedAt: new Date().toISOString() });

  const nextKey = agent.threadId || agent.name;
  if (agentKey !== nextKey) {
    delete registry.agents[agentKey];
    registry.agents[nextKey] = agent;
  }

  saveRegistry(registry);
  return agent;
}

export function getAgent(config, query) {
  if (!query) return null;
  const registry = loadRegistry(config);
  if (registry.agents[query]) return registry.agents[query];

  for (const a of Object.values(registry.agents)) {
    if (a.threadId === query || a.name === query) {
      return a;
    }
  }

  const normalizedQuery = normalizeName(query);
  for (const a of Object.values(registry.agents)) {
    if (normalizeName(a.name) === normalizedQuery) {
      return a;
    }
  }
  return null;
}

export function listAgents(config) {
  return Object.values(loadRegistry(config).agents).sort((a, b) => a.name.localeCompare(b.name));
}

export function getMostRecentlyUsedAgent(config) {
  const agents = Object.values(loadRegistry(config).agents);
  if (!agents.length) return null;
  agents.sort((a, b) => new Date(b.updatedAt || 0) - new Date(a.updatedAt || 0));
  return agents[0];
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
  if (!agentName) return rows;

  const registry = loadRegistry();
  const targetIds = new Set([agentName, normalizeName(agentName)]);
  for (const a of Object.values(registry.agents)) {
    if (a.threadId === agentName || a.name === agentName || normalizeName(a.name) === normalizeName(agentName)) {
      if (a.threadId) targetIds.add(a.threadId);
      targetIds.add(a.name);
      targetIds.add(normalizeName(a.name));
    }
  }

  return rows.filter((row) => {
    return targetIds.has(row.targetAgent) || targetIds.has(normalizeName(row.targetAgent));
  });
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
