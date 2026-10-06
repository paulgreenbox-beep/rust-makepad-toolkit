# AI UI Test Tool — Session Attach / Resume Feasibility

Status: NOT IMPLEMENTED / UNSUPPORTED in Script API v1  
Updated: 2026-08-23

There is no public `session.attach` or `session.resume` operation. A session is an owned lifecycle created by `session.open`; it carries a child process identity, discovered windows, action/evidence history, and any input lease. The current public contract therefore supports a fresh session only.

## Required invariant

| Requirement | Current state | Decision |
| --- | --- | --- |
| Known PID and executable ownership | Available only for a process spawned by the current managed session | Insufficient for arbitrary attach |
| Stable top-level window identity | Refreshed while the owned process lives | Insufficient across host restart |
| Input-lock ownership and stale lock recovery | Bound to the current session | Cannot be safely resumed |
| Evidence continuity and redaction context | Bound to current runner output and session action log | Cannot be reconstructed reliably |
| Telemetry node identity/version | Configurable per active app, not persisted as a resumable lease | Must be rediscovered |
| Scheduler lease/worker identity | Current job owns it | Requires a separate durable contract |
| PID reuse / foreign-process protection | No public attach verification protocol | High safety risk |

## Product decision

Do not claim attach/resume support. The safe public recovery path is:

```text
session.close (best effort / auto-cleanup) -> session.open -> inspect_ui ->
rediscover AutomationIds -> reacquire any physical input lease -> continue
```

A future attach/resume feature would need explicit ownership proof, PID creation-time validation, window identity continuity, durable lease transfer rules, evidence chain semantics, cleanup on loss, scheduler integration, schema/catalog entries, and real Windows failure-injection evidence. Until then its status remains `UNSUPPORTED`, not a partially implemented fallback.
