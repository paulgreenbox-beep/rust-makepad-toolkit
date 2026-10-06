//! Automatic semantic projection of a Makepad runtime widget tree.
//!
//! This module uses the public `WidgetRef`/`CxWidgetExt` runtime APIs. It does
//! not ask Windows UI Automation to guess controls from pixels and it does not
//! require an application to publish one snapshot per widget.

use makepad_widgets::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use toolkit_windows_automation::{
    AccessibilityAction, AccessibilityActions, AccessibilityNode, AccessibilityRect,
    AccessibilityRole, AccessibilitySelection, AccessibilityTree,
};

#[cfg(windows)]
use windows_uia::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::ClientToScreen,
    UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::GetWindowRect},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeGeometry {
    pub client_origin_x: f64,
    pub client_origin_y: f64,
    pub dpi_factor: f64,
    pub window_bounds: AccessibilityRect,
}

#[cfg(windows)]
impl ProbeGeometry {
    pub fn for_window(hwnd: HWND) -> Result<Self, String> {
        let mut origin = POINT::default();
        unsafe { ClientToScreen(hwnd, &mut origin) }
            .ok()
            .map_err(|error| error.to_string())?;
        let mut window_rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut window_rect) }.map_err(|error| error.to_string())?;
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        Ok(Self {
            client_origin_x: origin.x as f64,
            client_origin_y: origin.y as f64,
            dpi_factor: dpi as f64 / 96.0,
            window_bounds: AccessibilityRect {
                x: window_rect.left as f64,
                y: window_rect.top as f64,
                width: (window_rect.right - window_rect.left).max(0) as f64,
                height: (window_rect.bottom - window_rect.top).max(0) as f64,
            },
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProbeConfig {
    /// Optional overrides keyed by a Makepad runtime path. Empty config uses
    /// stable path IDs derived from named children and WidgetUid fallbacks.
    pub semantic_ids: BTreeMap<String, String>,
    /// Stable semantic ID of the top-level Makepad window represented by this
    /// bridge. When more than one top-level window exists, callers must set
    /// this value instead of relying on widget-tree traversal order.
    pub target_window_id: Option<String>,
    /// Runtime-owned visibility overrides for overlay widgets whose drawing
    /// implementation keeps the Widget mounted while closed (Modal is one).
    /// The tree walk remains automatic; the app reports only state that cannot
    /// be read through the public Widget trait.
    pub visibility_overrides: BTreeMap<String, bool>,
    /// Runtime state overrides for composite widgets whose state is owned by
    /// the application container rather than by the child widget itself.
    pub selected_overrides: BTreeMap<String, bool>,
}

#[derive(Clone, Debug)]
pub struct ProbeSnapshot {
    pub tree: AccessibilityTree,
    pub widget_types: Vec<String>,
}

#[derive(Default)]
pub struct MakepadUiProbe {
    config: ProbeConfig,
    generation: u64,
    widgets: HashMap<String, WidgetRef>,
    widget_types: BTreeSet<String>,
}

impl MakepadUiProbe {
    pub fn new(config: ProbeConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    pub fn set_visibility_override(&mut self, node_id: impl Into<String>, visible: bool) {
        self.config
            .visibility_overrides
            .insert(node_id.into(), visible);
    }

    pub fn set_selected_override(&mut self, node_id: impl Into<String>, selected: bool) {
        self.config
            .selected_overrides
            .insert(node_id.into(), selected);
    }

    pub fn snapshot(
        &mut self,
        cx: &Cx,
        ui: &WidgetRef,
        geometry: ProbeGeometry,
    ) -> Result<ProbeSnapshot, String> {
        self.generation = self.generation.wrapping_add(1);
        self.widgets.clear();
        self.widget_types.clear();

        // Makepad's own WidgetTree is the runtime observation point. Its
        // snapshot supplies type inventory; the recursive WidgetRef walk keeps
        // the parent/children relationship needed by the accessibility bridge.
        for snapshot in cx.widget_tree().snapshot(cx) {
            if !snapshot.widget_type.is_empty() && snapshot.widget_type != "-" {
                self.widget_types.insert(snapshot.widget_type);
            }
        }

        let runtime_root =
            self.build_node(cx, ui, "runtime.root", "runtime.root", None, true, geometry);
        let mut root =
            match select_snapshot_root(&runtime_root, self.config.target_window_id.as_deref()) {
                Ok(root) => root,
                Err(error) => {
                    self.widgets.clear();
                    return Err(error);
                }
            };
        let selected_window_id = (root.role == AccessibilityRole::Window).then(|| root.id.clone());
        if root.role != AccessibilityRole::Window {
            root.id = "window.main".to_owned();
            root.name = "Makepad application".to_owned();
            root.role = AccessibilityRole::Window;
        }
        root.bounds = geometry.window_bounds;

        if let Some(selected_window_id) = selected_window_id {
            let prefix = format!("{selected_window_id}/");
            self.widgets
                .retain(|id, _| id == &selected_window_id || id.starts_with(&prefix));
        }

        let tree = AccessibilityTree::new(root)
            .map(|tree| tree.with_generation(self.generation))
            .map_err(|error| error.to_string())?;

        for widget_type in self.widgets.values().map(widget_type_name) {
            self.widget_types.insert(widget_type.to_owned());
        }
        Ok(ProbeSnapshot {
            tree,
            widget_types: self.widget_types.iter().cloned().collect(),
        })
    }

    pub fn dispatch_action(
        &mut self,
        cx: &mut Cx,
        node_id: &str,
        action: &AccessibilityAction,
    ) -> Result<(), String> {
        let widget = self
            .widgets
            .get(node_id)
            .cloned()
            .ok_or_else(|| format!("automatic Probe has no live widget for {node_id}"))?;
        if widget.disabled(cx) && !matches!(action, AccessibilityAction::Focus) {
            return Err(format!("{node_id} is disabled"));
        }
        match action {
            AccessibilityAction::Focus => {
                widget.set_key_focus(cx);
                Ok(())
            }
            AccessibilityAction::SetValue(value) => {
                if widget.borrow::<TextInput>().is_some() {
                    widget.set_text(cx, value);
                    return Ok(());
                }
                if widget.borrow::<DropDown>().is_some() {
                    if widget.borrow::<DropDown>().is_some() {
                        widget.as_drop_down().set_selected_by_label(value, cx);
                        return Ok(());
                    }
                }
                Err(format!(
                    "{node_id} does not expose a settable TextInput or DropDown"
                ))
            }
            AccessibilityAction::Invoke => self.invoke(cx, node_id, &widget),
            AccessibilityAction::Select
            | AccessibilityAction::AddToSelection
            | AccessibilityAction::RemoveFromSelection => self.select(cx, node_id, &widget, action),
        }
    }

    fn invoke(&mut self, cx: &mut Cx, node_id: &str, widget: &WidgetRef) -> Result<(), String> {
        let uid = widget
            .try_widget_uid()
            .ok_or_else(|| format!("{node_id} is currently borrowed or unavailable"))?;
        if let Some(check_box) = widget.borrow::<CheckBox>() {
            let next = !check_box.active(cx);
            drop(check_box);
            if let Some(mut check_box) = widget.borrow_mut::<CheckBox>() {
                check_box.set_active(cx, next, Animate::No);
            }
            cx.widget_action(uid, CheckBoxAction::Change(next));
            return Ok(());
        }
        if let Some(radio) = widget.borrow::<RadioButton>() {
            let already_active = radio.active(cx);
            drop(radio);
            if !already_active {
                if let Some(mut radio) = widget.borrow_mut::<RadioButton>() {
                    radio.set_active(cx, true, Animate::No);
                }
                cx.widget_action(uid, RadioButtonAction::Clicked);
            }
            return Ok(());
        }
        if widget.borrow::<Button>().is_some() {
            cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default()));
            let _ = cx.with_vm(|vm| widget.script_call(vm, live_id!(on_click), NIL));
            return Ok(());
        }
        if widget.borrow::<LinkLabel>().is_some() {
            let result = cx.with_vm(|vm| widget.script_call(vm, live_id!(on_click), NIL));
            if matches!(result, ScriptAsyncResult::MethodNotFound) {
                return Err(format!("{node_id} has no invokable Makepad callback"));
            }
            return Ok(());
        }
        Err(format!(
            "automatic Probe cannot invoke Makepad widget type {} at {node_id}",
            widget_type_name(widget)
        ))
    }

    fn select(
        &mut self,
        cx: &mut Cx,
        node_id: &str,
        widget: &WidgetRef,
        action: &AccessibilityAction,
    ) -> Result<(), String> {
        if let Some(check_box) = widget.borrow::<CheckBox>() {
            let active = check_box.active(cx);
            drop(check_box);
            let value = !matches!(action, AccessibilityAction::RemoveFromSelection);
            if active != value {
                if let Some(mut check_box) = widget.borrow_mut::<CheckBox>() {
                    check_box.set_active(cx, value, Animate::No);
                }
                cx.widget_action(widget.widget_uid(), CheckBoxAction::Change(value));
            }
            return Ok(());
        }
        Err(format!(
            "automatic Probe cannot select Makepad widget type {} at {node_id}",
            widget_type_name(widget)
        ))
    }

    fn build_node(
        &mut self,
        cx: &Cx,
        widget: &WidgetRef,
        raw_path: &str,
        semantic_id: &str,
        parent_role: Option<AccessibilityRole>,
        ancestor_visible: bool,
        geometry: ProbeGeometry,
    ) -> AccessibilityNode {
        let role = role_for(widget, raw_path, parent_role);
        self.widget_types
            .insert(widget_type_name(widget).to_owned());
        let widget_text = widget.text();
        let name = accessible_name(role, semantic_id, &widget_text);
        let value = if role == AccessibilityRole::ComboBox {
            widget.as_drop_down().selected_label().into()
        } else {
            matches!(role, AccessibilityRole::Edit | AccessibilityRole::Text)
                .then_some(widget_text.clone())
                .filter(|value| !value.is_empty())
        };
        let checked = widget
            .borrow::<CheckBox>()
            .map(|check_box| check_box.active(cx))
            .or_else(|| widget.borrow::<RadioButton>().map(|radio| radio.active(cx)));
        let selected = self
            .config
            .selected_overrides
            .get(semantic_id)
            .copied()
            .or(checked);
        let area = widget.area();
        let has_rect = area.is_valid(cx);
        let clipped_rect = area.clipped_rect(cx);
        let has_clipped_rect = clipped_rect.size.x > 0.0 && clipped_rect.size.y > 0.0;
        let bounds = if role == AccessibilityRole::Window {
            geometry.window_bounds
        } else if has_rect {
            let rect = area.rect(cx);
            AccessibilityRect {
                x: geometry.client_origin_x + rect.pos.x * geometry.dpi_factor,
                y: geometry.client_origin_y + rect.pos.y * geometry.dpi_factor,
                width: rect.size.x.max(0.0) * geometry.dpi_factor,
                height: rect.size.y.max(0.0) * geometry.dpi_factor,
            }
        } else {
            AccessibilityRect::default()
        };
        let clipping_matters =
            role == AccessibilityRole::List || parent_role == Some(AccessibilityRole::List);
        let base_visible = ancestor_visible
            && widget.visible()
            && (role == AccessibilityRole::Window
                || (has_rect
                    && bounds.width > 0.0
                    && bounds.height > 0.0
                    && (!clipping_matters || has_clipped_rect)));
        let visible = self
            .config
            .visibility_overrides
            .get(semantic_id)
            .copied()
            .unwrap_or(base_visible);
        let mut node = AccessibilityNode::new(semantic_id, role, name, bounds);
        node.value = value;
        node.visible = visible;
        node.enabled = !widget.disabled(cx);
        node.focused = has_rect && !area.is_empty() && widget.key_focus(cx);
        // The generic bridge currently exposes UIA SelectionItem state. For a
        // CheckBox this is the portable checked-state carrier until the bridge
        // grows a dedicated TogglePattern.
        node.selected = selected;
        node.actions = actions_for(role);
        if matches!(role, AccessibilityRole::List | AccessibilityRole::ComboBox) {
            node.selection = Some(AccessibilitySelection {
                can_select_multiple: role == AccessibilityRole::List,
                is_selection_required: false,
            });
        }
        self.widgets.insert(semantic_id.to_owned(), widget.clone());

        let mut children = Vec::new();
        widget.try_children(&mut |child_name, child| {
            if child.is_empty() {
                return;
            }
            let raw_segment = live_id_token(child_name, child.widget_uid());
            let child_raw_path = if raw_path == "runtime.root" {
                raw_segment.clone()
            } else {
                format!("{raw_path}/{raw_segment}")
            };
            let child_semantic_id = self.semantic_id(&child_raw_path, semantic_id, &raw_segment);
            children.push((child_raw_path, child_semantic_id, child));
        });
        for (child_raw_path, child_semantic_id, child) in children {
            node.children.push(self.build_node(
                cx,
                &child,
                &child_raw_path,
                &child_semantic_id,
                Some(role),
                visible,
                geometry,
            ));
        }
        node
    }

    fn semantic_id(&self, raw_path: &str, parent_id: &str, raw_segment: &str) -> String {
        if let Some(id) = self.config.semantic_ids.get(raw_path) {
            return id.clone();
        }
        let segment = semantic_segment(raw_segment);
        if raw_path == "main_window" || segment == "window.main" {
            return "window.main".to_owned();
        }
        if parent_id == "runtime.root" {
            segment
        } else {
            format!("{parent_id}/{segment}")
        }
    }
}

fn select_snapshot_root(
    runtime_root: &AccessibilityNode,
    target_window_id: Option<&str>,
) -> Result<AccessibilityNode, String> {
    let windows = runtime_root
        .children
        .iter()
        .filter(|child| child.role == AccessibilityRole::Window)
        .collect::<Vec<_>>();
    match target_window_id {
        Some(target) => windows
            .iter()
            .find(|child| child.id == target)
            .cloned()
            .cloned()
            .ok_or_else(|| {
                let available = windows
                    .iter()
                    .map(|child| child.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "automatic Probe could not find configured target_window_id {target:?}; available top-level windows: [{available}]"
                )
            }),
        None if windows.len() <= 1 => Ok(windows
            .first()
            .map(|node| (*node).clone())
            .unwrap_or_else(|| runtime_root.clone())),
        None => {
            let available = windows
                .iter()
                .map(|child| child.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "automatic Probe found multiple top-level windows [{available}]; configure target_window_id explicitly"
            ))
        }
    }
}

fn role_for(
    widget: &WidgetRef,
    raw_path: &str,
    parent_role: Option<AccessibilityRole>,
) -> AccessibilityRole {
    if widget.borrow::<Window>().is_some() {
        return AccessibilityRole::Window;
    }
    if widget.borrow::<Modal>().is_some() {
        return AccessibilityRole::Pane;
    }
    if widget.borrow::<Button>().is_some() {
        return if matches!(parent_role, Some(AccessibilityRole::List)) {
            AccessibilityRole::ListItem
        } else if matches!(
            parent_role,
            Some(AccessibilityRole::Menu) | Some(AccessibilityRole::MenuBar)
        ) {
            AccessibilityRole::MenuItem
        } else {
            AccessibilityRole::Button
        };
    }
    if widget.borrow::<TextInput>().is_some() {
        return AccessibilityRole::Edit;
    }
    if widget.borrow::<CheckBox>().is_some() {
        return AccessibilityRole::CheckBox;
    }
    if widget.borrow::<RadioButton>().is_some() {
        return AccessibilityRole::RadioButton;
    }
    if widget.borrow::<DropDown>().is_some() {
        return AccessibilityRole::ComboBox;
    }
    if widget.borrow::<Slider>().is_some() {
        return AccessibilityRole::Slider;
    }
    if widget.borrow::<LinkLabel>().is_some() {
        return AccessibilityRole::Hyperlink;
    }
    if widget.borrow::<PortalList>().is_some() || widget.borrow::<FlatList>().is_some() {
        return AccessibilityRole::List;
    }
    let lower = raw_path.to_ascii_lowercase();
    if lower.contains("menu") {
        return if lower.contains("item") {
            AccessibilityRole::MenuItem
        } else {
            AccessibilityRole::Menu
        };
    }
    if lower.contains("list") {
        return AccessibilityRole::List;
    }
    if widget.borrow::<Label>().is_some() {
        AccessibilityRole::Text
    } else if parent_role == Some(AccessibilityRole::List) {
        AccessibilityRole::ListItem
    } else {
        AccessibilityRole::Group
    }
}

fn actions_for(role: AccessibilityRole) -> AccessibilityActions {
    let mut actions = AccessibilityActions::default();
    match role {
        AccessibilityRole::Edit => {
            actions.focus = true;
            actions.set_value = true;
        }
        AccessibilityRole::Button
        | AccessibilityRole::ListItem
        | AccessibilityRole::MenuItem
        | AccessibilityRole::CheckBox
        | AccessibilityRole::RadioButton
        | AccessibilityRole::Hyperlink => {
            actions.invoke = true;
            actions.focus = true;
        }
        AccessibilityRole::ComboBox => {
            actions.select = true;
            actions.focus = true;
            actions.set_value = true;
        }
        _ => {}
    }
    actions
}

fn widget_type_name(widget: &WidgetRef) -> &'static str {
    if widget.borrow::<Window>().is_some() {
        "Window"
    } else if widget.borrow::<Modal>().is_some() {
        "Modal"
    } else if widget.borrow::<Button>().is_some() {
        "Button"
    } else if widget.borrow::<TextInput>().is_some() {
        "TextInput"
    } else if widget.borrow::<CheckBox>().is_some() {
        "CheckBox"
    } else if widget.borrow::<RadioButton>().is_some() {
        "RadioButton"
    } else if widget.borrow::<DropDown>().is_some() {
        "DropDown"
    } else if widget.borrow::<Slider>().is_some() {
        "Slider"
    } else if widget.borrow::<LinkLabel>().is_some() {
        "LinkLabel"
    } else if widget.borrow::<PortalList>().is_some() {
        "PortalList"
    } else if widget.borrow::<FlatList>().is_some() {
        "FlatList"
    } else if widget.borrow::<Label>().is_some() {
        "Label"
    } else if widget.borrow::<View>().is_some() {
        "View"
    } else {
        "Custom"
    }
}

fn accessible_name(role: AccessibilityRole, id: &str, text: &str) -> String {
    if matches!(
        role,
        AccessibilityRole::Button
            | AccessibilityRole::ListItem
            | AccessibilityRole::CheckBox
            | AccessibilityRole::RadioButton
            | AccessibilityRole::Hyperlink
    ) && !text.trim().is_empty()
    {
        return text.to_owned();
    }
    id.rsplit('/').next().unwrap_or(id).replace('.', " ")
}

fn live_id_token(id: LiveId, uid: WidgetUid) -> String {
    if id == LiveId(0) {
        return format!("runtime.{:x}", uid.0);
    }
    id.as_string(|name| {
        name.map(str::to_owned)
            .unwrap_or_else(|| format!("runtime.{:x}", uid.0))
    })
}

fn semantic_segment(raw: &str) -> String {
    if raw == "main_window" || raw == "window" {
        return "window.main".to_owned();
    }
    raw.split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_segments_do_not_use_coordinates() {
        assert_eq!(semantic_segment("main_window"), "window.main");
        assert_eq!(semantic_segment("button_create"), "button.create");
        assert_eq!(semantic_segment("item_3"), "item.3");
    }

    #[test]
    fn multiple_top_level_windows_require_an_explicit_target() {
        let mut runtime = AccessibilityNode::new(
            "runtime.root",
            AccessibilityRole::Pane,
            "runtime",
            AccessibilityRect::default(),
        );
        runtime.children = vec![
            AccessibilityNode::new(
                "window.main",
                AccessibilityRole::Window,
                "main",
                AccessibilityRect::default(),
            ),
            AccessibilityNode::new(
                "aux.window",
                AccessibilityRole::Window,
                "auxiliary",
                AccessibilityRect::default(),
            ),
        ];

        let error = select_snapshot_root(&runtime, None).unwrap_err();
        assert!(error.contains("target_window_id"));
        assert_eq!(
            select_snapshot_root(&runtime, Some("window.main"))
                .unwrap()
                .id,
            "window.main"
        );
    }
}
