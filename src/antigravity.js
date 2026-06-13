import fs from "node:fs";
import path from "node:path";
import os from "node:os";

const BRAIN_DIR = path.join(os.homedir(), ".gemini", "antigravity", "brain");

// Bootstrap / Auto-Discovery / OAuth Phase
export function bootstrapAntigravity(logFunc) {
  logFunc("bootstrap.antigravity.start", { message: "Verifying Codex and Antigravity Environments..." });

  logFunc("bootstrap.antigravity.info", {
    message: "External CLI probing is disabled. CAM only installs local messaging skill metadata.",
  });
  installAntigravitySkills(logFunc);
  installCodexSkills(logFunc);

  logFunc("bootstrap.antigravity.complete", { message: "Environment Verification Complete" });
}

function installAntigravitySkills(logFunc) {
  const skillsDir = path.join(os.homedir(), ".gemini", "antigravity", "skills");
  const camSkillDir = path.join(skillsDir, "qexow-cam-messaging");
  const camExe = process.env.CAM_NODE_EXE || process.execPath;
  
  if (!fs.existsSync(camSkillDir)) {
    fs.mkdirSync(camSkillDir, { recursive: true });
  }

  const skillDef = {
    name: "cam_send_message",
    description: `Send a message to another agent via the Qexow CAM (CAM) protocol. Use this to respond to incoming requests from other agents. This basic Antigravity send skill cannot satisfy strict CAM GUI diagnostics unless it can preserve correlationId and messageType cam-gui-test-reply. Boss Agents: If you are a Boss agent, please read the rules of engagement at: ${path.join(os.homedir(), ".qexow-cam", "boss.md")}`,
    entrypoint: `"${camExe}" send "{{TargetAgent}}" "{{MessageText}}" --from antigravity`,
    parameters: {
      type: "object",
      properties: {
        TargetAgent: { type: "string", description: "The name of the target agent to send the message to." },
        MessageText: { type: "string", description: "The text body of the message." }
      },
      required: ["TargetAgent", "MessageText"]
    }
  };

  fs.writeFileSync(path.join(camSkillDir, "skill.json"), JSON.stringify(skillDef, null, 2), "utf8");
  logFunc("bootstrap.antigravity.skill", { message: `Skill 'cam_send_message' successfully installed at ${camSkillDir}` });

  // Install Check Inbox Skill
  const inboxSkillDir = path.join(skillsDir, "qexow-cam-inbox");
  if (!fs.existsSync(inboxSkillDir)) {
    fs.mkdirSync(inboxSkillDir, { recursive: true });
  }

  const inboxSkillDef = {
    name: "cam_check_inbox",
    description: "Check your Qexow CAM inbox for any pending messages from other agents. Set WaitSeconds to block and wait for a response if none are currently available.",
    entrypoint: `"${camExe}" inbox antigravity --wait {{WaitSeconds}}`,
    parameters: {
      type: "object",
      properties: {
        WaitSeconds: { type: "integer", description: "Optional. Number of seconds to block and wait for a message if the inbox is currently empty (up to 30). Defaults to 20." }
      },
      required: []
    }
  };

  fs.writeFileSync(path.join(inboxSkillDir, "skill.json"), JSON.stringify(inboxSkillDef, null, 2), "utf8");
  logFunc("bootstrap.antigravity.skill", { message: `Skill 'cam_check_inbox' successfully installed at ${inboxSkillDir}` });

  // Install Eavesdrop Skill
  const eavesdropSkillDir = path.join(skillsDir, "qexow-cam-eavesdrop");
  if (!fs.existsSync(eavesdropSkillDir)) {
    fs.mkdirSync(eavesdropSkillDir, { recursive: true });
  }

  const eavesdropSkillDef = {
    name: "cam_eavesdrop",
    description: "Look back over the shoulder of another agent and retrieve their most recent execution history. This will show you exactly what they thought, the tools they executed, and the tool outputs. Use this to review their progress.",
    entrypoint: `"${camExe}" agent read "{{TargetAgent}}" --turns {{Turns}}`,
    parameters: {
      type: "object",
      properties: {
        TargetAgent: { type: "string", description: "The name of the agent to eavesdrop on." },
        Turns: { type: "integer", description: "The number of recent turns to retrieve. Defaults to 5." }
      },
      required: ["TargetAgent"]
    }
  };

  fs.writeFileSync(path.join(eavesdropSkillDir, "skill.json"), JSON.stringify(eavesdropSkillDef, null, 2), "utf8");
  logFunc("bootstrap.antigravity.skill", { message: `Skill 'cam_eavesdrop' successfully installed at ${eavesdropSkillDir}` });
}

function installCodexSkills(logFunc) {
  const skillsDir = path.join(os.homedir(), ".codex", "skills");
  const camSkillDir = path.join(skillsDir, "qexow-cam-messaging");

  if (!fs.existsSync(camSkillDir)) {
    fs.mkdirSync(camSkillDir, { recursive: true });
  }

  const camDir = path.join(os.homedir(), ".qexow-cam");
  const destBossMd = path.join(camDir, "boss.md");
  const srcBossMd = path.join(process.cwd(), "boss.md");
  if (fs.existsSync(srcBossMd)) {
    if (!fs.existsSync(camDir)) fs.mkdirSync(camDir, { recursive: true });
    fs.copyFileSync(srcBossMd, destBossMd);
  }

  const skillMd = `---
name: qexow-cam-messaging
description: Send and receive messages to/from other agents using the Qexow CAM protocol.
---
# Instructions

You are connected to the Qexow CAM messaging fabric. Communicate through the local Qexow CAM daemon only.

> **Boss Agents:** If you are a Boss agent, please read the rules of engagement at:
> \`${destBossMd}\`

## Sending a Message
To send a message to another agent, use the installed \`scripts/Send-AgentMessage.ps1\` helper from this skill directory. It posts to the loopback Qexow CAM daemon using the local token and does not use retired \`codex-agent-manager\` paths.

**Example PowerShell call:**
\`\`\`powershell
& "$env:USERPROFILE\\.codex\\skills\\qexow-cam-messaging\\scripts\\Send-AgentMessage.ps1" -TargetAgent "operator" -MessageText "Hello" -SourceAgent "coder-bot"
\`\`\`

When replying to a CAM GUI diagnostic test, send a CAM message back to the requested target mailbox and preserve the incoming \`correlationId\`. Use \`messageType "cam-gui-test-reply"\` when the incoming message asks for it. Chat-only replies do not pass the GUI diagnostic. A valid GUI diagnostic reply must be accepted by the daemon as \`delivery: "received"\`.

**Example diagnostic reply:**
\`\`\`powershell
& "$env:USERPROFILE\\.codex\\skills\\qexow-cam-messaging\\scripts\\Send-AgentMessage.ps1" -TargetAgent "CAM test, Kexau CAM test suite mailbox" -MessageText "CAM_GUI_TEST_RESPONSE <testId>. Agent: coder-bot. Node: RyzenLaptop. Status: idle." -SourceAgent "coder-bot" -CorrelationId "<testId>" -MessageType "cam-gui-test-reply"
\`\`\`

## Checking Your Inbox
To check for incoming messages, use the installed \`scripts/Check-AgentMessages.ps1\` helper from this skill directory.

**Example CLI call:**
\`\`\`powershell
& "$env:USERPROFILE\\.codex\\skills\\qexow-cam-messaging\\scripts\\Check-AgentMessages.ps1" -AgentName "coder-bot" -WaitSeconds 15
\`\`\`
`;

  fs.writeFileSync(path.join(camSkillDir, "SKILL.md"), skillMd.trim(), "utf8");
  logFunc("bootstrap.antigravity.skill", { message: `Codex global CAM skills successfully installed/updated at ${camSkillDir}` });
}
