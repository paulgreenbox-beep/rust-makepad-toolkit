//! Opt-in CUDA acceleration for derived mesh QTO and conservative AABB tests.
#[cfg(windows)]
mod cuda;
mod math;
#[cfg(test)]
mod tests;
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use viewer_core::{Mesh, Vec3};
pub const GPU_MESH_BATCH_THRESHOLD: usize = 2048;
pub const GPU_AABB_BATCH_THRESHOLD: usize = 4096;
const MAX_TEMPORARY_BYTES: u64 = 256 * 1024 * 1024;
const BUDGET_MAX_AGE: Duration = Duration::from_secs(5);
#[derive(Default)]
struct State {
    luid: Option<[u8; 8]>,
    budget_bytes: u64,
    budget_sample: Option<Instant>,
    #[cfg(windows)]
    runtime: Option<cuda::Runtime>,
    initialization_failed: bool,
    mesh_validated: bool,
    bounds_validated: bool,
    mesh_batches: u64,
    qto_gpu_elements: u64,
    qto_cpu_elements: u64,
    bounds_batches: u64,
    gpu_items: u64,
    cpu_fallback_batches: u64,
    last_reason: Option<String>,
    last_duration_us: u64,
    peak_device_bytes: u64,
    last_cleanup_observation: Option<Value>,
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| Mutex::new(State::default()))
}
/// Enable only for the same current Windows adapter LUID as the D3D11 device.
pub fn configure_gpu_compute(luid: [u8; 8], local_headroom_bytes: u64) {
    let Ok(mut state) = state().lock() else {
        return;
    };
    dispose_runtime(&mut state);
    let last_cleanup_observation = state.last_cleanup_observation.take();
    *state = State {
        luid: Some(luid),
        last_cleanup_observation,
        budget_bytes: local_headroom_bytes,
        budget_sample: Some(Instant::now()),
        ..State::default()
    };
}
/// Refresh before geometry work; stale snapshots force visible CPU fallback.
pub fn update_gpu_compute_budget(local_headroom_bytes: u64) {
    if let Ok(mut state) = state().lock() {
        state.budget_bytes = local_headroom_bytes;
        state.budget_sample = Some(Instant::now());
    }
}
/// Release the module and CUDA context when the consumer closes.
pub fn shutdown_gpu_compute() {
    if let Ok(mut state) = state().lock() {
        #[cfg(windows)]
        {
            dispose_runtime(&mut state);
        }
        state.luid = None;
        state.last_reason = Some("compute_disabled".into());
    }
}
pub fn gpu_compute_observe() -> Value {
    let Ok(state) = state().lock() else {
        return json!({"status":"lock_poisoned"});
    };
    #[cfg(windows)]
    let device = state.runtime.as_ref().map(|runtime| runtime.identity());
    #[cfg(not(windows))]
    let device: Option<Value> = None;
    let owned_device_bytes = device
        .as_ref()
        .and_then(|device| device["owned_device_bytes"].as_u64());
    json!({
        "schema_version": 1, "backend": "cuda_driver_static_ptx",
        "enabled": state.luid.is_some(), "ready": device.is_some(), "actual_device": device,
        "requested_luid_bytes": state.luid, "allocation_kind": "cuMemAlloc_device",
        "managed_memory": false, "max_temporary_device_bytes": MAX_TEMPORARY_BYTES,
        "local_budget_headroom_bytes": state.budget_bytes,
        "budget_sample_age_ms": state.budget_sample.map(|t|t.elapsed().as_millis().min(u64::MAX as u128) as u64),
        "owned_device_bytes": owned_device_bytes, "last_cleanup":state.last_cleanup_observation, "peak_device_bytes": state.peak_device_bytes,
        "mesh_batch_threshold": GPU_MESH_BATCH_THRESHOLD, "aabb_batch_threshold": GPU_AABB_BATCH_THRESHOLD,
        "mesh_cpu_reference_validated": state.mesh_validated, "aabb_cpu_reference_validated": state.bounds_validated,
        "mesh_gpu_batches": state.mesh_batches, "qto_gpu_elements":state.qto_gpu_elements, "qto_cpu_elements":state.qto_cpu_elements, "aabb_gpu_batches": state.bounds_batches,
        "gpu_items": state.gpu_items, "cpu_fallback_batches": state.cpu_fallback_batches,
        "last_duration_us": state.last_duration_us, "last_fallback_reason": state.last_reason,
        "precision": "f64", "scope": "derived_mesh_qto_and_conservative_aabb"
    })
}
fn dispose_runtime(state: &mut State) {
    #[cfg(windows)]
    if let Some(mut runtime) = state.runtime.take() {
        let result = runtime.shutdown();
        state.last_cleanup_observation = Some(runtime.identity());
        if let Err(error) = result {
            state.initialization_failed = true;
            state.last_reason = Some(error);
        }
    }
    state.mesh_validated = false;
    state.bounds_validated = false;
}
fn fallback(state: &mut State, reason: impl Into<String>) {
    state.cpu_fallback_batches = state.cpu_fallback_batches.saturating_add(1);
    state.last_reason = Some(reason.into());
}
fn admission(state: &mut State, bytes: usize) -> bool {
    if state.luid.is_none() {
        fallback(state, "compute_disabled");
        return false;
    }
    if state
        .budget_sample
        .is_none_or(|sample| sample.elapsed() > BUDGET_MAX_AGE)
    {
        fallback(state, "local_budget_snapshot_stale");
        return false;
    }
    let cap = state
        .budget_bytes
        .saturating_sub(64 * 1024 * 1024)
        .min(MAX_TEMPORARY_BYTES);
    if bytes as u64 > cap {
        fallback(state, "local_budget_or_temporary_allocation_limit");
        return false;
    }
    if state.initialization_failed {
        fallback(state, "cuda_initialization_previously_failed");
        return false;
    }
    true
}
#[cfg(windows)]
fn runtime(state: &mut State) -> Result<&mut cuda::Runtime, String> {
    if state.runtime.is_none() {
        match cuda::Runtime::new(state.luid.ok_or("no_requested_luid")?) {
            Ok(runtime) => {
                state.mesh_validated = false;
                state.bounds_validated = false;
                state.runtime = Some(runtime);
            }
            Err(error) => {
                state.initialization_failed = true;
                return Err(error);
            }
        }
    }
    Ok(state.runtime.as_mut().expect("initialized runtime"))
}
pub(crate) fn close_enough(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-10 + 1e-9 * a.abs().max(b.abs())
}
fn sum_contributions(values: &[[f64; 2]]) -> (f64, f64) {
    let (mut volume, mut area) = (0.0, 0.0);
    for &[v, a] in values {
        volume += v;
        area += a;
    }
    (volume.abs(), area)
}
fn cpu_mesh(mesh: &Mesh, origin: Vec3) -> (f64, f64) {
    let (mut volume, mut area) = (0.0, 0.0);
    for triangle in mesh.triangles() {
        let [a, b, c] = triangle.vertices;
        area += (b - a).cross(c - a).length() * 0.5;
        volume += (a - origin).dot((b - origin).cross(c - origin)) / 6.0;
    }
    (volume.abs(), area)
}
pub(crate) fn mesh_quantities(mesh: &Mesh) -> Option<(f64, f64)> {
    mesh_quantities_batch(&[mesh]).and_then(|values| values.into_iter().next())
}
/// One upload/launch for many small meshes, with a separate origin and reduction
/// range for each element; opposite winding cannot cancel between elements.
pub(crate) fn mesh_quantities_batch(meshes: &[&Mesh]) -> Option<Vec<(f64, f64)>> {
    let count = meshes.iter().try_fold(0usize, |count, mesh| {
        count.checked_add(mesh.triangles().len())
    })?;
    if count < GPU_MESH_BATCH_THRESHOLD {
        return None;
    }
    let bytes = count.checked_mul(136)?;
    let Ok(mut state) = state().lock() else {
        return None;
    };
    if !admission(&mut state, bytes) {
        return None;
    }
    #[cfg(not(windows))]
    {
        fallback(&mut state, "cuda_requires_windows");
        None
    }
    #[cfg(windows)]
    {
        let started = Instant::now();
        let mut triangles = Vec::with_capacity(count);
        let mut ranges = Vec::with_capacity(meshes.len());
        for mesh in meshes {
            let origin = mesh
                .bounds()
                .map(|b| (b.min + b.max) * 0.5)
                .unwrap_or(Vec3::ZERO);
            let start = triangles.len();
            for triangle in mesh.triangles() {
                let [a, b, c] = triangle.vertices;
                triangles.push(math::pack_triangle(
                    [a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z],
                    [origin.x, origin.y, origin.z],
                ));
            }
            ranges.push((start..triangles.len(), origin));
        }
        if triangles.iter().flatten().any(|v| !v.is_finite()) {
            fallback(&mut state, "cuda_non_finite_packed_mesh");
            return None;
        }
        if !admission(&mut state, bytes) {
            return None;
        }
        let values =
            match runtime(&mut state).and_then(|runtime| runtime.mesh_contributions(&triangles)) {
                Ok(values) => values,
                Err(error) => {
                    state.mesh_validated = false;
                    state.bounds_validated = false;
                    dispose_runtime(&mut state);
                    fallback(&mut state, error);
                    return None;
                }
            };
        if values.len() != count {
            fallback(&mut state, "cuda_mesh_result_length_mismatch");
            return None;
        }
        let answers: Vec<_> = ranges
            .iter()
            .map(|(range, _)| sum_contributions(&values[range.clone()]))
            .collect();
        if answers
            .iter()
            .any(|&(volume, area)| !volume.is_finite() || !area.is_finite() || area < 0.0)
        {
            fallback(&mut state, "cuda_non_finite_qto");
            return None;
        }
        if !state.mesh_validated {
            let contributions = math::mesh_contributions(&triangles);
            let contributions_match = values
                .iter()
                .zip(&contributions)
                .all(|(gpu, cpu)| close_enough(gpu[0], cpu[0]) && close_enough(gpu[1], cpu[1]));
            let original_cpu_matches =
                meshes
                    .iter()
                    .zip(&ranges)
                    .zip(&answers)
                    .all(|((mesh, (_, origin)), answer)| {
                        let cpu = cpu_mesh(mesh, *origin);
                        close_enough(cpu.0, answer.0) && close_enough(cpu.1, answer.1)
                    });
            if !contributions_match || !original_cpu_matches {
                state.initialization_failed = true;
                dispose_runtime(&mut state);
                fallback(&mut state, "cuda_qto_cpu_reference_mismatch");
                return None;
            }
            state.mesh_validated = true;
        }
        state.mesh_batches = state.mesh_batches.saturating_add(1);
        state.gpu_items = state.gpu_items.saturating_add(count as u64);
        state.peak_device_bytes = state.peak_device_bytes.max(bytes as u64);
        state.last_duration_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        state.last_reason = None;
        Some(answers)
    }
}
pub(crate) fn record_qto_cpu_fallback(elements: usize) {
    if let Ok(mut state) = state().lock() {
        state.qto_cpu_elements = state.qto_cpu_elements.saturating_add(elements as u64);
    }
}
pub(crate) fn record_qto_batch_usage(gpu_elements: usize, cpu_elements: usize) {
    if let Ok(mut state) = state().lock() {
        state.qto_gpu_elements = state.qto_gpu_elements.saturating_add(gpu_elements as u64);
        state.qto_cpu_elements = state.qto_cpu_elements.saturating_add(cpu_elements as u64);
        if cpu_elements > 0 {
            state.last_reason =
                Some("qto_fields_without_equivalent_mesh_reference_retained_on_cpu".into());
        }
    }
}
pub(crate) fn aabb_batch_admitted(count: usize) -> bool {
    if count < GPU_AABB_BATCH_THRESHOLD {
        return false;
    }
    let Some(bytes) = count.checked_mul(52).and_then(|v| v.checked_add(48)) else {
        return false;
    };
    let Ok(mut state) = state().lock() else {
        return false;
    };
    admission(&mut state, bytes)
}
/// An unavailable or different-adapter sample immediately invalidates headroom.
pub fn refresh_gpu_compute_budget(sample: Option<([u8; 8], u64)>) {
    if let Ok(mut state) = state().lock() {
        match sample {
            Some((luid, bytes)) if Some(luid) == state.luid => {
                state.budget_bytes = bytes;
                state.budget_sample = Some(Instant::now());
            }
            _ => {
                state.budget_bytes = 0;
                state.budget_sample = None;
                state.last_reason = Some("local_budget_unavailable_or_adapter_changed".into());
            }
        }
    }
}
pub(crate) fn aabb_overlaps(bounds: &[[f64; 6]], query: [f64; 6]) -> Option<Vec<bool>> {
    if bounds.len() < GPU_AABB_BATCH_THRESHOLD {
        return None;
    }
    if bounds
        .iter()
        .flatten()
        .chain(query.iter())
        .any(|v| !v.is_finite())
        || bounds
            .iter()
            .chain(std::iter::once(&query))
            .any(|b| b[0] > b[3] || b[1] > b[4] || b[2] > b[5])
    {
        return None;
    }
    let bytes = bounds.len().checked_mul(52)?.checked_add(48)?;
    let Ok(mut state) = state().lock() else {
        return None;
    };
    if !admission(&mut state, bytes) {
        return None;
    }
    #[cfg(not(windows))]
    {
        fallback(&mut state, "cuda_requires_windows");
        None
    }
    #[cfg(windows)]
    {
        let started = Instant::now();
        let values =
            match runtime(&mut state).and_then(|runtime| runtime.aabb_overlaps(bounds, query)) {
                Ok(values) => values,
                Err(error) => {
                    dispose_runtime(&mut state);
                    fallback(&mut state, error);
                    return None;
                }
            };
        // Validate the first packet against exact CPU predicates; subsequent packets use the same kernel and CPU-confirm positive candidates.
        if values.len() != bounds.len()
            || (!state.bounds_validated
                && values
                    .iter()
                    .zip(bounds)
                    .any(|(&hit, bounds)| hit != math::aabb_overlap(bounds, &query)))
        {
            state.initialization_failed = true;
            dispose_runtime(&mut state);
            fallback(&mut state, "cuda_bounds_cpu_reference_mismatch");
            return None;
        }
        state.bounds_validated = true;
        state.bounds_batches = state.bounds_batches.saturating_add(1);
        state.gpu_items = state.gpu_items.saturating_add(bounds.len() as u64);
        state.peak_device_bytes = state.peak_device_bytes.max(bytes as u64);
        state.last_duration_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        state.last_reason = None;
        Some(values)
    }
}
