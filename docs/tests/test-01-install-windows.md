# Test 01: Install Windows CAM - Progress Report

## Test Details
- **Test Name:** Install Windows CAM
- **Date:** June 12, 2026
- **Status:** Pending re-run after installer-state cleanup change
- **Execution Mode:** Headless source daemon, local user

## Steps Taken
1. Install from the GitHub-built installer, not from a loose local executable.
2. Confirm the installer kills old `qexow-cam-gui.exe`, `cam.exe`, and legacy process names.
3. Confirm volatile state under `C:\Users\kjhgf\.qexow-cam` is rotated to `install-backups\<timestamp>`: `agents.json`, `mailbox.jsonl`, `events.jsonl`, and `logs`.
4. Confirm durable state survives: `config.json`, `secrets\local-api-token`, and `boss.md` if present.
5. Query the local health endpoint at `http://127.0.0.1:37631/health`.
6. Confirm the GUI and `/health` show the same package version.

## Evaluation & Success Criteria
- **State Cleanup Verification:** The install rotates old volatile runtime state and does not allow stale registry/mailbox data to affect a fresh GUI test.
- **Daemon Status Verification:** The `/health` endpoint should return:
  ```json
  {
    "ok": true,
    "version": "2.1.25",
    "nodeName": "RyzenLaptop",
    "startedAt": "<fresh install timestamp>",
    "appServerInitialized": true
  }
  ```
- **Lifecycle Verification:** Exactly one `qexow-cam-gui.exe` and one `cam.exe` should be running after install.
- **Strict Test Verification:** A GUI test must fail immediately on queued/errored delivery and must pass only after the selected agent sends a CAM reply to the dedicated test mailbox.
