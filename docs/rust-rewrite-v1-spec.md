# Rust Rewrite V1 Spec

This document turns the initial rewrite outline into a practical v1 product spec for a ground-up Rust implementation of Qexow CAM. It keeps the current product contract where that contract is intentional and operator-facing, while deliberately dropping compatibility scaffolding and legacy machinery.

## 1. Must-Have V1

### Product goals

- Preserve stable named-agent identity as the primary invariant.
- Provide a loopback-only local daemon as the source of truth.
- Support explicit operator workflows for health, agents, send, inbox, logs, discovery, and remote mirror sync.
- Preserve truthful delivery semantics: `delivered`, `received`, `queued`, `failed`, `steered`.
- Keep remote coordination explicit and local-first: SSH to another CAM install, never exposed manager ports.
- Send responses to the agent's active attention channel, not merely to storage.

### Required v1 capabilities

- Local daemon start, stop, status, and health reporting.
- Local initialization and environment diagnostics.
- Agent create, resume, list, inspect, read transcript summary, and set model preferences.
- Message send with `source`, `correlation_id`, `message_type`, and `strict`.
- Mid-turn steering when the target agent already has an active turn.
- Wake/resume delivery when the target conversation is inactive but has a valid session/thread identity.
- Durable mailbox queueing only as fallback for offline, locked, virtual-inbox, or truly undeliverable targets.
- Built-in virtual mailbox targets for `operator`.
- Antigravity treated as a session-addressable conversational target when AGY session ID delivery is available.
- Local discovery of Codex chats from Codex-managed state and session metadata.
- Trust-gated discovery classification: `approved`, `candidate`, `quarantined`, `rejected`.
- Peer enrollment and remote inventory sync.
- Local mirroring of remote agents with visible route distinction.
- Loopback-only HTML status page for desktop use, blocked in headless mode.

### Nice-to-have later, not required for v1

- Windows tray GUI.
- Installer and packaging polish.
- Linux service installation helpers.
- Peer metadata enrichment from docs or old backups.
- Advanced GUI test tooling and rollout verification helpers.

## 2. Precise Data Model

The Rust rewrite should use explicit typed state instead of loosely-shaped JSON blobs in memory. Persisted disk format can still be JSON or JSONL for easy inspection, but the model should be strict in-process.

### Agent

Fields:
- `name`: stable operator alias, unique.
- `kind`: `codex`, `virtual_inbox`, `agy_session`, or `remote_mirror`.
- `thread_id`: nullable for mailbox-only targets.
- `thread_source`: `codex`, `agy_session`, `mailbox`, `gui_only`, or `remote_mirror`.
- `cwd`: nullable path, required for local Codex-backed agents.
- `route`: `local` or `peer:<peer_name>`.
- `status`: `idle`, `active`, `error`, `unknown`.
- `active_turn_id`: nullable.
- `last_turn_id`: nullable.
- `model`: nullable.
- `model_provider`: nullable.
- `effort`: nullable enum `minimal|low|medium|high|xhigh`.
- `service_tier`: nullable string.
- `created_at`
- `updated_at`
- `last_error`: nullable string.

Notes:
- `name` and `thread_id` mapping is the core identity contract.
- Model changes must not mutate the mapping.
- Remote mirrors must never be mistaken for local agents.
- Antigravity with a known AGY session ID should be modeled as conversationally deliverable, not mailbox-first.

### Message

Fields:
- `message_id`
- `target_agent`
- `source_agent`
- `source_node`
- `body`
- `correlation_id`: nullable.
- `message_type`: nullable.
- `delivery`: enum `started|delivered|received|queued|failed|steered`.
- `strict`: boolean.
- `error`: nullable string.
- `thread_id`: nullable.
- `turn_id`: nullable.
- `created_at`
- `updated_at`

Notes:
- `delivery` is a contract field, not a logging hint.
- `received` is reserved for actual inbound receipt into CAM's receiver path.
- `queued` is valid for async workflows, but not a pass condition for strict diagnostics.
- A stored queued message is deferred state, not proof that the target agent noticed the response.

### DiscoveryRow

Fields:
- `thread_id`
- `title`
- `cwd`
- `source`: `codex_state`, `session_index`, `rollout`, `remote_inventory`
- `route`
- `peer_name`: nullable.
- `thread_source`
- `updated_at`
- `disposition`: `approved`, `candidate`, `quarantined`, `rejected`
- `reason`

Notes:
- Discovery and promotion are separate phases.
- A row may exist without being promoted to an active local agent.

### Peer

Fields:
- `name`
- `transport`: `ssh` or `codex_managed`
- `ssh_target`: nullable `user@host`
- `key_path`: nullable.
- `remote_root`: nullable or `auto`
- `state`: `unknown`, `verified`, `mirrored`, `mirrored_degraded`, `sync_failed`
- `remote_node_name`: nullable.
- `last_sync_at`: nullable.
- `last_sync_error`: nullable.
- `inventory_source`: nullable.
- `created_at`
- `updated_at`

Notes:
- V1 should support explicit peer enrollment cleanly.
- Peer auto-enrichment can be deferred or removed.

### Local state files

Recommended persisted files:
- `config.json`
- `agents.json`
- `mailbox.jsonl`
- `events.jsonl`
- `daemon.json`
- `service.json`
- `logs/daemon.log`
- `secrets/local-api-token`

## 3. Proposed CLI Surface

The Rust CLI should be smaller and clearer than the current surface. Keep stable operator-facing workflows. Fold legacy and compatibility helpers out of the default interface.

### Keep in v1

```text
cam init
cam doctor

cam daemon start [--headless]
cam daemon stop
cam daemon status

cam agent create <name> --cwd <path> [--thread-id <id>] [--source <codex|antigravity>] [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]
cam agent resume <name>
cam agent list
cam agent status <name>
cam agent read <name> [--latest]
cam agent set-model <name> [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]

cam send <target-agent> <message> [--from <agent-name>] [--source-node <node>] [--correlation-id <id>] [--message-type <type>] [--strict]
cam inbox [agent-name] [--wait <seconds>]
cam logs

cam discover local

cam peer add <name> --ssh <user@host> [--key <path>] [--remote-root <path>]
cam peer list
cam peer sync [peer-name]

cam inventory export
```

### Simplifications from the current CLI

- Rename `node` to `peer`. `peer` is the clearer product concept.
- Drop `daemon launch` from the public contract. It can remain as an internal GUI helper, but `daemon start` is the operator-facing primitive.
- Replace `node discover` with either:
  - no public command in v1, or
  - a future `peer discover` admin/debug command if truly needed.
- Keep `inventory export` as the stable remote mirror contract.
- Keep `install-service` and `uninstall-service` out of the core operator surface for v1. They are packaging concerns, not product essence.
- Keep `verify-rollout` out of v1. It is useful as an internal test helper, but not part of the clean product contract.
- Reject `--recreate` everywhere.

### CLI behavior notes

- `cam doctor` should check:
  - local config presence
  - local token presence
  - daemon health
  - Codex CLI availability
  - Codex auth state
  - Codex app-server stdio probe
- `cam agent list` should produce stable tabular output plus a machine-readable `--json` mode in Rust.
- `cam send` should return a structured result including `delivery`, `queued`, `received`, `message_id`, and `error`.
- `cam send` should prefer the foreground daemon when reachable so active attention delivery uses daemon memory and retained provider owner state.
- If daemon-first `cam send` cannot connect before writing the request, it may fall back to the direct CLI path with a loud route event. Once a request may have reached the daemon, it must not retry direct delivery and risk duplicate sends.
- `cam agent resume` should prefer the foreground daemon when reachable, so readiness checks and Codex owner attach use daemon memory and retained provider owner state. Explicit one-shot Codex stdio options stay on the direct CLI path.
- `cam inbox` should support both instant read and bounded wait.
- `GET /v1/inbox` should expose the same inbox result shape as the wait-capable service: `messages`, `wait_seconds`, and `timed_out`.
- `cam discover local` should trigger discovery and print a summary:
  - discovered rows
  - approved promotions
  - quarantined count
  - rejected count
- Response routing policy should be:
  - active target conversation: steer into the live turn
  - inactive but known target conversation: wake/resume and deliver as a new turn
  - queue only when immediate delivery is impossible or the target is intentionally virtual-inbox-only

## 4. Proposed Local HTTP API

The Rust daemon should expose a smaller, explicit loopback API. Keep the endpoints the CLI and tray actually need. Remove state-mutation routes that exist only as convenience shims for the current implementation.

### Public local endpoints for v1

#### Unauthenticated loopback-only

These are the only endpoints that may be accessed without the local API token:

```text
GET /health
GET /status-ui
```

`GET /health`
- Purpose: liveness and startup progress.
- Returns:
  - `ok`
  - `version`
  - `node_name`
  - `started_at`
  - `app_server_initialized`

`GET /status-ui`
- Purpose: human local status page for tray/desktop workflows.
- Behavior:
  - enabled only in non-headless mode
  - returns `403` in headless mode

#### Authenticated loopback-only

```text
GET  /v1/agents
POST /v1/agents
GET  /v1/agents/{name}
GET  /v1/agents/{name}/thread
POST /v1/agents/{name}/resume
POST /v1/agents/{name}/model

POST /v1/messages
GET  /v1/inbox
GET  /v1/logs

POST /v1/discovery/local:run

GET  /v1/peers
POST /v1/peers
POST /v1/peers:sync
POST /v1/peers/{name}/sync

GET  /v1/inventory
POST /shutdown
```

### Endpoint intent

`GET /v1/agents`
- List all agents, including local, mailbox-only, and remote-mirror agents.

`POST /v1/agents`
- Create a new agent.
- Request body:
  - `name`
  - `cwd`
  - `thread_id`
  - `thread_source`
  - `model`
  - `model_provider`
  - `effort`
  - `service_tier`

`GET /v1/agents/{name}`
- Return full agent metadata.

`GET /v1/agents/{name}/thread`
- Return thread summary by default.
- Optional query:
  - `include_turns=true`
  - `latest=true`
  - `turns=<n>`
- Bounded turn expansion returns local CAM mailbox evidence with `turns=<n>` capped by `MAX_AGENT_READ_TURNS`.
- Provider transcript fields must still report whether real provider transcript evidence was available; local mailbox evidence must not pretend to be provider transcript evidence.
- For remote mirror agents, read the owning peer CAM over SSH and validate that the returned remote snapshot matches the local mirror's peer namespace and thread identity.

`POST /v1/agents/{name}/resume`
- Ensure the local mapping is active and ready.
- For local Codex agents whose thread is not currently owned by the daemon, the daemon may start a daemon-owned Codex app-server, call the real provider resume primitive, and retain the owner only when readiness and turn proof are returned.
- For remote mirror agents, resume through the owning peer CAM over SSH and validate that the returned remote result matches the mirror's namespace and thread identity before updating local mirror readiness.

`POST /v1/agents/{name}/model`
- Update model settings without changing identity.
- Must reject recreate-style semantics.

`POST /v1/messages`
- Primary send endpoint.
- Request body:
  - `target_agent`
  - `message`
  - `source_agent`
  - `source_node`
  - `correlation_id`
  - `message_type`
  - `strict`
- Response:
  - `ok`
  - `delivered`
  - `queued`
  - `received`
  - `message`

Delivery intent:
- For conversational targets, the daemon should prefer steer or wake/deliver over queueing.
- For virtual inbox targets, the daemon may store directly as inbound receiver state.
- For remote mirror targets, the daemon should use the enrolled peer route and send through the remote CAM over SSH, preserving delivery proof and route distinction.
- Remote CAM-to-CAM sends should preserve the original `source_node`; when no source node is supplied, the sending CAM stamps its local node name.
- If AGY's real primitive is `send to session ID`, the Rust rewrite should use that directly instead of treating Antigravity as mailbox-backed by default.

`GET /v1/inbox`
- Query params:
  - `agent=<name>`
  - `wait=<seconds>`
- Response:
  - `messages`
  - `wait_seconds`
  - `timed_out`
- Bounded wait may poll persisted mailbox state. Foreground daemon implementations should handle wait-capable inbox reads without blocking unrelated loopback requests; mutable state/runtime operations may remain serialized.

`GET /v1/logs`
- Return recent structured log rows.

`POST /v1/discovery/local:run`
- Trigger local discovery and promotion pass.
- Response:
  - `ok`
  - `rows_discovered`
  - `approved`
  - `candidate`
  - `quarantined`
  - `rejected`
  - `promoted`

`GET /v1/peers`
- Return peer list and sync health summary.

`POST /v1/peers`
- Add or update an explicit peer enrollment.
- Request body:
  - `name`
  - `transport`
  - `ssh_target`
  - `key_path`
  - `remote_root`

`POST /v1/peers:sync`
- Run remote inventory sync for all enrolled peers.
- Behavior:
  - attempts every enrolled peer
  - keeps going after individual peer failures
  - records visible peer failure state
  - returns aggregate sync counters and per-peer results

`POST /v1/peers/{name}/sync`
- Run remote inventory sync for one peer.

`GET /v1/inventory`
- Trusted export for remote CAM sync.
- Should contain:
  - local node metadata
  - local agents
  - peer summary if needed
  - discovery counts if useful

`POST /shutdown`
- Keep as a local lifecycle helper for desktop control.

### Endpoints to omit from v1

- `POST /agents/set-status`
  - This is too low-level and invites state mutation without real causal actions.
- `POST /nodes/discover`
  - Useful as legacy/operator recovery logic, but not part of a clean v1 contract.
- Any endpoint dedicated to direct remote registry scraping.
- Any endpoint that exposes app-server internals directly.

## 5. Acceptance Test Matrix

### Identity preservation

- Creating an agent with a given name produces a stable alias-to-thread mapping.
- `agent set-model` changes model settings without changing alias or thread ID.
- Any recreate-style request fails clearly at both CLI and API layers.
- Mid-turn follow-up during an active turn returns `delivery:"steered"` and preserves the active turn ID.

### Delivery semantics

- Strict send to an unknown target returns `ok:false`, `delivered:false`, `queued:false`.
- Normal send to a mailbox-only target can return `queued:true`.
- A real conversational response to an active local Codex or AGY session is steered into the live turn.
- A real conversational response to an inactive but known local Codex or AGY session wakes/delivers as a new turn instead of queueing.
- GUI diagnostic inbound confirmation path returns `received:true` and `message.delivery:"received"`.
- Queued messages persist durably and are readable via inbox.

### Discovery and trust

- Local discovery uses Codex-managed state and session metadata, not external DB probes.
- Missing thread ID or missing in-project workspace yields non-approval.
- Machine-spawned subagents and CAM-message sessions are quarantined or rejected, not auto-promoted.
- Registry contains zero invalid local Codex thread IDs after discovery.

### Peer sync

- Peer sync prefers remote `inventory export`.
- Older peer fallback, if still retained, is compatibility-only and not the primary design.
- Mirrored remote agents are imported with distinct route metadata and cannot be mistaken for local agents.

### Runtime boundaries

- Health binds on loopback and becomes reachable early during startup.
- The daemon endpoint defaults to `127.0.0.1:37631`.
- The daemon port may be configured so Rust CAM can run beside another local CAM daemon without disturbing it.
- A configured non-default port is still local-only; it does not create a public API surface.
- Status UI works only on loopback and returns `403` in headless mode.
- No public or private network exposure of the CAM daemon or Codex app-server.

## 6. Explicit V1 Omissions

These are the features or behaviors that should not shape the Rust v1 design, even if pieces of them still exist in the current codebase.

### Hard non-goals

- No UI scraping or direct Codex Desktop UI mutation.
- No public daemon port or public app-server port.
- No recreate workflows.
- No remote shell bootstrap or autonomous remote deployment.
- No remote mailbox polling loops.
- No exponential backoff machinery for removed polling behavior.
- No external helper scripting via VBScript, `mshta`, or similar launch hacks.
- No direct state mutation endpoints whose only job is to patch status by hand.
- No mailbox-first Antigravity design when AGY session delivery is available.

### Compatibility baggage to omit from the clean surface

- `daemon launch` as a first-class operator command.
- `node discover` as part of the clean default surface.
- Backup-registry recovery as a core peer enrollment mechanism.
- Markdown/doc scraping as a core peer enrollment mechanism.
- Legacy repo-run command forms as anything except migration notes.
- Windows installer choreography as product architecture.
- Linux service installation helpers as part of the logical product core.
- `verify-rollout` as part of the operator-facing product surface.
- Direct remote registry file reads as a primary sync method.

## 7. Recommended Simplification Decisions

- Treat the daemon API as versioned from day one with `/v1/...`.
- Use `peer` consistently instead of `node`.
- Keep the status page, but make it clearly separate from the typed API.
- Make `inventory export` the only supported remote sync contract in the long term.
- Prefer explicit peer enrollment over inferred peer recovery.
- Keep mailbox-only targets as a formal concept in the model instead of special-casing them ad hoc.
- Keep virtual inbox targets and session-addressable conversational targets as different model concepts.
- Keep structured logs and JSONL event history because they are useful operator evidence, not just debugging residue.

## 8. Rewrite Summary

The Rust rewrite should feel smaller, stricter, and more typed than the current implementation, while preserving the actual personality of the tool.

The product we are rebuilding is:
- a local control plane
- for stable named-agent identity
- with truthful message semantics
- strict discovery trust gates
- explicit operator workflows
- and remote mirroring through known CAM peers

What we should carry forward is the discipline.

What we should leave behind is the compatibility sprawl.
