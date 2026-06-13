# CAM 100-Issue Implementation Audit for v2.1.31

Date: 2026-06-12 local

Scope: audit the last 100 issues listed in chat against the current local source tree after commit `bf0e2c1` / release `v2.1.31`.

Tests run:

- `npm run test:strict-contract`: PASS.
- Native discovery smoke: `discoverThreads()` returned `count=30`, `dogmeat=true`, `workhorse=false`, `remoteCount=0`, `antigravityCount=15`.
- Runtime/installer forbidden-launch scan over `src/daemon.js`, `src/windows/QexowCamGui.cs`, `src/tray/CamTray.cs`, and `installer.iss`: no matches for `query_threads.py`, `FileName = "python"`, `execFile(`, `tryPython`, `python3`, `cmd.exe`, `bash.exe`, `conhost.exe`, `powershell.exe`, or `RunPreinstallCleanupPowerShell`.
- GUI matcher inspection: current GUI still requires `CAM_GUI_TEST_RESPONSE`, exact source/target/correlation/message type/timestamp, and `delivery=received`; it does not require Jefferson/Missouri content.
- Daemon delivery inspection: strict send, no queue on strict failure, stale repair, `received` mailbox replies, and delivered outbound events exist.
- Installer inspection: reinstall resets runtime state; uninstall deletes `.qexow-cam`; installer no longer ships `query_threads.py`; installer no longer invokes PowerShell cleanup.

## Results

1. CAM test passed without checking for `Jefferson`.
Test: searched GUI matcher for `Jefferson` and `Missouri`.
Implemented: no.
Worked: no.
Why not: GUI still passes on envelope plus `CAM_GUI_TEST_RESPONSE`; semantic keyword check is absent.

2. Test only verified transport/envelope, not semantic content.
Test: inspected `FindMailboxResponse()`.
Implemented: partially.
Worked: transport works; semantics do not.
Why not: validation checks source, target, correlation, message type, timestamp, delivery, and marker, but not an answer to a natural-language question.

3. Dogmeat reply body was allowed to be formulaic.
Test: inspected current test prompt and matcher.
Implemented: no.
Worked: no.
Why not: prompt still asks for a fixed marker plus agent/node/status.

4. Dogmeat also posted a normal chat confirmation.
Test: inspected Dogmeat rollout transcript.
Implemented: no.
Worked: no.
Why not: CAM can require mailbox reply, but it does not currently prevent or penalize a visible chat follow-up.

5. GUI treated mailbox receipt as enough for pass.
Test: inspected pass branch after `WaitForMailboxResponse()`.
Implemented: partially.
Worked: partly.
Why not: it now requires `delivery=received`, but still treats a matched mailbox response as pass without semantic challenge.

6. GUI did not prove CAM-only behavior.
Test: inspected transcript and GUI matcher.
Implemented: no.
Worked: no.
Why not: no check exists that the agent avoided a normal chat final answer.

7. GUI did not require "capital of Missouri" answer.
Test: searched GUI code for `Missouri`, `Jefferson`.
Implemented: no.
Worked: no.
Why not: the requested content test was documented but not coded.

8. Test prompt did not ask a natural language question.
Test: inspected GUI test prompt at `QexowCamGui.cs`.
Implemented: no.
Worked: no.
Why not: prompt remains a technical protocol instruction.

9. Test prompt over-instructed the technical reply path.
Test: inspected GUI test prompt.
Implemented: no.
Worked: no.
Why not: prompt still explicitly tells the agent the target, correlation ID, message type, marker, and response fields.

10. Test body could be satisfied by copying boilerplate.
Test: inspected matcher and prompt.
Implemented: no.
Worked: no.
Why not: body marker check is static and easy to echo.

11. Dedicated mailbox existed but was only a target row.
Test: inspected `#ensureBuiltinMailboxAgents()` and mailbox branch.
Implemented: partially.
Worked: partly.
Why not: mailbox exists and receives messages, but there is no separate receiver object beyond mailbox-agent convention.

12. Mailbox reply was accepted as truth without semantic validation.
Test: inspected `FindMailboxResponse()`.
Implemented: partially.
Worked: transport truth only.
Why not: no semantic or external status verification is attached to the reply.

13. `tests.jsonl` lacked final `passed` state.
Test: inspected daemon `appendTestEvent()` calls.
Implemented: no.
Worked: no.
Why not: daemon writes `started`, `outbound_delivered`, `reply_received`, and `failed`, but no final `passed` from GUI.

14. `agents.json` had stale `lastDelivery.delivery`.
Test: inspected daemon send path.
Implemented: no.
Worked: no.
Why not: `setAgent(... lastDelivery: message)` still happens while delivery is `started`; after `message.delivery = "delivered"`, registry is not updated again.

15. Events said delivered while registry said started.
Test: same as item 14.
Implemented: no.
Worked: no.
Why not: event append happens after mutation; registry was already written before final mutation.

16. GUI/log active counts disagreed.
Test: inspected GUI active filter and daemon registry source.
Implemented: partially.
Worked: likely improved, not proven in installed GUI.
Why not: GUI now uses daemon registry instead of its own Python classifier, but installed GUI was not relaunched/tested after v2.1.31.

17. Daemon skipped workspace-missing threads repeatedly.
Test: inspected `skippedThreadReasons` handling.
Implemented: yes.
Worked: likely yes.
Why not if not: it logs only when the reason changes; not live-run verified after reinstall.

18. Startup briefly showed zero mappings.
Test: inspected GUI startup/load flow.
Implemented: partially.
Worked: not proven.
Why not: GUI has better daemon registry path, but there is no explicit startup state machine test proving zero is suppressed.

19. Remote chats were attributed as local.
Test: native discovery smoke and route metadata code inspection.
Implemented: partially.
Worked: inconclusive.
Why not: code preserves remote metadata, but current smoke returned `remoteCount=0`, so no live remote attribution proof.

20. Discovery metadata did not preserve real route strongly enough.
Test: strict contract checks route metadata.
Implemented: yes.
Worked: code-level pass.
Why not if not: live remote route evidence was unavailable in this local smoke.

21. Discovery relied too much on one source earlier.
Test: inspected `src/thread-discovery.js`.
Implemented: yes.
Worked: yes.
Why not if not: current native discovery merges rollout, session index, state keys, and Antigravity.

22. Old active chat was missing from SQLite discovery.
Test: Dogmeat native discovery smoke.
Implemented: yes for rollout-discovered active chats.
Worked: yes.
Why not if not: SQLite is no longer the only discovery source.

23. Rollout/session discovery was needed as a normal source.
Test: native discovery found Dogmeat from rollout.
Implemented: yes.
Worked: yes.
Why not if not: no issue observed.

24. Archived chats must stay excluded.
Test: native discovery checked archived Workhorse session ID.
Implemented: yes.
Worked: yes.
Why not if not: `workhorse=false`.

25. New chats were discovered, old archived ones excluded.
Test: native discovery smoke.
Implemented: yes.
Worked: yes.
Why not if not: `dogmeat=true`, `workhorse=false`.

26. Some Antigravity chats lacked valid workspace paths.
Test: native discovery smoke and daemon apply inspection.
Implemented: partially.
Worked: partly.
Why not: Antigravity rows can still have `outside-of-project`; daemon skips invalid workspace for registry sync.

27. Skipped-thread logs were noisy.
Test: inspected `skippedThreadReasons`.
Implemented: yes.
Worked: likely yes.
Why not if not: not live-run verified for multiple sync intervals after v2.1.31.

28. Registry was additive and could preserve stale rows.
Test: inspected daemon apply and installer reset.
Implemented: partially.
Worked: install-time reset works; runtime remains additive.
Why not: daemon still logs prune skipped to avoid deleting active local agents.

29. Reinstall did not fully reset runtime state before fixes.
Test: strict contract installer checks.
Implemented: yes.
Worked: code-level pass.
Why not if not: not manually run through installer in this audit.

30. `agents.json` survived reinstall before fixes.
Test: installer inspection and strict contract.
Implemented: yes.
Worked: code-level pass.
Why not if not: installer deletes `agents.json` on install.

31. `mailbox.jsonl` survived reinstall before fixes.
Test: installer inspection.
Implemented: yes.
Worked: code-level pass.
Why not if not: installer deletes `mailbox.jsonl`.

32. `events.jsonl` survived reinstall before fixes.
Test: installer inspection.
Implemented: yes.
Worked: code-level pass.
Why not if not: installer deletes `events.jsonl`.

33. Old daemon metadata survived reinstall before fixes.
Test: installer inspection.
Implemented: yes.
Worked: code-level pass.
Why not if not: installer deletes `daemon.pid`, `daemon.json`, `tray.lock`, and `service.json`.

34. Installer cleanup claims were too broad.
Test: installer inspection.
Implemented: partially.
Worked: partly.
Why not: cleanup is now simpler and no-PowerShell; it cannot do every old process/path trick without external shell helpers.

35. Uninstall needed to remove all local CAM state.
Test: strict contract and installer `[UninstallDelete]`.
Implemented: yes.
Worked: code-level pass.
Why not if not: not manually uninstalled in this audit.

36. Old installs could remain side by side.
Test: installer `[InstallDelete]` and `InitializeSetup()`.
Implemented: partially.
Worked: likely improved.
Why not: installer removes known old directories, but no live machine install audit was run after v2.1.31.

37. Two CAM processes could run at once.
Test: current process check earlier showed 0 after kill; installer kills known names.
Implemented: partially.
Worked: current old processes stopped.
Why not: no postinstall one-GUI/one-daemon test was run for v2.1.31.

38. Installer did not reliably kill old versions.
Test: installer inspection.
Implemented: partially.
Worked: likely improved for known executable names.
Why not: PowerShell process-by-path cleanup was removed to satisfy no-popup rule, so weird legacy process names may still need native cleanup later.

39. Running raw executable bypassed installer lifecycle.
Test: package scripts and release workflow inspection from strict contract.
Implemented: partially.
Worked: release artifact path improved.
Why not: a developer can still run local executables manually if they already exist; policy is not absolute OS enforcement.

40. Startup registry could relaunch broken builds.
Test: registry removal command after stopping old CAM.
Implemented: yes for current machine emergency state.
Worked: yes.
Why not if not: `StartupRunValue` was empty after removal.

41. System tray icon had repeatedly failed.
Test: no GUI visual run in this audit.
Implemented: unknown.
Worked: not tested.
Why not: v2.1.31 was not installed/launched locally.

42. Status window had repeatedly failed to appear.
Test: no GUI visual run in this audit.
Implemented: unknown.
Worked: not tested.
Why not: v2.1.31 was not installed/launched locally.

43. GUI had removed older red/green status indicators.
Test: not audited in code.
Implemented: unknown.
Worked: not tested.
Why not: current task focused on diagnostics/popups; visual status indicator regression was not verified here.

44. GUI had shown wrong chats.
Test: native discovery and GUI now using daemon registry.
Implemented: partially.
Worked: likely improved.
Why not: installed GUI list was not tested after v2.1.31.

45. Active/archive classifier regressed.
Test: native discovery Dogmeat/Workhorse smoke.
Implemented: yes.
Worked: yes.
Why not if not: current discovery excludes archived Workhorse and includes active Dogmeat.

46. Test button had weak validation.
Test: inspected `ValidateStrictSend()` and matcher.
Implemented: partially.
Worked: envelope validation improved.
Why not: semantic content validation still missing.

47. GUI pass/fail wording was misleading.
Test: inspected output strings.
Implemented: partially.
Worked: partly.
Why not: it now has clearer states, but still prints `TEST PASS` for envelope-only success.

48. `delivery: queued` previously could appear near pass behavior.
Test: matcher inspection.
Implemented: yes.
Worked: code-level yes.
Why not if not: current matcher returns `TEST_FAIL` for non-`received` delivery.

49. Reply delivery states were overloaded.
Test: daemon branch inspection.
Implemented: yes.
Worked: code-level yes.
Why not if not: mailbox reply uses `received`; outbound thread send uses `delivered`; queued remains deferred.

50. `queued`, `delivered`, and `received` semantics were blurred.
Test: daemon branch inspection.
Implemented: yes.
Worked: code-level yes.
Why not if not: no mismatch found in inspected branches except stale registry issue.

51. Normal async messaging and diagnostic strict messaging were blurred.
Test: daemon `strict` path inspection.
Implemented: yes.
Worked: code-level yes.
Why not if not: strict mode exists and does not queue unresolved failures.

52. Strict mode needed hard failure on stale routes.
Test: strict contract.
Implemented: yes.
Worked: yes.
Why not if not: strict-contract passed.

53. Strict mode needed no silent queueing.
Test: strict contract and daemon inspection.
Implemented: yes.
Worked: yes.
Why not if not: unresolved strict failures return `queued:false`.

54. Stale thread IDs needed repair or hard failure.
Test: strict contract checks `#repairStaleThreadAndEnsure`.
Implemented: yes.
Worked: code-level pass.
Why not if not: no live stale-thread scenario was run.

55. Wrong-source replies needed to be ignored.
Test: matcher inspection.
Implemented: yes.
Worked: code-level yes.
Why not if not: wrong source is skipped or summarized.

56. Wrong-source replies must never pass.
Test: matcher inspection.
Implemented: yes.
Worked: code-level yes.
Why not if not: exact source check is enforced.

57. Chat-only replies must fail diagnostic tests.
Test: matcher design inspection.
Implemented: yes for mailbox absence.
Worked: code-level yes.
Why not if not: if no mailbox reply arrives, GUI times out and fails.

58. Reply `correlationId` was often missing earlier.
Test: helper/skill contract checks.
Implemented: yes.
Worked: code-level yes.
Why not if not: reply helper supports correlation ID; GUI accepts correlation or body ID, so strictness is not absolute.

59. Helper needed correlation/message type support.
Test: strict contract.
Implemented: yes.
Worked: yes.
Why not if not: CLI/helper support is covered by contract checks.

60. Skill instructions were doing too much of the protocol work.
Test: inspected current prompt and generated skill checks.
Implemented: no.
Worked: no.
Why not: agents are still instructed to use skill/helper and carry message fields manually.

61. Direct CAM HTTP was exposed conceptually in prompts.
Test: inspected daemon prompt construction.
Implemented: no.
Worked: no.
Why not: prompt still says use skill or send via CAM HTTP.

62. User expected Qexow to be the skill-facing interface.
Test: skill/prompt inspection.
Implemented: partially.
Worked: partly.
Why not: Qexow skill exists, but prompt still mentions direct CAM HTTP as an option.

63. Agents were told about helper scripts instead of a clean command.
Test: skill instruction inspection from transcript and antigravity generation check.
Implemented: no.
Worked: no.
Why not: skill still documents `Send-AgentMessage.ps1`.

64. Dogmeat used `Send-AgentMessage.ps1`.
Test: transcript inspection.
Implemented: not applicable.
Worked: mechanically yes.
Why not: this was evidence of current design, not a code defect by itself.

65. PowerShell helper usage was surprising and undesirable.
Test: skill instruction inspection.
Implemented: no.
Worked: no.
Why not: agent-facing Windows helper is still PowerShell; only CAM runtime/installer popup paths were removed.

66. Runtime Python popups appeared from CAM.
Test: forbidden-launch scan.
Implemented: yes.
Worked: code-level yes.
Why not if not: installed old version is stopped; v2.1.31 not locally installed.

67. `cam.exe -> python.exe -> conhost.exe` happened.
Test: daemon code scan after v2.1.31.
Implemented: yes.
Worked: code-level yes.
Why not if not: no Python launch path remains in daemon/GUI runtime files.

68. `query_threads.py` was launched every poll.
Test: daemon scan and strict contract.
Implemented: yes.
Worked: yes.
Why not if not: daemon now calls `discoverThreads()` directly.

69. Daemon spawned Python for discovery.
Test: daemon scan.
Implemented: yes.
Worked: yes.
Why not if not: `execFile` import and Python launch path removed.

70. GUI also had a Python classifier path.
Test: GUI scan.
Implemented: yes.
Worked: yes.
Why not if not: GUI now reads daemon registry.

71. Installer shipped `query_threads.py`.
Test: installer scan.
Implemented: yes.
Worked: yes.
Why not if not: installer no longer includes that file.

72. Old tray code referenced extracting `query_threads.py`.
Test: tray source scan.
Implemented: yes.
Worked: yes.
Why not if not: extraction line removed.

73. Hiding Python was not enough.
Test: architecture inspection.
Implemented: yes.
Worked: code-level yes.
Why not if not: runtime now avoids Python instead of hiding it.

74. Runtime must not spawn Python at all.
Test: forbidden-launch scan.
Implemented: yes.
Worked: code-level yes.
Why not if not: no Python launch path found in runtime/installer files scanned.

75. Runtime must not spawn terminal-backed helpers.
Test: forbidden-launch scan.
Implemented: partially.
Worked: runtime yes; agent skill no.
Why not: CAM runtime scan is clean, but qexow skill still uses a PowerShell helper when an agent sends a message.

76. Installer still had PowerShell cleanup paths.
Test: installer scan.
Implemented: yes.
Worked: yes.
Why not if not: PowerShell cleanup routines removed.

77. Installer PowerShell path was removed after stricter standard.
Test: installer scan and strict contract.
Implemented: yes.
Worked: yes.
Why not if not: strict contract passed.

78. Native JS discovery replaced Python discovery.
Test: source inspection and native smoke.
Implemented: yes.
Worked: yes.
Why not if not: `discoverThreads()` found current sessions.

79. Native discovery must preserve robust multi-source behavior.
Test: strict contract and native smoke.
Implemented: yes.
Worked: mostly.
Why not: it preserves rollout/session/state/Antigravity paths, but no SQLite direct read is included.

80. Native discovery must find Dogmeat.
Test: native smoke.
Implemented: yes.
Worked: yes.
Why not if not: `dogmeat=true`.

81. Native discovery must exclude archived Workhorse.
Test: native smoke.
Implemented: yes.
Worked: yes.
Why not if not: `workhorse=false`.

82. Native discovery currently skips SQLite direct reads.
Test: inspected `thread-discovery.js`.
Implemented: no.
Worked: not applicable.
Why not: no native SQLite library is installed/used; current design avoids Python and does not read SQLite.

83. SQLite-only state may need native library later.
Test: dependency/package inspection.
Implemented: no.
Worked: not applicable.
Why not: no native SQLite dependency or app-server SQLite source is added.

84. Build needed a strict no-Python/no-query payload check.
Test: strict contract.
Implemented: yes.
Worked: yes.
Why not if not: contract checks daemon, GUI, and installer.

85. Build needed a no-PowerShell installer check.
Test: strict contract.
Implemented: yes.
Worked: yes.
Why not if not: contract checks installer has no `powershell.exe`.

86. Version needed bump to avoid confusing old build.
Test: strict contract and release check.
Implemented: yes.
Worked: yes.
Why not if not: version is `2.1.31`; release build succeeded.

87. Old installed CAM remained stopped.
Test: process/registry check after emergency stop.
Implemented: yes for current machine.
Worked: yes.
Why not if not: process count was zero and startup entry empty after command.

88. Auto-start registry entry was removed.
Test: registry check.
Implemented: yes for current machine.
Worked: yes.
Why not if not: `StartupRunValue` was empty.

89. New installer was built but not locally installed yet.
Test: GitHub release check.
Implemented: built yes; installed no.
Worked: release yes.
Why not: local install was intentionally not run in this audit after stopping old CAM.

90. Test evidence must include actual sent body.
Test: prior log/transcript extraction.
Implemented: yes for manual reporting.
Worked: yes.
Why not if not: not part of automated GUI.

91. Test evidence must include actual received body.
Test: mailbox/log extraction.
Implemented: yes for manual reporting.
Worked: yes.
Why not if not: GUI also prints reply body.

92. Test evidence must distinguish CAM mailbox from chat transcript.
Test: manual report and code inspection.
Implemented: partially.
Worked: partly.
Why not: GUI says it is reading mailbox, but it does not show separate chat transcript evidence.

93. Test should require both send and receive.
Test: GUI strict send plus mailbox wait inspection.
Implemented: yes.
Worked: code-level yes.
Why not if not: it requires outbound delivery and inbound mailbox reply.

94. Test should prove selected agent responded.
Test: matcher inspection.
Implemented: yes for source identity.
Worked: code-level yes.
Why not if not: it requires exact `sourceAgent`.

95. Test should prove the agent understood a fresh prompt.
Test: searched for semantic check.
Implemented: no.
Worked: no.
Why not: no Jefferson/Missouri or equivalent fresh-content challenge is implemented.

96. Missouri/Jefferson keyword check was requested.
Test: searched GUI code.
Implemented: no.
Worked: no.
Why not: requested check is only in docs/issue list, not production code.

97. That keyword check was only written into issue docs.
Test: inspected docs and code.
Implemented: yes as documentation only.
Worked: no as software.
Why not: documentation does not affect GUI pass/fail.

98. That keyword check was not yet in GUI code.
Test: searched GUI code.
Implemented: no.
Worked: no.
Why not: no match for `Jefferson` or `Missouri`.

99. The issue list/document was not the same as implementation.
Test: compared issue doc to code/contract.
Implemented: yes, this audit records that gap.
Worked: yes as audit.
Why not if not: several issue-list items remain unimplemented.

100. Some fixes shipped in `v2.1.31`, but the full diagnostic issue plan is still incomplete.
Test: combined test results above.
Implemented: partially.
Worked: partially.
Why not: no-popup/native discovery shipped; semantic diagnostic, final passed ledger, stale registry delivery update, chat-only penalty, clean command abstraction, and local install verification remain incomplete.

## Highest Priority Remaining Fixes

1. Add the Missouri/Jefferson semantic challenge to the GUI prompt and matcher.
2. Add a final `passed` event to `tests.jsonl` only after the GUI matcher passes all requirements.
3. Update `lastDelivery` after outbound delivery changes from `started` to `delivered`.
4. Remove direct CAM HTTP from agent-facing prompts; make Qexow skill the only documented agent-facing path.
5. Replace `Send-AgentMessage.ps1` with a non-console Qexow command or native helper if the no-PowerShell standard applies to agent tools as well as CAM runtime.
6. Install and visually/runtime-test `v2.1.31` only after deciding whether the remaining PowerShell skill helper is acceptable.

## Post-Fix Update for v2.1.32

Implemented after this audit:

1. The GUI test prompt now asks: "Hello, how is your day?" and asks for the capital of Missouri.
2. The GUI matcher now fails a matched reply unless the body contains `Jefferson City`, `Jefferson`, or `city`.
3. The daemon now exposes `/tests/pass`, and the GUI records a final `passed` event after the semantic matcher passes.
4. The daemon now updates `lastDelivery` after mutating outbound delivery to `delivered`.
5. The daemon prompt no longer offers direct CAM HTTP as a reply path.
6. The generated Qexow skill text no longer points agents to `Send-AgentMessage.ps1` or `Check-AgentMessages.ps1`; it points to `cam send` and `cam inbox`.
7. The local installed Qexow skill markdown was also updated on this workstation to stop pointing agents at the PowerShell helpers.
8. Version was bumped to `2.1.32`.

Verification after fixes:

- `npm run test:strict-contract`: PASS.
- Native discovery smoke: `count=30`, `dogmeat=true`, `workhorse=false`.
- Runtime/installer forbidden-launch scan: no matches for `query_threads.py`, `FileName = "python"`, `execFile(`, `tryPython`, `python3`, `cmd.exe`, `bash.exe`, `conhost.exe`, `powershell.exe`, `Send-AgentMessage.ps1`, `Check-AgentMessages.ps1`, or `send via CAM HTTP` in daemon, GUI, generated skill text, installer, or tray source.

Still not proven in this pass:

1. The `v2.1.32` installer has not been locally installed and visually tested.
2. Tray icon/status window behavior was not visually verified.
3. GUI/log count agreement was not verified in the installed app.
4. Remote attribution still lacks a live remote-route proof in this local smoke.
5. Native discovery still does not read SQLite directly; if a chat exists only in SQLite and not rollout/session/state sources, a native SQLite source or app-server source is still needed.
6. The old PowerShell helper files still exist in the local skill folder, but current skill instructions no longer reference them.
