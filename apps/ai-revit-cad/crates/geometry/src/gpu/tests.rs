use super::{
    BUDGET_MAX_AGE, GPU_AABB_BATCH_THRESHOLD, GPU_MESH_BATCH_THRESHOLD, State, aabb_overlaps,
    admission, mesh_quantities,
};
#[cfg(windows)]
use super::{
    configure_gpu_compute, gpu_compute_observe, mesh_quantities_batch, shutdown_gpu_compute,
};
use std::time::{Duration, Instant};
use viewer_core::{Mesh, Triangle, Vec3};

fn local_state(budget_bytes: u64, budget_sample: Option<Instant>) -> State {
    State {
        luid: Some([0; 8]),
        budget_bytes,
        budget_sample,
        ..State::default()
    }
}

#[test]
fn admission_applies_fresh_headroom_and_temporary_allocation_caps() {
    let mib = 1024_u64 * 1024;
    let max_temporary = usize::try_from(256 * mib).unwrap();
    let mut capped = local_state(1024 * mib, Some(Instant::now()));
    assert!(admission(&mut capped, max_temporary));
    let mut over_cap = local_state(1024 * mib, Some(Instant::now()));
    assert!(!admission(&mut over_cap, max_temporary + 1));
    assert_eq!(
        over_cap.last_reason.as_deref(),
        Some("local_budget_or_temporary_allocation_limit")
    );

    let constrained = usize::try_from(32 * mib).unwrap();
    let mut headroom_limited = local_state(96 * mib, Some(Instant::now()));
    assert!(admission(&mut headroom_limited, constrained));
    let mut above_headroom = local_state(96 * mib, Some(Instant::now()));
    assert!(!admission(&mut above_headroom, constrained + 1));
}

#[test]
fn admission_rejects_missing_and_stale_configuration() {
    let mut disabled = State::default();
    assert!(!admission(&mut disabled, 1));
    assert_eq!(disabled.last_reason.as_deref(), Some("compute_disabled"));

    let sample = Instant::now() - BUDGET_MAX_AGE - Duration::from_millis(1);
    let mut stale = local_state(1024 * 1024 * 1024, Some(sample));
    assert!(!admission(&mut stale, 1));
    assert_eq!(
        stale.last_reason.as_deref(),
        Some("local_budget_snapshot_stale")
    );

    let mut unsampled = local_state(1024 * 1024 * 1024, None);
    assert!(!admission(&mut unsampled, 1));
    assert_eq!(
        unsampled.last_reason.as_deref(),
        Some("local_budget_snapshot_stale")
    );
}

#[test]
fn batch_thresholds_reject_work_before_any_device_admission() {
    let degenerate = Triangle::new(1, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
    let below_mesh = Mesh::new(vec![degenerate; GPU_MESH_BATCH_THRESHOLD - 1]);
    assert!(mesh_quantities(&below_mesh).is_none());

    let below_bounds = vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]; GPU_AABB_BATCH_THRESHOLD - 1];
    assert!(aabb_overlaps(&below_bounds, [0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).is_none());
}

#[cfg(windows)]
mod physical {
    use super::*;

    struct ShutdownOnDrop;
    impl Drop for ShutdownOnDrop {
        fn drop(&mut self) {
            shutdown_gpu_compute();
        }
    }

    fn parse_test_luid(raw: &str) -> [u8; 8] {
        assert!(
            raw.len() == 16 && raw.is_ascii() && raw.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "AI_REVIT_CUDA_TEST_LUID must be exactly 16 hexadecimal characters. Each byte pair is in little-endian order."
        );
        std::array::from_fn(|index| {
            u8::from_str_radix(&raw[index * 2..index * 2 + 2], 16)
                .expect("validated hexadecimal LUID byte")
        })
    }

    fn require_gpu_result<T>(result: Option<T>, context: &str) -> T {
        result.unwrap_or_else(|| panic!("{context}; GPU observation: {}", gpu_compute_observe()))
    }

    fn legacy_mesh_quantities(mesh: &Mesh) -> (f64, f64) {
        let origin = mesh
            .bounds()
            .map(|bounds| (bounds.min + bounds.max) * 0.5)
            .unwrap_or(Vec3::ZERO);
        let (mut volume, mut area) = (0.0, 0.0);
        for triangle in mesh.triangles() {
            let [a, b, c] = triangle.vertices;
            area += (b - a).cross(c - a).length() * 0.5;
            volume += (a - origin).dot((b - origin).cross(c - origin)) / 6.0;
        }
        (volume.abs(), area)
    }

    fn assert_quantities_close(actual: (f64, f64), expected: (f64, f64), label: &str) {
        for (name, actual, expected) in [
            ("volume", actual.0, expected.0),
            ("area", actual.1, expected.1),
        ] {
            let tolerance = 1.0e-10 + 1.0e-9 * actual.abs().max(expected.abs());
            assert!(
                actual.is_finite()
                    && expected.is_finite()
                    && (actual - expected).abs() <= tolerance,
                "{label} {name}: GPU returned {actual:?}; legacy CPU returned {expected:?}; tolerance {tolerance:?}"
            );
        }
    }

    fn repeated_cube(
        repetitions: usize,
        translation: Vec3,
        reverse_winding: bool,
        add_degenerate: bool,
    ) -> Mesh {
        let cube = Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0));
        let mut triangles = Vec::with_capacity(repetitions * cube.triangles().len() + 1);
        for _ in 0..repetitions {
            for source in cube.triangles() {
                let mut vertices = source.vertices.map(|vertex| vertex + translation);
                if reverse_winding {
                    vertices.swap(1, 2);
                }
                let triangle_id = u32::try_from(triangles.len() + 1)
                    .expect("physical test triangle count fits u32");
                triangles.push(Triangle::new(
                    triangle_id,
                    vertices[0],
                    vertices[1],
                    vertices[2],
                ));
            }
        }
        if add_degenerate {
            let point = translation + Vec3::new(3.0, 3.0, 3.0);
            let triangle_id =
                u32::try_from(triangles.len() + 1).expect("physical test triangle count fits u32");
            triangles.push(Triangle::new(triangle_id, point, point, point));
        }
        Mesh::new(triangles)
    }

    fn large_extent_mesh() -> Mesh {
        let cube = Mesh::cuboid(Vec3::new(2.0, 2.0, 2.0));
        let repetitions = 171;
        let mut triangles = Vec::with_capacity(repetitions * cube.triangles().len() + 2);
        for _ in 0..repetitions {
            for source in cube.triangles() {
                let triangle_id = u32::try_from(triangles.len() + 1)
                    .expect("physical test triangle count fits u32");
                triangles.push(Triangle::new(
                    triangle_id,
                    source.vertices[0],
                    source.vertices[1],
                    source.vertices[2],
                ));
            }
        }

        // The distant finite point moves the mesh origin far enough that
        // recentering this 0.25 edge would erase it from the area calculation.
        let point = Vec3::new(1.0e16, 0.0, 0.0);
        let triangle_id =
            u32::try_from(triangles.len() + 1).expect("physical test triangle count fits u32");
        triangles.push(Triangle::new(triangle_id, point, point, point));

        let triangle_id =
            u32::try_from(triangles.len() + 1).expect("physical test triangle count fits u32");
        triangles.push(Triangle::new(
            triangle_id,
            Vec3::ZERO,
            Vec3::new(0.25, 0.0, 0.0),
            Vec3::new(0.0, 0.25, 0.0),
        ));
        Mesh::new(triangles)
    }

    fn bounds_batch(patterns: [[f64; 6]; 4]) -> Vec<[f64; 6]> {
        for (index, bounds) in patterns.iter().enumerate() {
            assert!(
                bounds.iter().all(|coordinate| coordinate.is_finite()),
                "AABB fixture {index} contains a non-finite coordinate: {bounds:?}"
            );
            assert!(
                bounds[0] <= bounds[3] && bounds[1] <= bounds[4] && bounds[2] <= bounds[5],
                "AABB fixture {index} has reversed min/max coordinates: {bounds:?}"
            );
        }
        (0..GPU_AABB_BATCH_THRESHOLD)
            .map(|index| patterns[index % patterns.len()])
            .collect()
    }

    #[test]
    #[ignore = "requires an explicitly configured Windows NVIDIA adapter LUID in AI_REVIT_CUDA_TEST_LUID"]
    fn physical_cuda_qto_and_overlap() {
        let _shutdown_on_exit = ShutdownOnDrop;
        let luid_text = std::env::var("AI_REVIT_CUDA_TEST_LUID")
            .expect("set AI_REVIT_CUDA_TEST_LUID to 16 hex characters in little-endian byte order");
        let luid = parse_test_luid(&luid_text);
        configure_gpu_compute(luid, 1024 * 1024 * 1024);

        let translation = Vec3::new(1.0e9, -1.0e9, 1.0e9);
        let benign = repeated_cube(171, translation, false, false);
        assert!(benign.triangles().len() >= GPU_MESH_BATCH_THRESHOLD);
        let benign_expected = legacy_mesh_quantities(&benign);
        let benign_actual = require_gpu_result(
            mesh_quantities(&benign),
            "large translated closed cube batch must execute on the configured GPU",
        );
        assert_quantities_close(benign_actual, benign_expected, "translated outward cube");
        assert_quantities_close(
            benign_actual,
            (171.0 * 8.0, 171.0 * 24.0),
            "known cube totals",
        );

        let small_meshes: Vec<Mesh> = (0..256)
            .map(|index| {
                let translation = Vec3::new(
                    1.0e9 + index as f64 * 16.0,
                    -1.0e9 - index as f64 * 8.0,
                    2.0e9 + index as f64 * 4.0,
                );
                repeated_cube(1, translation, index % 2 == 1, false)
            })
            .collect();
        assert!(
            small_meshes
                .iter()
                .all(|mesh| mesh.triangles().len() < GPU_MESH_BATCH_THRESHOLD)
        );
        let small_mesh_refs: Vec<&Mesh> = small_meshes.iter().collect();
        let small_expected: Vec<_> = small_meshes.iter().map(legacy_mesh_quantities).collect();
        let small_actual = require_gpu_result(
            mesh_quantities_batch(&small_mesh_refs),
            "256 small meshes should cross the aggregate GPU triangle threshold",
        );
        assert_eq!(small_actual.len(), small_meshes.len());
        for (index, (actual, expected)) in small_actual.iter().zip(&small_expected).enumerate() {
            assert_quantities_close(*actual, *expected, &format!("small mesh {index}"));
            assert_quantities_close(*actual, (8.0, 24.0), &format!("small mesh {index} totals"));
        }

        let reversed = repeated_cube(171, translation, true, true);
        let reversed_expected = legacy_mesh_quantities(&reversed);
        let reversed_actual = require_gpu_result(
            mesh_quantities(&reversed),
            "reversed winding with a finite degenerate triangle must execute on GPU",
        );
        assert_quantities_close(
            reversed_actual,
            reversed_expected,
            "reversed cube and degenerate",
        );

        // Run after the benign batch has set the one-time CPU reference flag.
        // The origin is about 5e15 while the small triangle's edge is 0.25.
        let outlier = large_extent_mesh();
        let outlier_expected = legacy_mesh_quantities(&outlier);
        let outlier_actual = require_gpu_result(
            mesh_quantities(&outlier),
            "large-extent triangle batch must execute on the configured GPU",
        );
        assert_quantities_close(
            outlier_actual,
            outlier_expected,
            "distant outlier area regression",
        );
        assert!(
            (outlier_actual.1 - outlier_expected.1).abs()
                <= 1.0e-10 + 1.0e-9 * outlier_expected.1.abs()
        );
        let expected_outlier_area = 171.0 * 24.0 + 0.03125;
        let area_tolerance = 32.0 * f64::EPSILON * expected_outlier_area;
        assert!(
            (outlier_expected.1 - expected_outlier_area).abs() <= area_tolerance,
            "legacy CPU area must retain the small triangle edge"
        );

        let query = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let small_bounds = bounds_batch([
            [1.0, 0.0, 0.0, 2.0, 1.0, 1.0],
            [1.25, 0.0, 0.0, 2.0, 1.0, 1.0],
            [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5],
            [1.0, 1.0, 1.0, 2.0, 2.0, 2.0],
        ]);
        let small_overlaps = require_gpu_result(
            aabb_overlaps(&small_bounds, query),
            "4096 finite AABBs must execute on the configured GPU",
        );
        assert!(small_overlaps[0], "face touching is inclusive");
        assert!(!small_overlaps[1], "a separated AABB must not overlap");
        assert!(small_overlaps[2], "an interior AABB overlaps");
        assert!(small_overlaps[3], "corner touching is inclusive");

        let base = 2.0_f64.powi(50);
        let large_query = [base, -base, base, base + 8.0, -base + 8.0, base + 8.0];
        let large_bounds = bounds_batch([
            [
                base + 8.0,
                -base + 2.0,
                base + 2.0,
                base + 16.0,
                -base + 4.0,
                base + 4.0,
            ],
            [
                base + 16.0,
                -base + 2.0,
                base + 2.0,
                base + 24.0,
                -base + 4.0,
                base + 4.0,
            ],
            [
                base + 2.0,
                -base + 2.0,
                base + 2.0,
                base + 6.0,
                -base + 6.0,
                base + 6.0,
            ],
            [
                base + 8.0,
                -base + 8.0,
                base + 8.0,
                base + 16.0,
                -base + 16.0,
                base + 16.0,
            ],
        ]);
        let large_overlaps = require_gpu_result(
            aabb_overlaps(&large_bounds, large_query),
            "4096 large-coordinate finite AABBs must execute on the configured GPU",
        );
        assert!(
            large_overlaps[0],
            "large-coordinate face touching is inclusive"
        );
        assert!(!large_overlaps[1], "large-coordinate gap is disjoint");
        assert!(large_overlaps[2], "large-coordinate interior box overlaps");
        assert!(
            large_overlaps[3],
            "large-coordinate corner touching is inclusive"
        );

        let observation = gpu_compute_observe();
        assert_eq!(observation["ready"], true, "CUDA runtime should be ready");
        assert_eq!(
            observation["actual_device"]["matches_d3d11_luid"], true,
            "observed CUDA adapter must match the explicitly requested D3D11 LUID"
        );
        let actual_luid: Vec<u8> = observation["actual_device"]["luid_bytes"]
            .as_array()
            .expect("observed LUID byte array")
            .iter()
            .map(|byte| u8::try_from(byte.as_u64().expect("LUID byte is numeric")).unwrap())
            .collect();
        assert_eq!(
            actual_luid,
            luid.to_vec(),
            "runtime selected the requested LUID bytes"
        );
        assert_eq!(observation["mesh_cpu_reference_validated"], true);
        assert_eq!(observation["aabb_cpu_reference_validated"], true);
        assert!(
            observation["mesh_gpu_batches"].as_u64().unwrap_or(0) >= 4,
            "all QTO batches should have reached CUDA"
        );
        assert!(
            observation["aabb_gpu_batches"].as_u64().unwrap_or(0) >= 2,
            "both 4096-bound batches should have reached CUDA"
        );
        assert!(
            observation["gpu_items"].as_u64().unwrap_or(0) > 0,
            "observed CUDA item count should be positive"
        );
        assert_eq!(
            observation["owned_device_bytes"].as_u64(),
            Some(0),
            "all per-call device allocations should have been released"
        );
    }
}
