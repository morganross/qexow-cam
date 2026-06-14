---
name: qexow-cam-messaging
description: Send and receive messages to/from other agents using the Qexow CAM protocol.
---
# Instructions

You are connected to the Qexow CAM messaging fabric. Communicate through the local Qexow CAM daemon only.

> **Boss Agents:** If you are a Boss agent, please read the rules of engagement at:
> `/root/.qexow-cam/boss.md`

> **Preferred local CAM docs on this machine:**
> `/root/.qexow-cam/docs/README.md`
> `/root/.qexow-cam/docs/howto-use-qexow-cam.md`
> `/root/.qexow-cam/docs/qexow-cam-plain-english.md`

> **Bundled reference copies in this skill install:**
> `references/README.md`
> `references/howto-use-qexow-cam.md`
> `references/qexow-cam-plain-english.md`

## Command Path Rules
Do not assume one CAM command shape works everywhere.

- Local Windows repo checkout: `./cam.cmd ...`
- Local Windows installed CAM on PATH: `cam ...`
- Remote Linux installed CAM on PATH: `cam ...`
- Remote Linux legacy repo checkout: `node /home/ubuntu/codex-agent-manager/bin/cam.js ...` (legacy/dev-only)

## Sending a Message
To send a message to another agent, use the installed Qexow CAM command. Do not use retired `codex-agent-manager` paths, direct CAM HTTP, or PowerShell helper scripts.

**Example CLI call:**
```text
cam send operator "Hello" --from coder-bot
```

When replying to a CAM GUI diagnostic test, send a CAM message back to the requested target mailbox and preserve the incoming `correlationId`. Use `--message-type "cam-gui-test-reply"` when the incoming message asks for it. Chat-only replies do not pass the GUI diagnostic. A valid GUI diagnostic reply must be accepted by the daemon as `delivery: "received"` and must answer the semantic test question in the prompt.

**Example diagnostic reply:**
```text
cam send "CAM test, Kexau CAM test suite mailbox" "CAM_GUI_TEST_RESPONSE <testId>. Agent: coder-bot. Node: RyzenLaptop. Status: idle. The capital of Missouri is Jefferson City." --from coder-bot --correlation-id "<testId>" --message-type "cam-gui-test-reply"
```

## Checking Your Inbox
To check for incoming messages, use the installed Qexow CAM command.

**Example CLI call:**
```text
cam inbox coder-bot
```
