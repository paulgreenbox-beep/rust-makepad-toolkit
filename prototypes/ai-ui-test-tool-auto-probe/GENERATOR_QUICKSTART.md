# Automatic Test Generator and UI Coverage

Status: CURRENT  
Updated: 2026-08-22  
Contract/API version: AI UI Test Tool Generator v1

The generator is a public Script API consumer. It does not read private
Makepad internals, ReliabilityManager internals or probe-only data structures.
It works from a compact public inventory, creates a deterministic TestPlan,
compiles ordinary .aiui.json files and can execute them through the existing
runner.

## Source of truth

- operations.json defines operation names, required fields, capabilities and
  physical-input classification.
- ai-ui-test-script.schema.json defines generated script shape.
- test-intent.schema.json and test-plan.schema.json define AI planning inputs
  and deterministic plan output.
- test-templates.json is the versioned built-in template catalog.

## Commands

Run these from prototypes/ai-ui-test-tool-auto-probe after building the
application under test.

~~~powershell
cargo run --locked --bin ai-ui-test -- inventory path\to\app.exe --window-title "Application title" --output inventory-out
cargo run --locked --bin ai-ui-test -- generate path\to\app.exe --window-title "Application title" --mode standard --output generated-tests --plan-only
cargo run --locked --bin ai-ui-test -- generate path\to\app.exe --window-title "Application title" --mode standard --output generated-tests
cargo run --locked --bin ai-ui-test -- generate path\to\app.exe --window-title "Application title" --mode standard --output generated-tests --run
cargo run --locked --bin ai-ui-test -- audit path\to\app.exe --window-title "Application title" --mode standard --output audit-report
cargo run --locked --bin ai-ui-test -- coverage generated-tests --inventory generated-tests\inventory.json --reports generated-tests\reports --output generated-tests
~~~

Use generated output under reports/local/ or another reviewed directory while
experimenting. Do not overwrite manual/regression scripts without reviewing
the generated metadata and stale-target result.

## Inventory and planning

Inventory records stable AutomationId, parent/children, role/control type,
name, value/selection, visibility/offscreen/enabled/interactable state, bounds,
semantic/spatial capabilities, safety class and collection families. It is
collected through session.open, inspect_ui, inspect_spatial_target and
session.close.

The planner classifies each intent as Safe, StatefulRecoverable,
PotentiallyDestructive or Unknown. Destructive/unknown mutation is blocked
unless the relevant generator approval option, fixture and semantic target
evidence are supplied. Stateful spatial gestures require explicit approval.

## Fixtures and secrets

Use ai-ui-fixtures.example.json as the starting point. Generated scenarios
reference fixture values; secrets are redacted from runner reports and evidence
metadata. Screenshot bitmap redaction is not performed, so do not capture
secret-bearing UI unless that exposure has been reviewed.

## AI-assisted planning and coverage

An AI supplies validated intents, not arbitrary DSL. The validator checks
template/operation/target/capability compatibility, related IDs, destructive
approval and secret parameters before deterministic compilation.

Coverage is calculated from inventory, generated scripts and real reports.
States are Uncovered, Generated, ExecutedPassed, ExecutedFailed, Blocked and
RequiresFixture. The main outputs are coverage.json, coverage.md and
ai-coverage.json.

Current end-to-end evidence is indexed in
../../reports/README.md; the generator baseline is
reports/generator-e2e-20260821-135240/audit-final/. The saved final report
records 24 generated scripts executed successfully and the
AI_UI_TEST_GENERATOR_PASSED marker.


## Observation workflow templates (v1)

The versioned template registry includes opt-in templates for runtime.task,
external.request, file.output, trace.milestone, and visual.region. They compile
only declared public operations and fixed fixture/observer inputs; the generator
does not invent an HTTP request, a shell action, or an arbitrary filesystem path.

generator-classification.json classifies all 104 public operations. The current
registry records 23 templates, five observation templates, and zero required
operations without a template. Use generated-plan metadata to show why a trusted
observation scenario was selected; use the explicit async-observation examples
for deterministic failure modes and real Windows verification.
## Generator classification
+
The generator is not required to support all 104 public operations with
templates. Classification is explicit in generator-classification.json:
+
| Classification | Count | Meaning |
| --- | ---: | --- |
| GENERATOR_REQUIRED | 41 | A generic versioned template must cover the operation. |
| GENERATOR_OPTIONAL | 54 | The template needs app-specific state, fixture or approval. |
| INFRASTRUCTURE_ONLY | 8 | Runner/scheduler scaffolding, not a generated UI scenario. |
| NEGATIVE_ONLY | 1 | A rejection or contract-negative path. |
| NOT_APPLICABLE | 0 | Explicitly not applicable in the current public catalog. |
+
Required templates missing: 0. This classification is independent from real
Windows E2E coverage; every public operation still has a reference entry.

## Physical window templates

`generate_window_physical` is `false` by default. With that default, the
generator may select the read-only `window.interaction` template but will not
produce desktop gestures. Setting it to `true` enables only the public
`window.move`, `window.frame_resize`, and `window.state` templates. Their
scripts require the shared `windows.interactive_desktop` scheduler resource and
record post-gesture bounds, DPI, semantic-layout, runtime-health and pointer
cleanup evidence.

`windows_set_window_bounds` is programmatic setup/recovery, not a substitute
for a user-visible title drag or edge/corner resize proof.
