//! Graphics-observation capability boundary for the public Makepad SDK.
//! The application does not infer adapter identity, capacity or admission.
use makepad_widgets::Cx;
use serde_json::{Value, json};

pub fn observe(cx: &Cx) -> Value {
    #[cfg(target_os = "windows")]
    {
        windows::observe(cx)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = cx;
        json!({
            "schema_version": 1,
            "backend": null,
            "status": "unavailable",
            "actual_adapter": null,
            "memory_segments": [],
            "error": {"code": "UNSUPPORTED_PLATFORM", "message": "D3D11 diagnostics require Windows"}
        })
    }
}

/// Returns an observed actual-adapter LUID and LOCAL headroom when supported.
///
/// LUID bytes are LowPart followed by HighPart in Windows little-endian layout.
/// No result is returned if hardware identity or the process budget is unknown.
/// The public SDK cannot supply either observation, so admission remains closed.
pub fn compute_device_budget(cx: &Cx) -> Option<([u8; 8], u64)> {
    #[cfg(target_os = "windows")]
    {
        windows::compute_device_budget(cx)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = cx;
        None
    }
}

// The pinned public SDK exposes neither the live D3D11 device nor an
// equivalent adapter/budget observation contract. Unknown identity must keep
// compute admission closed; SDK rendering still follows its normal lifecycle.
#[cfg(target_os = "windows")]
mod windows {
    use super::*;

    pub(super) fn observe(_: &Cx) -> Value {
        json!({
            "schema_version": 1,
            "backend": null,
            "status": "unsupported",
            "actual_adapter": null,
            "memory_segments": [],
            "gpu_qualification": "NOT_RUN",
            "error": {
                "code": "UNSUPPORTED_PUBLIC_SDK",
                "message": "The public Makepad SDK does not expose the application's live D3D11 device for adapter and process-budget observation"
            }
        })
    }

    pub(super) fn compute_device_budget(_: &Cx) -> Option<([u8; 8], u64)> {
        None
    }
}
