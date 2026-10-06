# Architecture and trust model

## Ownership

Toolkit provides application-neutral typed runtime identity, pure layout/render
planning, Makepad adapters and UI verification. Consumer format parsers,
database/domain engines, sessions and persistence stay in the application.

~~~text
rudra-runtime                  makepad-toolkit-core (pure Rust)
                                            |
                                            v
                              makepad-toolkit-widgets
                                            |
                                            v
                               consumer domain adapter
                                            |
                                            v
                                   consumer application
~~~

The existing rudra-runtime crate name is retained as a public compatibility
contract. The generic components were extracted from an application-neutral
UI module; [COMPONENTS.md](COMPONENTS.md) and
[EXTRACTION_INVENTORY.md](EXTRACTION_INVENTORY.md) record module provenance.
A consumer compatibility facade may re-export Toolkit but must not duplicate
its implementation.

## Verification trust model

~~~text
application says completed
        |
        v
Toolkit independently verifies
        |
        +-> state / postcondition
        +-> runtime health
        +-> required evidence
        |
        v
PASS / FAIL
~~~

dispatch != success. A successful invoke, input injection, process launch or
HTTP response alone does not prove the requested outcome. The public runner
records verification and typed failures; unsupported or unobservable facts
must remain Unknown / NotTested.

## Public boundaries

~~~text
Makepad application + published semantic/telemetry snapshot
   -> Windows accessibility bridge / semantic probe
   -> operations.json + .aiui.json runner
   -> generator / coverage
   -> Scheduler + public execution-environment contract
   -> LocalWorker | ContainerWorker Level 0
   -> reports / evidence
~~~

The Windows GUI MCP is a separate ID-addressed UIA/Win32 sidecar:

~~~text
agent / MCP client
   -> GuiToolRegistry
   -> WindowsAutomation
   -> UI Automation / Win32
   -> bounded EvidenceLog
~~~

The MCP server does not expose the Script API or Scheduler and does not provide
the Makepad semantic probe by itself. An application must publish its Windows
accessibility provider to expose custom widget semantics.

## Layer responsibilities

Core modules own geometry, transforms, page placement, bounded visible ranges,
rulers, text-layout inputs and RuntimeLayout. Makepad adapters own drawing,
events, focus, clipping, redraw and bounded GPU resources. Domain projection
creates a DocumentFrame; CanvasRuntime prepares a pure RuntimeLayout consumed
by VirtualCanvas.

The Auto Probe workspace owns the public Script API, observation, generation,
reliability and Windows interaction layers. Its Scheduler launches only the
public runner. Trusted application and environment catalogs are operator
configuration; scripts cannot introduce arbitrary executables or shell code.

## Dependency and publication boundary

Makepad is a separately obtained public upstream dependency under ignored
.deps/makepad. The bootstrap records a public revision; an existing local
vendor directory is not evidence of its Git revision. Toolkit does not
redistribute that checkout, its fonts or binaries. Third-party licensing is
described in [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).

Private consumer projects and optional external runtime integrations are not
Toolkit source dependencies. Current source publication does not change the
production vendor or claim Linux / WSLg / wslc acceptance.
