# Qexow CAM

`qexow-cam` is a local daemon, CLI, and Windows tray GUI for managing named Codex-facing agents, routing messages to them, mirroring remote CAM agents into a local registry, and keeping agent identity separate from raw Codex Desktop UI state.

CAM is designed around one core rule: agent identity must stay stable. A named agent should keep the same chat/session mapping unless an operator intentionally changes it. The tool exists to make that operationally manageable across local and remote nodes without scraping or mutating Codex Desktop UI state directly.

## What CAM does

CAM maintains a local registry of agents and peers, sends messages into registered agent threads, records mailbox and event history, and can mirror agent inventories from remote nodes that already have CAM installed.

At a high level, CAM provides:

- a local HTTP daemon bound to loopback only
- a CLI for agent creation, reading, send/receive, peer enrollment, and diagnostics
- a Windows tray GUI for status and test workflows
- local discovery of Codex chats
- remote discovery and mirroring of remote CAM agents over SSH
- durable local state for agent mappings, mailbox rows, and event history

## Hard boundaries

CAM has two strict runtime boundaries:

- Codex app-server is started only as `codex app-server --listen stdio://`
- the CAM HTTP API binds only to `127.0.0.1:37631`

CAM does not expose Codex app-server or the CAM daemon on the public or private network. Remote coordination is performed by SSHing to a node that already has CAM installed and asking that remote CAM CLI for its own status or inventory.

## Architecture

The system has four main pieces:

1. `cam.exe`
   The main Node SEA executable. This contains the daemon and CLI logic.

2. `qexow-cam-gui.exe`
   The Windows GUI/tray executable. This is the user-facing Windows app entry point.

3. Local state under `~\.qexow-cam`
   This holds config, agent registry, mailbox, event history, logs, and the local API token.

4. Remote peer links
   CAM can enroll remote peers, recover SSH metadata from prior registry backups, scrape operator docs for likely peer facts, and then ask remote CAM instances for inventory.

## Install and first run

From this directory on Windows:

```powershell
.\cam.cmd init
.\cam.cmd doctor
.\cam.cmd daemon start
.\cam.cmd daemon status
```

The wrapper uses `CAM_NODE_EXE` when set; otherwise it uses `node` from `PATH`:

```powershell
$env:CAM_NODE_EXE = "C:\path\to\node.exe"
```

`cam init` creates the local config and token material if they do not already exist. As of the current version, a clean init defaults to loopback port `37631` if `CAM_PORT` is not explicitly set.

`cam doctor` checks the local Codex/CAM environment, including Codex CLI presence, Codex auth state, local registry/token files, and daemon health.

## Windows installer behavior

The Windows installer is intentionally aggressive.

Default reinstall behavior:

- kills known old CAM executable names
- removes old startup hooks and old Run entries
- removes stale install roots
- removes stale PATH fragments
- wipes both `~\.qexow-cam` and `~\.codex-agent-manager`

This is the default because stale registry and mailbox state caused too many false positives and confusing cross-version behavior.

There is one optional installer checkbox:

- keep old CAM registry/state data

If that is selected, the installer preserves the old CAM homes and only clears volatile process markers instead of doing a full home wipe.

Uninstall removes CAM local state as well. For test/lifecycle smoke runs, the installer and uninstaller also support explicit override paths for CAM home cleanup so destructive lifecycle behavior can be tested safely against isolated temp directories.

## What "install-service" means

`install-service` does not create a real Windows Service.

It records CAM startup metadata locally:

```powershell
.\cam.cmd install-service
```

This records local startup intent and writes `service.json`. It does not create scheduled tasks, systemd units, cron jobs, shell scripts, or hidden helper launchers.

The actual tray/GUI process is still the user-facing Windows application entry point.

## Core commands

### Create and inspect agents

```powershell
.\cam.cmd agent create frontend-local --cwd "C:\path\to\workspace"
.\cam.cmd agent create backend-local --cwd "C:\path\to\workspace"
.\cam.cmd agent list
.\cam.cmd agent status frontend-local
.\cam.cmd agent read frontend-local
.\cam.cmd inbox frontend-local
.\cam.cmd logs
```

### Send a message

```powershell
.\cam.cmd send backend-local "Please reply with your node name and cwd."
```

Optionally specify a sender:

```powershell
.\cam.cmd send backend-local "Are you alive?" --from frontend-local
```

The CLI also supports correlation IDs, message types, and strict send mode. Those are especially important for diagnostic/test flows.

### Change model, effort, and speed

Each agent can store a preferred model, model provider, reasoning effort, and service tier without changing its chat/session UUID or agent alias.

```powershell
.\cam.cmd agent set-model backend-local --model gpt-5.3-codex-spark --model-provider openai
.\cam.cmd agent set-model frontend-local --model gpt-5.5 --model-provider openai
.\cam.cmd agent set-model frontend-local --model gpt-5.5 --model-provider openai --effort medium --speed standard
.\cam.cmd agent set-model frontend-local --effort xhigh --speed fast
```

Do not use `--recreate`. Recreate-style behavior can break the one-agent-to-one-session mapping and is specifically not the safe operating model for CAM.

Rules:

- `--speed standard` means no service tier override
- `--speed fast` maps to `serviceTier: "fast"`
- `--service-tier` may be used directly for advanced cases
- `default` is not a valid service tier
- `extra-high` is normalized to `xhigh`

## Local discovery

CAM discovers local Codex chats from Codex-managed state and session metadata. It does not run Python helpers or external database probes.

The discovery path is intentionally layered:

- Codex-managed state rows
- session index information
- rollout/session metadata

These are not treated as hidden fallbacks; they are explicit discovery sources that are compared and merged. Discovery also preserves route/source metadata so the GUI and registry can distinguish local records from mirrored remote ones.

Quick disk-level visual cues:

- active transcripts usually live under `$env:USERPROFILE\.codex\sessions\...`
- archived transcripts usually live under `$env:USERPROFILE\.codex\archived_sessions\...`

Those paths are useful for orientation, but CAM's actual classification uses Codex-managed state rather than a naive folder-name check.

## Remote peers and mirrored remote agents

CAM can discover, enroll, and sync remote peers.

Remote peer facts can come from:

- Codex-managed remote peer evidence
- prior registry backups
- operator markdown/docs that mention peer names, IPs, SSH targets, or hostnames

Once CAM has enough peer metadata, it can SSH to the remote node and ask the remote CAM instance for inventory. The preferred path is:

- `cam inventory export`

If the remote node is on an older CAM install, CAM can fall back to:

- `cam daemon status`
- `cam agent list`

Those results are then mirrored locally. Mirrored agents get names like:

```text
frontend::github-agent-dev-agent
backend::auth-and-dev-agent
```

Those mirrored rows:

- keep the real remote thread IDs
- record route metadata like `peer:frontend`
- remain distinct from local agents
- are not treated as local sessions

This is how CAM avoids the old bug where remote chats could be discovered but mislabeled as local.

## Message routing and mailbox behavior

CAM sends messages through the registered thread mapping for the target agent. If a send cannot be completed through the real thread path, CAM can persist the message in the durable mailbox.

Important distinction:

- `delivered` means the message actually entered the selected agent thread and a turn ID exists
- `received` means a reply was successfully received into the CAM receiver/mailbox path
- `queued` means delivery was deferred
- `failed` means the strict send path failed

Queued delivery is acceptable for normal async messaging. It is not acceptable as a passing condition for strict GUI round-trip diagnostics.

## GUI diagnostic semantics

The GUI test flow is intentionally stricter than normal messaging.

A GUI diagnostic pass requires both legs:

1. the outbound test message must be delivered to the selected agent's real thread
2. the reply must come back through the CAM receiver path from the exact selected agent with the expected correlation/message type semantics

It is not enough for a chat log to contain a plausible response. It is not enough for a random mailbox row to look similar. It is not enough for a wrong agent to respond.

That is why the GUI test path now distinguishes:

- outbound delivered
- waiting for reply
- reply received
- ignored reply
- queued-only reply
- pass
- fail

## Storage and files

Default local state roots:

```text
C:\Users\<user>\.qexow-cam
/home/ubuntu/.qexow-cam
/home/ubuntu/.codex-agent-manager   legacy path still seen on older nodes
```

Important files:

```text
config.json
agents.json
mailbox.jsonl
events.jsonl
tests.jsonl
logs/daemon.log
secrets/local-api-token
service.json
```

Set `CAM_HOME` if you need to use a non-default state directory.

## Build and release notes

Important local/CI rules:

- `npm run build` is intentionally blocked for ordinary local use
- `npm run build:installer-payload` is the release payload build path
- the release workflow compiles the installer and runs installer smoke checks

Current release verification includes:

- strict contract verification
- payload build
- Inno Setup compile
- stale-install cleanup checks
- reinstall map deletion checks
- uninstall cleanup checks

The repo also includes tooling to purge or watch for unwanted standalone EXE artifacts in the workspace.

## Security model

- Codex app-server is spawned over stdio only
- the local CAM HTTP API binds to `127.0.0.1`
- CLI requests require `secrets/local-api-token`
- remote CAM inventory sync and remote CAM message delivery use SSH with already-known peer keys
- raw app-server and manager ports stay loopback-only
- tokens, mailbox rows, logs, and generated local state are ignored by git

Do not open public ports for Codex app-server or the CAM daemon.

## Testing references

The detailed test notes live under [docs/tests](C:/Users/kjhgf/OneDrive/Documents/New%20project/codex-agent-manager/docs/tests):

- install and uninstall lifecycle
- GUI status
- local discovery
- remote discovery
- registry integrity
- local-to-local and local-to-remote messaging
- backoff and steering behavior
- offline mailbox queueing
- autonomous remote deployment checks

## Current practical limits

- remote mirrored agents are inventory/mapping objects first; not every GUI round-trip path is appropriate for mirrored remote agents
- old remote nodes may not support `inventory export`, so CAM still needs the older `daemon status + agent list` path
- the tray GUI and the daemon are intentionally separate executables even though they are part of one Windows app experience

## Short summary

Qexow CAM is a loopback-only local manager for stable named-agent identity, strict message routing, remote CAM mirroring over SSH, and a Windows GUI/tray workflow that is backed by a real daemon and registry instead of loose UI scraping.
