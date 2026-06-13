import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const IPV4_RE = /\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)\b/g;
const SSH_AT_IP_RE = /\b([a-z_][a-z0-9_-]*)@((?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d))\b/ig;
const KEY_PATH_RE = /(?:[A-Za-z]:\\[^`"'|\r\n]+?\.(?:pem|ppk|key)|\/[A-Za-z0-9._/\-]+?\.(?:pem|ppk|key))/g;
const USER_LINE_RE = /(?:^|\b)(?:OS user|User)\s*:\s*`?([a-z_][a-z0-9_-]*)`?/i;
const CODE_FENCE_RE = /^```/;
const SKIP_DIR_RE = /^(?:node_modules|dist|build|coverage|vendor|tmp|temp|out|release|releases)$/i;
const PEER_NAME_ALIASES = new Map([
  ["frontend-dev-frontend", "frontend"],
  ["backend-dev-backend", "backend"],
  ["production-frontend", "prod-frontend"],
  ["production-backend", "prod-backend"],
  ["copilotkit-assistant", "copilotkit"],
  ["racknerd-vps", "racknerd-vpn-codex"],
  ["racknerd-vps-webmail", "racknerd-vpn-codex"],
]);

function normalizeName(value) {
  return String(value || "")
    .toLowerCase()
    .replace(/[`"'()]/g, "")
    .replace(/[^a-z0-9\s/-]/g, "")
    .trim()
    .replace(/[\s/]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function unique(values) {
  return [...new Set((values || []).filter(Boolean))];
}

function normalizePeerName(value) {
  const normalized = normalizeName(value);
  return PEER_NAME_ALIASES.get(normalized) || normalized;
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

function candidateRoots(codexHome) {
  const state = readJsonSafe(path.join(codexHome, ".codex-global-state.json"), {});
  const roots = [
    ...stateValue(state, "active-workspace-roots", []),
    ...stateValue(state, "electron-saved-workspace-roots", []),
    path.join(os.homedir(), "APICostX.com-PRIVATE-docs"),
  ].filter(Boolean);
  return unique(roots.map((root) => path.resolve(String(root))));
}

function walkMarkdownFiles(root, out = [], depth = 0) {
  if (!root || !fs.existsSync(root) || depth > 5) return out;
  let entries = [];
  try {
    entries = fs.readdirSync(root, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const entry of entries) {
    if (entry.name.startsWith(".git")) continue;
    if (/archive/i.test(entry.name)) continue;
    if (SKIP_DIR_RE.test(entry.name)) continue;
    const full = path.join(root, entry.name);
    if (entry.isDirectory()) {
      walkMarkdownFiles(full, out, depth + 1);
      continue;
    }
    if (!entry.isFile()) continue;
    if (/\.md$/i.test(entry.name)) out.push(full);
  }
  return out;
}

function extractIpv4s(text) {
  return unique(String(text || "").match(IPV4_RE) || []);
}

function extractKeyPaths(text) {
  return unique((String(text || "").match(KEY_PATH_RE) || []).map((value) => value.trim()));
}

function scanSection(sectionName, lines, file) {
  const normalized = normalizePeerName(sectionName);
  if (!normalized) return null;
  const ips = [];
  const privateIps = [];
  const sshTargets = [];
  const usernames = [];
  const hostnames = [];
  const sourceLines = [];
  let inFence = false;

  for (const raw of lines) {
    const line = String(raw || "");
    if (CODE_FENCE_RE.test(line.trim())) {
      inFence = !inFence;
    }
    if (/plugin secret|password/i.test(line)) {
      // We intentionally do not scrape key material or credential lines.
      continue;
    }
    const ipMatches = extractIpv4s(line);
    if (ipMatches.length) {
      ips.push(...ipMatches);
      sourceLines.push(line.trim());
      if (/private ip/i.test(line) || (inFence && /hostname -I|10\./i.test(line))) {
        privateIps.push(...ipMatches);
      }
    }
    const sshMatch = [...line.matchAll(SSH_AT_IP_RE)];
    for (const match of sshMatch) {
      sshTargets.push(`${match[1]}@${match[2]}`);
      usernames.push(match[1]);
      ips.push(match[2]);
      sourceLines.push(line.trim());
    }
    const userMatch = line.match(USER_LINE_RE);
    if (userMatch?.[1]) usernames.push(userMatch[1].trim());
    const hostMatch = line.match(/Hostname:\s*`?([^`\r\n]+)`?/i);
    if (hostMatch?.[1]) hostnames.push(hostMatch[1].trim());
  }

  if (!ips.length && !sshTargets.length && !hostnames.length && !usernames.length) return null;
  return {
    peerName: normalized,
    file,
    displaySection: sectionName,
    ips: unique(ips),
    privateIps: unique(privateIps),
    sshTargets: unique(sshTargets),
    usernames: unique(usernames),
    hostnames: unique(hostnames),
    sourceLines: unique(sourceLines).slice(0, 20),
  };
}

function splitTableRow(line) {
  if (!String(line || "").trim().startsWith("|")) return [];
  const parts = String(line)
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((part) => part.trim());
  return parts;
}

function parseNodeInventoryTable(lines, file) {
  const tables = [];
  for (let index = 0; index < lines.length; index++) {
    if (!String(lines[index] || "").trim().startsWith("|")) continue;
    const headerCells = splitTableRow(lines[index]);
    const separatorCells = splitTableRow(lines[index + 1] || "");
    if (!headerCells.length || headerCells.length < 2) continue;
    if (!separatorCells.length || !separatorCells.every((cell) => /^:?-{2,}:?$/.test(cell))) continue;
    const headerMap = headerCells.map((cell) => normalizeName(cell));
    const nodeIndex = headerMap.indexOf("node");
    if (nodeIndex < 0) continue;
    index += 2;
    while (index < lines.length && String(lines[index] || "").trim().startsWith("|")) {
      const rowCells = splitTableRow(lines[index]);
      if (rowCells.length < headerCells.length) {
        index += 1;
        continue;
      }
      const row = {};
      for (let cellIndex = 0; cellIndex < headerCells.length; cellIndex++) {
        row[headerCells[cellIndex]] = rowCells[cellIndex] || "";
      }
      const sectionName = row[headerCells[nodeIndex]] || "";
      const synthesizedLines = Object.entries(row).map(([key, value]) => `${key}: ${value}`);
      const scanned = scanSection(sectionName, synthesizedLines, file);
      if (scanned) tables.push(scanned);
      index += 1;
    }
    index -= 1;
  }
  return tables;
}

function parseMarkdownFile(file) {
  let text = "";
  try {
    text = fs.readFileSync(file, "utf8");
  } catch {
    return [];
  }
  const lines = text.split(/\r?\n/);
  const sections = [];
  let currentName = null;
  let currentLines = [];
  for (const line of lines) {
    const heading = line.match(/^#{2,}\s+(.+?)\s*$/);
    if (heading) {
      if (currentName) {
        if (normalizeName(currentName) === "node-inventory") {
          sections.push(...parseNodeInventoryTable(currentLines, file));
        } else {
          const scanned = scanSection(currentName, currentLines, file);
          if (scanned) sections.push(scanned);
        }
      }
      currentName = heading[1];
      currentLines = [];
      continue;
    }
    if (currentName) currentLines.push(line);
  }
  if (currentName) {
    if (normalizeName(currentName) === "node-inventory") {
      sections.push(...parseNodeInventoryTable(currentLines, file));
    } else {
      const scanned = scanSection(currentName, currentLines, file);
      if (scanned) sections.push(scanned);
    }
  }
  return sections;
}

export function discoverPeerFactsFromMarkdown({ codexHome }) {
  const roots = candidateRoots(codexHome);
  const docs = [];
  for (const root of roots) {
    for (const file of walkMarkdownFiles(root)) {
      docs.push(...parseMarkdownFile(file));
    }
  }
  const byPeer = new Map();
  for (const row of docs) {
    const existing = byPeer.get(row.peerName) || {
      peerName: row.peerName,
      files: [],
      displaySections: [],
      candidateIps: [],
      candidatePrivateIps: [],
      candidateSshTargets: [],
      candidateUsernames: [],
      candidateHostnames: [],
      sourceLines: [],
    };
    existing.files.push(row.file);
    existing.displaySections.push(row.displaySection);
    existing.candidateIps.push(...row.ips);
    existing.candidatePrivateIps.push(...row.privateIps);
    existing.candidateSshTargets.push(...row.sshTargets);
    existing.candidateUsernames.push(...row.usernames);
    existing.candidateHostnames.push(...row.hostnames);
    existing.sourceLines.push(...row.sourceLines);
    byPeer.set(row.peerName, existing);
  }
  return [...byPeer.values()].map((row) => ({
    ...row,
    files: unique(row.files),
    displaySections: unique(row.displaySections),
    candidateIps: unique(row.candidateIps),
    candidatePrivateIps: unique(row.candidatePrivateIps),
    candidateSshTargets: unique(row.candidateSshTargets),
    candidateUsernames: unique(row.candidateUsernames),
    candidateHostnames: unique(row.candidateHostnames),
    sourceLines: unique(row.sourceLines).slice(0, 20),
  }));
}

export function discoverSshKeyPathsFromMarkdown({ codexHome }) {
  const roots = candidateRoots(codexHome);
  const rows = [];
  for (const root of roots) {
    for (const file of walkMarkdownFiles(root)) {
      let text = "";
      try {
        text = fs.readFileSync(file, "utf8");
      } catch {
        continue;
      }
      const pathsFound = extractKeyPaths(text);
      for (const keyPath of pathsFound) {
        rows.push({
          file,
          keyPath,
        });
      }
    }
  }
  const seen = new Set();
  return rows.filter((row) => {
    const key = `${row.file}::${row.keyPath}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
