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
const threadDiscovery = read("src/thread-discovery.js");
const registry = read("src/registry.js");
const staleInstallAudit = read("scripts/assert-no-stale-installed-cam.ps1");

const checks = [
  ["package version is 2.1.31", pkg.version === "2.1.31"],
  ["daemon exposes CAM_VERSION 2.1.31", daemon.includes('const CAM_VERSION = "2.1.31";')],
  ["daemon health includes version", daemon.includes("version: CAM_VERSION")],
  ["daemon supports strict thread-not-found detection", daemon.includes("STRICT_THREAD_NOT_FOUND")],
  ["daemon strict send does not queue unresolved targets", daemon.includes("strict send cannot deliver") && daemon.includes("message.failed.strict")],
  ["daemon repairs stale thread once", daemon.includes("#repairStaleThreadAndEnsure")],
  ["daemon carries messageType", daemon.includes("messageType")],
  ["daemon records GUI test reply as received", daemon.includes("gui_test.reply.received") && daemon.includes('message.delivery = "received"')],
  ["daemon writes GUI test ledger", daemon.includes("appendTestEvent") && daemon.includes("outbound_delivered") && daemon.includes("reply_received")],
  ["daemon marks thread send delivered after turn id", daemon.includes('message.delivery = "delivered"') && daemon.includes('appendEvent("message.delivered", message)')],
  ["registry persists route metadata", registry.includes("sourceHost") && registry.includes("hostKind") && registry.includes("transport") && registry.includes("route")],
  ["native discovery emits route metadata", threadDiscovery.includes("inferRouteMetadata") && threadDiscovery.includes("remote-projects")],
  ["native discovery merges session index, state, and rollout sources", threadDiscovery.includes("loadSessionIndex") && threadDiscovery.includes("indexRollouts") && threadDiscovery.includes("discoverStateRows")],
  ["native discovery treats active sessions ahead of archived copies", threadDiscovery.includes('["sessions", 2]') && threadDiscovery.includes('["archived_sessions", 1]') && threadDiscovery.includes("info.rank !== 2")],
  ["daemon discovery does not launch Python", !daemon.includes('execFile(cmd, [scriptPath]') && !daemon.includes('tryPython("python")') && daemon.includes("discoverThreads()")],
  ["GUI active classifier does not launch Python", !gui.includes('psi.FileName = "python"') && !gui.includes("FindQueryThreadsScript") && gui.includes("source=daemon-registry")],
  ["installer does not ship query_threads.py", !installer.includes("query_threads.py")],
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
  ["installer deletes runtime map on reinstall", installer.includes("ResetCamRuntimeStateForInstall") && installer.includes("DeleteIfExists(CamHome + '\\agents.json')")],
  ["installer uses valid USERPROFILE env constant", installer.includes("ExpandConstant('{%USERPROFILE}\\.qexow-cam')") && !installer.includes("{userprofile}")],
  ["uninstaller removes all CAM local state", installer.includes('Type: filesandordirs; Name: "{%USERPROFILE}\\.qexow-cam"')],
  ["installer removes old per-user Qexow CAM install", installer.includes("{localappdata}\\Programs\\Qexow CAM") && installer.includes("RemoveDirIfExists")],
  ["installer has no PowerShell cleanup path", !installer.includes("powershell.exe") && !installer.includes("RunPreinstallCleanupPowerShell")],
  ["runtime and installer have no Python discovery payload", !daemon.includes("query_threads.py") && !gui.includes("query_threads.py") && !installer.includes("query_threads.py")],
  ["postinstall stale install auditor exists", staleInstallAudit.includes("stale CAM executable remains") && staleInstallAudit.includes("stale process")],
  ["installer app version matches package", installer.includes(`AppVersion=${pkg.version}`)],
  ["GUI version matches package", gui.includes(`get { return "${pkg.version}"; }`)],
  ["release workflow smoke tests installer", workflow.includes("Smoke test installer") && workflow.includes("Installation process succeeded")],
  ["release workflow tests stale per-user cleanup", workflow.includes("Programs\\Qexow CAM") && workflow.includes("assert-no-stale-installed-cam.ps1")],
  ["release workflow tests reinstall map deletion", workflow.includes("Reinstall did not remove stale agents.json map") && workflow.includes("tests.jsonl")],
  ["release workflow tests uninstall full CAM home deletion", workflow.includes("Uninstall did not remove CAM home") && workflow.includes("unins000.exe")],
];

const failed = checks.filter(([, ok]) => !ok);
for (const [label, ok] of checks) {
  console.log(`${ok ? "PASS" : "FAIL"} ${label}`);
}
if (failed.length) {
  process.exitCode = 1;
}
