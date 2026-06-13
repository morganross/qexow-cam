import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const IPV4_RE = /\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)\b/g;
const SSH_AT_IP_RE = /\b([a-z_][a-z0-9_-]*)@((?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d))\b/ig;
const CODE_FENCE_RE = /^```/;

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

function scanSection(sectionName, lines, file) {
  const normalized = normalizeName(sectionName);
  if (!normalized) return null;
  const ips = [];
  const privateIps = [];
  const sshTargets = [];
  const hostnames = [];
  const sourceLines = [];
  let inFence = false;

  for (const raw of lines) {
    const line = String(raw || "");
    if (CODE_FENCE_RE.test(line.trim())) {
      inFence = !inFence;
    }
    if (/key path|private key|authorized_keys|\.pem|plugin secret|password|username/i.test(line)) {
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
      ips.push(match[2]);
      sourceLines.push(line.trim());
    }
    const hostMatch = line.match(/Hostname:\s*`?([^`\r\n]+)`?/i);
    if (hostMatch?.[1]) hostnames.push(hostMatch[1].trim());
  }

  if (!ips.length && !sshTargets.length && !hostnames.length) return null;
  return {
    peerName: normalized,
    file,
    displaySection: sectionName,
    ips: unique(ips),
    privateIps: unique(privateIps),
    sshTargets: unique(sshTargets),
    hostnames: unique(hostnames),
    sourceLines: unique(sourceLines).slice(0, 20),
  };
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
    const heading = line.match(/^###\s+(.+?)\s*$/);
    if (heading) {
      if (currentName) {
        const scanned = scanSection(currentName, currentLines, file);
        if (scanned) sections.push(scanned);
      }
      currentName = heading[1];
      currentLines = [];
      continue;
    }
    if (currentName) currentLines.push(line);
  }
  if (currentName) {
    const scanned = scanSection(currentName, currentLines, file);
    if (scanned) sections.push(scanned);
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
      candidateHostnames: [],
      sourceLines: [],
    };
    existing.files.push(row.file);
    existing.displaySections.push(row.displaySection);
    existing.candidateIps.push(...row.ips);
    existing.candidatePrivateIps.push(...row.privateIps);
    existing.candidateSshTargets.push(...row.sshTargets);
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
    candidateHostnames: unique(row.candidateHostnames),
    sourceLines: unique(row.sourceLines).slice(0, 20),
  }));
}

