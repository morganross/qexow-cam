# Test 01: Install Windows CAM - Progress Report

## Test Details
- **Test Name:** Install Windows CAM
- **Date:** June 12, 2026
- **Status:** PASS against GitHub-built `v2.1.26` installer
- **Execution Mode:** GitHub release installer, local Windows user

## Steps Taken
1. Downloaded the GitHub release installer from `v2.1.26`, not a loose local executable.
2. Ran the installer with Inno logging enabled.
3. Confirmed volatile state under `C:\Users\kjhgf\.qexow-cam` rotated to `install-backups\2026-06-12-18-13-29`: `agents.json`, `mailbox.jsonl`, `events.jsonl`, and `logs`.
4. Confirmed durable state survived: `config.json`, `secrets\local-api-token`, and `boss.md`.
5. Queried the local health endpoint at `http://127.0.0.1:37631/health`.
6. Confirmed exactly one `qexow-cam-gui.exe` and one `cam.exe` process were running.
7. Sent a strict negative `/send` test to a bogus agent and confirmed it returned `ok:false`, `delivered:false`, `queued:false`.
8. Sent an API-level diagnostic reply to `CAM test, Kexau CAM test suite mailbox` and confirmed the daemon returned `received:true`, `queued:false`, and `message.delivery:"received"`.

## Evaluation & Success Criteria
- **State Cleanup Verification:** PASS. The install rotated old volatile runtime state and created a clean new `agents.json`, `events.jsonl`, and `logs` path.
- **Daemon Status Verification:** PASS. The `/health` endpoint returned:
  ```json
  {
    "ok": true,
    "version": "2.1.26",
    "nodeName": "RyzenLaptop",
    "startedAt": "2026-06-13T01:13:43.856Z",
    "appServerInitialized": true
  }
  ```
- **Lifecycle Verification:** PASS. Exactly one `qexow-cam-gui.exe` and one `cam.exe` were running after install.
- **Strict Test Verification:** PASS for API-level negative case. Strict send failed immediately on an unknown target with `queued:false`.
- **Diagnostic Reply Semantics:** PASS at API level. The dedicated GUI-test mailbox now returns `delivery:"received"` for successful intake; a queued-only matching reply is evidence, not a pass condition.
- **Installer Log Verification:** PASS. The Inno log recorded `Installation process succeeded`, `install-service` exit code `0`, and `Need to restart Windows? No`.

## Notes
The installed GUI still needs a human-visible positive round-trip click against a real selected agent for final UX confirmation. The underlying daemon semantics that caused the false pass were verified directly: mailbox receipt is now `received`, and strict send failure does not queue.
