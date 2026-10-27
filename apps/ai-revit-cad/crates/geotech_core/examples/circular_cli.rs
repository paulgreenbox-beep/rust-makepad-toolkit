//! cargo run -p geotech_core --offline --locked --example circular_cli -- request.json
use geotech_core::circular::{
    CIRCULAR_SOLVER_VERSION, CircularAnalysisConfig, CircularAnalysisInput, SearchOutputMode,
    SearchPublicRequestV1, analyze_specified_circle, diagnose_specified_circle,
    inspect_arithmetic_environment, search_critical_circle, source_fingerprint,
};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: CircularAnalysisInput,
    config: CircularAnalysisConfig,
}
fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(2);
    }
}
/// Actual caller/thread controls, observed read-only at the numerical boundary.
/// This is conditional arithmetic evidence, never engineering approval.
fn arithmetic_checkpoint(phase: &str, mode: &str) -> Result<(), String> {
    let report = inspect_arithmetic_environment();
    let accepted = report.accepted;
    let marker = serde_json::json!({
        "phase": phase, "mode": mode,
        "solver_version": CIRCULAR_SOLVER_VERSION,
        "source_fingerprint": source_fingerprint(),
        "arithmetic_environment": report,
    });
    eprintln!(
        "CIRCULAR_CLI_ARITHMETIC {}",
        serde_json::to_string(&marker).map_err(|e| e.to_string())?
    );
    if !accepted {
        return Err(format!(
            "CLI arithmetic environment not accepted at {phase}; no numerical result published"
        ));
    }
    Ok(())
}
fn run() -> Result<(), String> {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    let search_mode = args.first().and_then(|s| s.to_str()).and_then(|s| match s {
        "--search" => Some(false),
        "--search-diagnostic" => Some(true),
        _ => None,
    });
    if search_mode.is_some() {
        args.remove(0);
    }
    let diagnostic_only = args.first().is_some_and(|s| s == "--diagnostic");
    if diagnostic_only {
        args.remove(0);
    }
    if args.len() != 1 {
        return Err("Usage: circular_cli [--diagnostic | --search | --search-diagnostic] request.json (diagnostics are not accepted results)".into());
    }
    let metadata = std::fs::metadata(&args[0]).map_err(|e| e.to_string())?;
    if metadata.len() > 16 * 1024 * 1024 {
        return Err("Request exceeds the 16 MiB CLI input budget".into());
    }
    let bytes = std::fs::read(&args[0]).map_err(|e| e.to_string())?;
    if let Some(diagnostic) = search_mode {
        if diagnostic_only {
            return Err("Search flags cannot be combined with --diagnostic".into());
        }
        let mut request: SearchPublicRequestV1 =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid search request: {e}"))?;
        request.output_mode = if diagnostic {
            SearchOutputMode::Diagnostic
        } else {
            SearchOutputMode::Strict
        };
        let mode = if diagnostic {
            "search_diagnostic"
        } else {
            "search"
        };
        arithmetic_checkpoint("before", mode)?;
        let computed = search_critical_circle(&request)
            .map_err(|e| serde_json::to_string(&e).unwrap_or_else(|_| e.to_string()));
        // Core Result errors still receive an actual-thread after observation.
        // An unsupported post environment cannot reach successful stdout.
        arithmetic_checkpoint("after", mode)?;
        let result = computed?;
        if !diagnostic && !result.accepted {
            return Err(serde_json::to_string(&result).map_err(|e| e.to_string())?);
        }
        // Match the core's counted compact representation; pretty-printing
        // could exceed a successfully admitted aggregate output budget.
        serde_json::to_writer(std::io::stdout().lock(), &result).map_err(|e| e.to_string())?;
        println!();
        return Ok(());
    }
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid circular request: {e}"))?;
    let mode = if diagnostic_only {
        "specified_diagnostic"
    } else {
        "specified"
    };
    arithmetic_checkpoint("before", mode)?;
    let computed = if diagnostic_only {
        diagnose_specified_circle(&request.input, &request.config)
    } else {
        analyze_specified_circle(&request.input, &request.config)
    }
    .map_err(|e| serde_json::to_string(&e).unwrap_or_else(|_| e.to_string()));
    arithmetic_checkpoint("after", mode)?;
    let result = computed?;
    serde_json::to_writer_pretty(std::io::stdout().lock(), &result).map_err(|e| e.to_string())?;
    println!();
    Ok(())
}

#[cfg(all(test, any(target_arch = "x86", target_arch = "x86_64")))]
mod arithmetic_tests {
    use super::*;

    fn mxcsr() -> u32 {
        let mut value = 0_u32;
        // SAFETY: SSE is checked by both callers; read only this test thread.
        unsafe {
            core::arch::asm!("stmxcsr [{pointer}]", pointer=in(reg) &mut value,
                options(nostack, preserves_flags));
        }
        value
    }
    struct RestoreMxcsr(u32);
    impl Drop for RestoreMxcsr {
        fn drop(&mut self) {
            // SAFETY: restore exactly the word read on this disposable thread.
            unsafe {
                core::arch::asm!("ldmxcsr [{pointer}]", pointer=in(reg) &self.0,
                    options(nostack, preserves_flags));
            }
        }
    }

    #[test]
    fn actual_calling_thread_accepted_checkpoint_preserves_controls() {
        assert!(std::is_x86_feature_detected!("sse"));
        let before = mxcsr();
        let report = inspect_arithmetic_environment();
        assert!(
            report.accepted,
            "actual test caller must satisfy the conditional arithmetic boundary: {report:?}"
        );
        arithmetic_checkpoint("before", "specified").unwrap();
        arithmetic_checkpoint("after", "specified").unwrap();
        // Low six sticky exception flags are observations, not control modes.
        assert_eq!(mxcsr() & !0x3f, before & !0x3f);
    }

    #[test]
    fn actual_disposable_thread_ftz_rejects_before_numerical_publication() {
        assert!(std::is_x86_feature_detected!("sse"));
        let owner_controls = mxcsr();
        std::thread::spawn(|| {
            let original = mxcsr();
            let restore = RestoreMxcsr(original);
            let ftz = original | 0x8000;
            // TEST ONLY: change FTZ on this isolated disposable thread. No
            // production instruction changes controls; Drop restores on panic.
            unsafe {
                core::arch::asm!("ldmxcsr [{pointer}]", pointer=in(reg) &ftz,
                    options(nostack, preserves_flags));
            }
            let actual = inspect_arithmetic_environment();
            assert!(actual.flush_to_zero && !actual.accepted);
            let mut numerical_publication_reached = false;
            let attempt = (|| -> Result<(), String> {
                arithmetic_checkpoint("before", "specified")?;
                numerical_publication_reached = true;
                Ok(())
            })();
            assert!(attempt.is_err());
            assert!(!numerical_publication_reached);
            drop(restore);
            assert_eq!(mxcsr(), original);
        })
        .join()
        .unwrap();
        assert_eq!(mxcsr() & !0x3f, owner_controls & !0x3f);
    }
}
