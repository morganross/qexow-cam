1. The GUI marked `TEST PASS`, but the persistent test ledger never records a final `passed` state.  
`tests.jsonl` only has `started`, `outbound_delivered`, and `reply_received`. The only final success marker is `windows-gui.log: test-ok`, which is weaker and separate from the structured test record.

2. `agents.json` has stale delivery state.  
For `dogmeat-dog-meat-agent`, `lastDelivery.delivery` is still `started`, even though `events.jsonl` correctly records the same message as `delivery: delivered`.

3. The reply body is self-reported and not trustworthy enough.  
The received body says `Status: active`, but the registry later says the agent is `idle`. CAM accepted that text as a valid status without verifying it against daemon/app-server state.

4. The GUI pass condition is still too body-token based.  
It validates marker/correlation/source/target/type/delivery, but it does not require a structured status payload or daemon-stamped status. So a reply can be technically valid while semantically weak or misleading.

5. The GUI and logs disagree on counts.  
Your GUI showed `14 active/testable, 10 skipped/limited`; `windows-gui.log` says `active=24 total=25 skipped=1` and `agents-loaded count=24`; daemon logs repeatedly say `skippedThreads=5`. Those counters are not using one consistent definition.

6. Five Antigravity/session mappings are repeatedly skipped for missing workspace path.  
Examples include `qexow-agent`, `cam-and-codex-agy-bridge-agent`, `run-translation-agent-preset-agent`, `translate-searchbox-agent`, and one long scratch-directory agent. This is noisy and still repeats every sync.

7. There is startup race noise.  
GUI first logs `health-error Unable to connect`, then discovery returns `count=29`, then active filter applies `active=0 total=0`, then one second later it loads real agents. The GUI masks some of this, but the log still shows a race.

8. The target agent replied in chat after sending CAM.  
It did send the CAM reply correctly, but the transcript also contains a final chat answer: “CAM reply sent via Qexow CAM.” That is not necessarily fatal, but it means the test is not proving “CAM-only behavior.”

9. Discovery is now installed and working.  
Installed CAM is `2.1.30`, `query_threads.py` returns `29`, and it finds `Dogmeat dog meat`. Archived `workhorse worker` is not found, which is correct after you archived it.

10. One of these problems is extremely easy to fix.  
The test message will be: hello, how is your day? Respond with the technical data we're asking for, and what is the capital of Missouri? Period. We are going to do a keyword search. If the words Jefferson City, or Jefferson, or city appear or not appear, that is going to be the success or fail.
