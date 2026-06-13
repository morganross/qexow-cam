#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const read = (file) => fs.readFileSync(path.join(root, file), "utf8");
const pkg = JSON.parse(read("package.json"));
const daemon = read("src/daemon.js");
const cli = read("src/cli.js");
const antigravity = read("src/antigravity.js");
const gui = read("src/windows/QexowCamGui.cs");
const installer = read("installer.iss");
const workflow = read(".github/workflows/release.yml");
const queryThreads = read("src/query_threads.py");
const registry = read("src/registry.js");

const checks = [
  ["package version is 2.1.26", pkg.version === "2.1.26"],
  ["daemon exposes CAM_VERSION 2.1.26", daemon.includes('const CAM_VERSION = "2.1.26";')],
  ["daemon health includes version", daemon.includes("version: CAM_VERSION")],
  ["daemon supports strict thread-not-found detection", daemon.includes("STRICT_THREAD_NOT_FOUND")],
  ["daemon strict send does not queue unresolved targets", daemon.includes("strict send cannot deliver") && daemon.includes("message.failed.strict")],
  ["daemon repairs stale thread once", daemon.includes("#repairStaleThreadAndEnsure")],
  ["daemon carries messageType", daemon.includes("messageType")],
  ["daemon records GUI test reply as received", daemon.includes("gui_test.reply.received") && daemon.includes('message.delivery = "received"')],
  ["daemon writes GUI test ledger", daemon.includes("appendTestEvent") && daemon.includes("outbound_delivered") && daemon.includes("reply_received")],
  ["daemon marks thread send delivered after turn id", daemon.includes('message.delivery = "delivered"') && daemon.includes('appendEvent("message.delivered", message)')],
  ["registry persists route metadata", registry.includes("sourceHost") && registry.includes("hostKind") && registry.includes("transport") && registry.includes("route")],
  ["query discovery emits route metadata", queryThreads.includes("infer_route_metadata") && queryThreads.includes("remote-projects")],
  ["CLI can send correlation id", cli.includes("opts.correlationId") && cli.includes("payload.correlationId")],
  ["CLI can send message type", cli.includes("opts.messageType") && cli.includes("payload.messageType")],
  ["generated skill documents diagnostic reply pattern", antigravity.includes("cam-gui-test-reply") && antigravity.includes("-CorrelationId")],
  ["GUI sends strict diagnostic payload", gui.includes('payload["strict"] = true;')],
  ["GUI requires cam-gui-test-reply messageType when present", gui.includes('"cam-gui-test-reply"')],
  ["GUI validates strict send before polling", gui.includes("ValidateStrictSend(sendResult);")],
  ["GUI rejects non-received replies", gui.includes('!String.Equals(delivery, "received"') && gui.includes("STATE reply-queued-only")],
  ["GUI filters old mailbox replies by timestamp", gui.includes("IsCurrentTestTimestamp")],
  ["GUI exposes route/source/testable columns", gui.includes('"route"') && gui.includes('"source"') && gui.includes('"testable"')],
  ["GUI exact selected-agent source match remains enforced", gui.includes("!String.Equals(sourceAgent, expectedAgentName, StringComparison.OrdinalIgnoreCase)")],
  ["GUI exact mailbox target match remains enforced", gui.includes("String.Equals(targetAgent, CamTestMailboxAgent, StringComparison.Ordinal)")],
  ["GUI blocks stale/unbound preflight", gui.includes('String.Equals(status, "stale"') && gui.includes('String.Equals(status, "unbound"')],
  ["installer rotates volatile CAM state", installer.includes("ResetVolatileCamState") && installer.includes("install-backups")],
  ["installer uses valid USERPROFILE env constant", installer.includes("ExpandConstant('{%USERPROFILE}\\.qexow-cam')") && !installer.includes("{userprofile}")],
  ["installer preserves durable state comment", installer.includes("Preserve durable config/secrets/boss notes")],
  ["installer app version matches package", installer.includes(`AppVersion=${pkg.version}`)],
  ["GUI version matches package", gui.includes(`get { return "${pkg.version}"; }`)],
  ["release workflow smoke tests installer", workflow.includes("Smoke test installer") && workflow.includes("Installation process succeeded")],
];

const failed = checks.filter(([, ok]) => !ok);
for (const [label, ok] of checks) {
  console.log(`${ok ? "PASS" : "FAIL"} ${label}`);
}
if (failed.length) {
  process.exitCode = 1;
}
