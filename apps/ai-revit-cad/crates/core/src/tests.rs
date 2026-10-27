use super::*;

fn document_with_wall() -> (Document, CommitReceipt) {
    let mut document = Document::new("History regression").unwrap();
    let transaction = document
        .begin_transaction("create-wall", 0)
        .with(Command::CreateElement {
            name: "Wall".to_owned(),
            class_id: BUILTIN_WALL_CLASS_ID.into(),
            kind: ElementKind::Wall(WallSpec {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(4.0, 0.0),
                base_elevation_m: 0.0,
                height_m: 3.0,
                thickness_m: 0.2,
            }),
            properties: BTreeMap::new(),
        });
    let receipt = document.commit(transaction).unwrap();
    (document, receipt)
}

#[test]
fn exhausted_revision_preserves_undo_and_document() {
    let (mut document, receipt) = document_with_wall();
    let recoverable_revision = document.revision();
    document.revision = u64::MAX;
    document.record_revision().unwrap();
    let before = document.to_json_string().unwrap();

    assert!(matches!(document.undo(), Err(CoreError::RevisionExhausted)));
    assert_eq!(document.to_json_string().unwrap(), before);

    // Returning the counter to its prior value proves the history entry survived.
    document.revision_states.remove(&u64::MAX);
    document.revision = recoverable_revision;
    let undo = document
        .undo()
        .unwrap()
        .expect("undo entry must survive failure");
    assert_eq!(undo.command_id, receipt.command_id);
    assert!(document.element(receipt.created_elements[0]).is_none());
    assert!(document.operation(receipt.operation_ids[0]).is_none());
}

#[test]
fn exhausted_revision_preserves_redo_and_document() {
    let (mut document, receipt) = document_with_wall();
    document.undo().unwrap().expect("initial undo");
    let recoverable_revision = document.revision();
    document.revision = u64::MAX;
    document.record_revision().unwrap();
    let before = document.to_json_string().unwrap();

    assert!(matches!(document.redo(), Err(CoreError::RevisionExhausted)));
    assert_eq!(document.to_json_string().unwrap(), before);

    document.revision_states.remove(&u64::MAX);
    document.revision = recoverable_revision;
    let redo = document
        .redo()
        .unwrap()
        .expect("redo entry must survive failure");
    assert_eq!(redo.command_id, receipt.command_id);
    assert!(document.element(receipt.created_elements[0]).is_some());
    assert!(document.operation(receipt.operation_ids[0]).is_some());
}
