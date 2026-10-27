//! API adapters to the same native workspace/controller helpers as the UI.
//! Authentication is enforced by `drawing_api` before entering this dispatcher.
//! These routes never introduce another document, history, server or file path.
use crate::App;
use crate::drafting_canvas::{CanvasInput, PointerButton, PointerPhase};
use crate::drafting_controller::Tool;
use ai_revit_core::{DraftEntityId, DrawingId, ElementId};
use drafting_core::{Camera2D, Point2};
use makepad_widgets::Cx;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Only these observations are exempt from the caller's API-token check.
/// In particular, picking changes transient selection and is not read-only.
pub(super) fn is_read_only(method: &str) -> bool {
    matches!(
        method,
        "drawing.view.state" | "drawing.ui.state" | "drawing.performance"
    )
}

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        "workspace.set"
            | "project.workspace.set"
            | "drawing.activate"
            | "drawing.select"
            | "drawing.view.state"
            | "drawing.ui.state"
            | "drawing.performance"
            | "drawing.camera.set"
            | "drawing.camera.pan"
            | "drawing.camera.zoom"
            | "drawing.camera.fit"
            | "drawing.view.camera"
            | "drawing.ui.fit"
            | "drawing.ui.tool"
            | "drawing.ui.numeric"
            | "drawing.ui.native_input"
            | "drawing.ui.pointer"
            | "drawing.ui.scroll"
            | "drawing.ui.key"
            | "drawing.ui.confirm"
            | "drawing.ui.cancel"
            | "drawing.ui.escape"
            | "drawing.ui.delete"
            | "drawing.selection.set"
            | "drawing.entity.pick"
    )
}

pub(super) fn dispatch(
    app: &mut App,
    cx: &mut Cx,
    method: &str,
    params: &Value,
) -> Option<Result<Value, String>> {
    if !handles(method) {
        return None;
    }
    Some((|| {
        if !params.is_object() {
            return Err("params must be a JSON object".into());
        }
        if method == "drawing.performance" {
            // No refresh, draw, query polling, export materialization or cache repair.
            return Ok(app.drawing_performance());
        }
        if matches!(method, "drawing.view.state" | "drawing.ui.state") {
            validate_context(app, cx, params, true)?;
            return read_ui_state(app, cx);
        }
        validate_context(
            app,
            cx,
            params,
            !matches!(
                method,
                "workspace.set" | "project.workspace.set" | "drawing.activate" | "drawing.select"
            ),
        )?;
        if matches!(
            method,
            "drawing.ui.tool"
                | "drawing.ui.numeric"
                | "drawing.ui.native_input"
                | "drawing.ui.pointer"
                | "drawing.ui.scroll"
                | "drawing.ui.key"
                | "drawing.ui.confirm"
                | "drawing.ui.cancel"
                | "drawing.ui.escape"
                | "drawing.ui.delete"
                | "drawing.entity.pick"
        ) && !app.drafting_ui.active_2d
        {
            return Err(
                "2D workspace is inactive; use workspace.set or drawing.activate first".into(),
            );
        }
        match method {
            "workspace.set" | "project.workspace.set" => {
                let workspace = text_alias(params, &["workspace", "active_workspace", "mode"], 32)?;
                if !matches!(
                    workspace.to_ascii_lowercase().as_str(),
                    "2d" | "drafting" | "3d" | "bim"
                ) {
                    return Err("workspace must be 2d or 3d".into());
                }
                shared_result(app.set_workspace(cx, workspace))
            }
            "drawing.activate" | "drawing.select" => {
                let id = DrawingId::new(decimal_alias(params, &["drawing_id", "id"])?);
                shared_result(app.activate_drawing(cx, id))
            }
            "drawing.camera.set" => {
                let proposed = parse_camera_set(&app.drafting_ui.camera, params)?;
                shared_result(app.drawing_set_camera(cx, proposed))
            }
            "drawing.camera.pan" => camera_pan(app, cx, params),
            "drawing.camera.zoom" => camera_zoom(app, cx, params),
            "drawing.camera.fit" | "drawing.ui.fit" => camera_fit(app, cx, params),
            "drawing.view.camera" => match text(params, "action", 32)? {
                "pan" => camera_pan(app, cx, params),
                "zoom" => camera_zoom(app, cx, params),
                "fit" => camera_fit(app, cx, params),
                "set" => {
                    let proposed = parse_camera_set(&app.drafting_ui.camera, params)?;
                    shared_result(app.drawing_set_camera(cx, proposed))
                }
                "resize" => {
                    let mut proposed = app.drafting_ui.camera.clone();
                    proposed
                        .resize(number(params, "width")?, number(params, "height")?)
                        .map_err(|e| e.to_string())?;
                    shared_result(app.drawing_set_camera(cx, proposed))
                }
                _ => Err("camera action must be set, pan, zoom, fit or resize".into()),
            },
            "drawing.ui.tool" => {
                let name = text_alias(params, &["tool", "name"], 32)?;
                let tool = Tool::parse(name)?;
                shared_result(app.drawing_tool(cx, tool.name()))
            }
            "drawing.ui.numeric" => {
                let input = text(params, "text", 4096)?;
                validate_numeric_input(input)?;
                shared_result(app.drawing_numeric(cx, input))
            }
            "drawing.ui.native_input"
            | "drawing.ui.pointer"
            | "drawing.ui.scroll"
            | "drawing.ui.key"
            | "drawing.ui.confirm"
            | "drawing.ui.cancel"
            | "drawing.ui.escape"
            | "drawing.ui.delete" => {
                let input = parse_native_input(method, params)?;
                // This is the actual Canvas input handler, distinct from a physical event.
                app.drawing_native_input(cx, input, "native_handler")
            }
            "drawing.selection.set" => set_selection(app, cx, params),
            "drawing.entity.pick" => {
                let screen = screen_point(params)?;
                // Explicit picking enters SELECT; it must not add a LINE construction point.
                app.drawing_tool(cx, "select")?;
                app.drawing_native_input(
                    cx,
                    CanvasInput::Pointer {
                        phase: PointerPhase::Down,
                        button: PointerButton::Left,
                        screen,
                    },
                    "native_handler",
                )
            }
            _ => unreachable!("known read routes returned above"),
        }
    })())
}

fn shared_result(result: Result<Value, String>) -> Result<Value, String> {
    result.map(|mut value| {
        if let Some(object) = value.as_object_mut() {
            object.insert("origin".into(), json!("api_shared_controller"));
        } else {
            value = json!({"origin":"api_shared_controller","result":value});
        }
        value
    })
}

fn read_ui_state(app: &App, cx: &Cx) -> Result<Value, String> {
    let mut state = app.drawing_ui_state(cx)?;
    let token = state
        .pointer("/draw/snapshot_token")
        .cloned()
        .unwrap_or(Value::Null);
    if let Some(object) = state.as_object_mut() {
        object.insert("snapshot_token".into(), token);
        object.insert(
            "document_revision".into(),
            app.document
                .as_ref()
                .map(|d| json!(d.revision()))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "source".into(),
            app.drafting_ui
                .frame
                .as_ref()
                .map(|frame| json!(frame.source))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "prepared_snapshot_token".into(),
            app.drafting_ui
                .frame
                .as_ref()
                .map(|frame| json!(frame.token))
                .unwrap_or(Value::Null),
        );
    }
    Ok(state)
}

fn camera_pan(app: &mut App, cx: &mut Cx, params: &Value) -> Result<Value, String> {
    let (dx, dy) = (number(params, "dx")?, number(params, "dy")?);
    // Validate the complete proposed camera before the helper records a native action.
    let mut proposed = app.drafting_ui.camera.clone();
    proposed.pan_pixels(dx, dy).map_err(|e| e.to_string())?;
    shared_result(app.drawing_pan(cx, dx, dy))
}
fn camera_zoom(app: &mut App, cx: &mut Cx, params: &Value) -> Result<Value, String> {
    let screen = screen_point(params)?;
    let factor = number(params, "factor")?;
    let mut proposed = app.drafting_ui.camera.clone();
    proposed
        .zoom_at(screen, factor)
        .map_err(|e| e.to_string())?;
    shared_result(app.drawing_zoom(cx, screen, factor))
}
fn camera_fit(app: &mut App, cx: &mut Cx, params: &Value) -> Result<Value, String> {
    if let Some(value) = params.get("padding") {
        if finite_value(value, "padding")? != 24.0 {
            return Err(
                "native fit uses 24 logical pixels of padding; omit padding or supply 24".into(),
            );
        }
    }
    shared_result(app.drawing_fit(cx))
}

fn parse_camera_set(current: &Camera2D, params: &Value) -> Result<Camera2D, String> {
    let source = params.get("camera").unwrap_or(params);
    if !source.is_object() {
        return Err("camera must be an object".into());
    }
    let mut proposed = current.clone();
    let mut changed = false;
    if let Some(center) = source.get("center") {
        if !center.is_object() {
            return Err("camera center must be an object with finite x and y".into());
        }
        proposed.center = Point2::new(number(center, "x")?, number(center, "y")?);
        changed = true;
    }
    for (key, target) in [
        ("center_x", &mut proposed.center.x),
        ("center_y", &mut proposed.center.y),
        ("pixels_per_metre", &mut proposed.pixels_per_metre),
        ("viewport_width", &mut proposed.viewport_width),
        ("viewport_height", &mut proposed.viewport_height),
    ] {
        if let Some(value) = source.get(key) {
            *target = finite_value(value, key)?;
            changed = true;
        }
    }
    if !changed {
        return Err("camera set requires center, center_x/center_y, pixels_per_metre or viewport dimensions".into());
    }
    if let Some(revision) = source.get("revision") {
        revision
            .as_u64()
            .ok_or("camera revision, when present, must be an unsigned integer")?;
    }
    proposed.validate().map_err(|e| e.to_string())?;
    current
        .revision
        .checked_add(1)
        .ok_or("camera revision exhausted")?;
    Ok(proposed)
}

/// Optional guards bind a request to the observed native frame without forcing a draw.
fn validate_context(app: &App, cx: &Cx, params: &Value, bind_drawing: bool) -> Result<(), String> {
    if let Some(expected) = params.get("expected_revision") {
        let expected = expected
            .as_u64()
            .ok_or("expected_revision must be an unsigned integer")?;
        let actual = app
            .document
            .as_ref()
            .ok_or("no document is open")?
            .revision();
        if actual != expected {
            return Err(format!(
                "stale_ui_context: expected project revision {expected}, current {actual}"
            ));
        }
    }
    if let Some(expected) = params.get("document_epoch") {
        let expected = expected
            .as_u64()
            .ok_or("document_epoch must be an unsigned integer")?;
        if expected != app.api_document_generation {
            return Err("stale_ui_context: document epoch changed".into());
        }
    }
    if bind_drawing {
        if let Some(value) = params.get("drawing_id") {
            let drawing = DrawingId::new(decimal(value, "drawing_id")?);
            if app.drafting_current_drawing_id() != Some(drawing) {
                return Err(
                    "stale_ui_context: requested drawing is not active; activate it explicitly"
                        .into(),
                );
            }
        }
    }
    if let Some(token) = params.get("snapshot_token") {
        let token = token
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("snapshot_token must be a non-empty string")?;
        app.validate_drawing_snapshot(cx, token)?;
    }
    Ok(())
}

fn parse_native_input(method: &str, params: &Value) -> Result<CanvasInput, String> {
    let (kind, source) = match method {
        "drawing.ui.pointer" => ("pointer", params),
        "drawing.ui.scroll" => ("scroll", params),
        "drawing.ui.key" => ("key", params),
        "drawing.ui.confirm" => ("confirm", params),
        "drawing.ui.cancel" | "drawing.ui.escape" => ("cancel", params),
        "drawing.ui.delete" => ("delete", params),
        "drawing.ui.native_input" => {
            let source = params.get("input").unwrap_or(params);
            if !source.is_object() {
                return Err("input must be an object".into());
            }
            (text_alias(source, &["type", "kind"], 32)?, source)
        }
        _ => return Err("unsupported native input route".into()),
    };
    match kind {
        "pointer" => {
            let phase = match optional_text(source, "phase", "up")? {
                "down" => PointerPhase::Down,
                "move" => PointerPhase::Move,
                "up" => PointerPhase::Up,
                "hover" => PointerPhase::Hover,
                "leave" => PointerPhase::Leave,
                _ => return Err("pointer phase must be down, move, up, hover or leave".into()),
            };
            let button = match optional_text(source, "button", "left")? {
                "left" => PointerButton::Left,
                "middle" => PointerButton::Middle,
                "right" => PointerButton::Right,
                _ => return Err("pointer button must be left, middle or right".into()),
            };
            Ok(CanvasInput::Pointer {
                phase,
                button,
                screen: screen_point(source)?,
            })
        }
        "scroll" => Ok(CanvasInput::Scroll {
            screen: screen_point(source)?,
            delta_y: number(source, "delta_y")?,
        }),
        "confirm" => Ok(CanvasInput::Confirm),
        "cancel" | "escape" => Ok(CanvasInput::Cancel),
        "delete" => Ok(CanvasInput::Delete),
        "key" => {
            let key = text_alias(source, &["key", "key_code", "code"], 64)?.to_ascii_lowercase();
            match key.as_str(){
                "enter"|"return"|"returnkey"=>Ok(CanvasInput::Confirm),
                "escape"|"esc"=>Ok(CanvasInput::Cancel),
                "delete"|"backspace"=>Ok(CanvasInput::Delete),
                _=>Err("native controller keys are Enter, Escape, Delete and Backspace; use numeric for text".into()),
            }
        }
        _ => Err("input type must be pointer, scroll, key, confirm, cancel or delete".into()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum QualifiedSelection {
    Draft {
        drawing: DrawingId,
        entity: DraftEntityId,
    },
    Bim(ElementId),
}

fn parse_selection(params: &Value) -> Result<Vec<QualifiedSelection>, String> {
    let value=params.get("selection").or_else(||params.get("objects")).or_else(||params.get("object"))
        .ok_or("selection requires qualified object references, for example {selection:[{domain:'drafting',drawing_id:'1',entity_id:'2'}]}")?;
    let values = if let Some(array) = value.as_array() {
        array.iter().collect::<Vec<_>>()
    } else {
        vec![value]
    };
    if values.len() > drafting_core::MAX_DOCUMENT_ENTITIES {
        return Err("selection exceeds the entity budget".into());
    }
    let mut unique = BTreeSet::new();
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let object = value
            .as_object()
            .ok_or("each selected object must be domain-qualified")?;
        let reference = match text(value, "domain", 32)? {
            "drafting" => {
                if object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "domain" | "drawing_id" | "entity_id"))
                {
                    return Err(
                        "drafting reference permits only domain, drawing_id and entity_id".into(),
                    );
                }
                QualifiedSelection::Draft {
                    drawing: DrawingId::new(decimal_field(value, "drawing_id")?),
                    entity: DraftEntityId::new(decimal_field(value, "entity_id")?),
                }
            }
            "bim" => {
                if object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "domain" | "element_id"))
                {
                    return Err("BIM reference permits only domain and element_id".into());
                }
                let id = value.get("element_id").ok_or("element_id is required")?;
                let id = if id.is_string() {
                    decimal(id, "element_id")?
                } else {
                    id.as_u64().filter(|id|*id>0).ok_or("BIM element_id must be a positive unsigned integer or canonical decimal string")?
                };
                QualifiedSelection::Bim(ElementId::new(id))
            }
            _ => return Err("selection domain must be bim or drafting".into()),
        };
        if !unique.insert(reference) {
            return Err("selection contains a duplicate qualified ID".into());
        }
        result.push(reference);
    }
    Ok(result)
}

fn set_selection(app: &mut App, cx: &mut Cx, params: &Value) -> Result<Value, String> {
    let selection = parse_selection(params)?;
    let document = app.document.as_ref().ok_or("no document is open")?;
    let mut drawing = None;
    let mut entities = Vec::new();
    let mut bim = None;
    for reference in selection {
        match reference {
            QualifiedSelection::Draft {
                drawing: id,
                entity,
            } => {
                if drawing.is_some_and(|other| other != id) {
                    return Err(
                        "one selection may contain drafting entities from one drawing".into(),
                    );
                }
                if document.draft_entity(id, entity).is_none() {
                    return Err(format!("drafting object {id}/{entity} does not exist"));
                }
                drawing = Some(id);
                entities.push(entity);
            }
            QualifiedSelection::Bim(id) => {
                if bim.is_some() {
                    return Err("native inspector supports one BIM source at a time".into());
                }
                if document.element(id).is_none() {
                    return Err(format!("BIM element {} does not exist", id.get()));
                }
                bim = Some(id);
            }
        }
    }
    if bim.is_some() && !entities.is_empty() {
        return Err("native selection supports one BIM source or manual entities in one drawing; mixed-domain multiselect is unsupported".into());
    }
    // All references are validated before changing workspace or selection.
    if let Some(id) = bim {
        app.select_element(cx, id);
        app.refresh_drafting(cx, true);
        return shared_result(Ok(
            json!({"selection":[{"domain":"bim","element_id":id.get()}],"document_mutated":false}),
        ));
    }
    if let Some(drawing) = drawing {
        app.activate_drawing(cx, drawing)?;
    }
    shared_result(app.drawing_select_ids(cx, entities))
}

fn screen_point(params: &Value) -> Result<Point2, String> {
    let source = params.get("screen").unwrap_or(params);
    if !source.is_object() {
        return Err("screen must be an object with finite x and y".into());
    }
    Ok(Point2::new(number(source, "x")?, number(source, "y")?))
}
fn number(params: &Value, key: &str) -> Result<f64, String> {
    finite_value(
        params
            .get(key)
            .ok_or_else(|| format!("{key} is required"))?,
        key,
    )
}
fn finite_value(value: &Value, key: &str) -> Result<f64, String> {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{key} must be a finite number"))
}
fn text<'a>(params: &'a Value, key: &str, max_bytes: usize) -> Result<&'a str, String> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))?;
    if value.len() > max_bytes {
        return Err(format!("{key} exceeds its {max_bytes}-byte budget"));
    }
    Ok(value)
}
fn text_alias<'a>(params: &'a Value, keys: &[&str], max_bytes: usize) -> Result<&'a str, String> {
    for key in keys {
        if params.get(*key).is_some() {
            return text(params, key, max_bytes);
        }
    }
    Err(format!("{} is required", keys[0]))
}
fn optional_text<'a>(params: &'a Value, key: &str, default: &'a str) -> Result<&'a str, String> {
    if params.get(key).is_none() {
        Ok(default)
    } else {
        text(params, key, 64)
    }
}
fn decimal(value: &Value, label: &str) -> Result<u64, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{label} must be a canonical decimal string"))?;
    if text.is_empty() || text.len() > 20 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "{label} must be a positive canonical decimal string"
        ));
    }
    let number = text
        .parse::<u64>()
        .map_err(|_| format!("{label} exceeds u64"))?;
    if number == 0 || number.to_string() != text {
        return Err(format!(
            "{label} must be a positive canonical decimal string"
        ));
    }
    Ok(number)
}
fn decimal_field(params: &Value, key: &str) -> Result<u64, String> {
    decimal(
        params
            .get(key)
            .ok_or_else(|| format!("{key} is required"))?,
        key,
    )
}
fn decimal_alias(params: &Value, keys: &[&str]) -> Result<u64, String> {
    for key in keys {
        if params.get(*key).is_some() {
            return decimal_field(params, key);
        }
    }
    Err(format!("{} is required", keys[0]))
}
fn validate_numeric_input(text: &str) -> Result<(), String> {
    let mut count = 0;
    for word in text
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter(|s| !s.is_empty())
    {
        count += 1;
        if count > 4 {
            return Err("too many numeric parameters".into());
        }
        let value = word
            .parse::<f64>()
            .map_err(|_| format!("invalid numeric parameter: {word}"))?;
        if !value.is_finite() {
            return Err("numeric parameters must be finite".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_selection_keeps_equal_numbers_in_distinct_domains() {
        let result = parse_selection(&json!({"selection":[{"domain":"bim","element_id":7},
            {"domain":"drafting","drawing_id":"2","entity_id":"7"}]}))
        .unwrap();
        assert_eq!(
            result,
            vec![
                QualifiedSelection::Bim(ElementId::new(7)),
                QualifiedSelection::Draft {
                    drawing: DrawingId::new(2),
                    entity: DraftEntityId::new(7)
                }
            ]
        );
    }
    #[test]
    fn qualified_selection_rejects_ambiguous_and_noncanonical_ids() {
        for bad in [
            json!({"selection":[7]}),
            json!({"selection":[{"entity_id":"7"}]}),
            json!({"selection":[{"domain":"drafting","drawing_id":2,"entity_id":"7"}]}),
            json!({"selection":[{"domain":"drafting","drawing_id":"02","entity_id":"7"}]}),
            json!({"selection":[{"domain":"drafting","drawing_id":"2","entity_id":"0"}]}),
            json!({"selection":[{"domain":"bim","element_id":7,"entity_id":"7"}]}),
            json!({"selection":[{"domain":"bim","element_id":7.0}]}),
            json!({"selection":[{"domain":"bim","element_id":7},{"domain":"bim","element_id":"7"}]}),
        ] {
            assert!(parse_selection(&bad).is_err(), "accepted {bad}");
        }
    }
    #[test]
    fn decimal_ids_preserve_full_u64_range_without_float_conversion() {
        assert_eq!(
            decimal(&json!("18446744073709551615"), "id").unwrap(),
            u64::MAX
        );
        for bad in [
            json!("18446744073709551616"),
            json!("+7"),
            json!(" 7"),
            json!("7e0"),
            json!("07"),
            json!(7),
        ] {
            assert!(decimal(&bad, "id").is_err());
        }
    }
    #[test]
    fn native_pointer_parser_preserves_secondary_drag_and_local_coordinates() {
        let input=parse_native_input("drawing.ui.native_input",&json!({"input":{"type":"pointer","phase":"move","button":"right","screen":{"x":-12.5,"y":40.25}}})).unwrap();
        assert!(
            matches!(input,CanvasInput::Pointer{phase:PointerPhase::Move,button:PointerButton::Right,screen}if screen==Point2::new(-12.5,40.25))
        );
    }
    #[test]
    fn native_input_rejects_invalid_numbers_and_unknown_discriminants() {
        for bad in [
            json!({"type":"pointer","phase":"wheel","x":1,"y":2}),
            json!({"type":"pointer","button":"other","x":1,"y":2}),
            json!({"type":"pointer","x":"1","y":2}),
            json!({"type":"scroll","x":1,"y":2,"delta_y":null}),
            json!({"type":"key","key":"A"}),
            json!({"type":"unknown"}),
        ] {
            assert!(parse_native_input("drawing.ui.native_input", &bad).is_err());
        }
    }
    #[test]
    fn camera_proposal_validation_does_not_change_original() {
        let camera = Camera2D::default();
        let original = camera.clone();
        for bad in [
            json!({"center_x":1e13}),
            json!({"pixels_per_metre":0}),
            json!({"viewport_width":0}),
            json!({"center":{"x":null,"y":0}}),
            json!({"camera":"bad"}),
        ] {
            assert!(parse_camera_set(&camera, &bad).is_err());
            assert_eq!(camera, original);
        }
        let proposed = parse_camera_set(
            &camera,
            &json!({"center_x":10,"center_y":20,"pixels_per_metre":100}),
        )
        .unwrap();
        assert_eq!(proposed.center, Point2::new(10.0, 20.0));
        assert_eq!(proposed.pixels_per_metre, 100.0);
        assert_eq!(camera, original);
    }
    #[test]
    fn numeric_validation_rejects_nonfinite_and_unbounded_token_count() {
        for bad in ["NaN", "inf", "-inf", "1e999", "1 2 3 4 5"] {
            assert!(validate_numeric_input(bad).is_err());
        }
        for good in ["", "1 2", "1, 2; -0.5", "3 90 -180"] {
            assert!(validate_numeric_input(good).is_ok());
        }
    }
}
