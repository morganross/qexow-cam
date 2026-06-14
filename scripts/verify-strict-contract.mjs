#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const read = (file) => fs.readFileSync(path.join(root, file), "utf8");
const pkg = JSON.parse(read("package.json"));
const daemon = read("src/daemon.js");
const cli = read("src/cli.js");
const config = read("src/config.js");
const appServer = read("src/app-server.js");
const antigravity = read("src/antigravity.js");
const gui = read("src/windows/QexowCamGui.cs");
const installer = read("installer.iss");
const workflow = read(".github/workflows/release.yml");
const threadDiscovery = read("src/thread-discovery.js");
const registry = read("src/registry.js");
const discoveryPolicy = read("src/discovery-policy.js");
const staleInstallAudit = read("scripts/assert-no-stale-installed-cam.ps1");
const uninstallBranchIndex = cli.indexOf('if (cmd === "uninstall-service")');
const serviceLogEventIndex = cli.indexOf('logEvent("cli.service.action"');
const installerShipsQueryThreads =
  installer.includes('Source: "dist\\query_threads.py"') ||
  installer.includes('Source: "query_threads.py"');

const checks = [
  ["package version is 2.1.45", pkg.version === "2.1.45"],
  ["config uses explicit default CAM port 37631", config.includes("export const DEFAULT_CAM_PORT = 37631") && config.includes("const port = configuredPort || DEFAULT_CAM_PORT")],
  ["config does not hard-fail when Windows Codex is missing", config.includes('return "codex";') && !config.includes("Codex execution path not configured")],
  ["daemon exposes CAM_VERSION 2.1.45", daemon.includes('const CAM_VERSION = "2.1.45";')],
  ["daemon health includes version", daemon.includes("version: CAM_VERSION")],
  ["app-server spawn errors are handled", appServer.includes('this.child.on("error"') && appServer.includes("app-server.spawn.error") && appServer.includes("pending.reject(error)")],
  ["daemon supports strict thread-not-found detection", daemon.includes("STRICT_THREAD_NOT_FOUND")],
  ["daemon strict send does not queue unresolved targets", daemon.includes("strict send cannot deliver") && daemon.includes("message.failed.strict")],
  ["daemon repairs stale thread once", daemon.includes("#repairStaleThreadAndEnsure")],
  ["daemon carries messageType", daemon.includes("messageType")],
  ["daemon records GUI test reply as received", daemon.includes("gui_test.reply.received") && daemon.includes('message.delivery = "received"')],
  ["daemon writes GUI test ledger", daemon.includes("appendTestEvent") && daemon.includes("outbound_delivered") && daemon.includes("reply_received")],
  ["daemon writes GUI final passed ledger", daemon.includes('url.pathname === "/tests/pass"') && daemon.includes('appendTestEvent(body.correlationId, "passed"')],
  ["daemon marks thread send delivered after turn id", daemon.includes('message.delivery = "delivered"') && daemon.includes('appendEvent("message.delivered", message)')],
  ["daemon updates lastDelivery after delivered mutation", daemon.includes('setAgent(this.config, target.name, { lastDelivery: message })')],
  ["registry persists route metadata", registry.includes("sourceHost") && registry.includes("hostKind") && registry.includes("transport") && registry.includes("route")],
  ["native discovery emits route metadata", threadDiscovery.includes("inferRouteMetadata") && threadDiscovery.includes("remote-projects")],
  ["native discovery merges session index, state, and rollout sources", threadDiscovery.includes("loadSessionIndex") && threadDiscovery.includes("indexRollouts") && threadDiscovery.includes("discoverStateRows")],
  ["native discovery treats active sessions ahead of archived copies", threadDiscovery.includes('["sessions", 2]') && threadDiscovery.includes('["archived_sessions", 1]') && threadDiscovery.includes("info.rank !== 2")],
  ["discovery policy quarantines machine subagents", discoveryPolicy.includes("machine-spawned-subagent") && discoveryPolicy.includes("source?.subagent?.thread_spawn")],
  ["discovery policy keeps temporary work out of active roster", discoveryPolicy.includes("TEMPORARY_NAME_RE") && discoveryPolicy.includes("temporary-work-title")],
  ["inventory export is schema 2 with raw discovery report", registry.includes("inventorySchema: 2") && registry.includes("approved-agents-only-plus-raw-discovery-report") && registry.includes("localDiscoveries")],
  ["daemon stores local discovery classifications", daemon.includes("saveLocalDiscoveries") && daemon.includes("sync.discovery.classified") && daemon.includes("discoveryDisposition")],
  ["daemon stores remote raw discovery snapshots", daemon.includes("saveRemoteDiscoverySnapshot") && daemon.includes("remoteRawDiscoveries") && daemon.includes("remoteInventoryDegraded")],
  ["peer sync mirrors approved agents only", daemon.includes("canonicalizeTrustedInventoryAgents(Object.values(remoteRegistry?.agents || {}))") && daemon.includes("approvedAgents: remoteAgents")],
  ["remote command prefers installed cam before legacy repo", daemon.includes("if command -v cam >/dev/null 2>&1; then cam") && daemon.includes("elif [ -x /usr/local/bin/cam ]")],
  ["GUI displays raw approved quarantined rejected peer counts", gui.includes('"raw"') && gui.includes('"approved"') && gui.includes('"quarantined"') && gui.includes('"rejected"')],
  ["daemon discovery does not launch Python", !daemon.includes('execFile(cmd, [scriptPath]') && !daemon.includes('tryPython("python")') && daemon.includes("discoverThreads()")],
  ["GUI active classifier does not launch Python", !gui.includes('psi.FileName = "python"') && !gui.includes("FindQueryThreadsScript") && gui.includes("source=daemon-registry")],
  ["installer does not ship query_threads.py", !installerShipsQueryThreads],
  ["CLI can send correlation id", cli.includes("opts.correlationId") && cli.includes("payload.correlationId")],
  ["CLI can send message type", cli.includes("opts.messageType") && cli.includes("payload.messageType")],
  ["CLI uninstall-service does not log before returning", uninstallBranchIndex >= 0 && serviceLogEventIndex >= 0 && uninstallBranchIndex < serviceLogEventIndex],
  ["generated skill documents diagnostic reply pattern", antigravity.includes("cam-gui-test-reply") && antigravity.includes("--correlation-id")],
  ["generated skill avoids PowerShell helper instructions", !antigravity.includes("Send-AgentMessage.ps1") && !antigravity.includes("Check-AgentMessages.ps1") && antigravity.includes("cam send")],
  ["daemon prompt does not offer direct CAM HTTP", !daemon.includes("send via CAM HTTP") && daemon.includes("Do not use direct CAM HTTP")],
  ["daemon GUI test prompt routes reply to mailbox target", daemon.includes('const replyTargetAgent = message.messageType === GUI_TEST_MESSAGE_TYPE') && daemon.includes('CAM_TEST_MAILBOX_AGENT') && daemon.includes('Use messageType "${GUI_TEST_REPLY_MESSAGE_TYPE}".')],
  ["GUI sends strict diagnostic payload", gui.includes('payload["strict"] = true;')],
  ["GUI asks Missouri semantic challenge", gui.includes("capital of Missouri") && gui.includes("Hello, how is your day?")],
  ["GUI requires Jefferson semantic answer", gui.includes("BodyContainsMissouriAnswer") && gui.includes("Jefferson City")],
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
  ["uninstaller removes all CAM local state", installer.includes("procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);") && installer.includes("FullWipeCamHomes();")],
  ["uninstaller force-kills known CAM processes", installer.includes("procedure KillKnownCamProcesses();") && installer.includes("KillKnownCamProcesses();")],
  ["installer has one custom postinstall startup path", installer.includes("procedure LaunchInstalledCamIfNeeded();") && installer.includes("if CurStep = ssPostInstall then begin") && installer.includes("LaunchInstalledCamIfNeeded();") && !installer.includes("[Run]")],
  ["CLI can launch daemon and wait for health", cli.includes('if (action === "launch")') && cli.includes("daemon launch did not become healthy") && cli.includes("child.unref()")],
  ["headless installer starts daemon without GUI", installer.includes("daemon launch --headless --wait-seconds 30") && installer.includes("if IsHeadlessInstall() then begin") && installer.includes("qexow-cam-gui.exe")],
  ["installer removes old per-user Qexow CAM install", installer.includes("{localappdata}\\Programs\\Qexow CAM") && installer.includes("RemoveDirIfExists")],
  ["installer has no PowerShell cleanup path", !installer.includes("powershell.exe") && !installer.includes("RunPreinstallCleanupPowerShell")],
  ["runtime and installer have no Python discovery payload", !daemon.includes("query_threads.py") && !gui.includes("query_threads.py") && !installerShipsQueryThreads],
  ["postinstall stale install auditor exists", staleInstallAudit.includes("stale CAM executable remains") && staleInstallAudit.includes("stale process")],
  ["installer app version matches package", installer.includes(`AppVersion=${pkg.version}`)],
  ["GUI version matches package", gui.includes(`get { return "${pkg.version}"; }`)],
  ["release workflow smoke tests installer", workflow.includes("Smoke test installer") && workflow.includes("Installation process succeeded")],
  ["release workflow verifies headless daemon startup", workflow.includes("Wait-CamHealth") && workflow.includes("Headless installer did not start a healthy CAM daemon") && workflow.includes("Headless install unexpectedly launched qexow-cam-gui.exe")],
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
