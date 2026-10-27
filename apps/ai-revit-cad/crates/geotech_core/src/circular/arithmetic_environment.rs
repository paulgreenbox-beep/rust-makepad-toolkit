//! Read the actual caller's floating-point controls. No control register is
//! written, no process-wide setting is changed, and no other thread is probed.
use super::{types::CircularErrorCode, validation::{CResult, error}};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArithmeticEnvironmentReport {
    pub architecture: String,
    pub mxcsr: Option<u32>,
    pub x87_control_word: Option<u16>,
    pub round_to_nearest: bool,
    pub flush_to_zero: bool,
    pub denormals_are_zero: bool,
    pub exceptions_masked: bool,
    pub gradual_underflow_probe_passed: bool,
    pub accepted: bool,
    pub scope: String,
}

pub fn inspect_arithmetic_environment() -> ArithmeticEnvironmentReport {
    let mut report = ArithmeticEnvironmentReport {
        architecture: std::env::consts::ARCH.into(), mxcsr: None, x87_control_word: None,
        round_to_nearest: false, flush_to_zero: false, denormals_are_zero: false,
        exceptions_masked: false, gradual_underflow_probe_passed: false, accepted: false,
        scope: "ACTUAL_CALLING_THREAD_ENTRY: read-only control inspection requires round-to-nearest, FTZ=false, DAZ=false, and all MXCSR/x87 floating exceptions masked to prevent hardware traps; exact normal-input gradual-output probe; no claim about a different worker, native integration, or all compiler arithmetic".into(),
    };
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if !std::is_x86_feature_detected!("sse") {
            report.scope.push_str("; SSE control inspection unsupported on this processor");
            return report;
        }
        let mut mxcsr = 0_u32;
        let mut control = 0_u16;
        // SAFETY: these instructions only store the calling thread's control
        // words into correctly sized live local variables. SSE was checked.
        unsafe {
            core::arch::asm!("stmxcsr [{pointer}]", pointer = in(reg) &mut mxcsr,
                options(nostack, preserves_flags));
            core::arch::asm!("fnstcw [{pointer}]", pointer = in(reg) &mut control,
                options(nostack, preserves_flags));
        }
        report.mxcsr = Some(mxcsr);
        report.x87_control_word = Some(control);
        report.round_to_nearest = mxcsr & 0x6000 == 0 && control & 0x0c00 == 0;
        report.flush_to_zero = mxcsr & 0x8000 != 0;
        report.denormals_are_zero = mxcsr & 0x0040 != 0;
        report.exceptions_masked = mxcsr & 0x1f80 == 0x1f80 && control & 0x003f == 0x003f;
        if report.round_to_nearest && !report.flush_to_zero && !report.denormals_are_zero
            && report.exceptions_masked {
            // Exact multiplication starts with two normal operands; it needs
            // neither a subnormal input nor an inexact result/sticky exception.
            let normal = std::hint::black_box(f64::MIN_POSITIVE);
            let half = std::hint::black_box(0.5_f64);
            let subnormal = std::hint::black_box(normal * half);
            report.gradual_underflow_probe_passed = subnormal.to_bits() == 0x0008_0000_0000_0000;
        }
        report.accepted = report.round_to_nearest && !report.flush_to_zero
            && !report.denormals_are_zero && report.exceptions_masked
            && report.gradual_underflow_probe_passed;
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    report.scope.push_str("; unsupported architecture fails closed");
    report
}

pub fn require_search_arithmetic_environment() -> CResult<ArithmeticEnvironmentReport> {
    let report = inspect_arithmetic_environment();
    if report.accepted { Ok(report) } else {
        Err(error(CircularErrorCode::InvalidParameter,
            "Search requires supported actual-thread round-to-nearest controls, FTZ=false, DAZ=false, masked floating exceptions, and gradual underflow"))
    }
}
