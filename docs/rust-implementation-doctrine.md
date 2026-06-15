# Rust Implementation Doctrine

Date:
- 2026-06-14

Status:
- Approved planning doctrine
- Written before Rust implementation begins

## Numbered Doctrine

1. The philosophy is strong, unusually coherent, and a little spicy in a good way. It is not "I don't know software." It is a very clear taste profile: local, strict, boring, inspectable, command-driven, no artificial platform theater, no hidden fallback theater.

2. This can be excellent if we protect it from two enemies.

3. First enemy: accidental architecture creep. The current spec still has a "local HTTP API" section, but the newer decision says no public API product surface, and we dislike apps talking to themselves internally using internet-shaped protocols by default. That needs reconciling. We may still need a local communication boundary, but it should be deliberate. CLI direct calls, local IPC, stdio, named pipe, or a tiny loopback health/status endpoint are different architectural choices. A network API is allowed for true remote CAM-to-CAM communication. SSH remains acceptable as the transport boundary, and a CAM-to-CAM protocol/API can ride through SSH if that is the cleanest design.

4. Second enemy: dependency confusion. Dependency count is not important by itself. Use engineering judgment. The right standard is not "many dependencies" or "zero dependencies." The right standard is whether each dependency earns its place. Use boring, trusted, well-maintained crates when they reduce real risk or complexity. Avoid dependencies that hide core behavior, create architecture gravity, or make simple logic harder to inspect.

5. The software flow, in plain English:

6. CAM starts and loads its JSON state into memory. From that point forward, memory is the active truth. JSON is the durable record. The program logs what it is doing constantly, but it does not treat logs as truth.

7. An operator or agent issues a command. The command is parsed into a concrete action. The core logic looks at the in-memory state and decides what should happen. It does not care whether the target is Codex, AGY, or future provider X yet. It decides the semantic intent: steer, wake, deliver, queue, fail, inspect, sync, list, etc.

8. Then the provider adapter performs the mechanical act. Codex adapter knows Codex. AGY adapter knows AGY. SSH peer adapter knows peer transport. The core does not become polluted with provider rituals.

9. For sending: CAM finds the target agent, checks its attention channel, and chooses the obvious path. If active, steer. If inactive but session/thread is known, wake and deliver. If impossible, queue loudly. If strict mode says no fallback, fail loudly. The response path follows the same idea in reverse: content goes to attention first, storage only as fallback.

10. For peer sync: local CAM asks remote CAM over SSH for trusted inventory. It imports remote agents as remote things, never as local things. That distinction matters.

11. For diagnostics: confirmations are programmatic protocol evidence, not conversational content. The Rust design should model the actual message flow and delivery proof without baking planning-conversation vocabulary into code.

12. On the development process: outline all pieces first, but do not define every file before starting. That is usually premature certainty. Define the architecture map, the core data types, the command list, and the first vertical slice. Then build one slice all the way through.

13. The best practice here is a thin end-to-end slice. It means the tiniest version of the whole program works end to end before we flesh out every feature.

14. Build it like this:

15. Create the Rust project with one binary and a small module map.

16. Define the core data types: agent, message, delivery state, provider kind, error, config.

17. Build memory-first state plus JSON load/save.

18. Build extreme structured logging early.

19. Build CLI command parsing for only `init`, `doctor`, and `agent list`.

20. Add one real send path for one provider.

21. Add response routing semantics: steer, wake/deliver, queue, fail.

22. Add integration tests around real behavior.

23. Add provider adapters one at a time.

24. Add peer communication after local semantics are solid.

25. Outline all pieces, but implement one complete behavior at a time.

26. Do not define all files before starting. Define likely modules, then let file boundaries settle as the code proves its shape. A good starting module map might be:

27. `main`

28. `cli`

29. `core`

30. `state`

31. `delivery`

32. `providers`

33. `providers/codex`

34. `providers/agy`

35. `peers`

36. `logging`

37. `errors`

38. That is enough structure to prevent soup, without pretending we already know every final file.

39. Industry-standard advice: build from the domain center outward. Do not start with GUI. Do not start with packaging. Do not start with peer sync. Start with the nouns and verbs that make the software true: agent, message, response, delivery, state, provider, error.

40. The most important improvement to the docs now is this Rust Implementation Doctrine, resolving these remaining tensions:

41. local HTTP API vs no public API posture

42. dependency judgment instead of dependency-count purity

43. exact JSON persistence timing

44. exact provider adapter contract

45. exact error taxonomy

46. exact backoff behavior

47. exact first vertical slice

48. Do not begin Rust coding until these principles are written down in one concise implementation doctrine. Not a huge document. Just enough to keep us from re-litigating architecture in the middle of code.

## Resolved Decisions

- No public API product surface.
- A network API is allowed for true remote CAM-to-CAM communication.
- SSH remains acceptable as the transport boundary for remote CAM-to-CAM operations.
- Dependency count is not important by itself.
- Dependency selection should use engineering judgment.
- Dependencies should be boring, trusted, and useful when they are introduced.
- Core behavior should remain inspectable even when dependencies are used.
- Build a thin end-to-end slice first.
- Start from the domain center.
- Implement one complete vertical slice at a time.

## First Build Sequence

1. Rust project shell
2. Core data types
3. Memory-first state and JSON persistence
4. Structured logging
5. Minimal CLI
6. First provider send path
7. Response routing semantics
8. Integration tests
9. Additional provider adapters
10. Peer communication

## Assumptions

- This is a planning document only, not code.
- This does not replace the existing architecture decisions doc.
- API is allowed only for controlled CAM-to-CAM communication, not as a public product surface.
- Dependency choice is delegated to engineering judgment, with clarity and maintainability as the standard.
