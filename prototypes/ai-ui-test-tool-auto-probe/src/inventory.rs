//! Compact semantic inventory built only through Public Tool API calls.

use crate::api::{AiUiClient, Transport};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const INVENTORY_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Bounds {
    pub fn valid(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum InteractiveClass {
    Informational,
    Actionable,
    Editable,
    Selectable,
    Container,
    Spatial,
    Transient,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SafetyClass {
    Safe,
    StatefulRecoverable,
    PotentiallyDestructive,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityState {
    pub name: String,
    pub supported: bool,
    pub available: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementDiagnostics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_element_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hwnd: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiElement {
    pub automation_id: String,
    pub role: String,
    pub control_type: String,
    pub name: String,
    pub visible: bool,
    #[serde(default)]
    pub offscreen: bool,
    pub enabled: bool,
    pub interactable: bool,
    pub selected: Option<bool>,
    pub value: Option<String>,
    pub bounds: Bounds,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub capabilities: Vec<CapabilityState>,
    pub spatial_capabilities: Vec<CapabilityState>,
    pub classification: InteractiveClass,
    pub safety: SafetyClass,
    pub dynamic: bool,
    pub transient: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_family: Option<String>,
    pub diagnostics: ElementDiagnostics,
}

impl UiElement {
    pub fn is_interactive(&self) -> bool {
        matches!(
            self.classification,
            InteractiveClass::Actionable
                | InteractiveClass::Editable
                | InteractiveClass::Selectable
                | InteractiveClass::Spatial
        )
    }

    pub fn capability(&self, name: &str) -> Option<&CapabilityState> {
        self.capabilities
            .iter()
            .chain(self.spatial_capabilities.iter())
            .find(|capability| capability.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventorySession {
    pub id: String,
    pub closed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryWindow {
    pub window_id: String,
    pub name: String,
    pub automation_id: String,
    pub bounds: Bounds,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityIssue {
    pub severity: IssueSeverity,
    pub code: String,
    pub automation_id: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionFamily {
    pub pattern: String,
    pub members: Vec<String>,
    pub representatives: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiInventory {
    pub version: u32,
    pub application: String,
    pub session: InventorySession,
    pub windows: Vec<InventoryWindow>,
    pub elements: Vec<UiElement>,
    pub collections: Vec<CollectionFamily>,
    pub quality_issues: Vec<QualityIssue>,
    pub markers: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InventoryOptions {
    #[serde(default)]
    pub element_hints: BTreeMap<String, ElementHint>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ElementHint {
    #[serde(default)]
    pub safety: Option<SafetyClass>,
    #[serde(default)]
    pub dynamic: Option<bool>,
    #[serde(default)]
    pub transient: Option<bool>,
}

pub fn collect<T: Transport>(
    client: &mut AiUiClient<T>,
    application: &str,
    window_title: Option<&str>,
    options: &InventoryOptions,
) -> Result<UiInventory, String> {
    let open = client.call(
        "session.open",
        json!({"application":application,"window_title":window_title}),
    );
    let session_id = successful_result(&open, "session.open")?
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "session.open result has no session_id".to_string())?
        .to_owned();
    let inspect = client.call(
        "inspect_ui",
        json!({"session_id":session_id,"compact":false}),
    );
    if let Err(error) = successful_result(&inspect, "inspect_ui") {
        let _ = client.call("session.close", json!({"session_id":session_id}));
        return Err(error);
    }
    let mut spatial = BTreeMap::new();
    if let Some(elements) = inspect["result"]["elements"].as_array() {
        for raw in elements.iter().filter(|raw| looks_spatial(raw)) {
            let Some(id) = raw.get("automation_id").and_then(Value::as_str) else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            let response = client.call(
                "inspect_spatial_target",
                json!({"session_id":session_id,"id":id}),
            );
            if response.get("success").and_then(Value::as_bool) == Some(true) {
                spatial.insert(id.to_owned(), response);
            }
        }
    }
    let mut inventory = from_public_responses(application, &open, &inspect, &spatial, options)?;
    let close = client.call("session.close", json!({"session_id":session_id}));
    successful_result(&close, "session.close")?;
    inventory.session.closed = true;
    inventory.markers.push("AI_UI_INVENTORY_PASSED".into());
    Ok(inventory)
}

pub fn from_public_responses(
    application: &str,
    open: &Value,
    inspect: &Value,
    spatial: &BTreeMap<String, Value>,
    options: &InventoryOptions,
) -> Result<UiInventory, String> {
    let open_result = successful_result(open, "session.open")?;
    let inspect_result = successful_result(inspect, "inspect_ui")?;
    let session_id = open_result
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "session.open result has no session_id".to_string())?
        .to_owned();
    let windows = open_result
        .get("windows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_window)
        .collect::<Vec<_>>();
    let raw_elements = inspect_result
        .get("elements")
        .and_then(Value::as_array)
        .ok_or_else(|| "inspect_ui result has no elements".to_string())?;
    let stable_by_element = raw_elements
        .iter()
        .filter_map(|raw| {
            let element_id = raw.get("element_id")?.as_str()?;
            let automation_id = raw.get("automation_id")?.as_str()?;
            (!automation_id.is_empty()).then(|| (element_id.to_owned(), automation_id.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut children_by_parent: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for raw in raw_elements {
        let Some(automation_id) = raw.get("automation_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(parent_element) = raw.get("parent_element_id").and_then(Value::as_str) else {
            continue;
        };
        if !automation_id.is_empty() {
            if let Some(parent) = stable_by_element.get(parent_element) {
                children_by_parent
                    .entry(parent.clone())
                    .or_default()
                    .push(automation_id.to_owned());
            }
        }
    }
    let mut elements = raw_elements
        .iter()
        .map(|raw| {
            parse_element(
                raw,
                &stable_by_element,
                &children_by_parent,
                spatial,
                options,
            )
        })
        .collect::<Vec<_>>();
    elements.sort_by(|left, right| left.automation_id.cmp(&right.automation_id));
    for element in &mut elements {
        element.children.sort();
    }
    let collections = detect_collections(&mut elements);
    let quality_issues = audit_quality(&elements);
    Ok(UiInventory {
        version: INVENTORY_VERSION,
        application: application.to_owned(),
        session: InventorySession {
            id: session_id,
            closed: false,
        },
        windows,
        elements,
        collections,
        quality_issues,
        markers: vec![],
    })
}

fn parse_window(raw: &Value) -> InventoryWindow {
    InventoryWindow {
        window_id: string(raw, "window_id"),
        name: string(raw, "name"),
        automation_id: string(raw, "automation_id"),
        bounds: parse_bounds(raw.get("bounds")),
        visible: bool_value(raw, "visible"),
    }
}

fn parse_element(
    raw: &Value,
    stable_by_element: &BTreeMap<String, String>,
    children_by_parent: &BTreeMap<String, Vec<String>>,
    spatial: &BTreeMap<String, Value>,
    options: &InventoryOptions,
) -> UiElement {
    let automation_id = string(raw, "automation_id");
    let role = raw
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| raw.get("control_type").and_then(Value::as_str))
        .unwrap_or("Unknown")
        .to_owned();
    let control_type = raw
        .get("control_type")
        .and_then(Value::as_str)
        .unwrap_or(&role)
        .to_owned();
    let visible = bool_value(raw, "visible");
    let offscreen = bool_value(raw, "offscreen");
    let enabled = bool_value(raw, "enabled");
    let interactable = raw
        .get("interactable")
        .and_then(Value::as_bool)
        .unwrap_or(visible && !offscreen && enabled && parse_bounds(raw.get("bounds")).valid());
    let mut capabilities = semantic_capabilities(raw, interactable);
    capabilities.sort_by(|left, right| left.name.cmp(&right.name));
    let mut spatial_capabilities = spatial
        .get(&automation_id)
        .map(spatial_capabilities_from_response)
        .unwrap_or_default();
    spatial_capabilities.sort_by(|left, right| left.name.cmp(&right.name));
    let parent_element_id = raw
        .get("parent_element_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let parent = parent_element_id
        .as_ref()
        .and_then(|id| stable_by_element.get(id))
        .cloned();
    let children = children_by_parent
        .get(&automation_id)
        .cloned()
        .unwrap_or_default();
    let inferred_transient = is_transient_role(&role)
        || contains_any(
            &format!("{} {}", automation_id, string(raw, "name")),
            &["tooltip", "context.menu", "popup", "modal"],
        );
    let hint = options.element_hints.get(&automation_id);
    let transient = hint
        .and_then(|hint| hint.transient)
        .unwrap_or(inferred_transient);
    let dynamic = hint
        .and_then(|hint| hint.dynamic)
        .unwrap_or(transient || !visible);
    let classification = classify(
        &role,
        &capabilities,
        &spatial_capabilities,
        !children.is_empty(),
        transient,
    );
    let safety = hint
        .and_then(|hint| hint.safety.clone())
        .unwrap_or_else(|| classify_safety(&automation_id, &string(raw, "name"), &classification));
    UiElement {
        automation_id,
        role,
        control_type,
        name: string(raw, "name"),
        visible,
        offscreen,
        enabled,
        interactable,
        selected: raw.get("selected").and_then(Value::as_bool),
        value: raw
            .get("value")
            .and_then(Value::as_str)
            .or_else(|| raw.get("text").and_then(Value::as_str))
            .map(str::to_owned),
        bounds: parse_bounds(raw.get("bounds")),
        parent,
        children,
        capabilities,
        spatial_capabilities,
        classification,
        safety,
        dynamic,
        transient,
        collection_family: None,
        diagnostics: ElementDiagnostics {
            element_id: raw
                .get("element_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            parent_element_id,
            hwnd: raw.get("hwnd").and_then(Value::as_str).map(str::to_owned),
        },
    }
}

fn semantic_capabilities(raw: &Value, interactable: bool) -> Vec<CapabilityState> {
    let Some(capabilities) = raw.get("capabilities").and_then(Value::as_object) else {
        return Vec::new();
    };
    [
        ("invoke", "invoke"),
        ("select", "select"),
        ("add_to_selection", "select.multiple"),
        ("remove_from_selection", "select.multiple"),
        ("select_range", "select.range"),
        ("value", "value"),
        ("expand_collapse", "expand_collapse"),
        ("native_action", "native_action"),
        ("synthesized_pointer", "pointer"),
    ]
    .into_iter()
    .map(|(field, name)| {
        let supported = capabilities
            .get(field)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        CapabilityState {
            name: name.into(),
            supported,
            available: supported && interactable,
        }
    })
    .filter(|capability| capability.supported)
    .collect()
}

fn spatial_capabilities_from_response(response: &Value) -> Vec<CapabilityState> {
    let result = response
        .pointer("/result/result")
        .or_else(|| response.get("result"))
        .unwrap_or(response);
    let available = result
        .get("interactable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    result
        .get("supported_spatial_actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|operation| CapabilityState {
            name: format!("spatial.{}", spatial_suffix(operation)),
            supported: true,
            available,
        })
        .collect()
}

fn spatial_suffix(operation: &str) -> &str {
    if matches!(operation, "capture_region" | "windows_capture_region") {
        return "capture";
    }
    let operation = operation.strip_prefix("windows_").unwrap_or(operation);
    operation.strip_suffix("_relative").unwrap_or(operation)
}

fn classify(
    role: &str,
    capabilities: &[CapabilityState],
    spatial: &[CapabilityState],
    has_children: bool,
    transient: bool,
) -> InteractiveClass {
    if transient {
        InteractiveClass::Transient
    } else if !spatial.is_empty() {
        InteractiveClass::Spatial
    } else if has_supported(capabilities, "value")
        && matches!(
            role.to_ascii_lowercase().as_str(),
            "edit" | "document" | "textbox" | "textinput" | "combobox"
        )
    {
        InteractiveClass::Editable
    } else if has_supported(capabilities, "select") {
        InteractiveClass::Selectable
    } else if has_supported(capabilities, "invoke") || has_supported(capabilities, "native_action")
    {
        InteractiveClass::Actionable
    } else if has_children {
        InteractiveClass::Container
    } else if matches!(
        role.to_ascii_lowercase().as_str(),
        "text" | "label" | "image" | "separator"
    ) {
        InteractiveClass::Informational
    } else {
        InteractiveClass::Unknown
    }
}

fn classify_safety(
    automation_id: &str,
    name: &str,
    classification: &InteractiveClass,
) -> SafetyClass {
    let signal = format!("{} {}", automation_id, name).to_ascii_lowercase();
    if contains_any(
        &signal,
        &[
            "delete",
            "remove",
            "destroy",
            "send",
            "purchase",
            "publish",
            "logout",
            "shutdown",
            "overwrite",
            "format",
            "crash",
            "runtime failure",
        ],
    ) {
        return SafetyClass::PotentiallyDestructive;
    }
    match classification {
        InteractiveClass::Informational | InteractiveClass::Container => SafetyClass::Safe,
        InteractiveClass::Editable | InteractiveClass::Selectable => {
            SafetyClass::StatefulRecoverable
        }
        InteractiveClass::Actionable
            if contains_any(
                &signal,
                &[
                    "open",
                    "close",
                    "show",
                    "create",
                    "enable",
                    "trigger",
                    "increment",
                    "decrement",
                ],
            ) =>
        {
            SafetyClass::StatefulRecoverable
        }
        InteractiveClass::Transient => SafetyClass::Unknown,
        InteractiveClass::Spatial | InteractiveClass::Actionable | InteractiveClass::Unknown => {
            SafetyClass::Unknown
        }
    }
}

fn audit_quality(elements: &[UiElement]) -> Vec<QualityIssue> {
    let mut issues = Vec::new();
    let mut counts = BTreeMap::<&str, usize>::new();
    for element in elements {
        if !element.automation_id.is_empty() {
            *counts.entry(&element.automation_id).or_default() += 1;
        }
    }
    for (id, count) in counts {
        if count > 1 {
            issues.push(issue(
                IssueSeverity::Error,
                "DUPLICATE_AUTOMATION_ID",
                Some(id),
                format!("{count} semantic elements use the same stable AutomationId"),
            ));
        }
    }
    for element in elements {
        if element.visible && !element.bounds.valid() {
            issues.push(issue(
                IssueSeverity::Error,
                "INVALID_BOUNDS",
                stable_id(element),
                "visible element has non-actionable bounds".into(),
            ));
        }
        if element.visible && element.is_interactive() && !element.interactable {
            issues.push(issue(
                IssueSeverity::Warning,
                "VISIBLE_NON_INTERACTABLE",
                stable_id(element),
                "visible interactive element is not currently interactable".into(),
            ));
        }
        if interactive_looking(&element.role) && element.capabilities.is_empty() {
            issues.push(issue(
                IssueSeverity::Warning,
                "INTERACTIVE_WITHOUT_CAPABILITY",
                stable_id(element),
                "interactive-looking node exposes no semantic action capability".into(),
            ));
        }
        if element.is_interactive() && element.automation_id.is_empty() {
            issues.push(issue(
                IssueSeverity::Error,
                "MISSING_AUTOMATION_ID",
                None,
                format!("{} control has no stable AutomationId", element.role),
            ));
        }
        if !element.visible && !element.bounds.valid() {
            issues.push(issue(
                IssueSeverity::Info,
                "HIDDEN_ZERO_BOUNDS",
                stable_id(element),
                "hidden element currently has zero bounds".into(),
            ));
        }
    }
    issues.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then(left.automation_id.cmp(&right.automation_id))
    });
    issues
}

fn detect_collections(elements: &mut [UiElement]) -> Vec<CollectionFamily> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for element in elements.iter() {
        if let Some(pattern) = repeated_pattern(&element.automation_id) {
            groups
                .entry(pattern)
                .or_default()
                .push(element.automation_id.clone());
        }
    }
    let mut families = Vec::new();
    for (pattern, mut members) in groups {
        if members.len() < 3 {
            continue;
        }
        members.sort();
        let middle = members.len() / 2;
        let mut representatives = vec![
            members[0].clone(),
            members[middle].clone(),
            members[members.len() - 1].clone(),
        ];
        representatives.dedup();
        for element in elements.iter_mut() {
            if members.binary_search(&element.automation_id).is_ok() {
                element.collection_family = Some(pattern.clone());
            }
        }
        families.push(CollectionFamily {
            pattern,
            members,
            representatives,
        });
    }
    families
}

fn repeated_pattern(id: &str) -> Option<String> {
    let digit_start = id
        .char_indices()
        .rev()
        .take_while(|(_, character)| character.is_ascii_digit())
        .last()
        .map(|(index, _)| index)?;
    (digit_start < id.len()).then(|| format!("{}*", &id[..digit_start]))
}

fn looks_spatial(raw: &&Value) -> bool {
    let role = raw
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| raw.get("control_type").and_then(Value::as_str))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let id = raw
        .get("automation_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(role.as_str(), "pane" | "document" | "custom" | "group")
        && contains_any(&id, &["canvas", "spatial", "viewport", "surface"])
}

fn successful_result<'a>(response: &'a Value, operation: &str) -> Result<&'a Value, String> {
    if response.get("success").and_then(Value::as_bool) == Some(true) {
        return response
            .get("result")
            .ok_or_else(|| format!("{operation} response has no result"));
    }
    let code = response
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("PUBLIC_API_ERROR");
    let message = response
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("operation failed");
    Err(format!("{operation}: {code}: {message}"))
}

fn parse_bounds(value: Option<&Value>) -> Bounds {
    let value = value.and_then(Value::as_object);
    Bounds {
        x: number(value, "x"),
        y: number(value, "y"),
        width: number(value, "width"),
        height: number(value, "height"),
    }
}

fn number(value: Option<&Map<String, Value>>, key: &str) -> i64 {
    value
        .and_then(|value| value.get(key))
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .and_then(|value| value.get(key))
                .and_then(Value::as_f64)
                .map(|value| value.round() as i64)
        })
        .unwrap_or_default()
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn bool_value(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn has_supported(capabilities: &[CapabilityState], name: &str) -> bool {
    capabilities
        .iter()
        .any(|capability| capability.name == name && capability.supported)
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    let value = value.to_ascii_lowercase();
    needles.iter().any(|needle| value.contains(needle))
}

fn is_transient_role(role: &str) -> bool {
    matches!(
        role.to_ascii_lowercase().as_str(),
        "tooltip" | "menu" | "menuitem" | "dialog"
    )
}

fn interactive_looking(role: &str) -> bool {
    matches!(
        role.to_ascii_lowercase().as_str(),
        "button" | "edit" | "checkbox" | "combobox" | "listitem" | "menuitem"
    )
}

fn stable_id(element: &UiElement) -> Option<&str> {
    (!element.automation_id.is_empty()).then_some(element.automation_id.as_str())
}

fn issue(
    severity: IssueSeverity,
    code: &str,
    automation_id: Option<&str>,
    message: String,
) -> QualityIssue {
    QualityIssue {
        severity,
        code: code.into(),
        automation_id: automation_id.map(str::to_owned),
        message,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiStateSnapshot {
    pub version: u32,
    pub visible_elements: Vec<String>,
    pub selected: BTreeMap<String, bool>,
    pub values: BTreeMap<String, String>,
}

pub fn state_snapshot(inventory: &UiInventory) -> UiStateSnapshot {
    let mut visible_elements = inventory
        .elements
        .iter()
        .filter(|element| element.visible)
        .map(|element| element.automation_id.clone())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    visible_elements.sort();
    let selected = inventory
        .elements
        .iter()
        .filter_map(|element| {
            element
                .selected
                .map(|selected| (element.automation_id.clone(), selected))
        })
        .collect();
    let values = inventory
        .elements
        .iter()
        .filter_map(|element| {
            element
                .value
                .clone()
                .map(|value| (element.automation_id.clone(), value))
        })
        .collect();
    UiStateSnapshot {
        version: 1,
        visible_elements,
        selected,
        values,
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityChange {
    pub automation_id: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryDiff {
    pub added_elements: Vec<String>,
    pub removed_elements: Vec<String>,
    pub changed_roles: Vec<String>,
    pub changed_capabilities: Vec<CapabilityChange>,
}

pub fn diff(before: &UiInventory, after: &UiInventory) -> InventoryDiff {
    let before_map = inventory_map(before);
    let after_map = inventory_map(after);
    let before_ids = before_map.keys().copied().collect::<BTreeSet<_>>();
    let after_ids = after_map.keys().copied().collect::<BTreeSet<_>>();
    let mut result = InventoryDiff {
        added_elements: after_ids
            .difference(&before_ids)
            .map(|id| (*id).to_owned())
            .collect(),
        removed_elements: before_ids
            .difference(&after_ids)
            .map(|id| (*id).to_owned())
            .collect(),
        ..Default::default()
    };
    for id in before_ids.intersection(&after_ids) {
        let before = before_map[id];
        let after = after_map[id];
        if before.role != after.role {
            result
                .changed_roles
                .push(format!("{id}: {} -> {}", before.role, after.role));
        }
        let before_caps = capability_names(before);
        let after_caps = capability_names(after);
        if before_caps != after_caps {
            result.changed_capabilities.push(CapabilityChange {
                automation_id: (*id).to_owned(),
                before: before_caps,
                after: after_caps,
            });
        }
    }
    result
}

fn inventory_map(inventory: &UiInventory) -> BTreeMap<&str, &UiElement> {
    inventory
        .elements
        .iter()
        .filter(|element| !element.automation_id.is_empty())
        .map(|element| (element.automation_id.as_str(), element))
        .collect()
}

fn capability_names(element: &UiElement) -> Vec<String> {
    let mut names = element
        .capabilities
        .iter()
        .chain(element.spatial_capabilities.iter())
        .filter(|capability| capability.supported)
        .map(|capability| capability.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names
}

pub fn diff_markdown(diff: &InventoryDiff) -> String {
    let mut lines = vec!["# UI change report".to_string(), String::new()];
    lines.extend(diff.added_elements.iter().map(|id| format!("+ {id}")));
    lines.extend(diff.removed_elements.iter().map(|id| format!("- {id}")));
    lines.extend(
        diff.changed_roles
            .iter()
            .map(|change| format!("~ {change}")),
    );
    lines.extend(diff.changed_capabilities.iter().map(|change| {
        format!(
            "~ {} capabilities: [{}] -> [{}]",
            change.automation_id,
            change.before.join(", "),
            change.after.join(", ")
        )
    }));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::InProcessTransport;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn control(id: &str, parent: Option<&str>, role: &str, invoke: bool, select: bool) -> Value {
        json!({
            "element_id":format!("e:{id}"),
            "parent_element_id":parent.map(|value|format!("e:{value}")),
            "automation_id":id,
            "control_type":role,
            "name":id,
            "bounds":{"x":10,"y":10,"width":100,"height":30},
            "visible":true,
            "enabled":true,
            "interactable":true,
            "selected":false,
            "capabilities":{
                "invoke":invoke,
                "select":select,
                "value":role == "Edit",
                "expand_collapse":false,
                "native_action":invoke,
                "synthesized_pointer":true
            }
        })
    }

    #[test]
    fn inventory_uses_only_public_transport_and_stable_relationships() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let observed = calls.clone();
        let transport = InProcessTransport::new(move |op: &str, args: Value| {
            observed.borrow_mut().push(op.to_owned());
            match op {
                "session.open" => {
                    json!({"success":true,"result":{"session_id":"s1","windows":[{"window_id":"w1","name":"Demo","automation_id":"window.main","bounds":{"x":0,"y":0,"width":800,"height":600},"visible":true}]}})
                }
                "inspect_ui" => json!({"success":true,"result":{"elements":[
                    control("window.main",None,"Window",false,false),
                    control("window.main/button.open",Some("window.main"),"Button",true,false),
                    control("window.main/canvas.main",Some("window.main"),"Pane",false,false)
                ]}}),
                "inspect_spatial_target" => {
                    assert_eq!(args["id"], "window.main/canvas.main");
                    json!({"success":true,"result":{"interactable":true,"supported_spatial_actions":["hover_relative","capture_region"]}})
                }
                "session.close" => json!({"success":true,"result":{"closed":true}}),
                _ => panic!("unexpected operation {op}"),
            }
        });
        let mut client = AiUiClient::new(transport);
        let inventory = collect(
            &mut client,
            "demo.exe",
            Some("Demo"),
            &InventoryOptions::default(),
        )
        .unwrap();
        assert!(inventory.session.closed);
        assert_eq!(
            calls.borrow().as_slice(),
            [
                "session.open",
                "inspect_ui",
                "inspect_spatial_target",
                "session.close"
            ]
        );
        let button = inventory
            .elements
            .iter()
            .find(|element| element.automation_id.ends_with("button.open"))
            .unwrap();
        assert_eq!(button.parent.as_deref(), Some("window.main"));
        assert_eq!(button.classification, InteractiveClass::Actionable);
        assert_eq!(button.safety, SafetyClass::StatefulRecoverable);
        let canvas = inventory
            .elements
            .iter()
            .find(|element| element.automation_id.ends_with("canvas.main"))
            .unwrap();
        assert_eq!(canvas.classification, InteractiveClass::Spatial);
        assert!(canvas.capability("spatial.hover").is_some());
        assert_eq!(inventory.markers, ["AI_UI_INVENTORY_PASSED"]);
    }

    #[test]
    fn nested_live_spatial_response_exports_capabilities() {
        let capabilities = spatial_capabilities_from_response(&json!({
            "success":true,
            "result":{
                "application_healthy":true,
                "result":{
                    "interactable":true,
                    "supported_spatial_actions":["capture_region","click_relative"]
                }
            }
        }));
        assert!(
            capabilities
                .iter()
                .any(|item| { item.name == "spatial.capture" && item.supported && item.available })
        );
        assert!(
            capabilities
                .iter()
                .any(|item| { item.name == "spatial.click" && item.supported && item.available })
        );
    }

    #[test]
    fn makepad_group_canvas_is_probed_for_spatial_capabilities() {
        let raw = json!({
            "automation_id":"window.main/body/canvas.main",
            "role":null,
            "control_type":"Group"
        });
        assert!(looks_spatial(&&raw));

        let ordinary_group = json!({
            "automation_id":"window.main/body/settings",
            "control_type":"Group"
        });
        assert!(!looks_spatial(&&ordinary_group));
    }

    #[test]
    fn duplicate_ids_and_repeated_items_are_reported_and_sampled() {
        let mut raw = vec![control("root", None, "Window", false, false)];
        for index in 0..5 {
            raw.push(control(
                &format!("root/item.{index}"),
                Some("root"),
                "ListItem",
                false,
                true,
            ));
        }
        raw.push(control(
            "root/item.0",
            Some("root"),
            "ListItem",
            false,
            true,
        ));
        let open = json!({"success":true,"result":{"session_id":"s","windows":[]}});
        let inspect = json!({"success":true,"result":{"elements":raw}});
        let inventory = from_public_responses(
            "demo",
            &open,
            &inspect,
            &BTreeMap::new(),
            &InventoryOptions::default(),
        )
        .unwrap();
        assert!(
            inventory
                .quality_issues
                .iter()
                .any(|issue| issue.code == "DUPLICATE_AUTOMATION_ID")
        );
        assert_eq!(inventory.collections.len(), 1);
        assert!(inventory.collections[0].representatives.len() <= 3);
    }

    #[test]
    fn inventory_diff_reports_stable_semantic_changes() {
        let open = json!({"success":true,"result":{"session_id":"s","windows":[]}});
        let before = from_public_responses(
            "demo",
            &open,
            &json!({"success":true,"result":{"elements":[control("a",None,"Button",true,false)]}}),
            &BTreeMap::new(),
            &InventoryOptions::default(),
        )
        .unwrap();
        let after = from_public_responses(
            "demo",
            &open,
            &json!({"success":true,"result":{"elements":[control("b",None,"Edit",false,false)]}}),
            &BTreeMap::new(),
            &InventoryOptions::default(),
        )
        .unwrap();
        let change = diff(&before, &after);
        assert_eq!(change.added_elements, ["b"]);
        assert_eq!(change.removed_elements, ["a"]);
    }
}
