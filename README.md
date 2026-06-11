# Codex Agent Manager

`codex-agent-manager` is a local daemon and CLI for routing messages between named Codex agents without touching Codex Desktop UI state or transcript files.

The manager has two strict boundaries:

- Codex app-server is started only as `codex app-server --listen stdio://`.
- The manager HTTP API binds only to `127.0.0.1:37631`.

Remote delivery uses SSH. Do not expose raw Codex app-server or the manager daemon on a public interface.

## Install

From this directory on Windows:

```powershell
.\cam.cmd init
.\cam.cmd doctor
.\cam.cmd daemon start
.\cam.cmd daemon status
```

The wrappers use `CAM_NODE_EXE` when set, otherwise they use `node` from `PATH`:

```powershell
$env:CAM_NODE_EXE = "C:\path\to\node.exe"
```

On Linux nodes:

```bash
node /home/ubuntu/codex-agent-manager/bin/cam.js init
node /home/ubuntu/codex-agent-manager/bin/cam.js daemon start
node /home/ubuntu/codex-agent-manager/bin/cam.js daemon status
```

Install login/reboot persistence:

```powershell
.\cam.cmd install-service
```

```bash
node /home/ubuntu/codex-agent-manager/bin/cam.js install-service
```

Windows first tries a logon scheduled task and falls back to a no-admin Startup-folder launcher if task creation is denied. Linux installs a user systemd unit when available and falls back to an `@reboot` cron entry if user systemd is unavailable.

On daemon start, CAM also rehydrates any already-registered agents with saved thread IDs so the next reboot usually needs less manual cleanup.

## Basic Use

```powershell
.\cam.cmd agent create frontend-local --cwd "C:\path\to\workspace"
.\cam.cmd agent create backend-local --cwd "C:\path\to\workspace"
.\cam.cmd agent create backend-local --cwd "C:\path\to\workspace" --model gpt-5.3-codex-spark --model-provider openai
.\cam.cmd agent list
.\cam.cmd agent read backend-local
.\codex-send.cmd backend-local "Please reply with your node name and cwd."
.\cam.cmd inbox backend-local
.\cam.cmd logs
```

### Bulk model / effort / speed switching

Each agent can store a preferred model, provider, reasoning effort, and service tier without changing its chat, session UUID, or agent alias.

```powershell
.\cam.cmd agent set-model backend-local --model gpt-5.3-codex-spark --model-provider openai
.\cam.cmd agent set-model frontend-local --model gpt-5.5 --model-provider openai
.\cam.cmd agent set-model frontend-local --model gpt-5.5 --model-provider openai --effort medium --speed standard
.\cam.cmd agent set-model frontend-local --effort xhigh --speed fast
```

Do not use `--recreate`. It can start a new thread and swap the agent to that new session, which breaks the one chat/session/agent identity map. Model changes should update the existing agent preference only.

The safe form is always `cam agent set-model <name> --model <id> --model-provider <provider> --effort <minimal|low|medium|high|xhigh> --speed <standard|fast>` with no recreate flag. Partial updates are allowed.

Use `--speed standard` for normal speed. CAM stores that as no service tier override. Use `--speed fast` for Fast mode, which CAM sends as `serviceTier: "fast"` on future `turn/start` calls. Advanced callers may use `--service-tier <tier>` instead of `--speed`, but never use `default`.

Reasoning effort accepts `minimal`, `low`, `medium`, `high`, or `xhigh`; `extra-high` is normalized to `xhigh`.

If delivery through `turn/start` or `turn/steer` fails, the message is saved in a durable mailbox. Queued messages are surfaced into the next successful turn for that target agent.

## SSH Peer Routing

Enroll a remote node from Windows:

```powershell
.\cam.cmd node enroll frontend --ssh ubuntu@example.com --key "C:\path\to\private-key.pem" --remote-root /home/ubuntu/codex-agent-manager
```

Then send to a named remote agent with the same command shape:

```powershell
.\codex-send.cmd frontend-agent "Reply with your node name and cwd."
```

The local CLI first tries the local daemon. If the target agent is unknown locally, it checks enrolled SSH peers and runs that peer's local `cam send` command over SSH. This works from the home PC behind NAT because the home PC initiates outbound SSH.

For cloud-to-cloud routing, enroll private-IP peers from nodes that already have an approved SSH key:

```bash
node /home/ubuntu/codex-agent-manager/bin/cam.js node enroll searchbox \
  --ssh ubuntu@10.0.0.10 \
  --key /path/to/private/key.pem \
  --remote-root /home/ubuntu/codex-agent-manager
```

## SSH Tunnels

Use tunnels when the home PC needs a stable local port through NAT to a cloud node's loopback manager:

```powershell
.\cam.cmd tunnel command frontend --local-port 37632
.\cam.cmd tunnel open frontend --local-port 37632 --background
.\cam.cmd tunnel status 37632
.\cam.cmd tunnel stop <pid>
```

That opens:

```text
127.0.0.1:37632 -> frontend:127.0.0.1:37631
```

The tunnel is optional for normal `codex-send` SSH peer routing. It is useful for diagnostics and future HTTP manager-to-manager transport, while still avoiding public manager ports.

## Storage

Default user-local state:

```text
C:\Users\<user>\.codex-agent-manager
/home/ubuntu/.codex-agent-manager
```

Important files:

```text
config.json
agents.json
mailbox.jsonl
events.jsonl
tunnels.json
logs/daemon.log
secrets/local-api-token
```

Set `CAM_HOME` to use a different state directory.

## Determine Active vs Archived Chats

CAM chat lifecycle is tracked in the local thread-state database (`state_*.sqlite`), not just in transcript folders.

- Active chats: `archived = 0`
- Archived chats: `archived = 1`

Run from PowerShell (adjust path if your CAM home is different):

```powershell
$db = "$env:USERPROFILE\.codex\state_5.sqlite"
$script = @'
import sqlite3

conn = sqlite3.connect(r"__DB__")
rows = conn.execute(
    "SELECT id, title, archived, archived_at, updated_at FROM threads ORDER BY archived, updated_at DESC"
).fetchall()
for row in rows:
    print(row)
'@
$py = $script -replace '__DB__', ($db -replace '\\', '\\')
python -c $py
```

Use the files on disk as a quick visual signal:

- Active transcripts: `$env:USERPROFILE\.codex\sessions\...`
- Archived transcripts: `$env:USERPROFILE\.codex\archived_sessions\...`

Count by status:

```powershell
$db = "$env:USERPROFILE\.codex\state_5.sqlite"
$script = @'
import sqlite3
conn = sqlite3.connect(r"__DB__")
for r in conn.execute("SELECT archived, COUNT(*) FROM threads GROUP BY archived ORDER BY archived"):
    print(r)
'@
python -c ($script -replace '__DB__', ($db -replace '\\', '\\'))
```

If your environment has a different `CAM_HOME` path than the default, use the path configured in that environment for `state_*.sqlite` and transcript folders.

## Security

- Codex app-server is spawned over stdio only.
- CLI-to-daemon API binds to `127.0.0.1`.
- CLI requests require `secrets/local-api-token`.
- Remote delivery uses SSH command execution or SSH tunnels.
- Home-PC NAT traversal is outbound SSH from Windows, not inbound public exposure.
- Tokens, logs, mailbox data, and generated local state are ignored by git.
- Do not open public ports for Codex app-server or the manager daemon.
