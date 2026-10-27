//! Explicit viewport-readback capability boundary for the public Makepad SDK.
//! Normal SDK rendering does not grant access to its native texture or device.

use makepad_widgets::{Cx, Texture};

#[cfg(target_os = "windows")]
pub(crate) struct GpuReadback {
    pub width: u32,
    pub height: u32,
    pub rgba_bytes: usize,
    pub row_pitch: usize,
}

#[cfg(target_os = "windows")]
impl GpuReadback {
    pub(crate) fn begin(_: &Cx, _: &Texture) -> Result<Self, String> {
        Err("UNSUPPORTED_PUBLIC_SDK: GPU viewport export requires live D3D11 device and texture access unavailable in the public Makepad SDK".to_owned())
    }

    pub(crate) fn try_read(&mut self) -> Result<Option<Vec<u8>>, String> {
        Err("UNSUPPORTED_PUBLIC_SDK: GPU viewport readback is unavailable".to_owned())
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) struct GpuReadback {
    pub width: u32,
    pub height: u32,
    pub rgba_bytes: usize,
    pub row_pitch: usize,
}
#[cfg(not(target_os = "windows"))]
impl GpuReadback {
    pub(crate) fn begin(_: &Cx, _: &Texture) -> Result<Self, String> {
        Err("GPU_EXPORT_UNSUPPORTED: D3D11 readback is Windows-only; CPU export requires explicit fallback".to_owned())
    }
    pub(crate) fn try_read(&mut self) -> Result<Option<Vec<u8>>, String> {
        Err("GPU_EXPORT_UNSUPPORTED".to_owned())
    }
}
