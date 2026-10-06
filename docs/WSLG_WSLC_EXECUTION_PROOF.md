# Linux / WSLg / wslc status

Status: EXPERIMENTAL. Windows remains the current Toolkit GUI automation target.

Historical isolated WSLg checks showed that a public Makepad counter example
could render frames and exit cleanly at a known-good revision and at upstream
fix [105158e0](https://github.com/makepad/makepad/commit/105158e08c89d47f57af4621420ed68eb3a147ce).
Those checks did not establish acceptance of the production-selected vendor
or the complete Toolkit GUI surface.

The isolated failure involved an unpackUnorm4x8 definition emitted into a
GLSL ES 3.00 shader. The upstream fix used explicit integer unpacking. This
source-level observation is not a claim about every Linux driver or widget.

The 3D shader/frame-submission smoke did not prove visual scene content and
normal window-close cleanup. wslc was unavailable in the tested environment;
no Toolkit container GUI proof was completed.

Before advertising Linux support, use a separately selected public upstream
checkout and retain real build, first-frame, postcondition, health, capture and
cleanup evidence. Run container/VM checks through their existing public
contracts. Historical host inventory, local paths, captures and raw logs are
excluded from this source release.
