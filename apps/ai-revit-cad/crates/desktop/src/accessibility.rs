//! Explicit public-build boundary for the optional Windows UIA bridge.
//!
//! The local development integration requires checked action tickets and
//! lifetime/generation fences not exported by the published Toolkit baseline.
//! This source distribution does not weaken them or alias to the legacy API.
//! Manual Makepad input and the CAD/engineering core remain independent.

pub(super) const STATUS: &str = "WINDOWS_UIA_UNSUPPORTED: the public Toolkit baseline lacks the checked bridge API; no provider is registered";

pub(super) fn validate_args() -> Result<(), String> {
    if std::env::args().any(|arg| arg.starts_with("--uia-fixture")) {
        return Err(format!("{STATUS}; private UIA fixture modes are not available"));
    }
    Ok(())
}
