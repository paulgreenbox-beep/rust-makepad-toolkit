//! Focused regressions for durable, source-bound circular report archives.
//! Explicit test setup generates archives from synthetic inputs once per process.
//! Their source identity is the actual compiled solver. Only that setup runs the
//! solver; archive open/inspection/persistence never do. Numerical goldens remain
//! separate. The old-identity control is synthetic, not historical solver evidence.
use crate::*;
use geotech_report::{Currentness, ReportArchive, ReportEvidence, ReportLimits};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    crate::test_fixtures::ensure_circular_archive_fixtures().join(name)
}

fn load_document(name: &str) -> Document {
    Document::open(fixture(name)).unwrap_or_else(|error| panic!("open {name}: {error}"))
}

fn load_archive(name: &str) -> ReportArchive {
    serde_json::from_reader(File::open(fixture(name)).unwrap())
        .unwrap_or_else(|error| panic!("read typed report archive {name}: {error}"))
}

#[test]
fn synthetic_historical_identity_stays_stale_without_rewriting_saved_evidence() {
    let document = load_document("historical.specified.native4.json");
    let id = sole_circular_case(&document);
    let saved = document.circular_case(id).unwrap().circular_snapshot.as_ref().unwrap().archive();
    assert_ne!(saved.bundle.context.archived_solver_source_fingerprint,
        geotech_core::circular::source_fingerprint(), "synthetic historical identity control");
    let saved_bytes = serde_json::to_vec(saved).unwrap();
    let before = document.to_json_string().unwrap();
    assert_eq!(document.circular_snapshot_currentness(id).unwrap(), Some(Currentness::Stale));
    let (currentness, captured) = document.capture_circular_snapshot_inspection(id, 8 << 20)
        .unwrap().unwrap().inspect().unwrap();
    assert_eq!(currentness, Currentness::Stale);
    assert_eq!(serde_json::to_vec(captured.archive()).unwrap(), saved_bytes);
    assert_eq!(document.to_json_string().unwrap(), before);
}

#[test]
fn circular_archive_owned_inspection_matches_authority_and_preserves_saved_bytes() {
    for name in ["specified.native4.json", "search.native4.json"] {
        let document = load_document(name);
        let id = sole_circular_case(&document);
        let before = document.to_json_string().unwrap();
        let expected = document.circular_snapshot_currentness(id).unwrap().unwrap();
        let captured = document.capture_circular_snapshot_inspection(id, 8 << 20).unwrap().unwrap();
        // Consuming the task has no access to Document or to a solver.
        let (currentness, snapshot) = captured.inspect().unwrap();
        assert_eq!(currentness, expected);
        assert_eq!(serde_json::to_vec(snapshot.archive()).unwrap(), serde_json::to_vec(
            document.circular_case(id).unwrap().circular_snapshot.as_ref().unwrap().archive()
        ).unwrap());
        assert_eq!(document.to_json_string().unwrap(), before);
    }
}

#[test]
fn circular_archive_owned_inspection_distinguishes_absent_unknown_and_copy_limit() {
    let document = load_document("specified.unsaved.native4.json");
    let id = sole_circular_case(&document);
    assert!(document.capture_circular_snapshot_inspection(id, 8 << 20).unwrap().is_none());
    assert!(document.capture_circular_snapshot_inspection(AnalysisCaseId::new(u64::MAX), 8 << 20).is_err());
    let document = load_document("specified.native4.json");
    let id = sole_circular_case(&document);
    assert!(matches!(document.capture_circular_snapshot_inspection(id, 1),
        Err(CoreError::CircularArchiveLimit { .. })));
}

#[test]
fn circular_archive_capture_defers_report_integrity_and_worker_rejects_corruption() {
    for variant in 0..3 {
        let mut document = load_document("specified.native4.json");
        let id = sole_circular_case(&document);
        let snapshot = document.engineering.circular_cases.get_mut(&id).unwrap()
            .circular_snapshot.as_mut().unwrap();
        match variant {
            0 => snapshot.archive.archive_digest.push('0'),
            1 => snapshot.archive.format = "unsupported-archive-format".into(),
            _ => {
                let ReportEvidence::Specified { result, .. } = &mut snapshot.archive.bundle.evidence else { panic!("specified fixture"); };
                result.total_area_m2 = f64::NAN;
            }
        }
        // The owner capture must not hash/format the report, even with a bad
        // report digest. The off-owner integrity boundary rejects all variants.
        assert!(document.resolve_circular_editor_input(id).is_ok());
        assert!(document.resolve_circular_input(id).is_err());
        let captured = document.capture_circular_snapshot_inspection(id, 8 << 20).unwrap().unwrap();
        assert!(captured.inspect().is_err());
        assert!(document.circular_snapshot_currentness(id).is_err());
    }
}

#[test]
fn circular_archive_editor_search_defers_integrity_without_bypassing_source_owners() {
    let mut document = load_document("search.native4.json");
    let id = sole_circular_case(&document);
    let expected = document.resolve_circular_search(id).unwrap().request;
    document.engineering.circular_cases.get_mut(&id).unwrap().circular_snapshot
        .as_mut().unwrap().archive.archive_digest.push('0');
    assert_eq!(document.resolve_circular_editor_search(id).unwrap().request, expected);
    assert!(document.resolve_circular_search(id).is_err());
    assert!(document.capture_circular_snapshot_inspection(id, 8 << 20).unwrap().unwrap().inspect().is_err());
    document.engineering.circular_cases.get_mut(&id).unwrap().source_operation_id = OperationId(u64::MAX);
    assert!(document.resolve_circular_editor_input(id).is_err());
    assert!(document.resolve_circular_editor_search(id).is_err());
}

#[test]
fn circular_archive_owned_inspection_uses_complete_source_context_and_keeps_stale_evidence() {
    let mut document = load_document("specified.native4.json");
    let id = sole_circular_case(&document);
    let saved = serde_json::to_vec(document.circular_case(id).unwrap().circular_snapshot.as_ref().unwrap().archive()).unwrap();
    let mut definition = document.circular_case(id).unwrap().definition.clone();
    definition.config.bishop.initial_factor += 0.125;
    commit_one(&mut document, "inspection-source-change", Command::Engineering(
        EngineeringCommand::UpdateCircular { analysis_id: id, definition, name: None }
    ));
    let (currentness, snapshot) = document.capture_circular_snapshot_inspection(id, 8 << 20)
        .unwrap().unwrap().inspect().unwrap();
    assert_eq!(currentness, Currentness::Stale);
    assert_eq!(document.circular_snapshot_currentness(id).unwrap(), Some(Currentness::Stale));
    assert_eq!(serde_json::to_vec(snapshot.archive()).unwrap(), saved);
}

fn sole_circular_case(document: &Document) -> AnalysisCaseId {
    assert_eq!(document.circular_cases().len(), 1);
    *document.circular_cases().keys().next().unwrap()
}

fn assert_archive_is_bound_to(document: &Document, id: AnalysisCaseId, archive: &ReportArchive) {
    let case = document.circular_case(id).expect("fixture circular case");
    let ground = document
        .ground_section(case.definition.ground_section_id)
        .expect("fixture ground section");
    let context = &archive.bundle.context;

    assert_eq!(context.project_id.0, document.document_uuid.to_string());
    assert_eq!(context.case_id.0, id.get());
    assert_eq!(context.ground_id.0, case.definition.ground_section_id.get());
    assert_eq!(
        context.ground_source_operation.0,
        ground.source_operation_id.get()
    );
    assert_eq!(
        context.case_source_operation.0,
        case.source_operation_id.get()
    );
    assert!(
        document
            .operation(case.source_operation_id)
            .expect("fixture case source operation")
            .revision
            <= context.captured_global_revision
    );
    assert!(
        document
            .operation(ground.source_operation_id)
            .expect("fixture ground source operation")
            .revision
            <= context.captured_global_revision
    );
    assert!(context.captured_global_revision <= document.revision());
    assert_eq!(
        context.archived_solver_version,
        geotech_core::circular::CIRCULAR_SOLVER_VERSION
    );
    assert_eq!(
        context.archived_solver_source_fingerprint,
        geotech_core::circular::source_fingerprint()
    );

    match &archive.bundle.evidence {
        ReportEvidence::Specified { input, config, .. } => {
            let resolved = document
                .resolve_circular_input(id)
                .expect("resolve specified live inputs");
            assert_eq!(input, &resolved.input);
            assert_eq!(config, &resolved.config);
            assert_eq!(
                context.ground_source_operation.0,
                resolved.ground_source_operation_id.get()
            );
            assert_eq!(
                context.case_source_operation.0,
                resolved.case_source_operation_id.get()
            );
        }
        ReportEvidence::Search { request, .. } => {
            let resolved = document
                .resolve_circular_search(id)
                .expect("resolve complete search request");
            assert_eq!(request, &resolved.request);
            assert_eq!(
                context.ground_source_operation.0,
                resolved.ground_source_operation_id.get()
            );
            assert_eq!(
                context.case_source_operation.0,
                resolved.case_source_operation_id.get()
            );
        }
    }
}

fn commit_one(document: &mut Document, command_id: &str, command: Command) -> CommitReceipt {
    let mut transaction = document.begin_transaction(command_id, document.revision());
    transaction.push(command);
    document.commit(transaction).unwrap()
}

fn without_archive_feature(native: &str) -> String {
    let key = native
        .find("\"required_features\"")
        .expect("native feature envelope");
    let array_start = key + native[key..].find('[').expect("feature array");
    let array_end = array_start + native[array_start..].find(']').expect("feature array end");
    let values: Vec<&str> = native[array_start + 1..array_end]
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "\"geotech_circular_archive_v1\"")
        .collect();
    let retained = values.join(",");
    assert_ne!(native[array_start + 1..array_end].trim(), retained);
    format!(
        "{}[{}]{}",
        &native[..array_start],
        retained,
        &native[array_end + 1..]
    )
}

fn archive_signature(
    snapshot: &CircularCalculationSnapshot,
) -> (String, String, String, String, String) {
    let archive = &snapshot.archive;
    (
        archive.bundle_digest.clone(),
        archive.report_digest.clone(),
        archive.archive_digest.clone(),
        archive.bundle.context.archived_solver_version.clone(),
        archive
            .bundle
            .context
            .archived_solver_source_fingerprint
            .clone(),
    )
}

/// Repackage a genuine numerical result after changing only its archived
/// solver/source identity metadata. This is a synthetic integrity probe, not
/// a historical solver report or provenance claim.
fn synthetic_identity_probe(archive: ReportArchive) -> ReportArchive {
    const PROBE_VERSION: &str = "synthetic-integrity-probe-version";
    const PROBE_SOURCE: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000001";
    synthetic_identity_with(archive, PROBE_VERSION, PROBE_SOURCE)
}

fn synthetic_identity_with(mut archive: ReportArchive, probe_version: &str, probe_source: &str) -> ReportArchive {
    archive.bundle.context.archived_solver_version = probe_version.into();
    archive.bundle.context.archived_solver_source_fingerprint = probe_source.into();
    let before_result = match &mut archive.bundle.evidence {
        ReportEvidence::Specified {
            input,
            config,
            result,
        } => {
            let before = result.clone();
            result.solver_version = probe_version.into();
            result.source_fingerprint = probe_source.into();
            result.input_fingerprint = geotech_report::archived_input_digest(
                probe_version,
                input,
                config,
                ReportLimits::default(),
            )
            .expect("pure archived input digest");
            if let Some(contact) = result.bishop_result.whole_arc_contact.as_mut() {
                contact.source_fingerprint = probe_source.into();
                let contact_bytes = serde_json::to_vec(&(
                    "whole-arc-contact-envelope-v1",
                    input,
                    config,
                    contact.factor_lower,
                    contact.factor_upper,
                    &contact.root_range_scope,
                    probe_source,
                ))
                .expect("serialize contact identity tuple");
                contact.contact_input_fingerprint =
                    format!("sha256:{:x}", Sha256::digest(&contact_bytes));
            }

            // Restore only the identity/digest labels below, then compare the
            // complete typed result. This proves all saved numeric and other
            // result fields stayed byte-for-byte-equivalent at the typed level.
            before
        }
        ReportEvidence::Search { .. } => panic!("specified probe archive changed mode"),
    };
    if let ReportEvidence::Specified { result, .. } = &archive.bundle.evidence {
        let mut normalized = result.clone();
        normalized.solver_version = before_result.solver_version.clone();
        normalized.source_fingerprint = before_result.source_fingerprint.clone();
        normalized.input_fingerprint = before_result.input_fingerprint.clone();
        if let (Some(normalized_contact), Some(before_contact)) = (
            normalized.bishop_result.whole_arc_contact.as_mut(),
            before_result.bishop_result.whole_arc_contact.as_ref(),
        ) {
            normalized_contact.source_fingerprint = before_contact.source_fingerprint.clone();
            normalized_contact.contact_input_fingerprint =
                before_contact.contact_input_fingerprint.clone();
        }
        assert_eq!(normalized, before_result);
    }

    geotech_report::create_archive(archive.bundle, ReportLimits::default())
        .expect("pure formatter recomputes integrity for synthetic identity probe")
}

#[test]
fn circular_archive_owned_inspection_positive_currentness_is_provenance_only() {
    let mut document = load_document("specified.unsaved.native4.json");
    let id = sole_circular_case(&document);
    // Synthetic identity-only repack of stored numbers: this tests provenance
    // state routing, never numerical equivalence of two solver versions.
    let archive = synthetic_identity_with(load_archive("specified.archive.json"),
        geotech_core::circular::CIRCULAR_SOLVER_VERSION,
        &geotech_core::circular::source_fingerprint());
    let snapshot = document.prepare_circular_snapshot(id, archive).unwrap();
    commit_one(&mut document, "inspection-synthetic-identity-save", Command::Engineering(
        EngineeringCommand::SaveCircularSnapshot { analysis_id: id, snapshot }
    ));
    let captured = document.capture_circular_snapshot_inspection(id, 8 << 20).unwrap().unwrap();
    let (currentness, snapshot) = captured.inspect().unwrap();
    assert_eq!(currentness, Currentness::Current);
    assert_eq!(document.circular_snapshot_currentness(id).unwrap(), Some(Currentness::Current));
    let ReportEvidence::Specified { result, .. } = &snapshot.archive().bundle.evidence else { panic!("specified fixture"); };
    assert!(!result.numerically_verified);
    assert!(result.engineering_review_required);
}

#[test]
fn genuine_specified_and_search_archives_bind_complete_live_inputs_and_both_sources() {
    for (document_name, archive_name) in [
        ("specified.unsaved.native4.json", "specified.archive.json"),
        ("search.unsaved.native4.json", "search.archive.json"),
    ] {
        let document = load_document(document_name);
        let id = sole_circular_case(&document);
        let archive = load_archive(archive_name);
        assert_archive_is_bound_to(&document, id, &archive);

        let revision = document.revision();
        let case_source = document.circular_case(id).unwrap().source_operation_id;
        let ground_source = document
            .ground_section(
                document
                    .circular_case(id)
                    .unwrap()
                    .definition
                    .ground_section_id,
            )
            .unwrap()
            .source_operation_id;
        let snapshot = document
            .prepare_circular_snapshot(id, archive)
            .expect("factory accepts exact producer archive");
        assert_eq!(
            snapshot.source_case_definition,
            document.circular_case(id).unwrap().definition
        );
        snapshot
            .validate_for_current_document(&document, id)
            .expect("snapshot remains bound to both current owners");
        assert_eq!(document.revision(), revision);
        assert_eq!(
            document.circular_case(id).unwrap().source_operation_id,
            case_source
        );
        assert_eq!(
            document
                .ground_section(snapshot.source_case_definition.ground_section_id)
                .unwrap()
                .source_operation_id,
            ground_source
        );
        assert_eq!(document.circular_snapshot_currentness(id).unwrap(), None);
        let native = document.to_json_string().unwrap();
        assert!(
            !native.contains("\"geotech_circular_archive_v1\""),
            "an unsaved case has no retained archive feature"
        );
    }

    for document_name in ["specified.native4.json", "search.native4.json"] {
        let document = load_document(document_name);
        let id = sole_circular_case(&document);
        let snapshot = document
            .circular_case(id)
            .unwrap()
            .circular_snapshot
            .as_ref()
            .expect("producer saved report archive");
        assert_archive_is_bound_to(&document, id, &snapshot.archive);
        assert_eq!(
            document.circular_snapshot_currentness(id).unwrap(),
            Some(Currentness::Current)
        );
    }

    // Both reports are individually valid producer outputs. Cross-binding one
    // to the other document must fail rather than importing its host identity.
    let specified = load_document("specified.unsaved.native4.json");
    let specified_id = sole_circular_case(&specified);
    assert!(
        specified
            .prepare_circular_snapshot(specified_id, load_archive("search.archive.json"))
            .is_err()
    );
}

#[test]
fn captured_revision_must_cover_source_operations_but_allows_later_unrelated_edits() {
    let mut document = load_document("specified.unsaved.native4.json");
    let id = sole_circular_case(&document);
    let archive = load_archive("specified.archive.json");
    let case_source = document.circular_case(id).unwrap().source_operation_id;
    let case_source_revision = document.operation(case_source).unwrap().revision;
    assert!(archive.bundle.context.captured_global_revision >= case_source_revision);

    // This synthetic provenance metadata probe changes no numerical fields;
    // the pure report formatter recomputes the archive's integrity tuple.
    let mut too_early = archive.bundle.clone();
    too_early.context.captured_global_revision = case_source_revision.saturating_sub(1);
    let too_early = geotech_report::create_archive(too_early, ReportLimits::default()).unwrap();
    assert!(document.prepare_circular_snapshot(id, too_early).is_err());

    let captured_revision = archive.bundle.context.captured_global_revision;
    let mut unrelated =
        document.begin_transaction("unrelated-revision-before-save", document.revision());
    unrelated.push(Command::CreateElement {
        name: "Unrelated revision wall".into(),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(3.0, 0.0),
            base_elevation_m: 0.0,
            height_m: 2.0,
            thickness_m: 0.2,
        }),
        properties: BTreeMap::new(),
    });
    document.commit(unrelated).unwrap();
    assert!(document.revision() > captured_revision);

    let snapshot = document.prepare_circular_snapshot(id, archive).unwrap();
    commit_one(
        &mut document,
        "save-before-unrelated-currentness-check",
        Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot,
        }),
    );
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
}

#[test]
fn synthetic_solver_identity_probe_preserves_numbers_and_round_trips_as_stale() {
    let mut document = load_document("specified.unsaved.native4.json");
    let id = sole_circular_case(&document);
    let probe_archive = synthetic_identity_probe(load_archive("specified.archive.json"));
    let snapshot = document
        .prepare_circular_snapshot(id, probe_archive)
        .unwrap();
    let expected_version = "synthetic-integrity-probe-version";
    let expected_source = "sha256:0000000000000000000000000000000000000000000000000000000000000001";
    assert_eq!(
        snapshot.archive().bundle.context.archived_solver_version,
        expected_version
    );
    assert_eq!(
        snapshot
            .archive()
            .bundle
            .context
            .archived_solver_source_fingerprint,
        expected_source
    );

    commit_one(
        &mut document,
        "save-synthetic-identity-integrity-probe",
        Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot,
        }),
    );
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Stale)
    );
    let native = document.to_json_string().unwrap();
    let reopened = Document::from_json_str(&native).unwrap();
    let reopened_snapshot = reopened
        .circular_case(id)
        .unwrap()
        .circular_snapshot
        .as_ref()
        .unwrap();
    assert_eq!(
        reopened_snapshot
            .archive()
            .bundle
            .context
            .archived_solver_version,
        expected_version
    );
    assert_eq!(
        reopened_snapshot
            .archive()
            .bundle
            .context
            .archived_solver_source_fingerprint,
        expected_source
    );
    assert_eq!(
        reopened.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Stale)
    );
}

#[test]
fn retention_audit_charges_duplicate_owners_and_quota_rejection_is_atomic() {
    let mut document = load_document("specified.unsaved.native4.json");
    let id = sole_circular_case(&document);
    let snapshot = document
        .prepare_circular_snapshot(id, load_archive("specified.archive.json"))
        .unwrap();
    let one_snapshot_bytes = crate::circular_archive::bounded_serialized_size(
        &snapshot,
        crate::circular_archive::MAX_CIRCULAR_SNAPSHOT_BYTES,
    )
    .unwrap();
    commit_one(
        &mut document,
        "save-for-retention-audit",
        Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot,
        }),
    );

    let live_usage = document.circular_archive_retention_usage().unwrap();
    let audited_usage =
        crate::circular_archive_budget::audit_circular_archive_retention(&document).unwrap();
    assert_eq!(live_usage, audited_usage);
    assert!(live_usage.snapshot_occurrences > 1);
    assert_eq!(
        live_usage.snapshot_bytes,
        one_snapshot_bytes * live_usage.snapshot_occurrences
    );
    assert_eq!(live_usage.max_snapshot_bytes, one_snapshot_bytes);

    let mut quota_document = load_document("specified.unsaved.native4.json");
    let quota_id = sole_circular_case(&quota_document);
    let repeated_snapshot = quota_document
        .prepare_circular_snapshot(quota_id, load_archive("specified.archive.json"))
        .unwrap();
    let per_copy = crate::circular_archive::bounded_serialized_size(
        &repeated_snapshot,
        crate::circular_archive::MAX_CIRCULAR_SNAPSHOT_BYTES,
    )
    .unwrap();
    let copies = (MAX_CIRCULAR_RETAINED_BYTES / 9 / per_copy + 1).max(2);
    let mut commands = Vec::with_capacity(copies);
    for _ in 1..copies {
        commands.push(Command::Engineering(
            EngineeringCommand::SaveCircularSnapshot {
                analysis_id: quota_id,
                snapshot: repeated_snapshot.clone(),
            },
        ));
    }
    commands.push(Command::Engineering(
        EngineeringCommand::SaveCircularSnapshot {
            analysis_id: quota_id,
            snapshot: repeated_snapshot,
        },
    ));
    let incoming_usage =
        crate::circular_archive_budget::audit_circular_archive_retention(&commands).unwrap();
    assert_eq!(incoming_usage.snapshot_occurrences, copies);
    assert_eq!(incoming_usage.snapshot_bytes, per_copy * copies);
    assert!(incoming_usage.snapshot_bytes * 9 > MAX_CIRCULAR_RETAINED_BYTES);
    assert!(
        crate::circular_archive::bounded_serialized_size(&commands, MAX_CIRCULAR_RETAINED_BYTES)
            .is_ok()
    );

    let before = quota_document.to_json_string().unwrap();
    let revision_before = quota_document.revision();
    let mut transaction =
        quota_document.begin_transaction("reject-over-quota-identical-archives", revision_before);
    for command in commands {
        transaction.push(command);
    }
    let error = quota_document.commit(transaction).unwrap_err();
    assert!(matches!(error, CoreError::CircularArchiveLimit { .. }));
    assert_eq!(quota_document.revision(), revision_before);
    assert_eq!(quota_document.to_json_string().unwrap(), before);
    assert!(
        quota_document
            .circular_case(quota_id)
            .unwrap()
            .circular_snapshot
            .is_none()
    );
}

#[test]
fn explicit_save_is_snapshot_only_and_survives_receipt_undo_redo_replay_and_open() {
    let mut document = load_document("specified.unsaved.native4.json");
    let id = sole_circular_case(&document);
    let archive = load_archive("specified.archive.json");
    assert_archive_is_bound_to(&document, id, &archive);

    let before_revision = document.revision();
    let before_case = document.circular_case(id).unwrap().clone();
    let before_ground_source = document
        .ground_section(before_case.definition.ground_section_id)
        .unwrap()
        .source_operation_id;
    let snapshot = document.prepare_circular_snapshot(id, archive).unwrap();
    let signature = archive_signature(&snapshot);
    let retry_snapshot = snapshot.clone();

    let receipt = commit_one(
        &mut document,
        "save-circular-report",
        Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot,
        }),
    );
    assert!(!receipt.replayed);
    assert_eq!(receipt.revision_before, before_revision);
    assert_eq!(receipt.revision_after, before_revision + 1);
    assert_eq!(receipt.operation_ids.len(), 1);

    let saved_case = document.circular_case(id).unwrap();
    assert_eq!(saved_case.name, before_case.name);
    assert_eq!(saved_case.definition, before_case.definition);
    assert_eq!(
        saved_case.source_operation_id,
        before_case.source_operation_id
    );
    assert_eq!(
        document
            .ground_section(saved_case.definition.ground_section_id)
            .unwrap()
            .source_operation_id,
        before_ground_source
    );
    assert_eq!(
        archive_signature(saved_case.circular_snapshot.as_ref().unwrap()),
        signature
    );
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );

    let source_operation = saved_case.source_operation_id;
    let save_operation = receipt.operation_ids[0];
    assert!(matches!(
        &document.operation(save_operation).unwrap().kind,
        OperationKind::EngineeringChanged {
            analysis_id,
            command: EngineeringCommand::SaveCircularSnapshot { .. },
        } if *analysis_id == id
    ));
    assert!(
        document
            .operation_children(source_operation)
            .any(|child| child.id == save_operation)
    );

    // A typed save request is not proof that the reported originating
    // operation/effect journal still exists. Probe the verifier with this
    // otherwise-valid receipt pointed at a non-existent operation ID.
    let receipt_command_id = receipt.command_id.clone();
    let original_operation_id = document
        .command_receipts
        .get(&receipt_command_id)
        .unwrap()
        .receipt
        .operation_ids[0];
    document
        .command_receipts
        .get_mut(&receipt_command_id)
        .unwrap()
        .receipt
        .operation_ids[0] = OperationId(u64::MAX);
    let missing_origin_error = document
        .verify_receipt_payload(
            &receipt_command_id,
            document.command_receipts.get(&receipt_command_id).unwrap(),
            None,
        )
        .unwrap_err();
    document
        .command_receipts
        .get_mut(&receipt_command_id)
        .unwrap()
        .receipt
        .operation_ids[0] = original_operation_id;
    assert!(
        missing_origin_error
            .to_string()
            .contains("no originating operation/effect journal")
    );

    // Replay keeps future receipts for audit, but the earlier selected
    // revision cannot treat the unoriginated save as a successful retry.
    let mut earlier = document.replay_revision(receipt.revision_before).unwrap();
    let diagnostic = earlier
        .receipt_payload_diagnostics()
        .unwrap()
        .into_iter()
        .find(|diagnostic| diagnostic.command_id == receipt_command_id)
        .expect("retained future-save diagnostic");
    assert_eq!(diagnostic.operation_count, 1);
    assert_eq!(diagnostic.verified_operation_count, 0);
    assert!(
        diagnostic
            .reason
            .contains("future circular archive receipt")
    );
    let future_retry = earlier
        .begin_transaction(receipt_command_id.clone(), earlier.revision())
        .with(Command::Engineering(
            EngineeringCommand::SaveCircularSnapshot {
                analysis_id: id,
                snapshot: retry_snapshot.clone(),
            },
        ));
    let future_retry_error = earlier.commit(future_retry).unwrap_err();
    assert!(
        future_retry_error
            .to_string()
            .contains("cannot satisfy an idempotent retry in this selected revision")
    );

    let retry = document
        .begin_transaction(receipt_command_id, document.revision())
        .with(Command::Engineering(
            EngineeringCommand::SaveCircularSnapshot {
                analysis_id: id,
                snapshot: retry_snapshot,
            },
        ));
    let replayed = document.commit(retry).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.operation_ids, receipt.operation_ids);
    assert_eq!(document.revision(), receipt.revision);

    let historical = document.replay_revision(receipt.revision).unwrap();
    assert_eq!(
        historical.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );

    document.undo().unwrap().expect("undo archive save");
    assert!(
        document
            .circular_case(id)
            .unwrap()
            .circular_snapshot
            .is_none()
    );
    assert_eq!(document.circular_snapshot_currentness(id).unwrap(), None);
    document.redo().unwrap().expect("redo archive save");
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );

    let native = document.to_json_string().unwrap();
    assert!(native.contains("\"geotech_circular_archive_v1\""));
    let reopened = Document::from_json_str(&native).unwrap();
    assert_eq!(
        reopened.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
    let stripped = without_archive_feature(&native);
    assert!(Document::from_json_str(&stripped).is_err());
}

#[test]
fn partial_save_failure_rolls_back_exactly_and_allows_same_id_retry() {
    let mut document = load_document("specified.unsaved.native4.json");
    let id = sole_circular_case(&document);
    let before = document.to_json_string().unwrap();
    let snapshot = document
        .prepare_circular_snapshot(id, load_archive("specified.archive.json"))
        .unwrap();
    let definition = document.circular_case(id).unwrap().definition.clone();

    let mut failed = document.begin_transaction("archive-atomic-retry", document.revision());
    failed.push(Command::Engineering(
        EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot,
        },
    ));
    failed.push(Command::Engineering(EngineeringCommand::UpdateCircular {
        analysis_id: AnalysisCaseId::new(u64::MAX),
        definition: definition.clone(),
        name: None,
    }));
    assert!(document.commit(failed).is_err());
    assert_eq!(document.to_json_string().unwrap(), before);
    assert!(
        document
            .circular_case(id)
            .unwrap()
            .circular_snapshot
            .is_none()
    );

    let retry = document
        .prepare_circular_snapshot(id, load_archive("specified.archive.json"))
        .unwrap();
    commit_one(
        &mut document,
        "archive-atomic-retry",
        Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot: retry,
        }),
    );
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
}

#[test]
fn rename_and_unrelated_edits_stay_current_but_source_edits_retain_stale_evidence() {
    let mut document = load_document("specified.native4.json");
    let id = sole_circular_case(&document);
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
    let initial_case = document.circular_case(id).unwrap();
    let initial_definition = initial_case.definition.clone();
    let signature = archive_signature(initial_case.circular_snapshot.as_ref().unwrap());
    let old_case_source = initial_case.source_operation_id;
    let ground_id = initial_definition.ground_section_id;
    let old_ground_source = document
        .ground_section(ground_id)
        .unwrap()
        .source_operation_id;

    commit_one(
        &mut document,
        "rename-circular-case",
        Command::Engineering(EngineeringCommand::UpdateCircular {
            analysis_id: id,
            definition: initial_definition.clone(),
            name: Some("Renamed archived case".into()),
        }),
    );
    assert_eq!(
        document.circular_case(id).unwrap().source_operation_id,
        old_case_source
    );
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );

    let mut unrelated = document.begin_transaction("unrelated-bim-and-draft", document.revision());
    unrelated.push(Command::CreateElement {
        name: "Archive coexistence wall".into(),
        class_id: BUILTIN_WALL_CLASS_ID.into(),
        kind: ElementKind::Wall(WallSpec {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(5.0, 0.0),
            base_elevation_m: 0.0,
            height_m: 3.0,
            thickness_m: 0.2,
        }),
        properties: BTreeMap::new(),
    });
    unrelated.push(Command::Draft(DraftCommand::CreateDrawing {
        name: "Archive coexistence section".into(),
        plane: DrawingPlane::default(),
    }));
    document.commit(unrelated).unwrap();
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );

    let mut changed_case = document.circular_case(id).unwrap().definition.clone();
    changed_case.config.slices.target_slices += 1;
    commit_one(
        &mut document,
        "change-circular-config",
        Command::Engineering(EngineeringCommand::UpdateCircular {
            analysis_id: id,
            definition: changed_case,
            name: None,
        }),
    );
    assert_ne!(
        document.circular_case(id).unwrap().source_operation_id,
        old_case_source
    );
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Stale)
    );
    assert_eq!(
        archive_signature(
            document
                .circular_case(id)
                .unwrap()
                .circular_snapshot
                .as_ref()
                .unwrap()
        ),
        signature
    );

    let mut changed_ground = document
        .ground_section(ground_id)
        .unwrap()
        .definition
        .clone();
    changed_ground.materials[0]
        .properties
        .effective_cohesion_kpa += 0.25;
    commit_one(
        &mut document,
        "change-circular-ground",
        Command::Engineering(EngineeringCommand::UpdateGround {
            ground_section_id: ground_id,
            definition: changed_ground,
            name: None,
        }),
    );
    assert_ne!(
        document
            .ground_section(ground_id)
            .unwrap()
            .source_operation_id,
        old_ground_source
    );
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Stale)
    );
    assert_eq!(
        archive_signature(
            document
                .circular_case(id)
                .unwrap()
                .circular_snapshot
                .as_ref()
                .unwrap()
        ),
        signature
    );

    let reopened = Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
    assert_eq!(
        reopened.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Stale)
    );
}

#[test]
fn retained_only_native4_requires_archive_feature_and_replays_its_saved_snapshot() {
    let retained = load_document("retained-only.native4.json");
    let id = sole_circular_case(&retained);
    assert!(
        retained
            .circular_case(id)
            .unwrap()
            .circular_snapshot
            .is_none()
    );
    assert_eq!(retained.circular_snapshot_currentness(id).unwrap(), None);

    let save_revision = retained
        .command_receipts.values()
        .find_map(|stored| {
            stored.recorded_commands.as_ref().is_some_and(|commands| commands.iter().any(|command|
                matches!(command, Command::Engineering(EngineeringCommand::SaveCircularSnapshot { analysis_id, .. }) if *analysis_id == id)))
                .then_some(stored.receipt.revision)
        })
        .expect("retained save receipt even though the live save operation was undone");
    let historical = retained.replay_revision(save_revision).unwrap();
    assert_eq!(
        historical.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );

    let native = retained.to_json_string().unwrap();
    assert!(native.contains("\"geotech_circular_archive_v1\""));
    assert!(Document::from_json_str(&without_archive_feature(&native)).is_err());

    let mut reopened = Document::from_json_str(&native).unwrap();
    assert!(
        reopened
            .circular_case(id)
            .unwrap()
            .circular_snapshot
            .is_none()
    );
    reopened.redo().unwrap().expect("redo retained save");
    assert_eq!(
        reopened.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
}

#[test]
fn migration_recounts_added_archive_owners_and_metrics_preserve_exact_byte_sizes() {
    let document = load_document("specified.native4.json");
    let metrics = document.history_metrics().unwrap();
    assert_eq!(
        metrics.document_serialized_bytes,
        serde_json::to_vec(&document).unwrap().len() as u64
    );
    let exact_history = serde_json::to_vec(&(
        &document.undo_stack,
        &document.redo_stack,
        &document.revision_states,
        &document.command_receipts,
    ))
    .unwrap()
    .len() as u64;
    assert_eq!(metrics.history_serialized_bytes, exact_history);

    // A resource-only legacy repair probe, not a valid new Native4 history or
    // a claim of numerical/producer authorization. All report numbers remain
    // the genuine producer output; only legacy checkpoint metadata is changed.
    let mut legacy = document;
    let mut checkpoint = legacy.revision_state().unwrap();
    checkpoint.delta = None;
    checkpoint.classes = None;
    checkpoint.operations = None;
    checkpoint.history_stack_anchor = None;
    legacy.revision_states.clear();
    legacy.revision_states.insert(legacy.revision(), checkpoint);
    legacy.circular_archive_usage = OnceLock::new();
    let before = legacy.circular_archive_retention_usage().unwrap();
    legacy.migrate_v1().unwrap();
    let after = legacy.circular_archive_retention_usage().unwrap();
    let measured =
        crate::circular_archive_budget::audit_circular_archive_retention(&legacy).unwrap();
    assert!(after.snapshot_occurrences > before.snapshot_occurrences);
    assert!(after.snapshot_bytes > before.snapshot_bytes);
    assert_eq!(after, measured);
    // Empty legacy history also passes through record_revision, which may
    // populate a pre-copy cache while constructing the replacement checkpoint.
    legacy.revision_states.clear();
    legacy.circular_archive_usage = OnceLock::new();
    legacy.migrate_v1().unwrap();
    let empty_history_repaired = legacy.circular_archive_retention_usage().unwrap();
    assert_eq!(
        empty_history_repaired,
        crate::circular_archive_budget::audit_circular_archive_retention(&legacy).unwrap()
    );
}

#[test]
fn archive_tampering_nonfinite_values_and_snapshot_byte_cap_are_rejected() {
    let document = load_document("specified.unsaved.native4.json");
    let id = sole_circular_case(&document);

    let mut tampered = load_archive("specified.archive.json");
    tampered.markdown.push(' ');
    assert!(document.prepare_circular_snapshot(id, tampered).is_err());

    let mut nonfinite = load_archive("specified.archive.json");
    match &mut nonfinite.bundle.evidence {
        ReportEvidence::Specified { input, .. } => input.extrusion_width_m = f64::NAN,
        ReportEvidence::Search { .. } => panic!("specified archive fixture changed mode"),
    }
    assert!(document.prepare_circular_snapshot(id, nonfinite).is_err());

    let mut oversized = load_archive("specified.archive.json");
    oversized.markdown = "x".repeat(crate::circular_archive::MAX_CIRCULAR_SNAPSHOT_BYTES + 1);
    let error = document
        .prepare_circular_snapshot(id, oversized)
        .expect_err("factory enforces the per-snapshot byte cap before archive hashing");
    assert!(matches!(error, CoreError::CircularArchiveLimit { .. }));

    let mut oversized_wire_archive = load_archive("specified.archive.json");
    oversized_wire_archive.markdown =
        "x".repeat(crate::circular_archive::MAX_CIRCULAR_SNAPSHOT_BYTES + 1);
    let raw = CircularCalculationSnapshot {
        schema_version: 1,
        archive: Box::new(oversized_wire_archive),
        source_case_definition: document.circular_case(id).unwrap().definition.clone(),
    };
    let wire = serde_json::to_string(&raw).unwrap();
    assert!(wire.len() > crate::circular_archive::MAX_CIRCULAR_SNAPSHOT_BYTES);
    let error = serde_json::from_str::<CircularCalculationSnapshot>(&wire)
        .expect_err("snapshot deserializer rejects an oversized raw archive");
    assert!(error.to_string().contains("CircularArchiveLimit"));
}
