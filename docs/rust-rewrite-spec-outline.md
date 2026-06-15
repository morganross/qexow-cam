# Rust Rewrite Spec Outline

This document captures the first clean rewrite-spec outline for a ground-up Rust version of Qexow CAM. It is intended to preserve product behavior and operational philosophy without carrying forward the current implementation shape.

## 1. Product Purpose

- Provide a local control plane for stable named Codex-facing agents.
- Preserve one-agent-to-one-thread identity unless an operator intentionally changes it.
- Route messages truthfully through real agent mappings, not vague chat guesses.
- Keep a trustworthy local daemon, registry, mailbox, and peer inventory.
- Support a Windows-first tray/GUI workflow on top of a real backend.
- Mirror remote CAM inventories over SSH without exposing manager ports publicly.

## 2. Core Invariants

- Agent identity is the top invariant.
- Model changes must not recreate the session or alias mapping.
- Delivery states must remain distinct: `delivered`, `received`, `queued`, `failed`, `steered`.
- Strict send must fail hard when true delivery is impossible.
- Discovery is trust-gated.
- Non-approved discoveries must not silently behave like valid local agents.
- Codex app-server stays `stdio` only.
- CAM daemon stays loopback only.
- Local state is part of the product contract, not an internal detail.
- Operator workflows must be explicit and inspectable.

## 3. Required Features

- `init`, `doctor`, daemon `start|launch|stop|status`, logs, inbox, and health/status inspection.
- Agent create, resume, list, status, read, and set-model.
- Message send with `from`, `correlation-id`, `message-type`, and `strict`.
- Mid-turn steering into an existing active turn.
- Responses should prefer immediate conversational delivery over passive storage.
- Durable mailbox queuing only as fallback when direct conversational delivery is impossible.
- Discovery from Codex-managed state, session index, and rollout/session metadata.
- Promotion/quarantine/rejection policy for discovered sessions.
- Peer enrollment, peer sync, and trusted remote inventory mirroring.
- Distinct handling for local agents, mirrored remote agents, virtual inbox targets like `operator`, and session-addressable Antigravity chats.
- Headless mode with blocked status UI.
- Aggressive cleanup on reinstall/uninstall, with explicit preserve-state opt-in.

## 4. State Model And Operator Workflows

- Persistent local state should include config, agent registry, mailbox history, event history, logs, local API token, and service metadata.
- The daemon is the source of truth; tray/GUI are clients of the daemon, not the authority.
- Discovery should produce candidate rows with source/route metadata, then policy should classify them as `approved`, `candidate`, `quarantined`, or `rejected`.
- Remote mirrored agents must stay visibly distinct from local agents.
- Responses must go to the agent's active attention channel, not merely to storage.
- For Antigravity, if AGY can deliver by known session ID, that conversational path should be primary:
  - active chat: steer
  - inactive but known session: wake/deliver
  - queue only as fallback
- Operators need first-class workflows for health, discovery, peer sync, sending, inbox review, and log inspection.
- Startup should be self-healing enough for desktop use, but never by exposing extra network surfaces.

## 5. Acceptance Tests

- Changing model/provider/effort/service-tier does not change thread/session UUID or alias.
- `--recreate` is rejected everywhere.
- Strict send to an unknown or undeliverable target returns `delivered:false` and `queued:false`.
- A GUI diagnostic pass requires a real outbound delivery and a real `received` inbound diagnostic message, not a queued approximation.
- Queued delivery persists durably and is visible through inbox reads.
- Discovery rejects rows with no thread ID or no in-project workspace.
- Machine-spawned subagents and CAM-message sessions do not auto-promote.
- Registry integrity checks yield zero invalid Codex thread IDs after discovery/import.
- Health binds on loopback and is available early in startup.
- Headless `/status-ui` returns `403`.
- Remote sync prefers `inventory export`, with older fallback paths treated as compatibility only.
- Default reinstall/uninstall prefers clean state; preserve-state is explicit.

## 6. Hard Non-Goals

- No UI scraping or direct Codex Desktop UI mutation.
- No public manager or app-server ports.
- No `--recreate` workflows.
- No remote shell bootstrap or `install-remote.sh`.
- No direct SSH payload delivery to remote Codex agents.
- No remote mailbox polling loops or their backoff machinery.
- No old Antigravity external launcher model.
- No mailbox-first modeling of Antigravity when a real AGY session delivery primitive exists.
- No helper-script sprawl like VBScript, `mshta`, or tray subprocess hacks.
- No legacy repo-run command shape as steady-state architecture.
- No build/release choreography shaping the runtime design.

## 7. Compatibility Baggage To Delete

- Prior installer-specific cleanup choreography.
- Legacy folder scrubbing rules and product-name cleanup lists.
- Heuristic-heavy title regexes as the main discovery classifier surface.
- Backup-registry and doc-scraping complexity for peer recovery, unless truly required.
- Branches kept only for removed SSH, polling, or bootstrap paths.

## 8. Rewrite Summary

The Rust rewrite should preserve the software's temperament, not its spaghetti.

Its identity is:

- strict
- local
- identity-preserving
- self-healing
- intolerant of stale ambiguity

## 9. Next Spec Pass

The next refinement pass should expand this outline into:

- `Must-have v1`
- `Nice-to-have later`
- `Precise data model`
- `CLI/API surface`
- `Acceptance test matrix`
