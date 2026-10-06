# Invariants

1. Toolkit crates do not depend on consumer applications, document-format engines, databases or private domain modules.
2. Dependency direction is pure core to Makepad adapter to domain adapter to application.
3. `EntityId<T>`, `IdAllocator<T>`, and `Registry<T>` keep their accepted API and runtime-local semantics.
4. Identity is not a path, index, `WidgetUid`, backend handle, or active-state marker.
5. Geometry, page placement, visible ranges, scroll clamp, identity, and render planning remain pure Rust.
6. `VirtualCanvas` is the single production Makepad implementation used by consumers.
7. One zoom owner, one scroll owner, and one effective clip owner exist per canvas.
8. Logical-to-screen conversion applies zoom, scroll, and viewport origin exactly once.
9. Page content is page-local; `PageStack` placement is added exactly once.
10. Rendering is bounded to visible items plus bounded overscan.
11. Image and texture caches have explicit finite limits and release with their owner.
12. Compatibility crates may re-export Toolkit APIs but may not copy implementation.
13. Makepad is upstream; `.deps/makepad` is ignored bootstrap state and never committed.
14. No Makepad vendor patch is allowed without an isolated reproducer proving a blocker.
15. Every development task states `CHEAPEST_VERIFICATION_PATH` and starts at the lowest sufficient L0–L4 level.
16. New reusable components require a generic contract, pure tests, isolated demo when visual, and one consumer integration.
17. Component maturity and source provenance are documented; extraction does not imply stable API.
18. Skills are loaded lazily from `skills/INDEX.md`; all full skills are never injected by default.
19. Windows automation uses one `GuiToolRegistry` and one session-scoped identity map; display names, indices, HWND values, and coordinates are not public Toolkit identity.
20. A GUI action targets `window_id` and, where applicable, `element_id`; not-found and ambiguity are explicit structured errors.
21. Computer Use is fallback-only after a native accessibility/action limitation is recorded in evidence; the backend never turns not-found into a coordinate guess.
22. GUI evidence is bounded and records request, execution method, resulting state, and screenshot path where applicable.
23. Consumer-specific accessibility hooks and vendor patches are outside the generic automation backend and require separate proof and approval.
