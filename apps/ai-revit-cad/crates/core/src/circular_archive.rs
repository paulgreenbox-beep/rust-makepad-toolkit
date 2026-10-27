//! Bounded, source-bound persistence for circular calculation evidence.
//!
//! Archive integrity checks establish internal consistency only. They do not
//! prove solver execution, numerical correctness, or engineering approval.
use crate::{
    AnalysisCaseId, CircularCaseDefinition, CoreError, Document, EngineeringCommand,
    GroundSectionId, OperationId, OperationKind,
};
use geotech_core::circular::{
    self as circular, GroundDefinition, GroundSection2D, SearchAnalysisConfig, SearchMode,
    SearchPublicRequestV1,
};
use geotech_report::{Currentness, ReportArchive, ReportError, ReportEvidence, ReportLimits};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Write};

pub const CIRCULAR_SNAPSHOT_MARKER: &str = "CircularCalculationSnapshotV1";
pub const MAX_CIRCULAR_SNAPSHOT_BYTES: usize = 16 << 20;
const CIRCULAR_SNAPSHOT_SCHEMA_VERSION: u16 = 1;

#[derive(Serialize)]
struct CircularSnapshotWireRef<'a> {
    schema_version: u16,
    archive: &'a ReportArchive,
    source_case_definition: &'a CircularCaseDefinition,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CircularSnapshotWire {
    schema_version: u16,
    archive: Box<ReportArchive>,
    source_case_definition: CircularCaseDefinition,
}

/// A bounded report archive together with the complete source case definition
/// it reproduces. Fields stay crate-private so construction passes validation.
#[derive(Clone)]
pub struct CircularCalculationSnapshot {
    pub(crate) schema_version: u16,
    pub(crate) archive: Box<ReportArchive>,
    pub(crate) source_case_definition: CircularCaseDefinition,
}

/// Opaque, bounded inspection input. Only the snapshot and the result of the
/// owner's complete source-binding check cross the worker boundary. No live
/// Document, journal, BIM state or calculation capability is retained.
pub struct CircularSnapshotInspection {
    snapshot: CircularCalculationSnapshot,
    matches_live_sources: bool,
}

impl CircularSnapshotInspection {
    /// Performs report/hash validation on the inspection worker. A caller must
    /// still fence publication against its actual document/runtime/source key.
    pub fn inspect(self) -> Result<(Currentness, CircularCalculationSnapshot), CoreError> {
        self.snapshot.validate_integrity()?;
        let currentness = self.snapshot.currentness_after_integrity(self.matches_live_sources);
        Ok((currentness, self.snapshot))
    }
}

impl CircularCalculationSnapshot {
    /// Borrow the saved report archive without exposing mutable snapshot state.
    pub fn archive(&self) -> &ReportArchive {
        &self.archive
    }

    fn wire_ref(&self) -> CircularSnapshotWireRef<'_> {
        CircularSnapshotWireRef {
            schema_version: self.schema_version,
            archive: &self.archive,
            source_case_definition: &self.source_case_definition,
        }
    }

    /// Verify the typed archive and its saved case-level binding without a
    /// Document, search admission, numerical solve, or approval inference.
    pub(crate) fn validate_integrity(&self) -> Result<(), CoreError> {
        let wire = self.wire_ref();
        bounded_serialized_size(&wire, MAX_CIRCULAR_SNAPSHOT_BYTES)?;
        if self.schema_version != CIRCULAR_SNAPSHOT_SCHEMA_VERSION {
            return Err(invalid_snapshot("unsupported snapshot schema version"));
        }
        self.source_case_definition.validate_local()?;

        let limits = circular_report_limits();
        geotech_report::verify_archive(&self.archive, limits)
            .map_err(|error| map_report_error(error, limits))?;
        if !archive_matches_case_definition(&self.archive, &self.source_case_definition) {
            return Err(invalid_snapshot(
                "archive evidence does not match the captured case definition",
            ));
        }
        Ok(())
    }

    /// Validate current-document binding for an explicit archive save. The
    /// saved global revision may precede unrelated later edits.
    pub(crate) fn validate_for_current_document(
        &self,
        document: &Document,
        id: AnalysisCaseId,
    ) -> Result<(), CoreError> {
        self.validate_integrity()?;
        self.validate_current_document_after_integrity(document, id)
    }

    fn validate_current_document_after_integrity(
        &self,
        document: &Document,
        id: AnalysisCaseId,
    ) -> Result<(), CoreError> {
        let context = &self.archive.bundle.context;
        if context.project_id.0 != document.document_uuid.to_string()
            || context.case_id.0 != id.get()
            || context.ground_id.0 != self.source_case_definition.ground_section_id.get()
            || context.captured_global_revision > document.revision()
        {
            return Err(invalid_snapshot(
                "document, case, ground, or revision binding differs",
            ));
        }

        let case = document
            .circular_case(id)
            .ok_or_else(|| invalid_snapshot("current circular case is missing"))?;
        case.definition.validate_local()?;
        if case.definition != self.source_case_definition
            || case.source_operation_id.get() != context.case_source_operation.0
        {
            return Err(invalid_snapshot("current circular case source differs"));
        }
        let ground = document
            .ground_section(case.definition.ground_section_id)
            .ok_or_else(|| invalid_snapshot("current ground section is missing"))?;
        if ground.source_operation_id.get() != context.ground_source_operation.0 {
            return Err(invalid_snapshot("current ground source differs"));
        }

        // Verify the authoritative source operations before using either
        // resolver to compare the archive with live inputs.
        document.verify_circular_owner_source(case)?;
        document.verify_ground_owner(ground)?;
        self.validate_historical_after_integrity(document, id)?;
        for source_id in [case.source_operation_id, ground.source_operation_id] {
            if document
                .retained_operation(source_id)
                .is_none_or(|operation| operation.revision > context.captured_global_revision)
            {
                return Err(invalid_snapshot(
                    "captured revision predates a source operation",
                ));
            }
        }
        match &self.archive.bundle.evidence {
            ReportEvidence::Specified { input, config, .. } => {
                let resolved = document.resolve_circular_input_after_archive_validation(id)?;
                if resolved.ground_source_operation_id.get() != context.ground_source_operation.0
                    || resolved.case_source_operation_id.get() != context.case_source_operation.0
                    || &resolved.input != input
                    || &resolved.config != config
                {
                    return Err(invalid_snapshot(
                        "specified input is not bound to current sources",
                    ));
                }
            }
            ReportEvidence::Search { request, .. } => {
                let resolved = document.resolve_circular_search_after_archive_validation(id)?;
                if resolved.ground_source_operation_id.get() != context.ground_source_operation.0
                    || resolved.case_source_operation_id.get() != context.case_source_operation.0
                    || &resolved.request != request
                {
                    return Err(invalid_snapshot(
                        "search request is not bound to current sources",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Prove that the archive's captured source definitions still exist in
    /// retained owner operations. Current case and ground state are irrelevant.
    pub(crate) fn validate_historical(
        &self,
        document: &Document,
        id: AnalysisCaseId,
    ) -> Result<(), CoreError> {
        self.validate_integrity()?;
        self.validate_historical_after_integrity(document, id)
    }

    fn validate_historical_after_integrity(
        &self,
        document: &Document,
        id: AnalysisCaseId,
    ) -> Result<(), CoreError> {
        let context = &self.archive.bundle.context;
        if context.project_id.0 != document.document_uuid.to_string()
            || context.case_id.0 != id.get()
            || context.ground_id.0 != self.source_case_definition.ground_section_id.get()
            || context.captured_global_revision > document.revision()
        {
            return Err(invalid_snapshot(
                "historical document, owner, or revision binding differs",
            ));
        }

        let case_source_id = OperationId(context.case_source_operation.0);
        let case_source = document
            .retained_operation(case_source_id)
            .ok_or_else(|| invalid_snapshot("captured case source operation is not retained"))?;
        if case_source.id != case_source_id
            || case_source.revision > context.captured_global_revision
        {
            return Err(invalid_snapshot("captured case operation identity differs"));
        }
        let captured_case = match &case_source.kind {
            OperationKind::EngineeringChanged {
                analysis_id,
                command: EngineeringCommand::CreateCircular { definition, .. },
            } if *analysis_id == id => Some(definition),
            OperationKind::EngineeringChanged {
                analysis_id,
                command:
                    EngineeringCommand::UpdateCircular {
                        analysis_id: command_id,
                        definition,
                        ..
                    },
            } if *analysis_id == id && *command_id == id => Some(definition),
            _ => None,
        };
        if captured_case != Some(&self.source_case_definition) {
            return Err(invalid_snapshot(
                "retained case source does not match captured definition",
            ));
        }

        let ground_source_id = OperationId(context.ground_source_operation.0);
        let ground_source = document
            .retained_operation(ground_source_id)
            .ok_or_else(|| invalid_snapshot("captured ground source operation is not retained"))?;
        if ground_source.id != ground_source_id
            || ground_source.revision > context.captured_global_revision
        {
            return Err(invalid_snapshot(
                "captured ground operation identity differs",
            ));
        }
        let archived_ground = archive_ground(&self.archive.bundle.evidence);
        let ground_id = GroundSectionId::new(context.ground_id.0);
        let ground_matches = match &ground_source.kind {
            OperationKind::EngineeringGroundChanged {
                ground_section_id,
                command: EngineeringCommand::CreateGround { definition, .. },
            } if *ground_section_id == ground_id => ground_definition_matches_archive(
                definition,
                archived_ground,
                context.ground_id.0,
                context.ground_source_operation.0,
            ),
            OperationKind::EngineeringGroundChanged {
                ground_section_id,
                command:
                    EngineeringCommand::UpdateGround {
                        ground_section_id: command_id,
                        definition,
                        ..
                    },
            } if *ground_section_id == ground_id && *command_id == ground_id => {
                ground_definition_matches_archive(
                    definition,
                    archived_ground,
                    context.ground_id.0,
                    context.ground_source_operation.0,
                )
            }
            _ => false,
        };
        if !ground_matches {
            return Err(invalid_snapshot(
                "retained ground source does not match archived numerical ground",
            ));
        }
        Ok(())
    }

    fn currentness_after_integrity(&self, matches_live_sources: bool) -> Currentness {
        let context = &self.archive.bundle.context;
        if matches_live_sources
            && context.archived_solver_version == circular::CIRCULAR_SOLVER_VERSION
            && context.archived_solver_source_fingerprint == circular::source_fingerprint()
        {
            Currentness::Current
        } else {
            Currentness::Stale
        }
    }
}

impl Serialize for CircularCalculationSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_newtype_struct(CIRCULAR_SNAPSHOT_MARKER, &self.wire_ref())
    }
}

impl<'de> Deserialize<'de> for CircularCalculationSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Borrow the raw JSON representation so the size cap is checked before
        // typed ReportArchive collections or strings are allocated. This path
        // intentionally requires a borrowing JSON deserializer.
        let raw = <&'de RawValue>::deserialize(deserializer)?;
        if raw.get().as_bytes().len() > MAX_CIRCULAR_SNAPSHOT_BYTES {
            return Err(<D::Error as serde::de::Error>::custom(format!(
                "CircularArchiveLimit: snapshot exceeds {MAX_CIRCULAR_SNAPSHOT_BYTES} bytes"
            )));
        }
        let wire: CircularSnapshotWire =
            crate::persistence::decode_authority(raw.get())
                .map_err(<D::Error as serde::de::Error>::custom)?;
        let snapshot = Self {
            schema_version: wire.schema_version,
            archive: wire.archive,
            source_case_definition: wire.source_case_definition,
        };
        snapshot
            .validate_integrity()
            .map_err(<D::Error as serde::de::Error>::custom)?;
        Ok(snapshot)
    }
}

impl PartialEq for CircularCalculationSnapshot {
    fn eq(&self, other: &Self) -> bool {
        match (serialized_hash(self), serialized_hash(other)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        }
    }
}

struct BoundedArchiveDebug<'a>(&'a str);
impl fmt::Debug for BoundedArchiveDebug<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix: String = self.0.chars().take(96).collect();
        formatter
            .debug_tuple("DigestPrefix")
            .field(&prefix)
            .finish()
    }
}

impl fmt::Debug for CircularCalculationSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let context = &self.archive.bundle.context;
        formatter
            .debug_struct("CircularCalculationSnapshot")
            .field("schema_version", &self.schema_version)
            .field("project_id", &BoundedArchiveDebug(&context.project_id.0))
            .field("case_id", &context.case_id.0)
            .field("ground_id", &context.ground_id.0)
            .field(
                "captured_global_revision",
                &context.captured_global_revision,
            )
            .field(
                "ground_source_operation",
                &context.ground_source_operation.0,
            )
            .field("case_source_operation", &context.case_source_operation.0)
            .field(
                "archive_digest",
                &BoundedArchiveDebug(&self.archive.archive_digest),
            )
            .finish()
    }
}

/// Count serialized JSON bytes without building a JSON `String` or byte vector.
pub(crate) fn bounded_serialized_size<T: Serialize + ?Sized>(
    value: &T,
    cap: usize,
) -> Result<usize, CoreError> {
    bounded_serialized_size_with_scope(value, cap, "circular snapshot serialization")
}

fn bounded_serialized_size_with_scope<T: Serialize + ?Sized>(
    value: &T,
    cap: usize,
    scope: &'static str,
) -> Result<usize, CoreError> {
    let mut sink = CountWrite {
        bytes: 0,
        cap,
        exceeded: false,
    };
    match serde_json::to_writer(&mut sink, value) {
        Ok(()) => Ok(sink.bytes),
        Err(_error) if sink.exceeded => Err(CoreError::CircularArchiveLimit { scope, limit: cap }),
        Err(error) => Err(CoreError::Json(error)),
    }
}

struct CountWrite {
    bytes: usize,
    cap: usize,
    exceeded: bool,
}
impl Write for CountWrite {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.cap.saturating_sub(self.bytes) {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "bounded JSON size exceeded",
            ));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Sha256Write {
    hash: Sha256,
    bytes: usize,
    cap: usize,
    exceeded: bool,
}
impl Sha256Write {
    fn new(cap: usize) -> Self {
        Self {
            hash: Sha256::new(),
            bytes: 0,
            cap,
            exceeded: false,
        }
    }

    fn finish(self) -> [u8; 32] {
        self.hash.finalize().into()
    }
}
impl Write for Sha256Write {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.cap.saturating_sub(self.bytes) {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "bounded SHA-256 input exceeded",
            ));
        }
        self.hash.update(bytes);
        self.bytes += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialized_hash(snapshot: &CircularCalculationSnapshot) -> Option<(usize, [u8; 32])> {
    let mut sink = Sha256Write::new(MAX_CIRCULAR_SNAPSHOT_BYTES);
    if serde_json::to_writer(&mut sink, &snapshot.wire_ref()).is_err() || sink.exceeded {
        return None;
    }
    let bytes = sink.bytes;
    Some((bytes, sink.finish()))
}

fn circular_report_limits() -> ReportLimits {
    ReportLimits {
        input_bytes: MAX_CIRCULAR_SNAPSHOT_BYTES,
        output_bytes: MAX_CIRCULAR_SNAPSHOT_BYTES,
        archive_bytes: MAX_CIRCULAR_SNAPSHOT_BYTES,
        ..ReportLimits::default()
    }
}

fn map_report_error(error: ReportError, limits: ReportLimits) -> CoreError {
    match error {
        ReportError::InputBudget => CoreError::CircularArchiveLimit {
            scope: "report input",
            limit: limits.input_bytes,
        },
        ReportError::OutputBudget => CoreError::CircularArchiveLimit {
            scope: "report output",
            limit: limits.output_bytes,
        },
        ReportError::ArchiveBudget => CoreError::CircularArchiveLimit {
            scope: "report archive",
            limit: limits.archive_bytes,
        },
        other => invalid_snapshot(format!("report archive integrity failed: {other:?}")),
    }
}

fn invalid_snapshot(message: impl fmt::Display) -> CoreError {
    CoreError::InvalidDocument(format!(
        "Circular calculation snapshot is invalid: {message}"
    ))
}

fn archive_matches_case_definition(
    archive: &ReportArchive,
    definition: &CircularCaseDefinition,
) -> bool {
    let context = &archive.bundle.context;
    if context.ground_id.0 != definition.ground_section_id.get() {
        return false;
    }
    match &archive.bundle.evidence {
        ReportEvidence::Specified { input, config, .. } => {
            input.slip_surface == definition.slip_surface
                && input.basis == definition.basis
                && input.units == definition.units
                && input.extrusion_width_m == definition.extrusion_width_m
                && config == &definition.config
        }
        ReportEvidence::Search { request, .. } => search_request_matches_case(request, definition),
    }
}

fn search_request_matches_case(
    request: &SearchPublicRequestV1,
    definition: &CircularCaseDefinition,
) -> bool {
    let Some(spec) = definition.search_spec.as_ref() else {
        return false;
    };
    let config = &definition.config;
    request.schema_version == spec.schema_version
        && request.mode == SearchMode::SearchCriticalCircle
        && request.domain == spec.domain
        && request.family_policy == spec.family_policy
        && request.target_method == spec.target_method
        && request.exploration_slices == spec.exploration_slices
        && request.final_pair == spec.final_pair
        && request.refinement == spec.refinement
        && request.sensitivity == spec.sensitivity
        && request.core_policy_profile == spec.core_policy_profile
        && request.output_mode == spec.output_mode
        && request.max_candidates == spec.max_candidates
        && request.max_wall_time_ms == spec.max_wall_time_ms
        && request.near_tie_abs == spec.near_tie_abs
        && request.near_tie_rel == spec.near_tie_rel
        && request.shared_analysis
            == (SearchAnalysisConfig {
                tolerances: config.tolerances,
                limits: config.limits,
                bishop: config.bishop,
                contact: config.contact,
            })
        && request.basis == definition.basis
        && request.units == definition.units
        && request.direction == definition.slip_surface.slide_direction
        && request.extrusion_width_m == definition.extrusion_width_m
}

fn archive_ground(evidence: &ReportEvidence) -> &GroundSection2D {
    match evidence {
        ReportEvidence::Specified { input, .. } => &input.ground,
        ReportEvidence::Search { request, .. } => &request.ground,
    }
}

fn ground_definition_matches_archive(
    definition: &GroundDefinition,
    archived: &GroundSection2D,
    expected_ground_id: u64,
    expected_source_operation: u64,
) -> bool {
    archived.section_id.0 == expected_ground_id.to_string()
        && archived.model_revision == expected_source_operation
        && definition.coordinate_system == archived.coordinate_system
        && definition.terrain_polyline == archived.terrain_polyline
        && definition.layer_interfaces == archived.layer_interfaces
        && definition.soil_regions == archived.soil_regions
        && definition.bottom_boundary == archived.bottom_boundary
        && definition.materials == archived.materials
}

impl Document {
    /// Capture once on an explicit archive-load/source-change action. Admission
    /// streams bytes before cloning the snapshot and does not verify/format a
    /// report. The same authoritative source check as currentness is retained.
    /// The caller may choose a smaller copy cap, never widen the snapshot cap.
    pub fn capture_circular_snapshot_inspection(
        &self,
        id: AnalysisCaseId,
        copy_bytes: usize,
    ) -> Result<Option<CircularSnapshotInspection>, CoreError> {
        let case = self.circular_case(id).ok_or_else(|| {
            CoreError::InvalidDocument(format!("Unknown circular analysis {id}"))
        })?;
        let Some(snapshot) = case.circular_snapshot.as_ref() else {
            return Ok(None);
        };
        bounded_serialized_size(snapshot, copy_bytes.min(MAX_CIRCULAR_SNAPSHOT_BYTES))?;
        let matches_live_sources = snapshot.validate_current_document_after_integrity(self, id).is_ok();
        Ok(Some(CircularSnapshotInspection {
            snapshot: snapshot.clone(),
            matches_live_sources,
        }))
    }

    /// Validate and prepare a report snapshot without mutating the document.
    /// Internal integrity and source binding are not producer authorization.
    /// App/API callers must require their own current, owned calculation job.
    pub fn prepare_circular_snapshot(
        &self,
        id: AnalysisCaseId,
        archive: ReportArchive,
    ) -> Result<CircularCalculationSnapshot, CoreError> {
        bounded_serialized_size_with_scope(
            &archive,
            MAX_CIRCULAR_SNAPSHOT_BYTES,
            "report archive",
        )?;
        let case = self
            .circular_case(id)
            .ok_or_else(|| invalid_snapshot(format!("unknown circular analysis {id}")))?;
        case.definition.validate_local()?;
        self.verify_circular_owner(case)?;
        let ground = self
            .ground_section(case.definition.ground_section_id)
            .ok_or_else(|| invalid_snapshot("current ground section is missing"))?;
        self.verify_ground_owner(ground)?;

        let snapshot = CircularCalculationSnapshot {
            schema_version: CIRCULAR_SNAPSHOT_SCHEMA_VERSION,
            archive: Box::new(archive),
            source_case_definition: case.definition.clone(),
        };
        snapshot.validate_for_current_document(self, id)?;
        Ok(snapshot)
    }

    /// Report whether a stored archive still matches the current live sources.
    /// This checks provenance only and never performs a search or calculation.
    pub fn circular_snapshot_currentness(
        &self,
        id: AnalysisCaseId,
    ) -> Result<Option<Currentness>, CoreError> {
        let case = self
            .circular_case(id)
            .ok_or_else(|| CoreError::InvalidDocument(format!("Unknown circular analysis {id}")))?;
        let Some(snapshot) = case.circular_snapshot.as_ref() else {
            return Ok(None);
        };
        snapshot.validate_integrity()?;
        Ok(Some(snapshot.currentness_after_integrity(
            snapshot.validate_current_document_after_integrity(self, id).is_ok(),
        )))
    }
}
