import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const SESSION_ID_RE = /([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i;

function normalizePath(value) {
  return String(value || "")
    .replace(/^\\\\\?\\/, "")
    .replace(/\//g, "\\")
    .toLowerCase()
    .trim();
}

function isInAnyWorkspace(cwd, roots) {
  if (!cwd || cwd === "outside-of-project") return false;
  const normalizedCwd = normalizePath(cwd);
  return roots.some((root) => {
    const normalizedRoot = normalizePath(root);
    return normalizedRoot && (normalizedCwd === normalizedRoot || normalizedCwd.startsWith(`${normalizedRoot}\\`));
  });
}

function readJsonSafe(file, fallback = {}) {
  try {
    if (!fs.existsSync(file)) return fallback;
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return fallback;
  }
}

function stateValue(state, key, fallback = null) {
  if (state && Object.prototype.hasOwnProperty.call(state, key)) return state[key];
  const persisted = state?.["electron-persisted-atom-state"];
  if (persisted && Object.prototype.hasOwnProperty.call(persisted, key)) return persisted[key];
  return fallback;
}

function collectIds(obj, out) {
  if (!obj) return;
  if (typeof obj === "string") {
    if (SESSION_ID_RE.test(obj) && obj.length === 36) out.add(obj);
    return;
  }
  if (Array.isArray(obj)) {
    for (const item of obj) collectIds(item, out);
    return;
  }
  if (typeof obj === "object") {
    for (const [key, value] of Object.entries(obj)) {
      if (SESSION_ID_RE.test(key) && key.length === 36) out.add(key);
      collectIds(value, out);
    }
  }
}

function loadSessionIndex(codexDir) {
  const names = new Map();
  const updated = new Map();
  const file = path.join(codexDir, "session_index.jsonl");
  if (!fs.existsSync(file)) return { names, updated };
  const lines = fs.readFileSync(file, "utf8").split(/\r?\n/).filter(Boolean);
  for (const line of lines) {
    try {
      const row = JSON.parse(line);
      if (!row.id) continue;
      if (row.thread_name) names.set(row.id, row.thread_name);
      if (row.updated_at) updated.set(row.id, row.updated_at);
    } catch {
      // Ignore one bad session-index row; discovery continues from other rows.
    }
  }
  return { names, updated };
}

function firstTextFromContent(content) {
  if (typeof content === "string") return content.trim();
  if (!Array.isArray(content)) return "";
  return content
    .map((item) => item?.text || item?.input_text || item?.output_text || "")
    .filter(Boolean)
    .join(" ")
    .trim();
}

function titleFromRollout(file) {
  try {
    const fd = fs.openSync(file, "r");
    const buffer = Buffer.alloc(512 * 1024);
    const bytes = fs.readSync(fd, buffer, 0, buffer.length, 0);
    fs.closeSync(fd);
    const lines = buffer.toString("utf8", 0, bytes).split(/\r?\n/).slice(0, 80);
    for (const line of lines) {
      if (!line) continue;
      try {
        const obj = JSON.parse(line);
        const payload = obj.payload || {};
        if (obj.type === "event_msg" && payload.type === "user_message") {
          const message = String(payload.message || "").trim();
          if (message && !message.startsWith("# AGENTS.md instructions")) return truncateTitle(message);
        }
        if (obj.type === "response_item" && payload.type === "message" && payload.role === "user") {
          const text = firstTextFromContent(payload.content);
          if (text && !text.startsWith("# AGENTS.md instructions") && !text.startsWith("<environment_context>")) {
            return truncateTitle(text);
          }
        }
      } catch {
        // Keep scanning early rollout lines.
      }
    }
  } catch {
    // Title is optional.
  }
  return "Codex Chat";
}

function truncateTitle(text) {
  return text.length > 80 ? `${text.slice(0, 80)}...` : text;
}

function readRolloutMeta(file) {
  try {
    const firstLine = fs.readFileSync(file, "utf8").split(/\r?\n/, 1)[0];
    const row = JSON.parse(firstLine);
    return row.type === "session_meta" ? (row.payload || {}) : {};
  } catch {
    return {};
  }
}

function walkJsonlFiles(rootDir, out = []) {
  if (!fs.existsSync(rootDir)) return out;
  for (const entry of fs.readdirSync(rootDir, { withFileTypes: true })) {
    const full = path.join(rootDir, entry.name);
    if (entry.isDirectory()) walkJsonlFiles(full, out);
    else if (entry.isFile() && entry.name.startsWith("rollout-") && entry.name.endsWith(".jsonl")) out.push(full);
  }
  return out;
}

function indexRollouts(codexDir) {
  const byId = new Map();
  for (const [subdir, rank] of [["sessions", 2], ["archived_sessions", 1]]) {
    for (const file of walkJsonlFiles(path.join(codexDir, subdir))) {
      const meta = readRolloutMeta(file);
      const match = SESSION_ID_RE.exec(path.basename(file));
      const id = meta.id || match?.[1];
      if (!id) continue;
      const current = { file, rank, mtime: fs.statSync(file).mtimeMs, meta };
      const previous = byId.get(id);
      if (!previous || rank > previous.rank || (rank === previous.rank && current.mtime >= previous.mtime)) {
        byId.set(id, current);
      }
    }
  }
  return byId;
}

function remoteConnectionAliases(state) {
  const aliases = new Map();
  for (const conn of stateValue(state, "codex-managed-remote-connections", []) || []) {
    const hostId = conn.hostId || "";
    const alias = conn.alias || conn.displayName || hostId.replace("remote-ssh-discovered:", "");
    if (hostId && alias) aliases.set(hostId, alias);
  }
  return aliases;
}

function inferRouteMetadata(cwd, threadSource, state) {
  const localNode = process.env.CAM_NODE_NAME || process.env.COMPUTERNAME || process.env.HOSTNAME || "local";
  const metadata = {
    nodeName: localNode,
    sourceHost: "local",
    hostKind: "local",
    transport: "local",
    route: "local",
  };
  if (threadSource === "antigravity") {
    return { ...metadata, transport: "antigravity", route: "antigravity-local" };
  }

  const aliases = remoteConnectionAliases(state);
  const normalized = String(cwd || "").replace(/^\\\\\?\\/, "").replace(/\\/g, "/");
  for (const project of stateValue(state, "remote-projects", []) || []) {
    const remotePath = String(project.remotePath || "").replace(/\/+$/, "");
    const hostId = project.hostId || "";
    const alias = aliases.get(hostId) || hostId.replace("remote-ssh-discovered:", "");
    if (remotePath && alias && normalized.startsWith(`${remotePath}/`)) {
      return {
        nodeName: alias,
        sourceHost: alias,
        hostKind: "remote",
        transport: "codex-managed",
        route: `codex-managed:${alias}`,
      };
    }
  }
  if (normalized.startsWith("/home/") || normalized.startsWith("/root/") || normalized.startsWith("/opt/")) {
    const selected = stateValue(state, "selected-remote-host-id", "") || "";
    const alias = aliases.get(selected) || selected.replace("remote-ssh-discovered:", "") || "remote";
    return {
      nodeName: alias,
      sourceHost: alias,
      hostKind: "remote",
      transport: "codex-managed",
      route: `codex-managed:${alias}`,
    };
  }
  return metadata;
}

function rowFromRollout(id, info, names, updated, state, source) {
  const meta = info.meta || {};
  if (meta.thread_source && meta.thread_source !== "user") return null;
  const cwd = meta.cwd || "outside-of-project";
  return {
    id,
    title: names.get(id) || titleFromRollout(info.file),
    agent_nickname: "",
    agent_role: "",
    cwd,
    source: meta.source || source,
    codex_thread_source: meta.thread_source || "user",
    rollout_path: info.file,
    created_at: meta.timestamp || null,
    updated_at: updated.get(id) || null,
    discovery_source: source,
    thread_source: "codex",
    ...inferRouteMetadata(cwd, "codex", state),
  };
}

function discoverStateRows(state, rollouts, names, updated, rowsById) {
  const ids = new Set();
  for (const key of ["unread-thread-ids-by-host-v1", "pinned-thread-ids", "thread-workspace-root-hints"]) {
    collectIds(stateValue(state, key, {}), ids);
  }
  const workspaceHints = stateValue(state, "thread-workspace-root-hints", {}) || {};
  for (const id of ids) {
    if (rowsById.has(id)) continue;
    const info = rollouts.get(id);
    if (info) {
      if (info.rank !== 2) continue;
      const row = rowFromRollout(id, info, names, updated, state, "codex-state");
      if (row) rowsById.set(id, row);
      continue;
    }
    const cwd = workspaceHints[id] || "outside-of-project";
    rowsById.set(id, {
      id,
      title: names.get(id) || "Codex Chat",
      agent_nickname: "",
      agent_role: "",
      cwd,
      source: "codex-state",
      codex_thread_source: "user",
      created_at: null,
      updated_at: updated.get(id) || null,
      discovery_source: "codex-state",
      thread_source: "codex",
      ...inferRouteMetadata(cwd, "codex", state),
    });
  }
}

function discoverAntigravity(state) {
  const brainDir = path.join(os.homedir(), ".gemini", "antigravity", "brain");
  if (!fs.existsSync(brainDir)) return [];
  const now = Date.now();
  const rows = [];
  for (const entry of fs.readdirSync(brainDir, { withFileTypes: true })) {
    if (!entry.isDirectory() || !SESSION_ID_RE.test(entry.name)) continue;
    const transcript = path.join(brainDir, entry.name, ".system_generated", "logs", "transcript.jsonl");
    if (!fs.existsSync(transcript)) continue;
    const mtime = fs.statSync(transcript).mtimeMs;
    if (now - mtime > 7 * 86400 * 1000) continue;
    let title = "Antigravity Chat";
    try {
      const lines = fs.readFileSync(transcript, "utf8").split(/\r?\n/).slice(0, 15);
      for (const line of lines) {
        if (!line) continue;
        const row = JSON.parse(line);
        if (row.type === "USER_INPUT" && row.content) {
          title = truncateTitle(String(row.content).replace(/<[^>]+>/g, "").trim());
          break;
        }
      }
    } catch {
      // Optional title only.
    }
    rows.push({
      id: entry.name,
      title,
      agent_nickname: "",
      agent_role: "",
      cwd: "outside-of-project",
      thread_source: "antigravity",
      ...inferRouteMetadata("", "antigravity", state),
    });
  }
  return rows;
}

export function discoverThreads(options = {}) {
  const codexDir = options.codexDir || path.join(os.homedir(), ".codex");
  const state = readJsonSafe(path.join(codexDir, ".codex-global-state.json"), {});
  const activeRoots = stateValue(state, "active-workspace-roots", []) || [];
  const savedRoots = stateValue(state, "electron-saved-workspace-roots", []) || [];
  const workspaceRoots = [...new Set([...activeRoots, ...savedRoots].filter(Boolean))];
  const { names, updated } = loadSessionIndex(codexDir);
  const rollouts = indexRollouts(codexDir);
  const rowsById = new Map();

  for (const [id, info] of rollouts.entries()) {
    if (info.rank !== 2) continue;
    const row = rowFromRollout(id, info, names, updated, state, "rollout");
    if (row) rowsById.set(id, row);
  }
  discoverStateRows(state, rollouts, names, updated, rowsById);

  let rows = [...rowsById.values(), ...discoverAntigravity(state)];
  if (workspaceRoots.length) {
    rows = rows.filter((row) => row.thread_source === "antigravity" || isInAnyWorkspace(row.cwd, workspaceRoots));
  }
  return rows;
}
