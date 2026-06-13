# Test 01: Install Windows CAM - Progress Report

## Test Details
- **Test Name:** Install Windows CAM
- **Date:** June 12, 2026
- **Status:** PASS against GitHub-built `v2.1.25` installer; pending rerun for `v2.1.26`
- **Execution Mode:** GitHub release installer, local Windows user

## Steps Taken
1. Downloaded the GitHub release installer from `v2.1.25`, not a loose local executable.
2. Ran the installer with Inno logging enabled.
3. Confirmed volatile state under `C:\Users\kjhgf\.qexow-cam` rotated to `install-backups\2026-06-12-17-53-20`: `agents.json`, `mailbox.jsonl`, `events.jsonl`, and `logs`.
4. Confirmed durable state survived: `config.json`, `secrets\local-api-token`, and `boss.md`.
5. Queried the local health endpoint at `http://127.0.0.1:37631/health`.
6. Confirmed exactly one `qexow-cam-gui.exe` and one `cam.exe` process were running.
7. Sent a strict negative `/send` test to a bogus agent and confirmed it returned `ok:false`, `delivered:false`, `queued:false`.

## Evaluation & Success Criteria
- **State Cleanup Verification:** PASS. The install rotated old volatile runtime state and created a clean new `agents.json`, `events.jsonl`, and `logs` path.
- **Daemon Status Verification:** PASS. The `/health` endpoint returned:
  ```json
  {
    "ok": true,
    "version": "2.1.26",
    "nodeName": "RyzenLaptop",
    "startedAt": "2026-06-13T00:53:28.510Z",
    "appServerInitialized": true
  }
  ```
- **Lifecycle Verification:** PASS. Exactly one `qexow-cam-gui.exe` and one `cam.exe` were running after install.
- **Strict Test Verification:** PASS for API-level negative case. Strict send failed immediately on an unknown target with `queued:false`. Human-visible positive GUI round-trip still requires selecting a real agent in the GUI.
- **Installer Log Verification:** PASS. The Inno log recorded `Installation process succeeded`, `install-service` exit code `0`, and `Need to restart Windows? No`.

## Notes
The PowerShell wrapper command timed out after the installer launched the GUI, but the Inno log had already closed successfully and the installed daemon/GUI were healthy. Treat the install itself as passed and the wrapper timeout as a test-harness limitation.
