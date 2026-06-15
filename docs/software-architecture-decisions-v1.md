# Software Architecture Decisions V1

Date:
- 2026-06-14

Status:
- Working decision record
- Captures current preferences for core software design direction

## 1. Overall Architecture

Selected:
- A + B

Interpretation:
- Monolithic but well-structured
- Strong internal separation by function category
- Not "modular" in the over-abstract sense
- Well-defined strong boundaries inside one monorepo

Notes:
- Distinct function categories
- Easy to logically separate and isolate

## 2. Runtime Shape

Selected:
- A

Interpretation:
- One binary

Notes:
- Strong preference for one binary
- One binary for desktop
- One binary for remote if needed operationally
- Still conceptually a one-binary style system rather than many cooperating services

## 3. Internal Organization

Selected:
- A, with one important exception

Interpretation:
- Unified codebase with light but strong boundaries
- Provider-specific adapters must be modular

Notes:
- We plan on supporting more than Codex and AGY in the future
- Provider-specific adapters must be separable
- One part of the software should know message semantics
- Another part should know how to send based on provider

Working design rule:
- Core software decides what should happen
- Provider adapters decide how it happens for each provider

## 4. State Model

Selected:
- Memory-first, JSON-persisted state

Interpretation:
- The working truth should live in memory while the program is running
- State should be serialized to JSON on meaningful change or on exit
- Persistence should be simple and inspectable

Notes:
- Write-only mindset for normal operation
- Read access should be restricted and intentional
- Avoid many competing truth sources

Working design rule:
- Memory is the active truth
- JSON is the durable record

## 5. Programming Style

Selected:
- Data-driven

Interpretation:
- Define the important data shapes first
- Move data through clear functions
- Avoid pretending this is a world of little objects with hidden behavior

Working design rule:
- Data structures and explicit transformations should be easier to see than clever object behavior

## 6. Control Flow Style

Selected:
- Strong command-driven

Interpretation:
- The system should mostly do explicit things when commanded to do them
- Avoid hidden self-activating behavior unless it is absolutely necessary and logged

Working design rule:
- Commands should be obvious, traceable, and human understandable

## 7. Concurrency Model

Selected:
- Low-rate, patient, conservative concurrency

Interpretation:
- This software will not be asked to do things at a high rate
- It does not need to be fast in the high-throughput sense
- Users and agents can wait for correctness

Working design rule:
- Prefer simple ordered work over complicated parallelism

## 8. Abstraction Style

Selected:
- Concrete-first

Interpretation:
- This is a small, one-off program with a small core purpose
- It is below the tradeoff point where abstractions reduce complexity
- Abstractions should not be introduced just to make hypothetical future expansion easier

Exception:
- Provider-specific knowledge should be isolated behind concrete provider adapters

Working design rule:
- Keep core logic concrete and readable
- Abstract only provider-specific edges

## 9. Dependency Strategy

Selected:
- Strong preference for zero dependencies

Interpretation:
- Prefer zero dependencies if possible

Notes:
- This should be treated as a design goal
- If any dependency is ever introduced, it should need a very strong justification

## 10. Boundary Design

Selected:
- A

Interpretation:
- Strong separation between domain logic and transport/storage/UI

Notes:
- Strong A

Working design rule:
- Business logic must stay isolated from transport, persistence, provider mechanics, and interface layers

## 11. Error Handling Style

Selected:
- Strong loud failure detection with continued operation

Interpretation:
- Detect failures aggressively
- Do not silently fallback to stale data
- Do not silently fallback to synthetic data
- Make failures very loud
- Keep moving when possible instead of exiting the whole program
- Keep trying with backoff timing
- Continue working around errors, but make the error visible

Notes:
- There is a GUI, so failures should be surfaced there
- Loud failure does not mean panic and die
- Loud failure means do not lie, do not hide, and do not pretend success

## 12. Delivery Philosophy

Selected:
- A

Interpretation:
- Direct active-attention delivery first
- Queue only as fallback

Notes:
- This logic should be chunky
- Human readable
- Easy to change and expand in the future
- The low-level send and response sequence may need human adjustment later

Working design rule:
- Delivery logic should be explicit, readable, and easy to modify by humans

## 13. Antigravity Model

Selected:
- A only

Interpretation:
- Antigravity is a session-addressable conversational target

Working design rule:
- Do not treat Antigravity as mailbox-first when session delivery is available

## 14. Module Size Philosophy

Selected:
- A

Interpretation:
- Fewer larger modules

Notes:
- Prefer larger, understandable units over excessive fragmentation

## 15. Testing Philosophy

Selected:
- Strong A

Interpretation:
- Integration-heavy real behavior tests

Notes:
- Strong preference for testing real behavior

## 16. Public API Philosophy

Selected:
- No public API component

Interpretation:
- The project should not expose a public API as a product surface
- Avoid building a single local app that talks to itself internally using internet protocols just because that is fashionable
- Internal local interfaces are allowed only when they are the simplest practical boundary
- CAM-to-CAM communication over a network is allowed and required
- CAM-to-CAM communication can use SSH baked into the app
- CAM-to-CAM protocol/API is acceptable if it is useful, but it should exist for peer communication, not as a public web API product

Working design rule:
- No public API surface
- Peer communication is a controlled internal/protocol concern

## 17. Logging / Observability Philosophy

Selected:
- A

Interpretation:
- Strong structured logs and explicit events

Notes:
- Extreme verbose logging
- We do not need to log the chats
- We therefore have room to log everything CAM does

Working design rule:
- Log nearly everything operational
- Do not waste logging budget on chat transcript storage unless explicitly needed

## 18. Design Philosophy

Selected:
- Strong Unix-like

Interpretation:
- Prefer simple tools
- Prefer clear inputs and outputs
- Prefer boring interfaces
- Prefer composable behavior where it naturally helps
- Prefer one job done clearly over broad magical behavior

Working design rule:
- The software should feel like a clear tool, not a sprawling platform

## 19. Simplicity Philosophy

Selected:
- Strong A

Interpretation:
- One obvious path

Notes:
- Strong preference for one canonical path over many alternatives

## 20. Code Style Reputation Target

Selected:
- A

Interpretation:
- Boring and trustworthy

Notes:
- This is the target reputation
- We do not want clever-looking chaos

## Consolidated Direction

Current preferred direction:
- One binary
- One monorepo
- Strong internal boundaries
- Unified system, not microservices
- Provider adapters separated from core logic
- Memory-first state with JSON persistence
- Data-driven implementation
- Strong command-driven control flow
- Low-rate conservative concurrency
- Concrete-first code
- Zero-dependency goal
- Strong domain boundary separation
- Loud error detection with continued operation
- Direct active-attention delivery first
- Antigravity as session-addressable only
- Fewer larger modules
- Integration-heavy testing
- No public API product surface
- Extreme verbose logging
- Strong Unix-like design philosophy
- One obvious path
- Boring and trustworthy code

## Open Items That Still Need Design Expansion

- Exact concurrency mechanics
- Exact JSON persistence timing
- Exact provider adapter boundary
- Exact peer communication protocol shape over SSH
- Exact shape of backoff timing after failures
- Exact GUI presentation for loud failures

## Working Software Rules Already Implied By These Decisions

- Core logic decides semantics
- Provider adapters implement provider-specific delivery
- Responses should go to active attention channels first
- Queueing is fallback only
- Memory is active truth
- JSON is durable record
- Direct command paths are preferred over hidden background behavior
- Failures must be loud, visible, and non-deceptive
- The program should keep trying when safe instead of exiting
- Avoid stale or synthetic fallback behavior
- Avoid public API/platform posture
- Boundaries should be obvious and strict
- Logging should be exhaustive for operational behavior
- The code should be easy for humans to inspect and change
- Prefer simple obvious implementation over clever abstraction

## Vocabulary Note

The initial uncertainty was mostly terminology. The underlying opinions were already strong. These software-engineering words should be treated as labels for choices, not as gatekeeping concepts.
