# Mission Critical Enrollment Software Correction

Date: 2026-06-15.

## User Statement

no. imagine you are buildging the entire computer system for a mission critical maned moon misiion, thats the level of detailed progress we need.

the software we make today for registing users will be 10 -100x bigger, more lines of code then whatever the origanl software was. we are make a whole new software in C that is a add on side load helper app. it does enrollement.

You are in extreme violation of written planning rules and documents, so you're going to need to write this to a new document. The biggest mistake you've been making is considering things as variables. What you see as a variable is should be a software. So we're not just picking up a value and using it. No, no, no. We have extremely aggressive, robust software simply for discovery, extremely aggressive, robust for writing it, with fallbacks and backup plans, extremely, extremely robust for healing it and checking it and monitoring it, and extremely robust logic for recovery. And that's for literally every variable. So you've been wrong about everything. You need to go back and fix the software from the beginning.

Your first task is to write down everything I said again.

## Fix Log So We Can Return Here

This section records the fixes and corrective direction so the work does not loop back into another explanation-only pass.

## Implemented Release Fixes

These fixes were implemented before this document and published in the Rust/Electron release commit.

### Remote Inventory Exports Metadata, Not Just Registered Agents

Problem:

```text
Remote CAM could read remote Codex thread metadata, but inventory export only sent already-registered CAM agents.
```

Effect:

```text
Local CAM never saw most remote active chat metadata.
```

Fix:

```text
Remote inventory export now includes explicit CAM agents plus classified Codex discovery metadata rows from the remote thread database.
```

Important meaning:

```text
Gathered metadata must be enrolled/exported, not merely discovered.
```

### Remote Metadata Classification Comes From The Thread Database

Problem:

```text
Remote active/archive metadata was being treated as hard or mysterious.
```

Fix:

```text
Remote CAM reads the remote Codex SQLite thread database and uses threads.archived.
```

Rule:

```text
threads.archived == 0 -> active
threads.archived != 0 -> archived
```

Source:

```text
thread_database
```

### Inventory-Only Metadata Records Are Real

Problem:

```text
The system confused permanent CAM agents with remote chat metadata records.
```

Fix:

```text
Inventory-only remote mirror records are synthesized from real Codex thread database metadata.
```

Important meaning:

```text
An inventory-only mirror is not fake data, mock data, demo data, placeholder data, or chat content.
It is an addressable metadata record derived from real provider metadata.
```

### Local Peer Sync Mirrors Remote Metadata

Problem:

```text
Remote discovery data existed but was not enrolled into local mirror registry records.
```

Fix:

```text
Local peer sync imports remote inventory metadata into local remote_mirror records.
```

The mirror preserves:

```text
thread_id
cwd
chat_status
chat_status_source
route
peer
registry name
```

### Local Desktop Overlay Is A Secondary Evidence Layer

Problem:

```text
The system discussed local Desktop knowledge but did not make the merge visible.
```

Fix:

```text
Local CAM scans both known local Desktop thread databases and attempts exact thread_id overlay onto remote mirrors.
```

Known local paths:

```text
C:\Users\kjhgf\.codex\state_5.sqlite
C:\Users\kjhgf\.codex\sqlite\state_5.sqlite
```

The overlay reports:

```text
attempted
source_paths
evidence_rows
remote_mirrors_seen
matched
applied
conflicts
unmatched
warning
```

### Peer Send Ensures Remote Registry Mapping Exists

Problem:

```text
Local CAM could mirror inventory-only remote metadata, but remote CAM might not have a persistent agent row for the synthesized mirror name.
```

Failure shape:

```text
unknown target and strict delivery is enabled
```

Fix:

```text
Before remote send, peer delivery checks remote agent status.
If the remote agent mapping is missing, it creates it from known thread_id and cwd.
Then it runs the real remote send.
```

Important meaning:

```text
The metadata registry write is part of delivery readiness.
```

### Peer Send Output Was Cleaned

Problem:

```text
The remote ensure/create step printed JSON before the send JSON, so the local peer protocol parser saw invalid JSON.
```

Fix:

```text
The remote ensure/create mapping output is silenced.
The only stdout consumed by the peer protocol is the actual send result JSON.
```

### Full Path Was Proven

Verified on 2026-06-15:

```text
frontend inventory export returned 322 records
frontend metadata classification included 87 active and 227 archived from thread_database
local peer sync imported the frontend mirror metadata
local CAM sent to existing frontend thread 019ea189-0bca-70a3-876f-17034c85b036
remote turn id returned 019ecb8a-76f8-7ce1-898d-14fc766ad7e0
remote content response read back CAM_RELEASE_VERIFICATION_20260615C_OK
```

## Current Uncommitted Fix

This fix was made after the release and is currently uncommitted at the time this section was written.

### Strong Thread Database Metadata Bypasses Project-Root Rejection

Problem:

```text
The classifier rejected local route metadata when workspace_in_project was false.
That was too broad.
It blocked strong Codex thread-database metadata even though the archive evidence was already present.
```

Original line:

```rust
if matches!(row.route, Route::Local) && !row.workspace_in_project {
```

First minimal fix:

```rust
if matches!(row.route, Route::Local) && !row.workspace_in_project && row.source != DiscoverySource::ThreadDatabase {
```

More robust fix now in code:

```rust
if matches!(row.route, Route::Local)
    && !row.workspace_in_project
    && !has_thread_database_archive_evidence(&row)
```

New predicate:

```rust
fn has_thread_database_archive_evidence(row: &DiscoveryRow) -> bool {
    row.source == DiscoverySource::ThreadDatabase
        && row.chat_status_source == ChatStatusSource::ThreadDatabase
        && row.chat_status != ChatStatus::Unknown
}
```

Reason text added for approved rows:

```text
trusted Codex thread database archive metadata
```

Build proof:

```text
cargo build --release
Finished release build successfully
```

## Terminology Fix

Correction:

```text
CAM does not manage chats.
CAM manages chat metadata.
```

The chat itself belongs to the provider.

CAM cares about metadata needed to:

```text
discover
classify
enroll
route
monitor
heal
recover
report
```

Therefore future docs and code comments should say:

```text
chat metadata
registry metadata records
archive metadata
remote mirror metadata
```

Do not imply CAM owns or manages the chat itself.

## Remaining Direction

The next work should not be another classifier discussion.

The next work should turn metadata enrollment into explicit software:

```text
discovery software
registry writer software
registry verifier software
monitoring software
healing software
recovery software
GUI/reporting software
proof command/software
```

For every important metadata value, the question is not:

```text
what variable stores this?
```

The question is:

```text
what subsystem owns this metadata lifecycle?
```

