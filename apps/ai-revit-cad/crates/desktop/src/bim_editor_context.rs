//! Captured native BIM editor authority and its single transaction boundary.
use ai_revit_core::{ClassId, Command, CommitReceipt, Document, ElementId, EntityRef, PropertyId, TypeId};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "editor", rename_all = "snake_case")]
pub(super) enum EditorTarget {
    Primary {
        owner: EntityRef,
        element_id: Option<ElementId>,
        type_id: Option<TypeId>,
        property_id: PropertyId,
    },
    Custom { element_id: ElementId, class_id: ClassId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct EditorContext {
    document_uuid: String,
    runtime_instance_id: String,
    document_epoch: u64,
    pub document_revision: u64,
    target: EditorTarget,
}

impl EditorContext {
    pub fn capture(document: &Document, epoch: u64, target: EditorTarget) -> Self {
        Self {
            document_uuid: document.document_uuid.to_string(),
            runtime_instance_id: document.runtime_instance_id(),
            document_epoch: epoch,
            document_revision: document.revision(),
            target,
        }
    }
    fn same_incarnation(&self, other: &Self) -> bool {
        self.document_uuid == other.document_uuid
            && self.runtime_instance_id == other.runtime_instance_id
            && self.document_epoch == other.document_epoch
    }
    pub fn is_current(&self, document: &Document, epoch: u64) -> bool {
        let current = Self::capture(document, epoch, self.target.clone());
        self.same_incarnation(&current) && self.document_revision == current.document_revision
    }
    pub fn require_current(&self, document: &Document, epoch: u64, target: &EditorTarget) -> Result<(), String> {
        if !self.is_current(document, epoch) || &self.target != target {
            return Err("stale_ui_intent: BIM-документ, ревизия или цель редактора изменены; выберите свойство заново".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditorRefresh { Reset, Update, Preserve }

#[derive(Default)]
pub(super) struct EditorState {
    pub context: Option<EditorContext>,
    pub dirty: bool,
}

impl EditorState {
    pub fn clear(&mut self) { self.context = None; self.dirty = false; }
    pub fn refresh(&mut self, document: Option<&Document>, epoch: u64, target: Option<EditorTarget>) -> EditorRefresh {
        let next = document.zip(target).map(|(document, target)| EditorContext::capture(document, epoch, target));
        let same_owner = self.context.as_ref().zip(next.as_ref())
            .is_some_and(|(old, next)| old.same_incarnation(next) && old.target == next.target);
        if !same_owner {
            self.context = next;
            self.dirty = false;
            return EditorRefresh::Reset;
        }
        if self.dirty { return EditorRefresh::Preserve; }
        self.context = next;
        EditorRefresh::Update
    }
    pub fn stale(&self, document: Option<&Document>, epoch: u64) -> bool {
        self.context.as_ref().is_some_and(|context| document.is_none_or(|document| !context.is_current(document, epoch)))
    }
}

fn command_matches_target(command: &Command, target: &EditorTarget) -> bool {
    match (target, command) {
        (EditorTarget::Primary { element_id, property_id, .. }, Command::SetProperty { element_id: actual, property_id: property, .. }
            | Command::ClearPropertyOverride { element_id: actual, property_id: property }) => element_id == &Some(*actual) && property_id == property,
        (EditorTarget::Primary { type_id, property_id, .. }, Command::UpdateType { type_id: actual, name, properties }) => {
            type_id == &Some(*actual) && name.is_none() && properties.len() == 1 && properties.contains_key(property_id)
        }
        (EditorTarget::Custom { element_id, .. }, Command::SetProperty { element_id: actual, .. }) => element_id == actual,
        (EditorTarget::Custom { class_id, .. }, Command::DefineProperty { class_id: actual, .. }) => class_id == actual,
        _ => false,
    }
}

/// Used by primary instance/type/clear and legacy custom native property forms.
/// Every refusal occurs before Core sees a transaction, including its allocator.
pub(super) fn commit_editor_commands(
    document: &mut Document, epoch: u64, context: &EditorContext,
    target: &EditorTarget, command_id: String, commands: Vec<Command>,
) -> Result<CommitReceipt, String> {
    context.require_current(document, epoch, target)?;
    if commands.is_empty() || !commands.iter().all(|command| command_matches_target(command, target)) {
        return Err("stale_ui_intent: BIM-команда не соответствует сохранённой цели редактора".into());
    }
    let mut transaction = document.begin_transaction(command_id, context.document_revision);
    for command in commands { transaction.push(command); }
    document.commit(transaction).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_revit_core::{PropertyDef, PropertyType, PropertyValue, Unit, BUILTIN_WALL_CLASS_ID};
    use std::collections::BTreeMap;

    fn execute(document: &mut Document, id: &str, commands: Vec<Command>) -> CommitReceipt {
        let mut transaction = document.begin_transaction(id.to_owned(), document.revision());
        for command in commands { transaction.push(command); }
        document.commit(transaction).unwrap()
    }
    fn number(value: f64) -> PropertyValue { PropertyValue::Number { value, unit: Unit::Meter } }
    fn fixture() -> (Document, TypeId) {
        let mut document = Document::from_json_str(include_str!("../../core/tests/fixtures/public/mixed-before.ai-revit.json")).unwrap();
        let type_id = execute(&mut document, "editor-fixture-type", vec![Command::CreateType {
            name: "Editor type".into(), class_id: BUILTIN_WALL_CLASS_ID.into(),
            properties: BTreeMap::from([(PropertyId::new("height"), number(3.0))]),
        }]).created_types[0];
        execute(&mut document, "editor-fixture-assign", vec![Command::AssignType { element_id: ElementId::new(1), type_id },
            Command::SetProperty { element_id: ElementId::new(1), property_id: PropertyId::new("height"), value: number(3.1) }]);
        (document, type_id)
    }
    fn cases(type_id: TypeId) -> Vec<(EditorTarget, Vec<Command>)> {
        let primary = EditorTarget::Primary { owner: EntityRef::Element(ElementId::new(1)), element_id: Some(ElementId::new(1)), type_id: Some(type_id), property_id: PropertyId::new("height") };
        vec![
            (primary.clone(), vec![Command::SetProperty { element_id: ElementId::new(1), property_id: PropertyId::new("height"), value: number(3.7) }]),
            (primary.clone(), vec![Command::ClearPropertyOverride { element_id: ElementId::new(1), property_id: PropertyId::new("height") }]),
            (primary, vec![Command::UpdateType { type_id, name: None, properties: BTreeMap::from([(PropertyId::new("height"), number(3.9))]) }]),
            (EditorTarget::Custom { element_id: ElementId::new(1), class_id: BUILTIN_WALL_CLASS_ID.into() }, vec![
                Command::DefineProperty { class_id: BUILTIN_WALL_CLASS_ID.into(), definition: PropertyDef::new("editor_note", "Editor note", PropertyType::Text) },
                Command::SetProperty { element_id: ElementId::new(1), property_id: PropertyId::new("editor_note"), value: PropertyValue::Text("typed before refresh".into()) },
            ]),
        ]
    }
    fn reject_unchanged(document: &mut Document, epoch: u64, context: &EditorContext, target: &EditorTarget, commands: Vec<Command>) {
        let before = document.to_json_string().unwrap();
        let error = commit_editor_commands(document, epoch, context, target, "editor-rejected".into(), commands).unwrap_err();
        assert!(error.starts_with("stale_ui_intent"), "{error}");
        assert_eq!(document.to_json_string().unwrap(), before, "authority/history/high-water allocators must all remain unchanged");
    }
    #[test]
    fn every_bim_property_editor_rejects_external_revision_before_transaction() {
        let (_, type_id) = fixture();
        for (target, commands) in cases(type_id) {
            let (mut document, _) = fixture();
            let context = EditorContext::capture(&document, 7, target.clone());
            execute(&mut document, "unrelated-revision", vec![Command::CreateLevel { name: "Other level".into(), elevation_m: 10.0, height_m: None }]);
            reject_unchanged(&mut document, 7, &context, &target, commands);
        }
    }
    #[test]
    fn every_bim_property_editor_rejects_same_uuid_revision_replacement_and_epoch() {
        let (document, type_id) = fixture();
        for (target, commands) in cases(type_id) {
            let context = EditorContext::capture(&document, 7, target.clone());
            let mut replacement = Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
            assert_eq!(replacement.document_uuid, document.document_uuid);
            assert_eq!(replacement.revision(), document.revision());
            assert_ne!(replacement.runtime_instance_id(), document.runtime_instance_id());
            reject_unchanged(&mut replacement, 7, &context, &target, commands.clone());
            let (mut current, _) = fixture();
            let current_context = EditorContext::capture(&current, 7, target.clone());
            reject_unchanged(&mut current, 8, &current_context, &target, commands);
        }
    }
    #[test]
    fn current_bim_property_editors_commit_exactly_one_project_revision() {
        let (_, type_id) = fixture();
        for (index, (target, commands)) in cases(type_id).into_iter().enumerate() {
            let (mut document, _) = fixture();
            let before = document.revision();
            let context = EditorContext::capture(&document, 7, target.clone());
            let receipt = commit_editor_commands(&mut document, 7, &context, &target, "current-editor".into(), commands).unwrap();
            assert_eq!(document.revision(), before + 1);
            assert_eq!(receipt.revision_before, before);
            assert_eq!(receipt.revision_after, before + 1);
            let wall = document.element(ElementId::new(1)).unwrap();
            match index {
                0 => assert_eq!(wall.properties.get(&PropertyId::new("height")), Some(&number(3.7))),
                1 => assert!(!wall.properties.contains_key(&PropertyId::new("height"))),
                2 => assert_eq!(document.element_type(type_id).unwrap().properties.get(&PropertyId::new("height")), Some(&number(3.9))),
                3 => assert_eq!(wall.properties.get(&PropertyId::new("editor_note")), Some(&PropertyValue::Text("typed before refresh".into()))),
                _ => unreachable!(),
            }
        }
    }
    #[test]
    fn dirty_editor_keeps_its_original_context_and_reset_requires_new_owner_or_incarnation() {
        let (mut document, type_id) = fixture();
        let target = cases(type_id).remove(0).0;
        let mut state = EditorState::default();
        assert_eq!(state.refresh(Some(&document), 7, Some(target.clone())), EditorRefresh::Reset);
        state.dirty = true;
        let original = state.context.clone();
        execute(&mut document, "unrelated-revision", vec![Command::CreateLevel { name: "Other level".into(), elevation_m: 10.0, height_m: None }]);
        assert_eq!(state.refresh(Some(&document), 7, Some(target.clone())), EditorRefresh::Preserve);
        assert_eq!(state.context, original);
        assert!(state.dirty && state.stale(Some(&document), 7));
        let replacement = Document::from_json_str(&document.to_json_string().unwrap()).unwrap();
        assert_eq!(state.refresh(Some(&replacement), 7, Some(target)), EditorRefresh::Reset);
        assert!(!state.dirty && !state.stale(Some(&replacement), 7));
    }
    #[test]
    fn current_context_cannot_retarget_another_property_or_execute_an_unrelated_command() {
        let (mut document, type_id) = fixture();
        let (target, commands) = cases(type_id).remove(0);
        let context = EditorContext::capture(&document, 7, target.clone());
        let mut other = target.clone();
        if let EditorTarget::Primary { property_id, .. } = &mut other { *property_id = PropertyId::new("thickness"); }
        reject_unchanged(&mut document, 7, &context, &other, commands);
        reject_unchanged(&mut document, 7, &context, &target, vec![Command::DeleteElement { element_id: ElementId::new(1) }]);
    }
}
