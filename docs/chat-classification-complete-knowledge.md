# Chat Classification Complete Knowledge

Last updated: 2026-06-15.

Purpose: record everything currently known about how CAM should classify chats as active, archived, or unknown.

## Core Meaning

CAM chat classification is about archive membership.

For CAM:

- `active` means the chat is not archived.
- `archived` means the chat is archived.
- `unknown` means CAM does not currently have archive-capable evidence.

This is intentionally different from runtime state.

Do not confuse CAM chat classification with:

- Codex runtime `active`.
- Codex runtime `idle`.
- Whether a turn is currently running.
- Whether a process exists.
- Whether a session file exists.
- Whether a chat has unread messages.
- Whether a thread can be resumed.

Codex may use words like active, idle, and unknown for turn/process state. CAM's `chat_status` must stay binary when evidence exists: active or archived.

Unknown is allowed only when archive evidence is unavailable.

## Strong Rule

The best classifier is the archive bit from a real Codex thread database.

For Codex thread databases:

```text
threads.archived == 0 -> active
threads.archived != 0 -> archived
```

Source name:

```text
thread_database
```

This is strong evidence.

## Local Codex Chat Classification

Local CAM classifies local Codex chats by reading the local Codex thread database.

Known local Windows database paths:

```text
C:\Users\kjhgf\.codex\state_5.sqlite
C:\Users\kjhgf\.codex\sqlite\state_5.sqlite
```

Known Linux database path:

```text
/home/ubuntu/.codex/state_5.sqlite
```

Relevant SQLite table:

```text
threads
```

Relevant columns:

```text
id
title
cwd
updated_at
updated_at_ms
archived
rollout_path
```

The core classifier query is:

```sql
SELECT id, title, updated_at, updated_at_ms, archived FROM threads
```

For each row:

```text
id -> thread_id
title -> display title / discovery title
archived -> active or archived
updated_at / updated_at_ms -> evidence timestamp
```

If the database can be opened and the row has `archived`, classification is strong.

If the database cannot be opened, CAM must fail loudly in logs/status and continue.

If the database exists but a thread is not present, CAM must not invent a status for that thread.

## Remote Codex Chat Classification

Remote CAM should classify chats local to that remote machine by reading that remote machine's own Codex thread database.

Example for frontend:

```text
frontend host: mbncytfvkju
frontend Codex DB: /home/ubuntu/.codex/state_5.sqlite
```

On 2026-06-15, frontend had:

```text
314 Codex thread rows
87 active rows
227 archived rows
```

This proved remote CAM can classify remote-local chats from the remote node's own `threads.archived` data.

Remote classification source is still:

```text
thread_database
```

Remote CAM should export that classification to local CAM through peer inventory.

## Important Remote Discovery Fix

The old broken behavior was:

```text
inventory export only included persistent CAM agents
```

That meant remote CAM could classify hundreds of remote Codex chats, but local CAM only saw the small subset already promoted to CAM agents.

That was wrong.

The corrected behavior is:

```text
inventory export includes explicit CAM agents plus classified Codex discovery rows from the remote thread database
```

This lets local CAM mirror remote chats even when the remote node has not permanently promoted every discovered chat into a CAM agent.

Inventory-only remote mirror records are synthesized for export.

They are real mirrors of real Codex threads.

They are not fake data.

They are not mock data.

They are not placeholder data.

They carry:

```text
thread_id
cwd
chat_status
chat_status_source
title-derived canonical name
original discovery disposition/reason in last_error
```

The `last_error` note on an inventory-only mirror is explanatory provenance, not a delivery failure.

## Local CAM Peer Sync Classification

Local CAM gets remote chat classification through `peer sync`.

Flow:

```text
local CAM -> SSH to remote CAM -> remote qexow-cam inventory export -> parse inventory -> mirror agents locally
```

For each remote inventory agent:

```text
remote chat_status -> local mirror chat_status
remote chat_status_source -> local mirror chat_status_source
remote thread_id -> local mirror thread_id
remote cwd -> local mirror cwd
remote route local -> local route peer:{peer_name}
```

Local mirror names use this shape:

```text
{peer_name}::{remote_agent_name}
```

Example:

```text
frontend::learn-as-much-as-you-can-about-our-software-you-are-on-the-frontend-it-s-a-wordpress-plugin-inside-of-wordpress-inside-of-a-container-we-use-bitnami-it-s-on-oci-amphir-arm64-read-only-mode-019ea189
```

After the 2026-06-15 fix, local CAM imported frontend mirrors with:

```text
87 active
227 archived
7 unknown
```

The 7 unknown records were existing CAM agents without archive-capable thread evidence.

## Local Desktop Overlay

Local CAM should also use local Codex Desktop knowledge when local Desktop has archive-capable evidence for a remote chat.

This is a second layer.

It does not replace remote CAM inventory.

It combines with remote CAM inventory.

Known local Desktop archive evidence sources:

```text
C:\Users\kjhgf\.codex\state_5.sqlite
C:\Users\kjhgf\.codex\sqlite\state_5.sqlite
```

Local CAM scans both paths for thread archive evidence.

For each peer mirror:

```text
if peer mirror thread_id exists in local Desktop thread database:
    apply Desktop archive classification
    set chat_status_source = desktop_thread_database
else:
    keep remote inventory classification
```

Desktop overlay matches by exact `thread_id`.

Do not match by title.

Do not match by fuzzy title.

Do not match by cwd alone.

Do not match by host name alone.

If local Desktop and remote inventory disagree, CAM should:

```text
record conflict count
set visible last_error on the mirror
preserve loud status/log evidence
continue running
```

The conflict should not be silent.

## What Local Desktop Actually Exposed On 2026-06-15

Local Desktop global state file:

```text
C:\Users\kjhgf\.codex\.codex-global-state.json
```

It exposed remote host/project metadata.

Known keys included:

```text
codex-managed-remote-connections
remote-projects
remote-connection-auto-connect-by-host-id
remote-connection-analytics-id-by-host-id
electron-persisted-atom-state.unread-thread-ids-by-host-v1
```

For frontend:

```text
hostId = remote-ssh-discovered:frontend
remotePath = /home/ubuntu
label = Stage Front
project id = 4ef78abe-e651-48a0-98d3-0f64c567cba9
```

It did not expose archive-capable records for the six named frontend chats searched during the investigation:

```text
GitHub
FlowLab front
website style stage front
chatbot style stage front
speed stage front
no more
```

It did expose an unread frontend thread id at one point:

```text
019ea189-0bca-70a3-876f-17034c85b036
```

Unread membership is not archive membership.

Unread membership must not classify active or archived.

Therefore, local Desktop global state can be used for remote node/project discovery, but not for chat active/archive classification unless it contains archive-capable evidence.

## Weak Evidence That Must Not Classify Chats

Never classify active/archive from:

```text
session file presence
rollout JSONL presence
runtime process presence
active turn id presence
last turn id presence
unread thread membership
pinned thread membership
prompt history presence
thread id presence alone
title presence alone
cwd presence alone
remote host/project presence alone
copied agents.json snapshots without archive evidence
old cache files without archive evidence
mailbox presence
queue presence
```

These may be useful for diagnostics.

They are not archive classification sources.

## Source Precedence

Strong classification sources:

```text
thread_database
desktop_thread_database
remote_inventory when it preserves a strong remote source
```

Weak/non-classifying sources:

```text
session_presence
runtime_presence
unread_state
prompt_history
rollout_presence
snapshot_presence
unknown
```

Current code treats old persisted `session_presence` as an alias for `unknown`.

This prevents stale weak classifications from surviving migrations.

## Merge Rules

Remote CAM should classify its own local chats first.

Local CAM should import remote CAM's classified inventory.

Local CAM should then overlay local Desktop archive evidence when it has exact thread-id evidence.

Practical order:

```text
1. Remote node reads its own Codex thread DB.
2. Remote node exports all classified Codex thread rows in inventory.
3. Local node imports peer inventory as remote mirrors.
4. Local node scans its local Desktop thread DBs.
5. Local node overlays Desktop classification only on exact thread-id matches.
6. Local node reports unmatched and conflicts loudly.
```

Do not silently degrade.

Do not silently fallback.

Do not erase a known active/archive status with unknown.

Do not classify from weak evidence just to avoid unknown.

## GUI Requirements

The GUI should show classification as health evidence, not as a vague failure.

For every chat or mirror, GUI should show:

```text
chat_status
chat_status_source
thread_id
route
peer_name when remote
last_error when present
```

For discovery/peer sync, GUI should show:

```text
remote agents seen
mirrors added
mirrors updated
mirrors skipped
mirrors stale
collision count
desktop archive merge attempted
desktop source paths
desktop evidence rows
desktop remote mirrors seen
desktop matches
desktop applied
desktop conflicts
desktop unmatched
desktop warning
```

A good GUI message is:

```text
Frontend peer inventory imported 322 records. 87 active and 227 archived came from the remote thread database. Local Desktop overlay scanned 1542 rows but matched 0 remote mirrors.
```

A bad GUI message is:

```text
Doesn't work.
```

## Logging Requirements

Logs must record:

```text
which classifier ran
which database path was read
whether database read succeeded
how many rows were read
how many rows were active
how many rows were archived
how many rows were unknown
how many peer mirrors were seen
how many Desktop overlay matches happened
how many conflicts happened
why a row remained unknown
```

Logs should be extreme verbose.

Logs should not log chat bodies unnecessarily.

It is acceptable to log metadata needed to debug classification:

```text
thread_id
title
cwd
route
source
status source
counts
error kind
```

## Delivery Implication

Classification affects routing decisions.

If a mirror is active or archived because it has a known thread id, CAM can attempt direct provider delivery against that known thread.

Inventory-only mirrors initially exist only in local CAM's mirror state.

The remote node may not have a persistent CAM agent row for the synthesized mirror name.

Therefore peer delivery must ensure the remote CAM agent mapping exists before sending.

Current fixed behavior:

```text
local mirror has thread_id and cwd
peer SSH command runs remote agent status
if missing, peer SSH command creates remote agent with the known thread_id and cwd
peer SSH command then runs remote send
remote send uses real provider delivery
```

This fixed the bug where local CAM could mirror an existing remote Codex thread but remote send failed with:

```text
unknown target and strict delivery is enabled
```

## Proven Full Path

Verified on 2026-06-15:

```text
local build succeeded
frontend ARM build succeeded
frontend install active
backend install active
frontend inventory export returned 322 agents
frontend inventory included 87 active and 227 archived from thread_database
local peer sync imported frontend mirrors
local send delivered to existing frontend thread 019ea189-0bca-70a3-876f-17034c85b036
remote turn id returned: 019ecb8a-76f8-7ce1-898d-14fc766ad7e0
remote content response read back: CAM_RELEASE_VERIFICATION_20260615C_OK
```

This is the acceptable full test shape:

```text
build app
install app on nodes
ask an existing agent/chat to send or receive a real message
read the real response content back in chat
```

## Known Remote Nodes

Known CAM peer nodes from current work:

```text
local Windows: RyzenLaptop
frontend: ubuntu@159.54.190.35
backend: ubuntu@152.70.122.218
```

Frontend identity:

```text
hostname: mbncytfvkju
Codex DB: /home/ubuntu/.codex/state_5.sqlite
```

Backend identity:

```text
hostname: backendfix523
Codex DB: /home/ubuntu/.codex/state_5.sqlite when Codex is used there
```

## Open Truths

Local Desktop may know remote nodes without locally caching archive-capable records for every remote chat.

Remote CLI nodes may have chats started by:

```text
local Codex Desktop
regular Codex CLI on that node
CAM delivery
other agents
```

The robust classifier must support all of those.

If the remote node's Codex DB has the thread row, remote CAM can classify it.

If local Desktop has archive-capable evidence for the same thread id, local CAM can overlay it.

If neither source has archive-capable evidence, status must remain unknown.

Unknown is not a failure by itself.

Unknown is a failure only when a strong evidence source exists and CAM failed to read or merge it.

## Compact Doctrine

Classify from archive-capable evidence only.

Remote CAM classifies remote-local chats.

Local CAM imports remote classification.

Local CAM overlays local Desktop classification when Desktop has exact thread-id archive evidence.

Every source and failure must be loud, visible, and inspectable.

Never let promotion policy hide classification evidence.

Never let weak evidence masquerade as active/archive truth.

