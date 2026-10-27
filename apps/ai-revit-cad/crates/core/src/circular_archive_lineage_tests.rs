//! Regressions for archive lineage across ordered commands and retained history.
//! Explicit synthetic setup runs once; lineage operations and archive reads never
//! invoke a solver. No generated result is treated as a numerical golden.
use crate::*;
use geotech_report::{
    Currentness, ReferenceAvailability, ReportArchive, ReportLimits, SourceReference,
};
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
        .unwrap_or_else(|error| panic!("read archive {name}: {error}"))
}

fn sole_case(document: &Document) -> AnalysisCaseId {
    assert_eq!(document.circular_cases().len(), 1);
    *document.circular_cases().keys().next().unwrap()
}

fn source_stamps(document: &Document, id: AnalysisCaseId) -> (OperationId, OperationId) {
    let case = document.circular_case(id).expect("circular case");
    let ground = document
        .ground_section(case.definition.ground_section_id)
        .expect("case ground section");
    (case.source_operation_id, ground.source_operation_id)
}

fn stored_archive_bytes(document: &Document, id: AnalysisCaseId) -> Vec<u8> {
    serde_json::to_vec(
        document
            .circular_case(id)
            .expect("circular case")
            .circular_snapshot
            .as_ref()
            .expect("saved circular archive")
            .archive(),
    )
    .unwrap()
}

fn rename_commands(
    document: &Document,
    id: AnalysisCaseId,
    count: usize,
    prefix: &str,
) -> Vec<Command> {
    let definition = document.circular_case(id).unwrap().definition.clone();
    (0..count)
        .map(|index| {
            Command::Engineering(EngineeringCommand::UpdateCircular {
                analysis_id: id,
                definition: definition.clone(),
                name: Some(format!("{prefix} {index}")),
            })
        })
        .collect()
}

fn commit_one(document: &mut Document, command_id: &str, command: Command) -> CommitReceipt {
    let mut transaction = document.begin_transaction(command_id, document.revision());
    transaction.push(command);
    document.commit(transaction).unwrap()
}

#[test]
fn save_then_bounded_renames_preserve_lineage_and_archive_history() {
    let mut document = load_document("specified.unsaved.native4.json");
    let id = sole_case(&document);
    assert_eq!(
        document
            .circular_archive_retention_usage()
            .unwrap()
            .snapshot_occurrences,
        0
    );
    let stamps = source_stamps(&document, id);
    let definition = document.circular_case(id).unwrap().definition.clone();
    let snapshot = document
        .prepare_circular_snapshot(id, load_archive("specified.archive.json"))
        .unwrap();
    let expected_archive = serde_json::to_vec(snapshot.archive()).unwrap();

    let mut transaction =
        document.begin_transaction("lineage-save-and-bounded-renames", document.revision());
    transaction.push(Command::Engineering(
        EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot,
        },
    ));
    for command in rename_commands(&document, id, 3, "same-transaction archive rename") {
        transaction.push(command);
    }
    let receipt = document.commit(transaction).unwrap();

    assert_eq!(
        source_stamps(&document, id),
        stamps,
        "name-only edits must retain the numerical source stamps"
    );
    assert_eq!(stored_archive_bytes(&document, id), expected_archive);
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
    assert!(
        receipt.operation_ids.iter().any(|operation_id| {
            matches!(
                document.operation(*operation_id).map(|record| &record.kind),
                Some(OperationKind::EngineeringChanged {
                    analysis_id,
                    command: EngineeringCommand::SaveCircularSnapshot { .. },
                }) if *analysis_id == id
            )
        }),
        "the saved archive remains in the operation history"
    );

    let historical = document.replay_revision(receipt.revision).unwrap();
    assert_eq!(stored_archive_bytes(&historical, id), expected_archive);
    assert_eq!(source_stamps(&historical, id), stamps);
    assert_eq!(
        historical.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );

    let later_rename = rename_commands(&document, id, 1, "later archive rename")
        .pop()
        .unwrap();
    commit_one(&mut document, "lineage-later-rename", later_rename);
    document.undo().unwrap().expect("undo later rename");
    assert_eq!(stored_archive_bytes(&document, id), expected_archive);
    assert_eq!(source_stamps(&document, id), stamps);
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
    document.redo().unwrap().expect("redo later rename");
    assert_eq!(stored_archive_bytes(&document, id), expected_archive);
    assert_eq!(source_stamps(&document, id), stamps);
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
}

#[test]
fn save_then_predicted_create_and_update_use_sequential_case_lineage() {
    let mut document = load_document("specified.unsaved.native4.json");
    let existing_id = sole_case(&document);
    assert_eq!(
        document
            .circular_archive_retention_usage()
            .unwrap()
            .snapshot_occurrences,
        0
    );
    let existing_stamps = source_stamps(&document, existing_id);
    let existing_definition = document
        .circular_case(existing_id)
        .unwrap()
        .definition
        .clone();
    let snapshot = document
        .prepare_circular_snapshot(existing_id, load_archive("specified.archive.json"))
        .unwrap();
    let expected_archive = serde_json::to_vec(snapshot.archive()).unwrap();

    // The first command consumes the next operation ID. CreateCircular receives
    // the following ID, which becomes the new analysis ID for the later update.
    let predicted_new_case = AnalysisCaseId::new(
        document
            .next_operation_id
            .checked_add(1)
            .expect("predicted case operation ID"),
    );
    let mut transaction =
        document.begin_transaction("lineage-save-create-update-sequence", document.revision());
    transaction.push(Command::Engineering(
        EngineeringCommand::SaveCircularSnapshot {
            analysis_id: existing_id,
            snapshot,
        },
    ));
    transaction.push(Command::Engineering(EngineeringCommand::CreateCircular {
        definition: existing_definition.clone(),
        name: "Predicted new circular case".into(),
    }));
    transaction.push(Command::Engineering(EngineeringCommand::UpdateCircular {
        analysis_id: predicted_new_case,
        definition: existing_definition,
        name: Some("Updated predicted circular case".into()),
    }));
    let receipt = document.commit(transaction).unwrap();

    assert_eq!(receipt.operation_ids.len(), 3);
    assert_eq!(receipt.operation_ids[1].get(), predicted_new_case.get());
    assert_eq!(source_stamps(&document, existing_id), existing_stamps);
    assert_eq!(
        stored_archive_bytes(&document, existing_id),
        expected_archive
    );
    assert_eq!(
        document.circular_snapshot_currentness(existing_id).unwrap(),
        Some(Currentness::Current)
    );
    let created = document
        .circular_case(predicted_new_case)
        .expect("CreateCircular ID is the predicted operation ID");
    assert_eq!(created.source_operation_id, receipt.operation_ids[1]);
    assert_eq!(created.name, "Updated predicted circular case");
    assert!(created.circular_snapshot.is_none());

    let historical = document.replay_revision(receipt.revision).unwrap();
    assert_eq!(
        stored_archive_bytes(&historical, existing_id),
        expected_archive
    );
    assert!(historical.circular_case(predicted_new_case).is_some());

    document
        .undo()
        .unwrap()
        .expect("undo save/create/update batch");
    assert!(document.circular_case(predicted_new_case).is_none());
    assert!(document
        .circular_case(existing_id)
        .unwrap()
        .circular_snapshot
        .is_none());
    document
        .redo()
        .unwrap()
        .expect("redo save/create/update batch");
    assert_eq!(
        stored_archive_bytes(&document, existing_id),
        expected_archive
    );
    assert_eq!(source_stamps(&document, existing_id), existing_stamps);
    assert!(document.circular_case(predicted_new_case).is_some());
    assert!(document
        .circular_case(predicted_new_case)
        .unwrap()
        .circular_snapshot
        .is_none());
}

fn enlarged_resource_probe_archive() -> (ReportArchive, Vec<u8>) {
    let mut bundle = load_archive("specified.archive.json").bundle;
    let unchanged_evidence = serde_json::to_vec(&bundle.evidence).unwrap();
    let payload = "bounded archive-retention resource-probe payload ".repeat(120);
    for index in 0..28 {
        bundle.source_references.push(SourceReference {
            title: format!("Synthetic test resource probe {index:02}"),
            locator: Some("synthetic-fixture:specified.archive.json".into()),
            publication_date: None,
            section_or_page: Some(format!("retention-probe-{index:02}")),
            availability: ReferenceAvailability::ProvidedUnverified,
            note: Some(format!(
                "Synthetic test-only resource metadata. The locator identifies the explicitly generated report fixture; this record makes no additional geotechnical-source or numerical-result claim. {payload}"
            )),
        });
    }

    let archive = geotech_report::create_archive(bundle, ReportLimits::default())
        .expect("bounded metadata-only archive probe");
    assert_eq!(
        serde_json::to_vec(&archive.bundle.evidence).unwrap(),
        unchanged_evidence,
        "source-reference probe must leave all numerical evidence unchanged"
    );
    (archive, unchanged_evidence)
}

fn replacement_commands(
    document: &Document,
    id: AnalysisCaseId,
    small_snapshot: &crate::CircularCalculationSnapshot,
    large_snapshot: &crate::CircularCalculationSnapshot,
    rename_count: usize,
) -> Vec<Command> {
    let mut commands = vec![
        Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot: small_snapshot.clone(),
        }),
        Command::Engineering(EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot: large_snapshot.clone(),
        }),
    ];
    commands.extend(rename_commands(
        document,
        id,
        rename_count,
        "replacement archive rename",
    ));
    commands
}

/// Reproduce the archived draft04 self-case estimate so this regression proves
/// that the old rule would admit the same valid command batch.
fn draft04_self_case_reservation(document: &Document, commands: &[Command]) -> usize {
    let current = document.circular_archive_retention_usage().unwrap();
    let incoming =
        crate::circular_archive_budget::audit_circular_archive_retention(commands).unwrap();
    let request =
        crate::circular_archive::bounded_serialized_size(commands, MAX_CIRCULAR_RETAINED_BYTES)
            .unwrap();
    let mut extra = request + incoming.snapshot_bytes * 9;
    for command in commands {
        if let Command::Engineering(
            EngineeringCommand::UpdateCircular { analysis_id, .. }
            | EngineeringCommand::SaveCircularSnapshot { analysis_id, .. },
        ) = command
        {
            if let Some(case) = document.circular_case(*analysis_id) {
                let self_case =
                    crate::circular_archive_budget::audit_circular_archive_retention(case).unwrap();
                extra += self_case.snapshot_bytes * 6;
            }
        }
    }
    let next = document.revision + 1;
    if document.revision_states.is_empty()
        || next % document.history_policy.checkpoint_interval == 0
    {
        extra += crate::circular_archive_budget::audit_circular_archive_retention(&(
            &document.engineering,
            &document.operations,
        ))
        .unwrap()
        .snapshot_bytes;
    }
    extra += current.max_snapshot_bytes.max(incoming.max_snapshot_bytes);
    current.snapshot_bytes + current.payload_json_bytes + extra
}

fn select_admitted_replacement_batch(
    document: &Document,
    id: AnalysisCaseId,
    small_snapshot: &crate::CircularCalculationSnapshot,
    large_snapshot: &crate::CircularCalculationSnapshot,
    large_snapshot_bytes: usize,
) -> (usize, Vec<Command>, usize, usize) {
    for rename_count in (1..=48).rev() {
        let commands =
            replacement_commands(document, id, small_snapshot, large_snapshot, rename_count);
        let Some(request_bytes) =
            crate::circular_archive::bounded_serialized_size(&commands, 1 << 20).ok()
        else {
            continue;
        };
        let legacy_reservation = draft04_self_case_reservation(document, &commands);
        let retained_owner_lower_bound = 4 * rename_count * large_snapshot_bytes;
        if legacy_reservation <= MAX_CIRCULAR_RETAINED_BYTES
            && retained_owner_lower_bound > MAX_CIRCULAR_RETAINED_BYTES
        {
            return (rename_count, commands, request_bytes, legacy_reservation);
        }
    }
    panic!("no bounded replacement batch distinguishes the draft04 estimate")
}

fn assert_rejected_before_revision_snapshot(
    document: &mut Document,
    id: AnalysisCaseId,
    commands: Vec<Command>,
) {
    let before = document.to_json_string().unwrap();
    document.scan_counters.reset();
    let mut transaction =
        document.begin_transaction("replacement-archive-preclone-quota", document.revision());
    for command in commands {
        transaction.push(command);
    }
    let error = document.commit(transaction).unwrap_err();
    let recorded_revision_snapshot_bytes = document.scan_counters.snapshot_bytes();

    assert!(
        matches!(
            error,
            CoreError::CircularArchiveLimit {
                scope: "transaction transient reservation",
                ..
            }
        ),
        "new archive ownership must be rejected during preclone reservation: {error:?}"
    );
    assert_eq!(
        recorded_revision_snapshot_bytes, 0,
        "the overquota revision snapshot must not be built before admission"
    );
    assert_eq!(
        document.to_json_string().unwrap(),
        before,
        "quota rejection must restore the complete native document exactly"
    );
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
}

#[test]
fn replacement_archive_growth_is_reserved_before_revision_snapshot() {
    let mut document = load_document("specified.native4.json");
    let id = sole_case(&document);
    assert_eq!(
        document.circular_snapshot_currentness(id).unwrap(),
        Some(Currentness::Current)
    );
    let small_snapshot = document
        .prepare_circular_snapshot(id, load_archive("specified.archive.json"))
        .unwrap();
    let small_snapshot_bytes = crate::circular_archive::bounded_serialized_size(
        &small_snapshot,
        MAX_CIRCULAR_SNAPSHOT_BYTES,
    )
    .unwrap();
    let (larger_archive, unchanged_evidence) = enlarged_resource_probe_archive();
    let large_snapshot = document
        .prepare_circular_snapshot(id, larger_archive)
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&large_snapshot.archive().bundle.evidence).unwrap(),
        unchanged_evidence
    );
    let large_snapshot_bytes = crate::circular_archive::bounded_serialized_size(
        &large_snapshot,
        MAX_CIRCULAR_SNAPSHOT_BYTES,
    )
    .unwrap();
    assert!(large_snapshot_bytes > small_snapshot_bytes);

    let (rename_count, commands, request_bytes, legacy_reservation) =
        select_admitted_replacement_batch(
            &document,
            id,
            &small_snapshot,
            &large_snapshot,
            large_snapshot_bytes,
        );
    let true_retained_owner_lower_bound = 4 * rename_count * large_snapshot_bytes;
    assert!(request_bytes < 1 << 20);
    assert!(legacy_reservation <= MAX_CIRCULAR_RETAINED_BYTES);
    assert!(true_retained_owner_lower_bound > MAX_CIRCULAR_RETAINED_BYTES);

    assert_rejected_before_revision_snapshot(&mut document, id, commands);
}

#[test]
fn initially_free_replacement_lineage_uses_the_larger_staged_archive() {
    let mut document = load_document("specified.unsaved.native4.json");
    let id = sole_case(&document);
    assert_eq!(
        document
            .circular_archive_retention_usage()
            .unwrap()
            .snapshot_occurrences,
        0
    );
    let small_snapshot = document
        .prepare_circular_snapshot(id, load_archive("specified.archive.json"))
        .unwrap();
    let (larger_archive, unchanged_evidence) = enlarged_resource_probe_archive();
    let large_snapshot = document
        .prepare_circular_snapshot(id, larger_archive)
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&large_snapshot.archive().bundle.evidence).unwrap(),
        unchanged_evidence
    );
    let large_snapshot_bytes = crate::circular_archive::bounded_serialized_size(
        &large_snapshot,
        MAX_CIRCULAR_SNAPSHOT_BYTES,
    )
    .unwrap();
    let (rename_count, commands, request_bytes, legacy_reservation) =
        select_admitted_replacement_batch(
            &document,
            id,
            &small_snapshot,
            &large_snapshot,
            large_snapshot_bytes,
        );
    assert!(request_bytes < 1 << 20);
    assert!(legacy_reservation <= MAX_CIRCULAR_RETAINED_BYTES);
    assert!(4 * rename_count * large_snapshot_bytes > MAX_CIRCULAR_RETAINED_BYTES);

    let before = document.to_json_string().unwrap();
    document.scan_counters.reset();
    let mut transaction = document.begin_transaction(
        "initially-free-replacement-lineage-quota",
        document.revision(),
    );
    for command in commands {
        transaction.push(command);
    }
    let error = document.commit(transaction).unwrap_err();
    assert!(matches!(
        error,
        CoreError::CircularArchiveLimit {
            scope: "transaction transient reservation",
            ..
        }
    ));
    assert_eq!(document.scan_counters.snapshot_bytes(), 0);
    assert_eq!(document.to_json_string().unwrap(), before);
}
