//! Compact read-only BIM observation. Geometry is read from the derived cache.
use super::*;
use ai_revit_core::{EntityRef, LevelId, RelationshipId, RelationshipType, TypeId};
use ai_revit_geometry::GeometryStatusKind;
use serde_json::{Value, json};

impl App {
    pub(super) fn layout_observation(&self, cx: &Cx) -> Value {
        let workspace = self.ui.widget(cx, ids!(workspace)).area().rect(cx);
        let mut panels = BTreeMap::new();
        for (name, path) in [
            ("navigator", ids!(history_panel)),
            ("viewport", ids!(viewport_panel)),
            ("inspector", ids!(inspector_panel)),
        ] {
            let area = self.ui.widget(cx, path).area();
            let rect = area.rect(cx);
            let clipped = area.clipped_rect(cx);
            let inside = rect.size.x > 1.0
                && rect.size.y > 1.0
                && rect.pos.x >= workspace.pos.x - 1.0
                && rect.pos.y >= workspace.pos.y - 1.0
                && rect.pos.x + rect.size.x <= workspace.pos.x + workspace.size.x + 1.0
                && rect.pos.y + rect.size.y <= workspace.pos.y + workspace.size.y + 1.0;
            panels.insert(name,json!({"x":rect.pos.x,"y":rect.pos.y,"width":rect.size.x,"height":rect.size.y,
                "clipped_width":clipped.size.x,"clipped_height":clipped.size.y,"fully_inside_workspace":inside}));
        }
        json!({"units":"logical_pixels","workspace":{"x":workspace.pos.x,"y":workspace.pos.y,"width":workspace.size.x,"height":workspace.size.y},"panels":panels})
    }

    pub(super) fn semantic_query(&self, method: &str, params: &Value) -> Result<Value, String> {
        let d = self.document.as_ref().ok_or("no document is open")?;
        let revision = d.revision();
        match method {
            "document.observe" => {
                let from = revision.saturating_sub(1);
                let recent = d.history_diff(from, revision).ok();
                let errors: Vec<_> = self.geometry_cache.geometry_errors().map(|e| json!({
                    "element_id":e.element_id,"host_id":e.host_id,"opening_id":e.opening_id,"error_code":e.error_code,
                    "canonical_error_code":e.canonical_error_code,"backend":e.backend,"representation_quality":e.representation_quality,
                    "code":format!("{:?}",e.code),"message":e.message
                })).chain(self.geometry_cache.resolution_errors().map(|(id,error)|json!({
                    "element_id":id,"code":"property_resolution","message":error
                }))).collect();
                Ok(
                    json!({"document_id":d.document_uuid,"name":d.name(),"revision":revision,
                    "session_id":super::agent_handler::api_session_id(),"open_generation":self.api_document_generation,
                    "active_workspace":if self.drafting_ui.active_2d {"2d"} else {"3d"},
                    "drafting":{"drawing_count":d.drawings().len(),"entity_count":d.drafting().entity_count(),
                        "view_definition_count":d.view_definitions().len(),"active_drawing_id":self.drafting_current_drawing_id()},
                    "element_count":d.elements().len(),"type_count":d.types().len(),
                    "material_count":d.materials().count(),"relationship_count":d.relationships().count(),
                    "levels":d.levels().collect::<Vec<_>>(),"selection":self.selected_element.into_iter().collect::<Vec<_>>(),
                    "semantic_selection":self.semantic_selection.into_iter().collect::<Vec<_>>(),
                    "geometry_errors":errors,"dirty_elements":d.dirty_ids(),
                    "recent_changes":recent,"geometry_rebuild":self.rebuild_observation(),
                    "history_metrics":d.history_metrics().map_err(|e|e.to_string())?,
                    "index_metrics":d.index_metrics(),"core_scan_metrics":d.scan_metrics(),
                    "geometry_metrics":self.geometry_cache.observation_metrics(),
                    "quantity_cache_metrics":self.geometry_cache.quantity_cache_metrics(),
                    "spatial_index_metrics":self.geometry_cache.spatial_index_metrics()}),
                )
            }
            "element.inspect" => {
                let id = element_id(params)?;
                let e = d
                    .element(id)
                    .ok_or_else(|| format!("element {id} does not exist"))?;
                let effective = d.effective_element(id).map_err(|e| e.to_string())?;
                Ok(json!({"element_id":id,"name":e.name,"class":e.class_id,
                    "type_id":e.type_id,"level_id":d.element_level(id),"properties":e.properties,
                    "resolved_properties":d.resolved_properties(id).map_err(|e|e.to_string())?,
                    "placement":effective.kind,"relationships":relationship_summary(d,id),
                    "geometry":self.geometry_observation(id),"revision":revision}))
            }
            "element.relationships" => {
                let id = require_element(d, params)?;
                Ok(
                    json!({"element_id":id,"relationships":d.element_relationships(id),
                    "summary":relationship_summary(d,id),"revision":revision}),
                )
            }
            "element.dependencies" => {
                let id = require_element(d, params)?;
                let mut result =
                    serde_json::to_value(d.dependency_info(id).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                result["revision"] = json!(revision);
                Ok(result)
            }
            "element.delete_impact" => {
                let id = require_element(d, params)?;
                Ok(
                    json!({"impact":d.delete_impact(id).map_err(|e|e.to_string())?,"revision":revision}),
                )
            }
            "element.neighbors" => {
                let id = require_element(d, params)?;
                let limit = params
                    .get("max_results")
                    .map(|v| {
                        v.as_u64()
                            .filter(|v| *v > 0 && *v <= 1000)
                            .ok_or("max_results must be 1..1000")
                    })
                    .transpose()?
                    .unwrap_or(64) as usize;
                let filters: Option<Vec<RelationshipType>> = params
                    .get("relationship_types")
                    .map(|v| {
                        serde_json::from_value(v.clone())
                            .map_err(|e| format!("invalid relationship_types: {e}"))
                    })
                    .transpose()?;
                let semantic = d
                    .semantic_neighbors(id, filters.as_deref(), limit)
                    .map_err(|e| e.to_string())?;
                let mut spatial = Vec::new();
                let mut spatial_scan_count=0usize;
                if let Some(radius_value) = params.get("spatial_radius") {
                    let radius = radius_value
                        .as_f64()
                        .filter(|v| v.is_finite() && *v >= 0.0)
                        .ok_or("spatial_radius must be finite and non-negative")?;
                    if let Some(origin) = self
                        .geometry_cache
                        .element_status(id)
                        .and_then(|s| s.bounds)
                    {
                        spatial_scan_count=d.elements().len().saturating_sub(1);
                        let mut nearby: Vec<_> = d
                            .elements()
                            .filter(|e| e.id != id)
                            .filter_map(|e| {
                                self.geometry_cache
                                    .element_status(e.id)
                                    .and_then(|s| s.bounds)
                                    .map(|bounds| (e.id, bounds_distance(origin, bounds)))
                            })
                            .filter(|(_, distance)| *distance <= radius)
                            .collect();
                        nearby.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
                        spatial = nearby.into_iter().take(limit).map(|(id,distance)|json!({"element_id":id,"bounds_distance_m":distance})).collect();
                    }
                }
                Ok(
                    json!({"element_id":id,"semantic_neighbors":semantic,"spatial_neighbors":spatial,
                    "spatial_basis":"derived_axis_aligned_bounds_distance","spatial_scan_count":spatial_scan_count,"max_results":limit,"revision":revision}),
                )
            }
            "types.list" => Ok(json!({"types":d.types().collect::<Vec<_>>(),"revision":revision})),
            "type.get" | "type.instances" => {
                let id = TypeId::new(numeric(params, "type_id")?);
                let t = d
                    .element_type(id)
                    .ok_or_else(|| format!("type {id} does not exist"))?;
                if method == "type.get" {
                    Ok(
                        json!({"type":t,"instance_count":d.type_instances(id).len(),"revision":revision}),
                    )
                } else {
                    Ok(json!({"type_id":id,"elements":d.type_instances(id),"revision":revision}))
                }
            }
            "levels.list" => {
                Ok(json!({"levels":d.levels().collect::<Vec<_>>(),"revision":revision}))
            }
            "level.get" | "level.elements" => {
                let id = LevelId::new(numeric(params, "level_id")?);
                let level = d
                    .level(id)
                    .ok_or_else(|| format!("level {id} does not exist"))?;
                if method == "level.get" {
                    Ok(json!({"level":level,"revision":revision}))
                } else {
                    Ok(json!({"level_id":id,"elements":d.level_elements(id),"revision":revision}))
                }
            }
            "relationship.get" => {
                let id = RelationshipId::new(numeric(params, "relationship_id")?);
                Ok(
                    json!({"relationship":d.relationship(id).ok_or_else(||format!("relationship {id} does not exist"))?,"revision":revision}),
                )
            }
            "relationship.list" => {
                let filter: Option<RelationshipType> = params
                    .get("relationship_type")
                    .map(|v| serde_json::from_value(v.clone()).map_err(|e| e.to_string()))
                    .transpose()?;
                let entity: Option<EntityRef> = params
                    .get("entity")
                    .map(|v| serde_json::from_value(v.clone()).map_err(|e| e.to_string()))
                    .transpose()?;
                let candidates: Vec<_> = if let Some(EntityRef::Element(id))=entity {
                    d.element_relationships(id)
                } else { d.relationships().collect() };
                let relationship_scan_count=candidates.len();
                let rows: Vec<_>=candidates.into_iter()
                    .filter(|r|filter.is_none_or(|f|r.relationship_type==f))
                    .filter(|r|entity.is_none_or(|e|r.source_id==e||r.target_id==e)).collect();
                Ok(json!({"relationships":rows,"relationship_scan_count":relationship_scan_count,"revision":revision}))
            }
            "history.diff" => {
                let from = numeric(params, "revision_from")?;
                let to = numeric(params, "revision_to")?;
                serde_json::to_value(d.history_diff(from, to).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())
            }
            _ => Err(format!("unknown semantic method {method}")),
        }
    }

    fn geometry_observation(&self, id: ElementId) -> Value {
        let quality=self.geometry_cache.record(id).map(|r|json!(r.representation_quality)).unwrap_or(Value::Null);
        match self.geometry_cache.element_status(id) {
            Some(s) => {
                json!({"representation_quality":quality,"status":match s.status {GeometryStatusKind::Valid=>"valid",GeometryStatusKind::Invalid=>"invalid",GeometryStatusKind::ResolutionFailed=>"resolution_failed"},
                "bounds":s.bounds.map(|b|json!({"min":[b.min.x,b.min.y,b.min.z],"max":[b.max.x,b.max.y,b.max.z]})),
                "mesh_revision":s.mesh_revision,"error":s.resolution_error.clone().or_else(||s.failure.as_ref().map(ToString::to_string))})
            }
            None => json!({"status":"not_built","bounds":null,"mesh_revision":0}),
        }
    }
    pub(super) fn rebuild_observation(&self) -> Value {
        serde_json::to_value(&self.last_rebuild).unwrap_or(Value::Null)
    }
}

fn numeric(params: &Value, key: &str) -> Result<u64, String> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .or_else(|| params.as_u64())
        .ok_or_else(|| format!("{key} must be an unsigned integer"))
}
fn element_id(params: &Value) -> Result<ElementId, String> {
    Ok(ElementId::new(numeric(params, "element_id")?))
}
fn require_element(d: &Document, params: &Value) -> Result<ElementId, String> {
    let id = element_id(params)?;
    d.element(id)
        .ok_or_else(|| format!("element {id} does not exist"))?;
    Ok(id)
}
fn relationship_summary(d: &Document, id: ElementId) -> Value {
    let mut rows: BTreeMap<&str, Vec<EntityRef>> = BTreeMap::new();
    for r in d.element_relationships(id) {
        let forward = r.source_id == EntityRef::Element(id);
        let (key, other) = match (r.relationship_type, forward) {
            (RelationshipType::Hosts, true) | (RelationshipType::HostedBy, false) => {
                ("hosts", if forward { r.target_id } else { r.source_id })
            }
            (RelationshipType::HostedBy, true) | (RelationshipType::Hosts, false) => {
                ("hosted_by", if forward { r.target_id } else { r.source_id })
            }
            (RelationshipType::Contains, true) | (RelationshipType::ContainedIn, false) => {
                ("contains", if forward { r.target_id } else { r.source_id })
            }
            (RelationshipType::ContainedIn, true) | (RelationshipType::Contains, false) => (
                "contained_in",
                if forward { r.target_id } else { r.source_id },
            ),
            (RelationshipType::Supports, true) | (RelationshipType::SupportedBy, false) => {
                ("supports", if forward { r.target_id } else { r.source_id })
            }
            (RelationshipType::SupportedBy, true) | (RelationshipType::Supports, false) => (
                "supported_by",
                if forward { r.target_id } else { r.source_id },
            ),
            (RelationshipType::ConnectsTo, _) => (
                "connected_to",
                if forward { r.target_id } else { r.source_id },
            ),
            (RelationshipType::Bounds, _) => {
                ("bounds", if forward { r.target_id } else { r.source_id })
            }
            (RelationshipType::References, _) => (
                "references",
                if forward { r.target_id } else { r.source_id },
            ),
            (RelationshipType::InstanceOf, _) => (
                "instance_of",
                if forward { r.target_id } else { r.source_id },
            ),
            (RelationshipType::BelongsToLevel, _) => {
                ("level", if forward { r.target_id } else { r.source_id })
            }
            (RelationshipType::BoundsSpace, true) => ("bounds_space",r.target_id),
            (RelationshipType::BoundsSpace, false) => ("bounded_by",r.source_id),
            (RelationshipType::ContainedInSpace, true) => ("contained_in_space",r.target_id),
            (RelationshipType::ContainedInSpace, false) => ("space_contains",r.source_id),
        };
        rows.entry(key).or_default().push(other);
    }
    for values in rows.values_mut() {
        values.sort();
        values.dedup();
    }
    json!(rows)
}
fn bounds_distance(a: viewer_core::Aabb, b: viewer_core::Aabb) -> f64 {
    let gap = |amin: f64, amax: f64, bmin: f64, bmax: f64| (amin - bmax).max(bmin - amax).max(0.0);
    let x = gap(a.min.x, a.max.x, b.min.x, b.max.x);
    let y = gap(a.min.y, a.max.y, b.min.y, b.max.y);
    let z = gap(a.min.z, a.max.z, b.min.z, b.max.z);
    (x * x + y * y + z * z).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spatial_distance_measures_bounds_not_centers() {
        let a = viewer_core::Aabb::new(Vec3::ZERO, Vec3::new(2.0, 2.0, 2.0));
        let touching = viewer_core::Aabb::new(Vec3::new(2.0, 0.0, 0.0), Vec3::new(3.0, 1.0, 1.0));
        let away = viewer_core::Aabb::new(Vec3::new(5.0, 6.0, 2.0), Vec3::new(6.0, 7.0, 3.0));
        assert_eq!(bounds_distance(a, touching), 0.0);
        assert_eq!(bounds_distance(a, away), 5.0);
        assert_eq!(bounds_distance(away, a), 5.0);
    }
    #[test]
    fn semantic_numeric_parameters_do_not_round_or_accept_strings() {
        assert_eq!(
            numeric(&json!({"element_id":9007199254740993u64}), "element_id").unwrap(),
            9007199254740993
        );
        assert!(numeric(&json!({"element_id":"3"}), "element_id").is_err());
        assert!(numeric(&json!({"element_id":-1}), "element_id").is_err());
    }
}
