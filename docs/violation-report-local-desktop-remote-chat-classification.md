# Violation Report: Local Desktop Remote Chat Classification

Date: 2026-06-15.

## Summary

CAM currently fails to combine remote CAM inventory with local CAM's own Codex Desktop knowledge for remote chats.

This is an extreme major violation of the intended CAM discovery and classification plan.

The current implementation treats remote chat status as mostly owned by remote CAM inventory, while local CAM's Desktop database knowledge is used only for local agents.

That is wrong.

## Expected Behavior

Local CAM should combine multiple truthful sources:

- Remote CAM inventory from the remote node.
- Local CAM's own Codex Desktop knowledge.
- Local Desktop's knowledge of remote nodes.
- Local Desktop's knowledge of remote chats.
- Local Desktop's active-versus-archived classification for remote chats when Desktop has that information.

The design expectation is not either/or.

The design expectation is combined evidence.

Remote CAM should classify its own local chats when it has enough information.

Local CAM should also use its own Desktop knowledge to classify remote chats when Desktop has the archive membership state.

## Current Incorrect Behavior

Local CAM currently uses the local Codex Desktop thread database for local agents.

Local CAM currently imports remote agents through `peer sync` remote inventory.

Local CAM does not currently perform the missing bridge step:

```text
local Codex Desktop remote-chat knowledge -> CAM remote mirror chat_status
```

Because that bridge is missing, remote chats that local Desktop knows are active or archived may remain `unknown` in CAM, or may depend only on what remote CAM can prove locally.

This means CAM is ignoring one of its most important local truth sources.

## Why This Is A Violation

The plan was that CAM-to-CAM communication would be combined with local CAM's own knowledge.

The plan was not:

```text
remote mirrors only trust remote CAM inventory
```

The plan was:

```text
remote CAM tells local CAM what the remote node knows
local CAM also checks what local Desktop knows
CAM combines both sources loudly and visibly
```

The current implementation violates that plan by failing to use local Desktop's remote-chat archive knowledge.

## Impact

The GUI can show incomplete or misleading remote chat classification.

Remote chats visible as active or archived in local Codex Desktop may appear as `unknown` in CAM.

CAM may make worse routing decisions because it lacks local Desktop's remote-chat archive membership data.

Operators may think remote CAM cannot classify a chat, when local Desktop may already have the needed classification.

The system loses a major robustness layer.

The system violates the intended "combine all real evidence" doctrine.

## Correct Rule

Remote chat classification should use a layered evidence model:

1. Ask remote CAM what the remote node knows.

2. Ask local Codex Desktop what the local Desktop knows about remote nodes and remote chats.

3. Merge those facts.

4. Keep source provenance visible.

5. Never silently fallback.

6. Never invent classification from weak evidence.

7. If sources disagree, show the conflict loudly.

## Required Correction

Implement a local Desktop remote-chat classification bridge.

The bridge must discover:

- Which remote nodes Codex Desktop knows about.
- Which remote chats Codex Desktop knows about.
- The local Desktop archive state for those remote chats.
- The mapping between Desktop remote chat identity and CAM remote mirror agent identity.

Then CAM must merge that evidence into remote mirror agents.

The merge must preserve provenance.

Possible future source names:

```text
desktop_remote_thread_database
desktop_remote_inventory
local_desktop_remote_chat_state
```

The exact source name should be chosen during implementation, but it must clearly mean:

```text
local Desktop archive evidence about a remote chat
```

## Required GUI Behavior

The GUI must show:

- Remote CAM's classification, if available.
- Local Desktop's classification for the same remote chat, if available.
- The merged CAM classification.
- The source of the merged classification.
- Any disagreement between remote CAM and local Desktop.

The GUI must not collapse this into a vague `does not work` status.

## Required Logging Behavior

Logs must show:

- Whether local Desktop remote-chat discovery was attempted.
- Which Desktop data source was read.
- How many remote nodes were found in Desktop data.
- How many remote chats were found in Desktop data.
- How many remote chats matched CAM remote mirror agents.
- How many classifications were applied.
- How many records could not be matched.
- Any source disagreement.

## Non-Negotiable Constraint

Do not classify from weak evidence:

- Session file presence is not enough.
- Thread id presence is not enough.
- Runtime active/idle is not enough.
- Rollout JSONL presence is not enough.
- Copied snapshots are not enough unless they include archive membership evidence.

Local Desktop remote-chat classification is only valid if it comes from archive-capable Desktop data.

## Status

Violation confirmed.

Corrective implementation completed for the real blocking behavior.

The investigation found that the local Codex Desktop global state currently exposes remote host/project metadata for `remote-ssh-discovered:frontend`, but it does not contain archive-capable records for the six exact frontend chat titles searched during this fix.

The implemented correction is therefore layered:

- Remote CAM exports all real local Codex thread-database rows that have `threads.archived` evidence, not only persistent CAM agent rows.
- Local CAM peer sync imports those remote records as remote mirrors with `chat_status` and `chat_status_source` preserved.
- Local CAM scans both known local Desktop thread databases and overlays archive evidence when a remote mirror thread id is actually present there.
- The Desktop overlay reports attempted source paths, evidence rows, remote mirrors seen, matches, applied rows, conflicts, and unmatched rows.
- Inventory-only remote mirrors can now be sent to because peer delivery ensures a remote CAM agent mapping exists for the mirrored thread before calling the real remote send path.

Verified on 2026-06-15:

- Frontend inventory export changed from 25 CAM-agent-only rows to 322 rows.
- Frontend remote classification included 87 active and 227 archived chats from the remote `threads.archived` source.
- Local peer sync imported frontend mirrors with 87 active and 227 archived classifications.
- A full real build/install/use path delivered to existing frontend thread `019ea189-0bca-70a3-876f-17034c85b036`.
- The remote agent response read back through CAM was `CAM_RELEASE_VERIFICATION_20260615C_OK`.
