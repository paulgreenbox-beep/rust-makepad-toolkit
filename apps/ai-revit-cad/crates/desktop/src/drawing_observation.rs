//! Bounded, read-only observations of the authoritative Core drafting model.
//!
//! This module deliberately does not prepare or synchronize the drawing cache,
//! take a snapshot, or clone the entity collection. Entity geometry is
//! serialized only for the requested page or a single inspected entity.

use super::*;
use ai_revit_core::{DraftEntityId, DrawingId};
use drafting_core::{DraftDrawing, DraftEntity, DraftLayer, LayerId};
use serde_json::{Value, json};

const DEFAULT_PAGE_LIMIT: u64 = 100;
const DEFAULT_OBSERVE_LIMIT: u64 = 64;
const MAX_PAGE_LIMIT: u64 = 500;

/// Returns `None` for methods owned by another API module. A recognized
/// drawing method always returns its result, including validation failures.
pub(super) fn query(app: &App, method: &str, params: &Value) -> Option<Result<Value, String>> {
    match method {
        "drawing.list" => Some(drawing_list(app, params)),
        "drawing.observe" => Some(drawing_observe(app, params)),
        "drawing.entities" => Some(drawing_entities(app, params)),
        "drawing.entity.inspect" => Some(drawing_entity_inspect(app, params)),
        "drawing.layers" => Some(drawing_layers(app, params)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
struct Page {
    offset: usize,
    limit: usize,
    end: usize,
}

impl Page {
    fn next_offset(self, total: usize) -> Option<usize> {
        (self.end < total).then_some(self.end)
    }
}

fn page(params: &Value, default_limit: u64) -> Result<Page, String> {
    page_with_keys(params, "offset", "limit", default_limit)
}

fn page_with_keys(
    params: &Value,
    offset_key: &str,
    limit_key: &str,
    default_limit: u64,
) -> Result<Page, String> {
    let offset = unsigned_field_or(params, offset_key, 0)?;
    let limit = unsigned_field_or(params, limit_key, default_limit)?;
    if limit == 0 || limit > MAX_PAGE_LIMIT {
        return Err(format!("{limit_key} must be in 1..={MAX_PAGE_LIMIT}"));
    }
    let end = offset
        .checked_add(limit)
        .ok_or_else(|| format!("{offset_key} + {limit_key} overflows"))?;
    let offset = usize::try_from(offset)
        .map_err(|_| format!("{offset_key} is outside this platform's index range"))?;
    let limit = usize::try_from(limit)
        .map_err(|_| format!("{limit_key} is outside this platform's index range"))?;
    let end = usize::try_from(end).map_err(|_| {
        format!("{offset_key} + {limit_key} is outside this platform's index range")
    })?;
    Ok(Page { offset, limit, end })
}

fn unsigned_field_or(params: &Value, key: &str, default: u64) -> Result<u64, String> {
    match params.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{key} must be an unsigned integer")),
    }
}

fn canonical_id(params: &Value, key: &str) -> Result<Option<u64>, String> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| format!("{key} must be a canonical decimal string"))?;
    let id = text
        .parse::<u64>()
        .map_err(|_| format!("{key} must be a canonical decimal string"))?;
    if id == 0 || text != id.to_string() {
        return Err(format!("{key} must be a nonzero canonical decimal string"));
    }
    Ok(Some(id))
}

fn params_object(params: &Value) -> Result<(), String> {
    if params.is_null() || params.is_object() {
        Ok(())
    } else {
        Err("params must be an object or null".to_owned())
    }
}

fn document(app: &App) -> Result<&Document, String> {
    app.document
        .as_ref()
        .ok_or_else(|| "no document is open".to_owned())
}

fn resolve_drawing<'a>(
    app: &App,
    document: &'a Document,
    params: &Value,
) -> Result<&'a DraftDrawing, String> {
    if let Some(raw_id) = canonical_id(params, "drawing_id")? {
        let id = DrawingId::new(raw_id);
        return document
            .drawing(id)
            .ok_or_else(|| format!("drawing {id} does not exist"));
    }
    if let Some(id) = app.drafting_current_drawing_id() {
        return document
            .drawing(id)
            .ok_or_else(|| format!("active drawing {id} does not exist"));
    }
    document
        .drawings()
        .values()
        .next()
        .ok_or_else(|| "document has no drawings".to_owned())
}

fn current_token<'a>(app: &'a App, document: &Document, drawing: &DraftDrawing) -> Option<&'a str> {
    let frame = app.drafting_ui.frame.as_deref()?;
    let source = &frame.source;
    let document_id = document.document_uuid.to_string();
    let runtime_instance_id = document.runtime_instance_id();
    (source.document_id == document_id
        && source.runtime_instance_id == runtime_instance_id
        && source.document_epoch == app.api_document_generation
        && source.document_revision == document.revision()
        && source.drawing_id == drawing.id
        && source.drawing_revision == drawing.revision)
        .then_some(frame.token.as_str())
}

fn qualified_entity_owner(drawing_id: DrawingId, entity_id: DraftEntityId) -> Value {
    json!({
        "domain": "drafting",
        "drawing_id": drawing_id,
        "entity_id": entity_id,
    })
}

fn entity_record(drawing_id: DrawingId, entity: &DraftEntity) -> Result<Value, String> {
    let mut record = serde_json::to_value(entity).map_err(|error| error.to_string())?;
    let object = record
        .as_object_mut()
        .ok_or_else(|| "serialized drafting entity is not an object".to_owned())?;
    object.insert(
        "qualified_owner".to_owned(),
        qualified_entity_owner(drawing_id, entity.id),
    );
    Ok(record)
}

fn layer_counts(drawing: &DraftDrawing) -> std::collections::BTreeMap<LayerId, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for entity in drawing.entities.values() {
        let count = counts.entry(entity.layer_id).or_insert(0usize);
        *count = count.saturating_add(1);
    }
    counts
}

fn layer_record(layer: &DraftLayer, entity_count: usize) -> Result<Value, String> {
    let mut record = serde_json::to_value(layer).map_err(|error| error.to_string())?;
    let object = record
        .as_object_mut()
        .ok_or_else(|| "serialized drafting layer is not an object".to_owned())?;
    object.insert("entity_count".to_owned(), json!(entity_count));
    Ok(record)
}

fn paged_meta(offset: usize, limit: usize, total: usize, next_offset: Option<usize>) -> Value {
    json!({"offset":offset,"limit":limit,"total":total,"next_offset":next_offset})
}

fn drawing_list(app: &App, params: &Value) -> Result<Value, String> {
    params_object(params)?;
    let page = page(params, DEFAULT_PAGE_LIMIT)?;
    let document = document(app)?;
    let drawings = document.drawings();
    let total = drawings.len();
    let records = drawings
        .values()
        .skip(page.offset)
        .take(page.limit)
        .map(|drawing| {
            json!({
                "drawing_id": drawing.id,
                "name": drawing.name,
                "drawing_revision": drawing.revision,
                "entity_count": drawing.entities.len(),
                "layer_count": drawing.layers.len(),
                "units": "metres",
                "angle_units": "radians",
                "representation_quality": "analytic_2d",
                "active": app.drafting_current_drawing_id() == Some(drawing.id),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "drawings": records,
        "project_revision": document.revision(),
        "pagination": paged_meta(page.offset, page.limit, total, page.next_offset(total)),
    }))
}

fn drawing_observe(app: &App, params: &Value) -> Result<Value, String> {
    params_object(params)?;
    let document = document(app)?;
    let drawing = resolve_drawing(app, document, params)?;
    let page = page_with_keys(params, "offset", "limit", DEFAULT_OBSERVE_LIMIT)?;
    let layer_total = drawing.layers.len();
    let layers = drawing
        .layers
        .values()
        .skip(page.offset)
        .take(page.limit)
        .map(|layer| serde_json::to_value(layer).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    let view_total = document
        .view_definitions()
        .values()
        .filter(|view| view.drawing_id == drawing.id)
        .count();
    let views = document
        .view_definitions()
        .values()
        .filter(|view| view.drawing_id == drawing.id)
        .skip(page.offset)
        .take(page.limit)
        .map(|view| {
            let mut record = serde_json::to_value(view).map_err(|error| error.to_string())?;
            let object = record
                .as_object_mut()
                .ok_or_else(|| "serialized drafting view is not an object".to_owned())?;
            object.insert(
                "qualified_owner".to_owned(),
                json!({"domain":"drafting","drawing_id":drawing.id,"view_definition_id":view.id}),
            );
            Ok(record)
        })
        .collect::<Result<Vec<_>, String>>()?;

    let token = current_token(app, document, drawing);
    Ok(json!({
        "drawing_id": drawing.id,
        "name": drawing.name,
        "source": {
            "authority": "core_document",
            "document_id": document.document_uuid,
            "project_revision": document.revision(),
            "drawing_revision": drawing.revision,
            "frame_is_current": token.is_some(),
        },
        "project_revision": document.revision(),
        "drawing_revision": drawing.revision,
        "units": "metres",
        "angle_units": "radians",
        "representation_quality": "analytic_2d",
        "current_token": token,
        "counts": {
            "entities": drawing.entities.len(),
            "layers": layer_total,
            "views": view_total,
            "by_type": Value::Null,
            "by_type_available": false,
        },
        "layers": layers,
        "views": views,
        "pagination": {
            "offset": page.offset,
            "limit": page.limit,
            "layers": paged_meta(page.offset, page.limit, layer_total, page.next_offset(layer_total)),
            "views": paged_meta(page.offset, page.limit, view_total, page.next_offset(view_total)),
        },
        "performance": app.drawing_performance(),
    }))
}

fn drawing_entities(app: &App, params: &Value) -> Result<Value, String> {
    params_object(params)?;
    let document = document(app)?;
    let drawing = resolve_drawing(app, document, params)?;
    let page = page(params, DEFAULT_PAGE_LIMIT)?;
    let total = drawing.entities.len();
    let entities = drawing
        .entities
        .values()
        .skip(page.offset)
        .take(page.limit)
        .map(|entity| entity_record(drawing.id, entity))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "drawing_id": drawing.id,
        "project_revision": document.revision(),
        "drawing_revision": drawing.revision,
        "units": "metres",
        "angle_units": "radians",
        "representation_quality": "analytic_2d",
        "current_token": current_token(app, document, drawing),
        "entities": entities,
        "pagination": paged_meta(page.offset, page.limit, total, page.next_offset(total)),
    }))
}

fn drawing_entity_inspect(app: &App, params: &Value) -> Result<Value, String> {
    params_object(params)?;
    let document = document(app)?;
    let drawing = resolve_drawing(app, document, params)?;
    let raw_id = canonical_id(params, "entity_id")?
        .ok_or_else(|| "entity_id is required as a canonical decimal string".to_owned())?;
    let id = DraftEntityId::new(raw_id);
    let entity = document
        .draft_entity(drawing.id, id)
        .ok_or_else(|| format!("entity {id} does not exist in drawing {}", drawing.id))?;
    Ok(json!({
        "drawing_id": drawing.id,
        "project_revision": document.revision(),
        "drawing_revision": drawing.revision,
        "units": "metres",
        "angle_units": "radians",
        "representation_quality": "analytic_2d",
        "current_token": current_token(app, document, drawing),
        "entity": entity_record(drawing.id, entity)?,
    }))
}

fn drawing_layers(app: &App, params: &Value) -> Result<Value, String> {
    params_object(params)?;
    let document = document(app)?;
    let drawing = resolve_drawing(app, document, params)?;
    let page = page(params, DEFAULT_PAGE_LIMIT)?;
    let total = drawing.layers.len();
    let layer_entity_counts = layer_counts(drawing);
    let layers = drawing
        .layers
        .values()
        .skip(page.offset)
        .take(page.limit)
        .map(|layer| {
            let count = layer_entity_counts.get(&layer.id).copied().unwrap_or(0);
            layer_record(layer, count)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "drawing_id": drawing.id,
        "project_revision": document.revision(),
        "drawing_revision": drawing.revision,
        "units": "metres",
        "representation_quality": "analytic_2d",
        "current_token": current_token(app, document, drawing),
        "layers": layers,
        "pagination": paged_meta(page.offset, page.limit, total, page.next_offset(total)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_ids_preserve_full_u64_precision() {
        let large = u64::MAX - 1;
        assert_eq!(
            canonical_id(&json!({"id":large.to_string()}), "id").unwrap(),
            Some(large)
        );
        assert!(canonical_id(&json!({"id":large}), "id").is_err());
        assert!(canonical_id(&json!({"id":"01"}), "id").is_err());
        assert!(canonical_id(&json!({"id":"0"}), "id").is_err());
    }

    #[test]
    fn page_limits_and_arithmetic_are_bounded() {
        assert!(page(&json!({"limit":501}), DEFAULT_PAGE_LIMIT).is_err());
        assert!(page(&json!({"limit":0}), DEFAULT_PAGE_LIMIT).is_err());
        assert!(page(&json!({"offset":u64::MAX,"limit":1}), DEFAULT_PAGE_LIMIT).is_err());
        assert_eq!(
            page(&json!({"offset":4,"limit":8}), DEFAULT_PAGE_LIMIT)
                .unwrap()
                .end,
            12
        );
    }
}
