import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { spawn, execSync, execFile } from "node:child_process";

const SCRATCH_DIR = path.join(os.homedir(), ".gemini", "antigravity", "scratch");
const BRAIN_DIR = path.join(os.homedir(), ".gemini", "antigravity", "brain");

// Auto-Discovery of AGY Path
function resolveAgyPath() {
  const localAppData = process.env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local");
  const agyExePath = path.join(localAppData, "Programs", "Antigravity", "resources", "bin", "language_server.exe");
  if (fs.existsSync(agyExePath)) return agyExePath;
  return "language_server.exe"; // Fallback to system PATH
}

const AGY_EXE = resolveAgyPath();

// Bootstrap / Auto-Discovery / OAuth Phase
export function bootstrapAntigravity(logFunc) {
  logFunc("bootstrap.antigravity.start", { message: "Verifying Codex and Antigravity Environments..." });

  // 1. Verify Antigravity CLI (agy)
  try {
    execSync('agy --version', { stdio: 'ignore' });
  } catch (e) {
    logFunc("bootstrap.antigravity.error", { message: "Antigravity CLI ('agy') not found in PATH." });
  }

  // 2. Verify Antigravity Auth
  try {
    const agyStatus = execSync('agy status', { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
    if (agyStatus.toLowerCase().includes('unauthenticated') || agyStatus.toLowerCase().includes('login required')) {
      throw new Error("Needs login");
    }
  } catch (e) {
    logFunc("bootstrap.antigravity.warn", { message: "Antigravity OAuth missing or expired. Run 'agy login'." });
  }

  // 3. Verify Codex CLI
  try {
    execSync('codex --version', { stdio: 'ignore' });
  } catch (e) {
    logFunc("bootstrap.antigravity.error", { message: "Codex CLI ('codex') not found in PATH." });
  }

  // 4. Verify Codex Auth
  try {
    execSync('codex whoami', { stdio: 'ignore' });
  } catch (e) {
    logFunc("bootstrap.antigravity.warn", { message: "Codex OAuth missing or expired. Run 'codex login'." });
  }

  // 5. Inject CAM Skills for Antigravity and Codex
  installAntigravitySkills(logFunc);
  installCodexSkills(logFunc);

  logFunc("bootstrap.antigravity.complete", { message: "Environment Verification Complete" });
}

function installAntigravitySkills(logFunc) {
  const skillsDir = path.join(os.homedir(), ".gemini", "antigravity", "skills");
  const camSkillDir = path.join(skillsDir, "qexow-cam-messaging");
  
  if (!fs.existsSync(camSkillDir)) {
    fs.mkdirSync(camSkillDir, { recursive: true });
  }

  const defaultPs1 = `
param (
    [string]$TargetAgent,
    [string]$MessageText
)

$tokenFile = "$env:USERPROFILE\\.qexow-cam\\secrets\\local-api-token"
$configFile = "$env:USERPROFILE\\.qexow-cam\\config.json"

if (-not (Test-Path $tokenFile)) {
    Throw "CAM token file not found at $tokenFile. Fallbacks are disabled."
}
if (-not (Test-Path $configFile)) {
    Throw "CAM config file not found at $configFile. Fallbacks are disabled."
}

$token = (Get-Content $tokenFile -Raw).Trim()
$config = Get-Content $configFile -Raw | ConvertFrom-Json
if (-not $config.port) {
    Throw "CAM port configuration is missing in $configFile. Fallbacks are disabled."
}
$port = $config.port

$body = @{
    targetAgent = $TargetAgent
    message = $MessageText
    sourceAgent = "antigravity"
} | ConvertTo-Json

$response = Invoke-RestMethod -Uri "http://127.0.0.1:$port/send" -Method Post -Headers @{ Authorization = "Bearer $token" } -Body $body -ContentType "application/json"
$response | ConvertTo-Json -Depth 5
`;
  const destPs1 = path.join(camSkillDir, "Send-AgentMessage.ps1");
  fs.writeFileSync(destPs1, defaultPs1.trim(), "utf8");

  const skillDef = {
    name: "cam_send_message",
    description: "Send a message to another agent via the Qexow CAM (CAM) protocol. Use this to respond to incoming requests from other agents.",
    entrypoint: "pwsh.exe -File .\\Send-AgentMessage.ps1 -TargetAgent \"{{TargetAgent}}\" -MessageText \"{{MessageText}}\"",
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

  const inboxPs1 = `
param (
    [int]$WaitSeconds = 20
)

$tokenFile = "$env:USERPROFILE\\.qexow-cam\\secrets\\local-api-token"
$configFile = "$env:USERPROFILE\\.qexow-cam\\config.json"

if (-not (Test-Path $tokenFile)) {
    Throw "CAM token file not found at $tokenFile. Fallbacks are disabled."
}
if (-not (Test-Path $configFile)) {
    Throw "CAM config file not found at $configFile. Fallbacks are disabled."
}

$token = (Get-Content $tokenFile -Raw).Trim()
$config = Get-Content $configFile -Raw | ConvertFrom-Json
if (-not $config.port) {
    Throw "CAM port configuration is missing in $configFile. Fallbacks are disabled."
}
$port = $config.port

$uri = "http://127.0.0.1:$port/inbox?agent=antigravity"
if ($WaitSeconds -gt 0) {
    $uri += "&wait=$WaitSeconds"
}

$response = Invoke-RestMethod -Uri $uri -Method Get -Headers @{ Authorization = "Bearer $token" }
$response | ConvertTo-Json -Depth 5
`;
  
  fs.writeFileSync(path.join(inboxSkillDir, "Check-AgentMessages.ps1"), inboxPs1.trim(), "utf8");

  const inboxSkillDef = {
    name: "cam_check_inbox",
    description: "Check your Qexow CAM inbox for any pending messages from other agents. Set WaitSeconds to block and wait for a response if none are currently available.",
    entrypoint: "pwsh.exe -File .\\Check-AgentMessages.ps1 -WaitSeconds {{WaitSeconds}}",
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
}

function installCodexSkills(logFunc) {
  const skillsDir = path.join(os.homedir(), ".codex", "skills");
  const camSkillDir = path.join(skillsDir, "qexow-cam-messaging");
  const scriptsDir = path.join(camSkillDir, "scripts");

  if (!fs.existsSync(scriptsDir)) {
    fs.mkdirSync(scriptsDir, { recursive: true });
  }

  const skillMd = `---
name: qexow-cam-messaging
description: Send and receive messages to/from other agents using the Qexow CAM protocol.
---
# Instructions

You are connected to the Qexow CAM messaging fabric. You can communicate with other agents (including \`antigravity\`) by running local scripts.

## Sending a Message
To send a message to another agent:
1. Run the PowerShell script \`./scripts/Send-AgentMessage.ps1\` with the following parameters:
   - \`-TargetAgent\`: The name of the agent you want to message (e.g., \`antigravity\`).
   - \`-MessageText\`: The body of your message.
   - \`-SourceAgent\`: Your agent name (e.g., \`coder-bot\`).

**Example CLI call:**
\`\`\`powershell
pwsh -File "$env:USERPROFILE\\.codex\\skills\\qexow-cam-messaging\\scripts\\Send-AgentMessage.ps1" -TargetAgent "antigravity" -MessageText "Hello" -SourceAgent "coder-bot"
\`\`\`

## Checking Your Inbox
To check for incoming messages:
1. Run the PowerShell script \`./scripts/Check-AgentMessages.ps1\` with the following parameters:
   - \`-AgentName\`: Your agent name (e.g., \`coder-bot\`).
   - \`-WaitSeconds\`: (Optional) The number of seconds to block and wait for a response if your inbox is currently empty (defaults to 20, up to 30).

**Example CLI call:**
\`\`\`powershell
pwsh -File "$env:USERPROFILE\\.codex\\skills\\qexow-cam-messaging\\scripts\\Check-AgentMessages.ps1" -AgentName "coder-bot" -WaitSeconds 15
\`\`\`
`;

  const sendPs1 = `
param (
    [string]$TargetAgent,
    [string]$MessageText,
    [string]$SourceAgent
)

$tokenFile = "$env:USERPROFILE\\.qexow-cam\\secrets\\local-api-token"
$configFile = "$env:USERPROFILE\\.qexow-cam\\config.json"

if (-not (Test-Path $tokenFile)) {
    Throw "CAM token file not found at $tokenFile. Fallbacks are disabled."
}
if (-not (Test-Path $configFile)) {
    Throw "CAM config file not found at $configFile. Fallbacks are disabled."
}

$token = (Get-Content $tokenFile -Raw).Trim()
$config = Get-Content $configFile -Raw | ConvertFrom-Json
if (-not $config.port) {
    Throw "CAM port configuration is missing in $configFile. Fallbacks are disabled."
}
$port = $config.port

$body = @{
    targetAgent = $TargetAgent
    message = $MessageText
    sourceAgent = $SourceAgent
} | ConvertTo-Json

$response = Invoke-RestMethod -Uri "http://127.0.0.1:$port/send" -Method Post -Headers @{ Authorization = "Bearer $token" } -Body $body -ContentType "application/json"
$response | ConvertTo-Json -Depth 5
`;

  const checkPs1 = `
param (
    [string]$AgentName,
    [int]$WaitSeconds = 20
)

$tokenFile = "$env:USERPROFILE\\.qexow-cam\\secrets\\local-api-token"
$configFile = "$env:USERPROFILE\\.qexow-cam\\config.json"

if (-not (Test-Path $tokenFile)) {
    Throw "CAM token file not found at $tokenFile. Fallbacks are disabled."
}
if (-not (Test-Path $configFile)) {
    Throw "CAM config file not found at $configFile. Fallbacks are disabled."
}

$token = (Get-Content $tokenFile -Raw).Trim()
$config = Get-Content $configFile -Raw | ConvertFrom-Json
if (-not $config.port) {
    Throw "CAM port configuration is missing in $configFile. Fallbacks are disabled."
}
$port = $config.port

$uri = "http://127.0.0.1:$port/inbox?agent=$AgentName"
if ($WaitSeconds -gt 0) {
    $uri += "&wait=$WaitSeconds"
}

$response = Invoke-RestMethod -Uri $uri -Method Get -Headers @{ Authorization = "Bearer $token" }
$response | ConvertTo-Json -Depth 5
`;

  fs.writeFileSync(path.join(camSkillDir, "SKILL.md"), skillMd.trim(), "utf8");
  fs.writeFileSync(path.join(scriptsDir, "Send-AgentMessage.ps1"), sendPs1.trim(), "utf8");
  fs.writeFileSync(path.join(scriptsDir, "Check-AgentMessages.ps1"), checkPs1.trim(), "utf8");
  logFunc("bootstrap.antigravity.skill", { message: `Codex global CAM skills successfully installed/updated at ${camSkillDir}` });
}

// Run language_server.exe natively
export function runAgyCommand(args, logFunc) {
  return new Promise((resolve, reject) => {
    const fullArgs = ["agentapi", ...args];
    logFunc("antigravity.agy.command", { command: `${AGY_EXE} ${fullArgs.join(" ")}` });
    if (!fs.existsSync(SCRATCH_DIR)) {
      fs.mkdirSync(SCRATCH_DIR, { recursive: true });
    }
    const child = spawn(AGY_EXE, fullArgs, {
      cwd: SCRATCH_DIR,
      windowsHide: true,
    });

    let stdout = "";
    let stderr = "";

    child.stdout.on("data", (data) => {
      stdout += data.toString();
    });

    child.stderr.on("data", (data) => {
      stderr += data.toString();
    });

    child.on("close", (code) => {
      if (code !== 0) {
        return reject(new Error(`Exit code ${code}. Stderr: ${stderr}`));
      }
      try {
        const parsed = JSON.parse(stdout);
        resolve(parsed);
      } catch (e) {
        reject(new Error(`Failed to parse response JSON: ${e.message}. Raw: ${stdout}`));
      }
    });
  });
}

// Watch transcript.jsonl natively
export async function pollAgyTranscript(conversationId, startByte = 0, logFunc) {
  const logDir = path.join(BRAIN_DIR, conversationId, ".system_generated", "logs");
  const logFile = path.join(logDir, "transcript.jsonl");
  logFunc("antigravity.agy.poll", { message: `Watching transcript: ${logFile} from byte ${startByte}` });

  let attempts = 0;
  while (!fs.existsSync(logDir) && attempts < 20) {
    await new Promise(r => setTimeout(r, 500));
    attempts++;
  }

  if (!fs.existsSync(logDir)) {
    throw new Error(`Directory ${logDir} was never created.`);
  }

  return new Promise((resolve, reject) => {
    let watcher;
    let fallbackInterval;

    const cleanup = () => {
      clearTimeout(timeout);
      if (watcher) {
        try { watcher.close(); } catch (e) {}
      }
      if (fallbackInterval) {
        clearInterval(fallbackInterval);
      }
    };

    const timeout = setTimeout(() => {
      cleanup();
      reject(new Error("Timeout waiting for Antigravity response"));
    }, 120000); // 2 min timeout

    const checkFile = () => {
      if (!fs.existsSync(logFile)) return;
      try {
        const currentSize = fs.statSync(logFile).size;
        if (currentSize > startByte) {
          const buffer = Buffer.alloc(currentSize - startByte);
          const fd = fs.openSync(logFile, "r");
          fs.readSync(fd, buffer, 0, buffer.length, startByte);
          fs.closeSync(fd);
          
          startByte = currentSize; // update startByte
          
          const text = buffer.toString("utf8");
          const lines = text.split(/\r?\n/).filter(Boolean);
          for (const line of lines) {
            try {
              const step = JSON.parse(line);
              if (step.source === "MODEL" && step.type === "PLANNER_RESPONSE" && step.status === "DONE") {
                cleanup();
                logFunc("antigravity.agy.response", { response: step.content });
                resolve(step.content);
                return;
              }
            } catch (e) {}
          }
        }
      } catch (e) {
        // file might be locked temporarily or deleted
      }
    };

    // Initial check
    checkFile();

    // Setup fs.watch
    try {
      watcher = fs.watch(logDir, (eventType, filename) => {
        if (filename !== "transcript.jsonl") return;
        checkFile();
      });
    } catch (watchErr) {
      logFunc("antigravity.agy.watch.error", { error: watchErr.message });
    }

    // Setup fallback polling
    fallbackInterval = setInterval(checkFile, 1000);
  });
}
