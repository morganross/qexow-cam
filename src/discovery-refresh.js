import { classifyThreadDiscovery } from "./discovery-policy.js";
import { appendEvent, listAgents, loadRegistry, saveLocalDiscoveries, saveRegistry, setAgent, upsertAgent } from "./registry.js";
import { paths, readJson, writeJsonAtomic } from "./paths.js";

export function removeNonApprovedLocalAgents(config, discoveryRows, threadToAgent = null, log = () => {}) {
  const rejectedThreadIds = new Set((discoveryRows || [])
    .filter((row) => row.approved !== true && row.id)
    .map((row) => row.id));
  if (!rejectedThreadIds.size) return false;
  const registry = loadRegistry(config);
  let changed = false;
  for (const [name, agent] of Object.entries(registry.agents || {})) {
    if (!agent?.threadId || !rejectedThreadIds.has(agent.threadId)) continue;
    if (String(agent.route || "").startsWith("peer:")) continue;
    if (agent.threadSource === "mailbox") continue;
    delete registry.agents[name];
    changed = true;
    if (threadToAgent) threadToAgent.delete(agent.threadId);
    log("sync.agent.demoted", {
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
  return changed;
}

export function refreshLocalRegistryFromThreads({ config, threads, threadToAgent = null, skippedThreadReasons = null, log = () => {} }) {
  const registry = listAgents(config);
  const existingThreadMap = new Map();
  for (const agent of registry) {
    if (agent.threadId && !String(agent.route || "").startsWith("peer:")) {
      existingThreadMap.set(agent.threadId, agent);
    }
  }

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

  for (const thread of threads || []) {
    const tid = thread.id;
    let name = normalizeName(thread.agent_nickname);
    if (!name) name = normalizeName(thread.title);
    if (!name) name = `agent-${String(tid || "").substring(0, 8)}`;
    if (name && !name.endsWith("-agent")) name = `${name}-agent`;

    let cwd = thread.cwd;
    if (cwd && cwd.startsWith("\\\\?\\")) cwd = cwd.substring(4);

    const classification = classifyThreadDiscovery(thread, name, cwd);
    discoveryRows.push({
      id: tid,
      name,
      title: thread.title || "",
      cwd: cwd || "",
      source: thread.source || null,
      sourceKind: classification.sourceKind || null,
      threadSource: thread.thread_source || "codex",
      nodeName: thread.nodeName || config.nodeName,
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
      if (skippedThreadReasons) {
        const previous = skippedThreadReasons.get(tid);
        if (previous !== errMsg) {
          log("sync.agent.classified_non_approved", {
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
          skippedThreadReasons.set(tid, errMsg);
        }
      }
      continue;
    }

    if (existingThreadMap.has(tid)) {
      const agent = existingThreadMap.get(tid);
      if (agent.name !== name) {
        let uniqueName = name;
        let counter = 1;
        const currentNames = new Set(listAgents(config).map((a) => a.name));
        currentNames.delete(agent.name);
        while (currentNames.has(uniqueName)) {
          counter += 1;
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
              log("sync.agent.renamed.delete-old", { oldName: agent.name, newName: uniqueName, threadId: tid });
            }
            upsertAgent(config, {
              name: uniqueName,
              node: thread.nodeName || agent.node || config.nodeName,
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
            if (threadToAgent) threadToAgent.set(tid, uniqueName);
            log("sync.agent.renamed.created-new", { oldName: agent.name, newName: uniqueName, threadId: tid });
            continue;
          } catch (error) {
            log("sync.agent.rename.failed", { threadId: tid, oldName: agent.name, newName: uniqueName, error: error.message });
          }
        }
      }

      if (threadToAgent) threadToAgent.set(tid, agent.name);
      setAgent(config, agent.name, {
        node: thread.nodeName || agent.node || config.nodeName,
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
    const currentNames = new Set(listAgents(config).map((a) => a.name));
    while (currentNames.has(uniqueName)) {
      counter += 1;
      uniqueName = `${name}-${counter}`;
    }

    try {
      const agent = upsertAgent(config, {
        name: uniqueName,
        node: thread.nodeName || config.nodeName,
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
      if (threadToAgent) threadToAgent.set(tid, uniqueName);
      appendEvent("agent.created", { agent });
      log("sync.agent.created", { name: uniqueName, threadId: tid, cwd });
    } catch (error) {
      log("sync.agent.create.failed", { threadId: tid, error: error.message });
    }
  }

  removeNonApprovedLocalAgents(config, discoveryRows, threadToAgent, log);
  const localDiscoveries = saveLocalDiscoveries(config, discoveryRows, "native-thread-discovery");
  log("sync.discovery.classified", { counts: localDiscoveries.counts });
  if (skippedThreadReasons) {
    log("sync.agent.prune.skipped", { reason: "discovery is additive to avoid deleting active approved local agents", skippedThreads: skippedThreadReasons.size });
  }
  return localDiscoveries;
}
