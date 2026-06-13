# CAM Diagnostic Delivery Semantics

CAM uses different words for different delivery stages. Do not collapse these states.

`queued` means delivery to a real agent/thread could not happen now, so the message is deferred for later surfacing. A GUI diagnostic test must never pass on `queued`.

`received` means the daemon accepted a message into a mailbox or diagnostic receiver. For the GUI test mailbox, this is the required inbound reply state.

`delivered` means CAM reached a real Codex thread and received a `turnId` from app-server. This is the required outbound state for GUI diagnostics.

`failed` means strict delivery failed and no queued fallback was used.

`ignored` is GUI/test-runner language for replies with the wrong source agent, target, message type, timestamp, marker, or correlation ID.

The GUI round-trip test has two legs: outbound delivery to the selected agent thread, then inbound receipt by the CAM GUI test receiver. Both legs must pass. A matching-looking mailbox row is evidence, not success, unless its delivery state is `received`.
