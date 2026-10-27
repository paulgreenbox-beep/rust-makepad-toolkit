//! CPU reference math and input packing for the static CUDA geometry kernels.
//!
//! Each packed triangle contains nine origin-relative vertex coordinates for
//! signed volume followed by the original b-a and c-a edge vectors for area.
//! Keeping the original edge differences avoids losing small edges when a mesh
//! has a distant outlier that moves its volume origin far from that triangle.

pub(crate) fn pack_triangle(triangle: [f64; 9], origin: [f64; 3]) -> [f64; 15] {
    let [ax, ay, az, bx, by, bz, cx, cy, cz] = triangle;
    [
        ax - origin[0],
        ay - origin[1],
        az - origin[2],
        bx - origin[0],
        by - origin[1],
        bz - origin[2],
        cx - origin[0],
        cy - origin[1],
        cz - origin[2],
        bx - ax,
        by - ay,
        bz - az,
        cx - ax,
        cy - ay,
        cz - az,
    ]
}

pub(crate) fn mesh_contributions(triangles: &[[f64; 15]]) -> Vec<[f64; 2]> {
    triangles
        .iter()
        .map(|triangle| {
            let ax = triangle[0];
            let ay = triangle[1];
            let az = triangle[2];
            let bx = triangle[3];
            let by = triangle[4];
            let bz = triangle[5];
            let cx = triangle[6];
            let cy = triangle[7];
            let cz = triangle[8];

            // Keep each multiply and add separate to mirror the PTX instruction
            // sequence and the Vec3 cross/dot operation order.
            let volume_cross_x = by * cz - bz * cy;
            let volume_cross_y = bz * cx - bx * cz;
            let volume_cross_z = bx * cy - by * cx;
            let volume_xy = ax * volume_cross_x + ay * volume_cross_y;
            let volume_dot = volume_xy + az * volume_cross_z;
            let signed_volume = volume_dot / 6.0;

            // Area edges were subtracted before recentering, preserving the
            // legacy area calculation even when the volume origin is distant.
            let abx = triangle[9];
            let aby = triangle[10];
            let abz = triangle[11];
            let acx = triangle[12];
            let acy = triangle[13];
            let acz = triangle[14];
            let area_cross_x = aby * acz - abz * acy;
            let area_cross_y = abz * acx - abx * acz;
            let area_cross_z = abx * acy - aby * acx;
            let area_xy = area_cross_x * area_cross_x + area_cross_y * area_cross_y;
            let area_squared = area_xy + area_cross_z * area_cross_z;
            let area = area_squared.sqrt() * 0.5;

            [signed_volume, area]
        })
        .collect()
}

/// Inclusive overlap test for finite, ordered AABBs packed as
/// [min_x, min_y, min_z, max_x, max_y, max_z].
///
/// The GPU wrapper validates finiteness and ordering before dispatch.
pub(crate) fn aabb_overlap(bounds: &[f64; 6], query: &[f64; 6]) -> bool {
    bounds[0] <= query[3]
        && bounds[3] >= query[0]
        && bounds[1] <= query[4]
        && bounds[4] >= query[1]
        && bounds[2] <= query[5]
        && bounds[5] >= query[2]
}

#[cfg(test)]
mod tests {
    use super::{aabb_overlap, mesh_contributions, pack_triangle};

    fn tetrahedron() -> [[f64; 9]; 4] {
        [
            // Faces are wound outward. The tetrahedron has volume 4.
            [0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 4.0],
            [0.0, 0.0, 0.0, 0.0, 0.0, 4.0, 0.0, 3.0, 0.0],
            [2.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 4.0],
        ]
    }

    fn pack_at_origin(triangles: &[[f64; 9]], origin: [f64; 3]) -> Vec<[f64; 15]> {
        triangles
            .iter()
            .map(|triangle| pack_triangle(*triangle, origin))
            .collect()
    }

    fn totals(contributions: &[[f64; 2]]) -> [f64; 2] {
        let mut volume = 0.0;
        let mut area = 0.0;
        for contribution in contributions {
            volume += contribution[0];
            area += contribution[1];
        }
        [volume, area]
    }

    fn assert_close(actual: f64, expected: f64) {
        let tolerance = 32.0 * f64::EPSILON * expected.abs().max(1.0);
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual {actual:?}, expected {expected:?}, tolerance {tolerance:?}"
        );
    }

    fn translate(triangles: &[[f64; 9]], offset: [f64; 3]) -> Vec<[f64; 9]> {
        triangles
            .iter()
            .map(|triangle| {
                std::array::from_fn(|coordinate| triangle[coordinate] + offset[coordinate % 3])
            })
            .collect()
    }

    #[test]
    fn relative_origin_keeps_closed_mesh_volume_stable_at_large_world_coordinates() {
        let triangles = tetrahedron();
        let origin = [1.0, 1.5, 2.0];
        let local = pack_at_origin(&triangles, origin);
        let large_translation = [2.0_f64.powi(50), -2.0_f64.powi(50), 2.0_f64.powi(50)];
        let translated_triangles = translate(&triangles, large_translation);
        let translated_origin = [
            origin[0] + large_translation[0],
            origin[1] + large_translation[1],
            origin[2] + large_translation[2],
        ];
        let translated = pack_at_origin(&translated_triangles, translated_origin);

        let local_totals = totals(&mesh_contributions(&local));
        let translated_totals = totals(&mesh_contributions(&translated));

        assert_close(local_totals[0], 4.0);
        assert_close(translated_totals[0], local_totals[0]);
        assert_close(local_totals[1], 13.0 + 61.0_f64.sqrt());
        assert_close(translated_totals[1], local_totals[1]);
    }

    #[test]
    fn reversing_winding_negates_volume_and_preserves_area() {
        let triangles = tetrahedron();
        let reversed: Vec<[f64; 9]> = triangles
            .iter()
            .map(|triangle| {
                [
                    triangle[0],
                    triangle[1],
                    triangle[2],
                    triangle[6],
                    triangle[7],
                    triangle[8],
                    triangle[3],
                    triangle[4],
                    triangle[5],
                ]
            })
            .collect();

        let forward = totals(&mesh_contributions(&pack_at_origin(&triangles, [0.0; 3])));
        let backward = totals(&mesh_contributions(&pack_at_origin(&reversed, [0.0; 3])));

        assert_close(forward[0], 4.0);
        assert_close(backward[0], -forward[0]);
        assert_close(backward[1], forward[1]);
    }

    #[test]
    fn area_edges_survive_an_extreme_volume_origin_shift() {
        let triangle = [0.0, 0.0, 0.0, 0.25, 0.0, 0.0, 0.0, 0.25, 0.0];
        let packed = pack_triangle(triangle, [5.0e15, 0.0, 0.0]);
        let contribution = mesh_contributions(&[packed])[0];

        assert_close(contribution[1], 0.03125);
    }

    #[test]
    fn aabb_overlap_includes_touching_faces_and_rejects_a_gap() {
        let left = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let touching = [1.0, 0.25, 0.25, 2.0, 0.75, 0.75];
        let separated = [1.0000000000000002, 0.25, 0.25, 2.0, 0.75, 0.75];

        assert!(aabb_overlap(&left, &touching));
        assert!(!aabb_overlap(&left, &separated));
    }

    #[test]
    fn aabb_overlap_handles_large_finite_coordinates() {
        let base = 2.0_f64.powi(50);
        let left = [base, -base, base, base + 8.0, -base + 8.0, base + 8.0];
        let touching = [
            base + 8.0,
            -base + 8.0,
            base + 8.0,
            base + 16.0,
            -base + 16.0,
            base + 16.0,
        ];
        let separated = [
            base + 16.0,
            -base + 8.0,
            base + 8.0,
            base + 24.0,
            -base + 16.0,
            base + 16.0,
        ];

        assert!(aabb_overlap(&left, &touching));
        assert!(!aabb_overlap(&left, &separated));
    }
}
