//! UI-facing adapters for the typed foundation/structural domain.
//!
//! The `Document` remains the sole authority. This module creates bounded
//! stage-one inputs, checks the UI's document binding, and projects the current
//! input and only fingerprint-matched results for drawing and reporting.

use ai_revit_core::{
    Command, Document, EngineeringSnapshotId, EngineeringSnapshotStatus, EngineeringSourceIdentity,
    FoundationLoadSource, FoundationStructuralCommand, FoundationStructuralInput,
    FoundationStructuralInputState, FoundationStructuralSnapshot,
};
use foundation_core::{PressureLoad, SupportField};
use sha2::{Digest, Sha256};
use std::env::{self, VarError};
use std::fs::File;
use std::io::Read;
use structural_core::{StructuralAnalysisResult, StructuralModel};

use super::foundation_structural_render::{
    MatDisplay, MatDisplaySource, PressureCell, SoilDisplayLayer,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FsBinding {
    pub document_uuid: String,
    pub runtime_id: String,
    pub open_generation: u64,
    pub revision: u64,
}

pub(super) fn binding(document: &Document, open_generation: u64) -> FsBinding {
    FsBinding {
        document_uuid: document.document_uuid.to_string(),
        runtime_id: document.runtime_instance_id(),
        open_generation,
        revision: document.revision(),
    }
}

pub(super) fn require_current(
    document: &Document,
    epoch: u64,
    expected: &FsBinding,
) -> Result<(), String> {
    if expected.open_generation != epoch {
        return Err("foundation/structural edit belongs to an earlier document generation".into());
    }
    if expected.document_uuid != document.document_uuid.to_string() {
        return Err("foundation/structural edit belongs to another document".into());
    }
    if expected.runtime_id != document.runtime_instance_id() {
        return Err("foundation/structural edit belongs to another document instance".into());
    }
    if expected.revision != document.revision() {
        return Err("foundation/structural edit is stale after a document revision".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FsEditValues {
    pub material_e_kn_m2: f64,
    pub material_g_kn_m2: f64,
    pub area_m2: f64,
    pub iy_m4: f64,
    pub iz_m4: f64,
    pub torsion_m4: f64,
    pub soil_e_kpa: f64,
    pub soil_poisson: f64,
    pub bedding_k_kn_m3: f64,
    pub mat_thickness_m: f64,
    pub manual_synthetic: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FsProjection {
    pub binding: FsBinding,
    pub input_hash: String,
    pub status: String,
    pub summary: String,
    pub report: String,
    pub model: StructuralModel,
    pub structural_result: Option<StructuralAnalysisResult>,
    pub mat: Option<MatDisplay>,
    pub editor: Option<FsEditValues>,
    pub current: bool,
}

pub(super) fn current_input(document: &Document) -> Option<&FoundationStructuralInputState> {
    document.foundation_structural().input()
}

pub(super) fn snapshots<'a>(
    document: &'a Document,
) -> impl DoubleEndedIterator<Item = (EngineeringSnapshotId, &'a FoundationStructuralSnapshot)>
+ ExactSizeIterator
+ 'a {
    document.foundation_structural().snapshots()
}

pub(super) fn snapshot_status(
    document: &Document,
    id: EngineeringSnapshotId,
) -> Option<EngineeringSnapshotStatus> {
    document.foundation_structural().snapshot_status(id)
}

/// Stage-one sample: an explicit braced frame, an unverified user-provided
/// combination, synthetic soil assumptions, and the 300 t reference load for
/// the optional ManualSynthetic mode. Structural reactions are the default.
pub(super) fn demo_input() -> Result<FoundationStructuralInput, String> {
    FoundationStructuralInput::stage1_demo().map_err(|error| error.to_string())
}

pub(super) fn replace_input_command(input: FoundationStructuralInput) -> Command {
    Command::FoundationStructural(FoundationStructuralCommand::ReplaceInput { input })
}

pub(super) fn save_snapshot_command(snapshot: FoundationStructuralSnapshot) -> Command {
    Command::FoundationStructural(FoundationStructuralCommand::SaveAnalysisSnapshot { snapshot })
}

/// The environment binding is supplied by the bounded source-freeze gate.
/// No fallback hash is invented when that receipt is absent.
pub(super) fn producer_from_environment() -> Result<EngineeringSourceIdentity, String> {
    let compiled_manifest =
        option_env!("AI_REVIT_FS_COMPILED_SOURCE_MANIFEST_SHA256").ok_or_else(|| {
            "AI_REVIT_FS_COMPILED_SOURCE_MANIFEST_SHA256 is not bound at compile time".to_owned()
        })?;
    let compiled_manifest = normalize_sha256(
        "AI_REVIT_FS_COMPILED_SOURCE_MANIFEST_SHA256",
        compiled_manifest,
    )?;
    let source_manifest_sha256 = required_sha256("AI_REVIT_FS_SOURCE_MANIFEST_SHA256")?;
    if source_manifest_sha256 != compiled_manifest {
        return Err(
            "runtime source manifest SHA-256 does not match the compiled source binding".into(),
        );
    }
    let actual_executable_sha256 = current_executable_sha256()?;
    let expected_executable_sha256 = optional_sha256("AI_REVIT_FS_EXECUTABLE_SHA256")?;
    if let Some(expected) = expected_executable_sha256.as_deref() {
        if actual_executable_sha256 != expected {
            return Err(
                "running executable SHA-256 does not match AI_REVIT_FS_EXECUTABLE_SHA256".into(),
            );
        }
    }
    Ok(EngineeringSourceIdentity {
        source_manifest_sha256,
        executable_sha256: Some(actual_executable_sha256),
        solver_version: format!("foundation-structural-stage1/{}", env!("CARGO_PKG_VERSION")),
        source_label: "Root-recorded Stage1 source identity; not engineering acceptance".into(),
    })
}

fn required_sha256(name: &str) -> Result<String, String> {
    match env::var(name) {
        Ok(value) => normalize_sha256(name, &value),
        Err(VarError::NotPresent) => Err(format!("{name} is required for an analysis receipt")),
        Err(VarError::NotUnicode(_)) => Err(format!("{name} must be a UTF-8 SHA-256 hex value")),
    }
}

fn optional_sha256(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => normalize_sha256(name, &value).map(Some),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(format!("{name} must be a UTF-8 SHA-256 hex value")),
    }
}

fn normalize_sha256(name: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "{name} must contain exactly 64 hexadecimal characters"
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn current_executable_sha256() -> Result<String, String> {
    let path = env::current_exe()
        .map_err(|error| format!("cannot identify running executable: {error}"))?;
    let mut file = File::open(&path)
        .map_err(|error| format!("cannot read running executable {}: {error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            format!("cannot hash running executable {}: {error}", path.display())
        })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(super) fn edit_values(input: &FoundationStructuralInput) -> Result<FsEditValues, String> {
    let material = input
        .structural_model
        .materials
        .first()
        .ok_or_else(|| "structural input has no material to edit".to_owned())?;
    let section = input
        .structural_model
        .sections
        .first()
        .ok_or_else(|| "structural input has no section to edit".to_owned())?;
    let foundation = input
        .foundation
        .as_ref()
        .ok_or_else(|| "foundation input is not present".to_owned())?;
    let layer = foundation
        .soil
        .layers
        .first()
        .ok_or_else(|| "foundation input has no soil layer to edit".to_owned())?;
    let bedding_k_kn_m3 = match &foundation.soil.support {
        SupportField::Uniform { k_kn_m3, .. } => *k_kn_m3,
        SupportField::PerCell { .. } => {
            return Err("per-cell bedding values need a dedicated editor".into());
        }
    };
    Ok(FsEditValues {
        material_e_kn_m2: material.elastic_modulus_kn_m2,
        material_g_kn_m2: material.shear_modulus_kn_m2,
        area_m2: section.area_m2,
        iy_m4: section.iy_m4,
        iz_m4: section.iz_m4,
        torsion_m4: section.torsional_constant_m4,
        soil_e_kpa: layer.young_modulus_kpa,
        soil_poisson: layer.poisson_ratio,
        bedding_k_kn_m3,
        mat_thickness_m: foundation.geometry.thickness_m,
        manual_synthetic: input.foundation_load_source == FoundationLoadSource::ManualSynthetic,
    })
}

pub(super) fn edit_input(
    input: &FoundationStructuralInput,
    values: &FsEditValues,
) -> Result<FoundationStructuralInput, String> {
    let mut edited = input.clone();
    let material = edited
        .structural_model
        .materials
        .first_mut()
        .ok_or_else(|| "structural input has no material to edit".to_owned())?;
    material.elastic_modulus_kn_m2 = values.material_e_kn_m2;
    material.shear_modulus_kn_m2 = values.material_g_kn_m2;

    let section = edited
        .structural_model
        .sections
        .first_mut()
        .ok_or_else(|| "structural input has no section to edit".to_owned())?;
    section.area_m2 = values.area_m2;
    section.iy_m4 = values.iy_m4;
    section.iz_m4 = values.iz_m4;
    section.torsional_constant_m4 = values.torsion_m4;

    let foundation = edited
        .foundation
        .as_mut()
        .ok_or_else(|| "foundation input is not present".to_owned())?;
    foundation.geometry.thickness_m = values.mat_thickness_m;
    let layer = foundation
        .soil
        .layers
        .first_mut()
        .ok_or_else(|| "foundation input has no soil layer to edit".to_owned())?;
    // E is illustrative metadata only; bedding stiffness remains independent.
    // Source type and site-verification flags stay owned by the domain input.
    layer.young_modulus_kpa = values.soil_e_kpa;
    layer.poisson_ratio = values.soil_poisson;
    match &mut foundation.soil.support {
        SupportField::Uniform { k_kn_m3, .. } => *k_kn_m3 = values.bedding_k_kn_m3,
        SupportField::PerCell { .. } => {
            return Err("per-cell bedding values need a dedicated editor".into());
        }
    }
    if values.manual_synthetic {
        if !foundation.load.components.iter().any(|component| {
            component.kind == foundation_core::LoadComponentKind::SyntheticReferenceMass
        }) {
            return Err(
                "manual synthetic mode requires an explicitly retained synthetic reference load"
                    .into(),
            );
        }
        if !foundation.load.original_structural_reactions.is_empty() {
            return Err("manual synthetic mode cannot retain structural reaction records".into());
        }
        edited.foundation_load_source = FoundationLoadSource::ManualSynthetic;
    } else {
        edited.foundation_load_source = FoundationLoadSource::StructuralReactions;
    }
    edited.validate().map_err(|error| error.to_string())?;
    Ok(edited)
}

/// Explicit Analyze action adapter. Core owns reaction transfer, solver calls,
/// and snapshot validation so the archived result is bound to the stored input.
pub(super) fn analyze_snapshot(
    input: &FoundationStructuralInputState,
    producer: EngineeringSourceIdentity,
) -> Result<FoundationStructuralSnapshot, String> {
    ai_revit_core::analyze_foundation_structural(input, producer).map_err(|error| error.to_string())
}

pub(super) fn projection(document: &Document, epoch: u64) -> Result<Option<FsProjection>, String> {
    let Some(input_state) = current_input(document) else {
        return Ok(None);
    };
    let definition = &input_state.definition;
    let editor = edit_values(definition).ok();
    let domain = document.foundation_structural();
    let snapshots: Vec<_> = domain.snapshots().collect();
    let latest_archive = snapshots.last().copied();
    let current_snapshot = snapshots
        .iter()
        .rev()
        .find(|(id, _)| domain.snapshot_status(*id) == Some(EngineeringSnapshotStatus::Current))
        .copied();
    let current = current_snapshot.is_some();
    let structural_result =
        current_snapshot.map(|(_, snapshot)| snapshot.structural_result.clone());

    let matched_effective_foundation =
        current_snapshot.and_then(|(_, snapshot)| snapshot.effective_foundation_input.as_ref());
    let mat_source = matched_effective_foundation.or(definition.foundation.as_ref());
    let mat_display_source = match (
        definition.foundation_load_source,
        matched_effective_foundation.is_some(),
    ) {
        (FoundationLoadSource::ManualSynthetic, _) => MatDisplaySource::ManualSynthetic,
        (FoundationLoadSource::StructuralReactions, true) => {
            MatDisplaySource::MatchedEffectiveFoundationInput
        }
        (FoundationLoadSource::StructuralReactions, false) => {
            MatDisplaySource::DeclaredInputWithoutCurrentReactionSnapshot
        }
    };
    let mat = mat_source.map(|foundation| {
        let pressure_cells = current_snapshot
            .and_then(|(_, snapshot)| snapshot.foundation_result.as_ref())
            .map(|result| {
                let half_dx = result.mesh.dx_m * 0.5;
                let half_dy = result.mesh.dy_m * 0.5;
                result
                    .cells
                    .iter()
                    .map(|cell| PressureCell {
                        vertices_xy_m: vec![
                            [cell.x_m - half_dx, cell.y_m - half_dy],
                            [cell.x_m + half_dx, cell.y_m - half_dy],
                            [cell.x_m + half_dx, cell.y_m + half_dy],
                            [cell.x_m - half_dx, cell.y_m + half_dy],
                        ],
                        pressure_kpa: cell.q_kpa,
                    })
                    .collect()
            })
            .unwrap_or_default();
        MatDisplay {
            source: mat_display_source,
            reference_point_m: definition.foundation_reference_point_m,
            length_x_m: foundation.geometry.width_x_m,
            width_y_m: foundation.geometry.length_y_m,
            thickness_m: foundation.geometry.thickness_m,
            soil_layers: foundation
                .soil
                .layers
                .iter()
                .map(|layer| SoilDisplayLayer {
                    depth_top_m: layer.depth_top_m,
                    thickness_m: layer.thickness_m,
                })
                .collect(),
            pressure_cells,
        }
    });

    let load_mode = match definition.foundation_load_source {
        FoundationLoadSource::StructuralReactions => "STRUCTURAL_REACTIONS",
        FoundationLoadSource::ManualSynthetic => "MANUAL_SYNTHETIC",
    };
    let model = definition.structural_model.clone();
    let summary = format!(
        "{} nodes · {} members · {} load cases · {} combinations · target {:?} · foundation load {}",
        model.nodes.len(),
        model.members.len(),
        model.load_cases.len(),
        model.combinations.len(),
        definition.analysis_target,
        load_mode,
    );

    let status = match current_snapshot {
        Some((_, snapshot)) if snapshot.foundation_result.is_some() => "PRELIMINARY_ESTIMATE",
        Some((_, snapshot))
            if snapshot.structural_result.diagnostics.status
                == structural_core::NumericalStatus::LinearSolutionResidualsVerified =>
        {
            "CALCULATED_AND_VERIFIED"
        }
        Some(_) => "NOT_IMPLEMENTED",
        None => "MODEL_INCOMPLETE",
    };
    let structural_summary = current_snapshot
        .map(|(_, snapshot)| {
            if snapshot.structural_result.diagnostics.status
                == structural_core::NumericalStatus::LinearSolutionResidualsVerified
            {
                "CALCULATED_AND_VERIFIED (numerical residual/balance scope only)"
            } else {
                "NOT_IMPLEMENTED"
            }
        })
        .unwrap_or(if latest_archive.is_some() {
            "STALE (archive is report-only)"
        } else {
            "INPUT_ONLY (not analyzed)"
        });
    let foundation_summary = current_snapshot
        .and_then(|(_, snapshot)| snapshot.foundation_result.as_ref())
        .map(|_| "PRELIMINARY_ESTIMATE (sampled spring/contact response only)")
        .unwrap_or(if definition.foundation.is_some() {
            if latest_archive.is_some() {
                "STALE (no current foundation response)"
            } else {
                "MODEL_INCOMPLETE (no current foundation response)"
            }
        } else {
            "NOT_IMPLEMENTED (no foundation input)"
        });
    let summary = format!(
        "{summary}\nModel status: {status}\nStructural: {structural_summary}\nFoundation: {foundation_summary}\nULS/SLS, strength, stability, bearing, settlement code checks and RC design: HOLD/NOT_IMPLEMENTED."
    );
    let report = build_report(
        input_state,
        load_mode,
        status,
        current_snapshot,
        latest_archive,
        domain,
    );

    Ok(Some(FsProjection {
        binding: binding(document, epoch),
        input_hash: input_state.input_fingerprint.clone(),
        status: status.into(),
        summary,
        report,
        model,
        structural_result,
        mat,
        editor,
        current,
    }))
}

fn build_report(
    input_state: &FoundationStructuralInputState,
    load_mode: &str,
    primary_status: &str,
    current_snapshot: Option<(EngineeringSnapshotId, &FoundationStructuralSnapshot)>,
    latest_archive: Option<(EngineeringSnapshotId, &FoundationStructuralSnapshot)>,
    domain: &ai_revit_core::FoundationStructuralDomain,
) -> String {
    let mut lines = vec![
        format!("Input fingerprint: {}", input_state.input_fingerprint),
        format!("Input source operation: {}", input_state.source_revision.operation_id),
        format!("Current foundation load source: {load_mode}"),
        "Engineering approval: NOT_IMPLEMENTED; ULS/SLS acceptance: HOLD".into(),
        "Scope holds: structural strength/code checks NOT_IMPLEMENTED; buckling/stability NOT_IMPLEMENTED; punching/shear and reinforcement NOT_IMPLEMENTED; ultimate soil bearing NOT_IMPLEMENTED; normative settlement/SLS NOT_IMPLEMENTED. No overall SAFE status is reported.".into(),
    ];
    if let Some(foundation) = &input_state.definition.foundation {
        let support = match &foundation.soil.support {
            SupportField::Uniform { k_kn_m3, gap_m } => {
                format!("uniform k={k_kn_m3:.3} kN/m³, gap={gap_m:.6} m")
            }
            SupportField::PerCell { k_kn_m3, .. } => {
                format!("per-cell support, {} cells", k_kn_m3.len())
            }
        };
        lines.push(format!(
            "Soil input: source {:?}; site verified {}; {support}. Layer E is illustrative metadata and is not converted into bedding k.",
            foundation.soil.source_type,
            foundation.soil.engineering_site_verified,
        ));
    }
    if let Some((id, snapshot)) = current_snapshot {
        lines.push(format!("Primary model status: {primary_status}"));
        lines.push(format!(
            "Current snapshot: {} ({})",
            id.get(),
            snapshot.content_fingerprint
        ));
        lines.push(format!(
            "Producer: {} / {}; source manifest SHA-256 {}; executable SHA-256 {}",
            snapshot.producer.source_label,
            snapshot.producer.solver_version,
            snapshot.producer.source_manifest_sha256,
            snapshot
                .producer
                .executable_sha256
                .as_deref()
                .unwrap_or("not recorded"),
        ));
        lines.push(format!(
            "Structural status: CALCULATED_AND_VERIFIED (numerical residual/balance scope only); numerical status {:?}; normative status {:?}; nodes {}; members {}; limitations: {}",
            snapshot.structural_result.diagnostics.status,
            snapshot.structural_result.normative_status,
            snapshot.structural_result.nodes.len(),
            snapshot.structural_result.members.len(),
            snapshot.structural_result.limitations.join("; "),
        ));
        match primary_status {
            "CALCULATED_AND_VERIFIED" => lines.push(
                "CALCULATED_AND_VERIFIED is limited to the structural solver's stated numerical residual scope; it is not engineering, code, ULS or SLS acceptance.".into(),
            ),
            "PRELIMINARY_ESTIMATE" => lines.push(
                "PRELIMINARY_ESTIMATE describes sampled spring/contact response only; bearing, reinforcement, normative settlement and ULS/SLS remain unverified or not implemented.".into(),
            ),
            _ => {}
        }
        if let Some(effective) = &snapshot.effective_foundation_input {
            lines.push(format!(
                "Effective foundation load: {}",
                foundation_load_details(load_mode, &effective.load),
            ));
        }
        if let Some(result) = &snapshot.foundation_result {
            let limits = &result.engineering_limits;
            lines.push(format!(
                "Foundation status: PRELIMINARY_ESTIMATE (sampled spring/contact response only); sampled contact {:?}, sampled pressure {:.3}..{:.3} kPa; site verified {}; bearing {:?}; settlement {:?}; reinforcement {:?}; groundwater {:?}",
                result.contact.state,
                result.contact.sampled_qmin_kpa,
                result.contact.sampled_qmax_kpa,
                limits.engineering_site_verified,
                limits.ultimate_bearing,
                limits.settlement,
                limits.reinforcement,
                limits.groundwater,
            ));
        } else {
            lines.push(
                "Foundation status: NOT_IMPLEMENTED (no foundation input in this snapshot).".into(),
            );
        }
    } else {
        let badge = if latest_archive.is_some() {
            "STALE"
        } else {
            "INPUT_ONLY"
        };
        lines.push(format!(
            "Primary model status: MODEL_INCOMPLETE; result badge: {badge}. No snapshot matches the current input fingerprint and source revision."
        ));
        match (load_mode, &input_state.definition.foundation) {
            ("MANUAL_SYNTHETIC", Some(foundation)) => lines.push(format!(
                "Declared manual foundation load: {}",
                foundation_load_details(load_mode, &foundation.load),
            )),
            ("STRUCTURAL_REACTIONS", _) => lines.push(
                "No current reaction-linked foundation input is available; declared raft load values are not used as frame reactions.".into(),
            ),
            _ => {}
        }
    }
    if let Some((id, snapshot)) = latest_archive {
        let freshness = domain
            .snapshot_status(id)
            .map(|status| format!("{status:?}"))
            .unwrap_or_else(|| "UNAVAILABLE".into());
        if freshness != "Current" {
            lines.push(format!(
                "Latest archived snapshot {} is {freshness}; retained for report history only, not used for current pressure/deformation rendering. Archived input fingerprint: {}. Archived producer: {} / {}. Archived load mode: {}.",
                id.get(),
                snapshot.input.input_fingerprint,
                snapshot.producer.source_label,
                snapshot.producer.solver_version,
                match snapshot.input.definition.foundation_load_source {
                    FoundationLoadSource::StructuralReactions => "STRUCTURAL_REACTIONS",
                    FoundationLoadSource::ManualSynthetic => "MANUAL_SYNTHETIC",
                },
            ));
            if let Some(effective) = &snapshot.effective_foundation_input {
                lines.push(format!(
                    "Archived effective foundation load: {}",
                    foundation_load_details(
                        match snapshot.input.definition.foundation_load_source {
                            FoundationLoadSource::StructuralReactions => "STRUCTURAL_REACTIONS",
                            FoundationLoadSource::ManualSynthetic => "MANUAL_SYNTHETIC",
                        },
                        &effective.load,
                    ),
                ));
            }
        }
    }
    lines.join("\n")
}

fn foundation_load_details(load_mode: &str, load: &PressureLoad) -> String {
    let components = load
        .components
        .iter()
        .map(|component| {
            format!(
                "{}:{:?} N={:.3} kN ({})",
                component.source_id, component.kind, component.n_kn, component.description
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    if load_mode == "MANUAL_SYNTHETIC" {
        format!(
            "manual synthetic input (not frame reactions); components [{}]",
            components
        )
    } else {
        format!(
            "{} original reactions retained; components [{}]",
            load.original_structural_reactions.len(),
            components
        )
    }
}
