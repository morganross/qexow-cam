const IPV4_RE = /\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)\b/g;

export const NODE_DISCOVERY_REQUEST_TYPE = "cam-node-discovery-request";
export const NODE_DISCOVERY_REPLY_TYPE = "cam-node-discovery-reply";
export const NODE_DISCOVERY_MARKER = "CAM_NODE_DISCOVERY";
export const NODE_DISCOVERY_END_MARKER = "END_CAM_NODE_DISCOVERY";

function toArray(value) {
  if (Array.isArray(value)) return value.filter(Boolean).map(String);
  if (typeof value === "string") return value.split(/[,\s]+/).map((part) => part.trim()).filter(Boolean);
  return [];
}

function unique(values) {
  return [...new Set(values.filter(Boolean))];
}

function normalizeFieldMap(input) {
  const raw = input || {};
  const privateIps = unique([
    ...toArray(raw.privateIps),
    ...toArray(raw.private_ips),
    ...toArray(raw.hostnameI),
    ...toArray(raw["hostname -I"]),
  ]);
  const publicIp = firstIpv4(raw.publicIp || raw.public_ip || raw.publicIpV4 || raw.public_ip_v4 || "");
  return {
    peerName: raw.peerName || raw.peer_name || null,
    hostname: raw.hostname || null,
    whoami: raw.whoami || null,
    privateIps,
    publicIp,
    camNodeName: raw.camNodeName || raw.cam_node_name || raw.nodeName || raw.node_name || null,
    camBindHost: raw.camBindHost || raw.cam_bind_host || null,
    camPort: normalizePort(raw.camPort || raw.cam_port),
    camConfigPath: raw.camConfigPath || raw.cam_config_path || null,
    camRegistryPath: raw.camRegistryPath || raw.cam_registry_path || null,
    camRoot: raw.camRoot || raw.cam_root || null,
    camOk: normalizeBool(raw.camOk ?? raw.cam_ok),
  };
}

function normalizePort(value) {
  if (value === null || value === undefined || value === "") return null;
  const num = Number(value);
  return Number.isFinite(num) ? num : null;
}

function normalizeBool(value) {
  if (value === null || value === undefined || value === "") return null;
  if (typeof value === "boolean") return value;
  const text = String(value).trim().toLowerCase();
  if (["true", "1", "yes", "ok"].includes(text)) return true;
  if (["false", "0", "no"].includes(text)) return false;
  return null;
}

function firstIpv4(text) {
  const match = String(text || "").match(IPV4_RE);
  return match?.[0] || null;
}

function extractJsonCandidate(text) {
  const source = String(text || "");
  const fenced = source.match(/```json\s*([\s\S]*?)```/i) || source.match(/```\s*([\s\S]*?)```/i);
  const candidates = [];
  if (fenced?.[1]) candidates.push(fenced[1].trim());
  const firstBrace = source.indexOf("{");
  const lastBrace = source.lastIndexOf("}");
  if (firstBrace >= 0 && lastBrace > firstBrace) candidates.push(source.slice(firstBrace, lastBrace + 1));
  for (const candidate of candidates) {
    try {
      return JSON.parse(candidate);
    } catch {
      // Try next candidate.
    }
  }
  return null;
}

function extractKeyValueBlock(text) {
  const source = String(text || "");
  const markerIndex = source.indexOf(NODE_DISCOVERY_MARKER);
  if (markerIndex < 0) return null;
  const endIndex = source.indexOf(NODE_DISCOVERY_END_MARKER, markerIndex);
  const block = source.slice(markerIndex, endIndex >= 0 ? endIndex : undefined);
  const lines = block.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const result = {};
  for (const line of lines) {
    if (line === NODE_DISCOVERY_MARKER || line === NODE_DISCOVERY_END_MARKER) continue;
    const eq = line.indexOf("=");
    const colon = line.indexOf(":");
    let split = -1;
    if (eq >= 0) split = eq;
    else if (colon >= 0) split = colon;
    if (split < 0) continue;
    const key = line.slice(0, split).trim();
    const value = line.slice(split + 1).trim();
    if (!key) continue;
    result[key] = value;
  }
  return Object.keys(result).length ? result : null;
}

export function parseNodeDiscoveryEvidence(text) {
  const json = extractJsonCandidate(text);
  if (json) {
    const normalized = normalizeFieldMap(json);
    if (normalized.hostname || normalized.publicIp || normalized.privateIps.length || normalized.camNodeName) {
      return { ok: true, mode: "json", data: normalized };
    }
  }

  const kv = extractKeyValueBlock(text);
  if (kv) {
    const normalized = normalizeFieldMap(kv);
    if (normalized.hostname || normalized.publicIp || normalized.privateIps.length || normalized.camNodeName) {
      return { ok: true, mode: "key-value", data: normalized };
    }
  }

  return { ok: false, mode: null, data: null };
}

export function buildNodeDiscoveryPrompt({ peerName }) {
  return [
    `Run a node discovery check for peer "${peerName}".`,
    "",
    "On the remote node, run local identity commands and reply in BOTH ways:",
    "1. Send a CAM reply with messageType cam-node-discovery-reply and the same correlationId.",
    `2. Paste the literal discovery block into this chat so the sender can verify raw command output behind your back.`,
    "",
    "Use this exact output shape in both places:",
    NODE_DISCOVERY_MARKER,
    `peer_name=${peerName}`,
    "hostname=<hostname output>",
    "whoami=<whoami output>",
    "private_ips=<hostname -I output>",
    "public_ip=<public IPv4 output or blank>",
    "cam_node_name=<CAM node name if known>",
    "cam_bind_host=<CAM bind host if known>",
    "cam_port=<CAM port if known>",
    "cam_config_path=<config path if known>",
    "cam_registry_path=<registry path if known>",
    "cam_root=<CAM home/root if known>",
    "cam_ok=<true|false if CAM is installed and readable>",
    NODE_DISCOVERY_END_MARKER,
    "",
    "If you can also provide JSON in the CAM reply body, do that too, but keep the literal block above in chat.",
    "Do not summarize. Do not paraphrase command output.",
  ].join("\n");
}

