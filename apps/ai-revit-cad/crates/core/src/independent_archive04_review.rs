//! Independent preclone quota regression. Only explicit synthetic fixture setup
//! runs the solver; admission/copy/archive operations under test never do.
use crate::*;
use geotech_report::ReportArchive;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    crate::test_fixtures::ensure_circular_archive_fixtures().join(name)
}

fn setup(saved: bool) -> (Document, AnalysisCaseId) {
    let name = if saved {
        "specified.native4.json"
    } else {
        "specified.unsaved.native4.json"
    };
    let document = Document::open(fixture(name)).unwrap();
    let id = *document.circular_cases().keys().next().unwrap();
    (document, id)
}

fn renames(document: &Document, id: AnalysisCaseId, count: usize) -> Vec<Command> {
    let definition = document.circular_case(id).unwrap().definition.clone();
    (0..count)
        .map(|index| {
            Command::Engineering(EngineeringCommand::UpdateCircular {
                analysis_id: id,
                definition: definition.clone(),
                name: Some(format!("Independent bounded archive rename {index}")),
            })
        })
        .collect()
}

#[test]
fn independent_archive04_new_archive_then_renames_reject_before_revision_snapshot() {
    let (mut document, id) = setup(false);
    assert_eq!(
        document
            .circular_archive_retention_usage()
            .unwrap()
            .snapshot_occurrences,
        0
    );
    let archive: ReportArchive =
        serde_json::from_str(&std::fs::read_to_string(fixture("specified.archive.json")).unwrap())
            .unwrap();
    let snapshot = document.prepare_circular_snapshot(id, archive).unwrap();
    let one =
        circular_archive::bounded_serialized_size(&snapshot, MAX_CIRCULAR_SNAPSHOT_BYTES).unwrap();
    // Measure the actual generated archive; its numerical values are irrelevant
    // to this quota assertion. Keep the bounded batch and byte-limit assertions.
    let count = 140;
    let journal_lower_bound = one * (1 + 2 * count);
    assert!(journal_lower_bound > MAX_CIRCULAR_RETAINED_BYTES);
    let mut commands = vec![Command::Engineering(
        EngineeringCommand::SaveCircularSnapshot {
            analysis_id: id,
            snapshot,
        },
    )];
    commands.extend(renames(&document, id, count));
    let command_id: CommandId = "independent-new-archive-propagation-quota".into();
    let admitted_before_clone = document
        .reserve_circular_commit(&command_id, &commands)
        .is_ok();
    let request_bytes =
        circular_archive::bounded_serialized_size(&commands, MAX_CIRCULAR_RETAINED_BYTES).unwrap();
    assert!(
        request_bytes < 1 << 20,
        "batch remains below existing transport input ceiling"
    );
    let before = document.to_json_string().unwrap();
    document.scan_counters.reset();
    let mut transaction = document.begin_transaction(command_id, document.revision());
    for command in commands {
        transaction.push(command);
    }
    let error = document.commit(transaction).unwrap_err();
    let recorded_revision_snapshot_bytes = document.scan_counters.snapshot_bytes();
    eprintln!(
        "INDEPENDENT_ARCHIVE04 quota=67108864 one_snapshot={one} rename_count={count} request_bytes={request_bytes} journal_snapshot_lower_bound={journal_lower_bound} admitted_before_clone={admitted_before_clone} recorded_revision_snapshot_bytes={recorded_revision_snapshot_bytes} actual_error={error:?}"
    );
    assert_eq!(
        document.to_json_string().unwrap(),
        before,
        "atomic rollback remains required"
    );
    assert!(
        matches!(
            error,
            CoreError::CircularArchiveLimit {
                scope: "transaction transient reservation",
                ..
            }
        ),
        "newly introduced archive propagation must be reserved before any copying, not rejected after a >64MiB revision snapshot"
    );
    assert_eq!(
        recorded_revision_snapshot_bytes, 0,
        "no overquota revision snapshot may be built before admission"
    );
}

#[test]
fn independent_archive04_existing_archive_batch_is_rejected_before_copy_control() {
    let (mut document, id) = setup(true);
    let commands = renames(&document, id, 140);
    let before = document.to_json_string().unwrap();
    document.scan_counters.reset();
    let mut transaction =
        document.begin_transaction("independent-existing-archive-control", document.revision());
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
