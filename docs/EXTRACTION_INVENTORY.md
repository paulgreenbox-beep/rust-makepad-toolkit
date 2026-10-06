# Extraction inventory and classification

The generic Toolkit components originated in an application-neutral runtime
and UI module. Consumer application sources, format engines, sessions and
private integrations are not part of this distribution.

| Classification | Components | Decision |
| --- | --- | --- |
| Generic Rust | rudra-runtime typed identity and registries | Retain the accepted public API as the Toolkit source of truth. |
| Generic Rust | Geometry, zoom/scroll, visibility, page layout, rulers and text-layout inputs | Pure makepad-toolkit-core without Makepad or domain dependencies. |
| Generic Makepad | VirtualCanvas, registration, drawing, events, textures and focus | makepad-toolkit-widgets consumes generic frame/page primitives. |
| Generic document UI | PageSurface, PageStack, page-local text/table/image/rectangle items and overlays | Retain format-neutral models in core. |
| Consumer-specific | Format parsing/projection, domain engines, sessions and persistence | Remain in separately maintained consumer adapters. |

Historical source-module names include office-ui-runtime/src/geometry.rs,
viewport.rs, pages.rs, rulers.rs, model.rs, content.rs, text_layout.rs and
virtual_canvas.rs. Their extraction does not imply current production
stability or give Toolkit ownership of the consumer application.

The provider names DocumentFrame and CanvasRuntime remove domain-specific
naming while preserving the accepted generic widget contracts. A consumer may
keep temporary aliases through a re-export facade. Component maturity and
current source paths are in [COMPONENTS.md](COMPONENTS.md).

The local upstream Makepad directory is a dependency checkout, not part of
Toolkit copyright. Its revision must be obtained from its own Git metadata
or the public bootstrap pin; a parent repository's HEAD is not provenance.
