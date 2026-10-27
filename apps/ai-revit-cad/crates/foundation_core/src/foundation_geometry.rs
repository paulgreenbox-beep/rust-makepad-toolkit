//! Geometry-only foundation layout records and derived dimensions.
//!
//! These values describe placement geometry and quantities only. They do not
//! define soil stiffness, bearing capacity, settlement, or design acceptance.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::{FoundationError, FoundationId};

const MAX_LAYOUT_ELEMENTS: usize = 1024;
const MAX_LAYOUT_TIERS: usize = 1024;
const LENGTH_TOLERANCE_M: f64 = 1.0e-9;

/// User-authored foundation placements to persist through the Core document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationLayout {
    /// Stable semantic ID within the owning document.
    pub id: String,
    pub elements: Vec<FoundationPlacement>,
}

/// Geometry of one foundation placement. Coordinates are metres in global
/// right-handed XYZ; `center_m[2]` is the lower face elevation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum FoundationPlacement {
    Raft {
        id: FoundationId,
        center_m: [f64; 3],
        length_x_m: f64,
        width_y_m: f64,
        thickness_m: f64,
    },
    Strip {
        id: FoundationId,
        center_m: [f64; 3],
        length_m: f64,
        width_m: f64,
        rotation_rad: f64,
        thickness_m: f64,
    },
    Isolated {
        id: FoundationId,
        center_m: [f64; 3],
        length_x_m: f64,
        width_y_m: f64,
        thickness_m: f64,
    },
    /// A bottom-up stack of nested rectangular tiers.
    Stepped {
        id: FoundationId,
        tiers: Vec<FoundationTier>,
    },
}

/// One rectangular prism in a stepped foundation. Its XY centre locates the
/// plan rectangle and its Z coordinate is the lower face elevation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationTier {
    pub id: String,
    pub center_m: [f64; 3],
    pub length_x_m: f64,
    pub width_y_m: f64,
    pub thickness_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FoundationPlacementKind {
    Raft,
    Strip,
    Isolated,
    Stepped,
}

/// One explicitly sampled plan location for a later Geo column query.
/// It records a point only; it does not assert coverage beneath the full shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationGeometrySamplePoint {
    pub placement_id: FoundationId,
    pub tier_id: Option<String>,
    pub xy_m: [f64; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationTierGeometry {
    pub id: String,
    pub center_m: [f64; 3],
    pub length_x_m: f64,
    pub width_y_m: f64,
    pub thickness_m: f64,
    pub plan_area_m2: f64,
    pub volume_m3: f64,
    pub perimeter_m: f64,
    pub corners_xy_m: [[f64; 2]; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationElementGeometry {
    pub id: FoundationId,
    pub kind: FoundationPlacementKind,
    /// Plan area of this element's base footprint. Stepped elements use their
    /// lowest tier footprint; tiers are also reported individually.
    pub plan_area_m2: f64,
    /// Sum of prism volumes within this element; no overlap union is applied.
    pub summed_volume_m3: f64,
    pub perimeter_m: f64,
    pub base_elevation_m: f64,
    pub corners_xy_m: [[f64; 2]; 4],
    pub tiers: Vec<FoundationTierGeometry>,
    /// Four vertices and the center of the contact footprint; stepped shapes
    /// use only the lowest tier's footprint and ID.
    pub sample_points: Vec<FoundationGeometrySamplePoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundationLayoutGeometry {
    pub layout_id: String,
    pub elements: Vec<FoundationElementGeometry>,
    /// Sum of element quantities. It is not the union area of overlapping
    /// placements and carries no engineering interpretation.
    pub summed_element_plan_area_m2: f64,
    pub summed_element_volume_m3: f64,
}

/// Validate only the serialized placement geometry and stable IDs.
pub fn validate_layout(layout: &FoundationLayout) -> Result<(), FoundationError> {
    validate_id(&layout.id, "layout.id")?;
    if layout.elements.is_empty() {
        return Err(invalid("elements", "at least one placement is required"));
    }
    if layout.elements.len() > MAX_LAYOUT_ELEMENTS {
        return Err(invalid("elements", "placement count exceeds the configured limit"));
    }

    let mut ids = BTreeSet::new();
    let mut total_tiers = 0usize;
    for (index, element) in layout.elements.iter().enumerate() {
        match element {
            FoundationPlacement::Raft {
                id,
                center_m,
                length_x_m,
                width_y_m,
                thickness_m,
            }
            | FoundationPlacement::Isolated {
                id,
                center_m,
                length_x_m,
                width_y_m,
                thickness_m,
            } => {
                add_unique_id(&mut ids, id.as_str(), &format!("elements/{index}/id"))?;
                validate_box(center_m, *length_x_m, *width_y_m, *thickness_m, index)?;
            }
            FoundationPlacement::Strip {
                id,
                center_m,
                length_m,
                width_m,
                rotation_rad,
                thickness_m,
            } => {
                add_unique_id(&mut ids, id.as_str(), &format!("elements/{index}/id"))?;
                validate_box(center_m, *length_m, *width_m, *thickness_m, index)?;
                if *length_m < *width_m {
                    return Err(invalid(
                        format!("elements/{index}/length_m"),
                        "strip length must be at least its width",
                    ));
                }
                finite(*rotation_rad, &format!("elements/{index}/rotation_rad"))?;
                rotated_corners(
                    [center_m[0], center_m[1]],
                    *length_m,
                    *width_m,
                    *rotation_rad,
                )?;
            }
            FoundationPlacement::Stepped { id, tiers } => {
                add_unique_id(&mut ids, id.as_str(), &format!("elements/{index}/id"))?;
                if tiers.len() < 2 {
                    return Err(invalid(
                        format!("elements/{index}/tiers"),
                        "a stepped placement requires at least two tiers",
                    ));
                }
                total_tiers = total_tiers.checked_add(tiers.len()).ok_or_else(|| {
                    invalid("elements/tiers", "tier count overflow")
                })?;
                if total_tiers > MAX_LAYOUT_TIERS {
                    return Err(invalid("elements/tiers", "tier count exceeds the configured limit"));
                }
                for (tier_index, tier) in tiers.iter().enumerate() {
                    add_unique_id(
                        &mut ids,
                        &tier.id,
                        &format!("elements/{index}/tiers/{tier_index}/id"),
                    )?;
                    validate_box(
                        &tier.center_m,
                        tier.length_x_m,
                        tier.width_y_m,
                        tier.thickness_m,
                        index,
                    )?;
                }
                validate_tier_stack(index, tiers)?;
            }
        }
    }
    Ok(())
}

/// Derive plan quantities and sample points without resolving soil properties
/// or invoking any foundation analysis.
pub fn derive_layout_geometry(
    layout: &FoundationLayout,
) -> Result<FoundationLayoutGeometry, FoundationError> {
    validate_layout(layout)?;
    let mut elements = Vec::with_capacity(layout.elements.len());
    let mut summed_area = 0.0;
    let mut summed_volume = 0.0;

    for placement in &layout.elements {
        let element = match placement {
            FoundationPlacement::Raft {
                id,
                center_m,
                length_x_m,
                width_y_m,
                thickness_m,
            } => rectangular_element(
                id,
                FoundationPlacementKind::Raft,
                *center_m,
                *length_x_m,
                *width_y_m,
                *thickness_m,
                0.0,
            )?,
            FoundationPlacement::Strip {
                id,
                center_m,
                length_m,
                width_m,
                rotation_rad,
                thickness_m,
            } => rectangular_element(
                id,
                FoundationPlacementKind::Strip,
                *center_m,
                *length_m,
                *width_m,
                *thickness_m,
                *rotation_rad,
            )?,
            FoundationPlacement::Isolated {
                id,
                center_m,
                length_x_m,
                width_y_m,
                thickness_m,
            } => rectangular_element(
                id,
                FoundationPlacementKind::Isolated,
                *center_m,
                *length_x_m,
                *width_y_m,
                *thickness_m,
                0.0,
            )?,
            FoundationPlacement::Stepped { id, tiers } => stepped_element(id, tiers)?,
        };
        summed_area += element.plan_area_m2;
        summed_volume += element.summed_volume_m3;
        if !summed_area.is_finite() || !summed_volume.is_finite() {
            return Err(invalid("elements", "derived layout totals are non-finite"));
        }
        elements.push(element);
    }

    Ok(FoundationLayoutGeometry {
        layout_id: layout.id.clone(),
        elements,
        summed_element_plan_area_m2: summed_area,
        summed_element_volume_m3: summed_volume,
    })
}

fn rectangular_element(
    id: &FoundationId,
    kind: FoundationPlacementKind,
    center_m: [f64; 3],
    length_m: f64,
    width_m: f64,
    thickness_m: f64,
    rotation_rad: f64,
) -> Result<FoundationElementGeometry, FoundationError> {
    let area = length_m * width_m;
    let volume = area * thickness_m;
    let perimeter = 2.0 * (length_m + width_m);
    if !area.is_finite() || !volume.is_finite() || !perimeter.is_finite() {
        return Err(invalid("geometry", "derived placement quantities are non-finite"));
    }
    let corners = rotated_corners([center_m[0], center_m[1]], length_m, width_m, rotation_rad)?;
    let sample_points = footprint_sample_points(
        id,
        None,
        [center_m[0], center_m[1]],
        corners,
    );
    Ok(FoundationElementGeometry {
        id: id.clone(),
        kind,
        plan_area_m2: area,
        summed_volume_m3: volume,
        perimeter_m: perimeter,
        base_elevation_m: center_m[2],
        corners_xy_m: corners,
        tiers: Vec::new(),
        sample_points,
    })
}

fn stepped_element(
    id: &FoundationId,
    tiers: &[FoundationTier],
) -> Result<FoundationElementGeometry, FoundationError> {
    let mut tier_geometry = Vec::with_capacity(tiers.len());
    let mut volume_sum = 0.0;
    for tier in tiers {
        let area = tier.length_x_m * tier.width_y_m;
        let volume = area * tier.thickness_m;
        let perimeter = 2.0 * (tier.length_x_m + tier.width_y_m);
        if !area.is_finite() || !volume.is_finite() || !perimeter.is_finite() {
            return Err(invalid("tiers", "derived tier quantities are non-finite"));
        }
        volume_sum += volume;
        if !volume_sum.is_finite() {
            return Err(invalid("tiers", "derived stepped volume is non-finite"));
        }
        tier_geometry.push(FoundationTierGeometry {
            id: tier.id.clone(),
            center_m: tier.center_m,
            length_x_m: tier.length_x_m,
            width_y_m: tier.width_y_m,
            thickness_m: tier.thickness_m,
            plan_area_m2: area,
            volume_m3: volume,
            perimeter_m: perimeter,
            corners_xy_m: axis_aligned_corners(
                [tier.center_m[0], tier.center_m[1]],
                tier.length_x_m,
                tier.width_y_m,
            ),
        });
    }
    let base = tier_geometry.first().ok_or_else(|| {
        invalid("tiers", "a stepped placement requires at least two tiers")
    })?;
    // Ground samples describe the sole contact footprint at the lowest tier.
    // Upper tier elevations are retained as geometry, not substituted as new
    // foundation base elevations or treated as separate soil interfaces.
    let sample_points = footprint_sample_points(
        id,
        Some(base.id.clone()),
        [base.center_m[0], base.center_m[1]],
        base.corners_xy_m,
    );
    Ok(FoundationElementGeometry {
        id: id.clone(),
        kind: FoundationPlacementKind::Stepped,
        plan_area_m2: base.plan_area_m2,
        summed_volume_m3: volume_sum,
        perimeter_m: base.perimeter_m,
        base_elevation_m: base.center_m[2],
        corners_xy_m: base.corners_xy_m,
        tiers: tier_geometry,
        sample_points,
    })
}

fn footprint_sample_points(
    id: &FoundationId,
    tier_id: Option<String>,
    center_xy: [f64; 2],
    corners_xy: [[f64; 2]; 4],
) -> Vec<FoundationGeometrySamplePoint> {
    let mut points = Vec::with_capacity(5);
    for xy_m in corners_xy {
        points.push(FoundationGeometrySamplePoint {
            placement_id: id.clone(),
            tier_id: tier_id.clone(),
            xy_m,
        });
    }
    points.push(FoundationGeometrySamplePoint {
        placement_id: id.clone(),
        tier_id,
        xy_m: center_xy,
    });
    points
}

fn validate_tier_stack(index: usize, tiers: &[FoundationTier]) -> Result<(), FoundationError> {
    for pair in tiers.windows(2) {
        let lower = &pair[0];
        let upper = &pair[1];
        let expected_upper_z = lower.center_m[2] + lower.thickness_m;
        if !expected_upper_z.is_finite()
            || (upper.center_m[2] - expected_upper_z).abs() > LENGTH_TOLERANCE_M
        {
            return Err(invalid(
                format!("elements/{index}/tiers"),
                "tiers must be ordered bottom-up and touch at their horizontal faces",
            ));
        }
        let lower_min_x = lower.center_m[0] - lower.length_x_m / 2.0;
        let lower_max_x = lower.center_m[0] + lower.length_x_m / 2.0;
        let lower_min_y = lower.center_m[1] - lower.width_y_m / 2.0;
        let lower_max_y = lower.center_m[1] + lower.width_y_m / 2.0;
        let upper_min_x = upper.center_m[0] - upper.length_x_m / 2.0;
        let upper_max_x = upper.center_m[0] + upper.length_x_m / 2.0;
        let upper_min_y = upper.center_m[1] - upper.width_y_m / 2.0;
        let upper_max_y = upper.center_m[1] + upper.width_y_m / 2.0;
        let tol = LENGTH_TOLERANCE_M;
        if upper_min_x < lower_min_x - tol
            || upper_max_x > lower_max_x + tol
            || upper_min_y < lower_min_y - tol
            || upper_max_y > lower_max_y + tol
            || (upper.length_x_m - lower.length_x_m).abs() <= tol
                && (upper.width_y_m - lower.width_y_m).abs() <= tol
                && (upper.center_m[0] - lower.center_m[0]).abs() <= tol
                && (upper.center_m[1] - lower.center_m[1]).abs() <= tol
        {
            return Err(invalid(
                format!("elements/{index}/tiers"),
                "each upper tier must be strictly smaller and contained in the tier below",
            ));
        }
    }
    Ok(())
}

fn rotated_corners(
    center_xy: [f64; 2],
    length_x_m: f64,
    width_y_m: f64,
    rotation_rad: f64,
) -> Result<[[f64; 2]; 4], FoundationError> {
    let (sin, cos) = rotation_rad.sin_cos();
    let local = [
        [-length_x_m / 2.0, -width_y_m / 2.0],
        [length_x_m / 2.0, -width_y_m / 2.0],
        [length_x_m / 2.0, width_y_m / 2.0],
        [-length_x_m / 2.0, width_y_m / 2.0],
    ];
    let mut corners = [[0.0; 2]; 4];
    for (index, [x, y]) in local.into_iter().enumerate() {
        corners[index] = [center_xy[0] + x * cos - y * sin, center_xy[1] + x * sin + y * cos];
        if corners[index].iter().any(|value| !value.is_finite()) {
            return Err(invalid("geometry", "derived footprint corner is non-finite"));
        }
    }
    Ok(corners)
}

fn axis_aligned_corners(center_xy: [f64; 2], length_x_m: f64, width_y_m: f64) -> [[f64; 2]; 4] {
    let half_x = length_x_m / 2.0;
    let half_y = width_y_m / 2.0;
    [
        [center_xy[0] - half_x, center_xy[1] - half_y],
        [center_xy[0] + half_x, center_xy[1] - half_y],
        [center_xy[0] + half_x, center_xy[1] + half_y],
        [center_xy[0] - half_x, center_xy[1] + half_y],
    ]
}

fn validate_box(
    center_m: &[f64; 3],
    length_m: f64,
    width_m: f64,
    thickness_m: f64,
    element_index: usize,
) -> Result<(), FoundationError> {
    for (axis, value) in center_m.iter().enumerate() {
        finite(*value, &format!("elements/{element_index}/center_m/{axis}"))?;
    }
    for (field, value) in [
        ("length", length_m),
        ("width", width_m),
        ("thickness", thickness_m),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(invalid(
                format!("elements/{element_index}/{field}"),
                "dimension must be finite and positive",
            ));
        }
    }
    let corners = axis_aligned_corners(
        [center_m[0], center_m[1]],
        length_m,
        width_m,
    );
    if corners.iter().flatten().any(|value| !value.is_finite())
        || !(center_m[2] + thickness_m).is_finite()
    {
        return Err(invalid(
            format!("elements/{element_index}/geometry"),
            "derived footprint bounds or top elevation are non-finite",
        ));
    }
    Ok(())
}

fn validate_id(value: &str, field: &str) -> Result<(), FoundationError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.@:/%".contains(&byte))
    {
        return Err(invalid(field, "ID must be 1..=128 allowed ASCII characters"));
    }
    Ok(())
}

fn add_unique_id(
    ids: &mut BTreeSet<String>,
    id: &str,
    field: &str,
) -> Result<(), FoundationError> {
    validate_id(id, field)?;
    if !ids.insert(id.to_owned()) {
        return Err(invalid(field, "duplicate layout or placement ID"));
    }
    Ok(())
}

fn finite(value: f64, field: &str) -> Result<(), FoundationError> {
    if !value.is_finite() {
        return Err(invalid(field, "value must be finite"));
    }
    Ok(())
}

fn invalid(field: impl Into<String>, reason: impl Into<String>) -> FoundationError {
    FoundationError::InvalidInput {
        field: field.into(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    fn foundation_id(value: &str) -> FoundationId {
        FoundationId::new(value).expect("test foundation ID should be valid")
    }

    fn tier(
        id: &str,
        center_m: [f64; 3],
        length_x_m: f64,
        width_y_m: f64,
        thickness_m: f64,
    ) -> FoundationTier {
        FoundationTier {
            id: id.into(),
            center_m,
            length_x_m,
            width_y_m,
            thickness_m,
        }
    }

    fn stepped_layout(tiers: Vec<FoundationTier>) -> FoundationLayout {
        FoundationLayout {
            id: "layout-step-test".into(),
            elements: vec![FoundationPlacement::Stepped {
                id: foundation_id("4"),
                tiers,
            }],
        }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 1.0e-10,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn derives_all_placement_shapes_and_only_samples_contact_footprints() {
        let layout = FoundationLayout {
            id: "layout-shapes".into(),
            elements: vec![
                FoundationPlacement::Raft {
                    id: foundation_id("1"),
                    center_m: [0.0, 0.0, 0.0],
                    length_x_m: 4.0,
                    width_y_m: 3.0,
                    thickness_m: 0.5,
                },
                FoundationPlacement::Strip {
                    id: foundation_id("2"),
                    center_m: [10.0, 0.0, 1.0],
                    length_m: 5.0,
                    width_m: 1.0,
                    rotation_rad: FRAC_PI_2,
                    thickness_m: 0.4,
                },
                FoundationPlacement::Isolated {
                    id: foundation_id("3"),
                    center_m: [20.0, 0.0, 2.0],
                    length_x_m: 2.0,
                    width_y_m: 2.0,
                    thickness_m: 0.3,
                },
                FoundationPlacement::Stepped {
                    id: foundation_id("4"),
                    tiers: vec![
                        tier("bottom", [30.0, 0.0, 0.0], 6.0, 4.0, 0.5),
                        tier("upper", [30.0, 0.0, 0.5], 4.0, 2.0, 0.4),
                    ],
                },
            ],
        };

        let derived = derive_layout_geometry(&layout).expect("valid geometry should derive");
        assert_eq!(derived.elements.len(), 4);
        assert_eq!(derived.elements[0].kind, FoundationPlacementKind::Raft);
        assert_eq!(derived.elements[1].kind, FoundationPlacementKind::Strip);
        assert_eq!(derived.elements[2].kind, FoundationPlacementKind::Isolated);
        assert_eq!(derived.elements[3].kind, FoundationPlacementKind::Stepped);
        assert_eq!(derived.elements.iter().map(|item| item.sample_points.len()).sum::<usize>(), 20);

        assert_close(derived.elements[0].plan_area_m2, 12.0);
        assert_close(derived.elements[0].summed_volume_m3, 6.0);
        assert_eq!(derived.elements[0].sample_points[0].xy_m, [-2.0, -1.5]);
        assert_eq!(derived.elements[0].sample_points[4].xy_m, [0.0, 0.0]);

        assert_close(derived.elements[1].plan_area_m2, 5.0);
        assert_close(derived.elements[1].summed_volume_m3, 2.0);
        assert_close(derived.elements[1].sample_points[0].xy_m[0], 10.5);
        assert_close(derived.elements[1].sample_points[0].xy_m[1], -2.5);

        assert_close(derived.elements[2].plan_area_m2, 4.0);
        assert_close(derived.elements[2].summed_volume_m3, 1.2);
        assert_eq!(derived.elements[2].sample_points[0].xy_m, [19.0, -1.0]);

        let stepped = &derived.elements[3];
        assert_close(stepped.plan_area_m2, 24.0);
        assert_close(stepped.summed_volume_m3, 15.2);
        assert_eq!(stepped.tiers.len(), 2);
        assert_eq!(stepped.sample_points.len(), 5);
        assert_eq!(stepped.base_elevation_m, 0.0);
        assert!(stepped
            .sample_points
            .iter()
            .all(|point| point.tier_id.as_deref() == Some("bottom")));
        assert_eq!(stepped.sample_points[0].xy_m, [27.0, -2.0]);
        assert_eq!(stepped.sample_points[4].xy_m, [30.0, 0.0]);
        assert_close(derived.summed_element_plan_area_m2, 45.0);
        assert_close(derived.summed_element_volume_m3, 24.4);
    }

    #[test]
    fn rejects_noncanonical_ids_and_duplicate_layout_entity_ids() {
        assert!(FoundationId::new("0").is_err());
        assert!(FoundationId::new("01").is_err());
        assert!(FoundationId::new("-1").is_err());

        let malformed_id = r#"{"kind":"RAFT","id":"01","center_m":[0.0,0.0,0.0],"length_x_m":2.0,"width_y_m":2.0,"thickness_m":0.3}"#;
        assert!(serde_json::from_str::<FoundationPlacement>(malformed_id).is_err());

        let duplicate_placements = FoundationLayout {
            id: "layout-duplicates".into(),
            elements: vec![
                FoundationPlacement::Raft {
                    id: foundation_id("7"),
                    center_m: [0.0, 0.0, 0.0],
                    length_x_m: 2.0,
                    width_y_m: 2.0,
                    thickness_m: 0.3,
                },
                FoundationPlacement::Isolated {
                    id: foundation_id("7"),
                    center_m: [4.0, 0.0, 0.0],
                    length_x_m: 1.0,
                    width_y_m: 1.0,
                    thickness_m: 0.3,
                },
            ],
        };
        assert!(validate_layout(&duplicate_placements).is_err());

        let duplicate_tiers = stepped_layout(vec![
            tier("same-tier", [0.0, 0.0, 0.0], 4.0, 4.0, 0.5),
            tier("same-tier", [0.0, 0.0, 0.5], 2.0, 2.0, 0.4),
        ]);
        assert!(validate_layout(&duplicate_tiers).is_err());

        let cross_kind_duplicate = FoundationLayout {
            id: "layout-cross-kind-duplicate".into(),
            elements: vec![
                FoundationPlacement::Raft {
                    id: foundation_id("1"),
                    center_m: [0.0, 0.0, 0.0],
                    length_x_m: 2.0,
                    width_y_m: 2.0,
                    thickness_m: 0.3,
                },
                FoundationPlacement::Stepped {
                    id: foundation_id("2"),
                    tiers: vec![
                        tier("1", [5.0, 0.0, 0.0], 4.0, 4.0, 0.5),
                        tier("top", [5.0, 0.0, 0.5], 2.0, 2.0, 0.4),
                    ],
                },
            ],
        };
        assert!(validate_layout(&cross_kind_duplicate).is_err());

        let invalid_layout_id = FoundationLayout {
            id: "layout with spaces".into(),
            elements: vec![duplicate_placements.elements[0].clone()],
        };
        assert!(validate_layout(&invalid_layout_id).is_err());
    }

    #[test]
    fn rejects_invalid_stepped_stack_order_containment_and_dimensions() {
        let single_tier = stepped_layout(vec![tier(
            "bottom",
            [0.0, 0.0, 0.0],
            4.0,
            4.0,
            0.5,
        )]);
        assert!(validate_layout(&single_tier).is_err());

        let gap_between_tiers = stepped_layout(vec![
            tier("bottom", [0.0, 0.0, 0.0], 4.0, 4.0, 0.5),
            tier("upper", [0.0, 0.0, 0.6], 2.0, 2.0, 0.4),
        ]);
        assert!(validate_layout(&gap_between_tiers).is_err());

        let upper_tier_outside_base = stepped_layout(vec![
            tier("bottom", [0.0, 0.0, 0.0], 4.0, 4.0, 0.5),
            tier("upper", [2.0, 0.0, 0.5], 2.0, 2.0, 0.4),
        ]);
        assert!(validate_layout(&upper_tier_outside_base).is_err());

        let identical_tier_footprint = stepped_layout(vec![
            tier("bottom", [0.0, 0.0, 0.0], 4.0, 4.0, 0.5),
            tier("upper", [0.0, 0.0, 0.5], 4.0, 4.0, 0.4),
        ]);
        assert!(validate_layout(&identical_tier_footprint).is_err());

        let nonpositive_tier = stepped_layout(vec![
            tier("bottom", [0.0, 0.0, 0.0], 4.0, 4.0, 0.5),
            tier("upper", [0.0, 0.0, 0.5], 0.0, 2.0, 0.4),
        ]);
        assert!(validate_layout(&nonpositive_tier).is_err());
    }
}
