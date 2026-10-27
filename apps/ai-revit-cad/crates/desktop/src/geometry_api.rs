//! Read-only observations of derived geometry. No mesh buffers leave this API.
use super::*;
use super::agent_handler::RequestFailure;
use ai_revit_geometry::{SectionPlane, QuantityGrouping, SpatialQuery};
use viewer_core::Aabb;

impl App {
    pub(super) fn geometry_query(&self, method: &str, params: &Value) -> Result<Value, RequestFailure> {
        if method == "geometry.capabilities" {
            return serde_json::to_value(self.geometry_cache.backend_capabilities())
                .map_err(|e|e.to_string().into());
        }
        let document = self.document.as_ref().ok_or("no document is open")?;
        match method {
            "quantities.get" => {
                let id=geometry_element_id(params)?;
                if document.element(id).is_none() { return Err(format!("element {id} does not exist").into()); }
                let result=self.geometry_cache.quantities_get(id)
                    .ok_or_else(||format!("valid quantities for element {id} are unavailable"))?;
                serde_json::to_value(result).map_err(|e|e.to_string().into())
            }
            "quantities.batch" => {
                let ids=observation_ids(params,20_000)?;
                serde_json::to_value(self.geometry_cache.quantities_batch(&ids))
                    .map_err(|e|e.to_string().into())
            }
            "quantities.by_class" | "quantities.by_type" | "quantities.by_level" => {
                let (grouping,field)=match method {
                    "quantities.by_class" => (QuantityGrouping::Class,"class_id"),
                    "quantities.by_type" => (QuantityGrouping::Type,"type_id"),
                    _ => (QuantityGrouping::Level,"level_id"),
                };
                if let Some(value)=params.get(field) {
                    if field=="class_id" { nonempty_string(value,field)?; }
                    else if !value.is_null() && value.as_u64().is_none() {
                        return Err(format!("{field} must be an unsigned integer or null").into());
                    }
                }
                let groups=self.geometry_cache.quantities_grouped(grouping);
                let mut serialized=serde_json::to_value(groups).map_err(|e|e.to_string())?;
                if let Some(value)=params.get(field) {
                    serialized.as_array_mut().expect("serialized groups").retain(|g|g.get(field)==Some(value));
                }
                Ok(json!({"group_by":grouping,"quality":"approximate_mesh","groups":serialized}))
            }
            "spatial.bounds_query" | "spatial.intersects_query" => {
                let query=spatial_query(params)?;
                if let Some(level_id)=query.level_id {
                    if document.level(level_id).is_none() {return Err(format!("level {level_id} does not exist").into());}
                }
                let result=if method=="spatial.bounds_query" {self.geometry_cache.spatial_bounds_query(&query)}
                    else {self.geometry_cache.spatial_intersects_query(&query)}
                    .map_err(|e|RequestFailure::geometry_error(e.to_string(),serde_json::to_value(&e).unwrap_or(Value::Null)))?;
                let mut result=serde_json::to_value(result).map_err(|e|e.to_string())?;
                result["representation_quality"]=json!("approximate_mesh");
                result["query_quality"]=json!("aabb_candidates");
                result["document_revision"]=json!(document.revision());
                Ok(result)
            }
            "geometry.inspect" | "geometry.dependencies" => {
                let id = geometry_element_id(params)?;
                if document.element(id).is_none() {
                    return Err(format!("element {id} does not exist").into());
                }
                let record = self.geometry_cache.record(id)
                    .ok_or_else(|| format!("geometry for element {id} is not built"))?;
                if method == "geometry.inspect" {
                    serde_json::to_value(record).map_err(|e|e.to_string().into())
                } else {
                    Ok(json!({"element_id":id,"representation_quality":record.representation_quality,
                        "geometry_revision":record.geometry_revision,"source_document_revision":record.source_document_revision,
                        "document_revision":document.revision(),"geometry_fingerprint":record.geometry_fingerprint,
                        "host_id":document.host_of(id),"mesh_revision":record.mesh_revision,
                        "derived_from":record.derived_from,"openings":record.openings}))
                }
            }
            "geometry.compare" => {
                let id = geometry_element_id(params)?;
                let before = unsigned_field(params,"geometry_revision_before")?;
                let after = unsigned_field(params,"geometry_revision_after")?;
                let result = self.geometry_cache.compare(id,before,after)
                    .map_err(|e|RequestFailure::geometry_error(e.to_string(),serde_json::to_value(&e).unwrap_or(Value::Null)))?;
                serde_json::to_value(result).map_err(|e|e.to_string().into())
            }
            "geometry.section" => {
                let raw = params.get("element_ids").and_then(Value::as_array)
                    .ok_or("geometry.section requires element_ids as an array")?;
                if raw.is_empty() || raw.len()>256 {return Err("element_ids must contain 1 through 256 IDs".into());}
                let mut ids = Vec::with_capacity(raw.len());
                for value in raw {
                    let id = ElementId::new(value.as_u64().ok_or("element IDs must be unsigned integers")?);
                    if document.element(id).is_none() {return Err(format!("element {id} does not exist").into());}
                    if !ids.contains(&id) {ids.push(id);}
                }
                let plane=params.get("plane").ok_or("geometry.section requires plane")?;
                let origin=vector3(plane.get("origin").ok_or("plane requires origin")?)?;
                let normal=vector3(plane.get("normal").ok_or("plane requires normal")?)?;
                let result=self.geometry_cache.section(&ids,SectionPlane{origin,normal})
                    .map_err(|e|RequestFailure::geometry_error(e.to_string(),serde_json::to_value(&e).unwrap_or(Value::Null)))?;
                serde_json::to_value(result).map_err(|e|e.to_string().into())
            }
            _ => Err(format!("unknown geometry method {method}").into()),
        }
    }
}

fn observation_ids(params:&Value,limit:usize)->Result<Vec<ElementId>,String>{
    let raw=params.get("element_ids").and_then(Value::as_array).ok_or("element_ids must be an array")?;
    if raw.is_empty()||raw.len()>limit{return Err(format!("element_ids must contain 1 through {limit} IDs"));}
    let mut ids=std::collections::BTreeSet::new();
    for value in raw {ids.insert(ElementId::new(value.as_u64().ok_or("element IDs must be unsigned integers")?));}
    Ok(ids.into_iter().collect())
}
fn nonempty_string<'a>(value:&'a Value,key:&str)->Result<&'a str,String>{
    value.as_str().filter(|v|!v.trim().is_empty()).ok_or_else(||format!("{key} must be a non-empty string"))
}
fn spatial_vector(value:&Value)->Result<Vec3,String>{
    if value.is_array(){return vector3(value);}
    let components=["x","y","z"].map(|key|value.get(key).and_then(Value::as_f64).filter(|v|v.is_finite()));
    match components { [Some(x),Some(y),Some(z)]=>Ok(Vec3::new(x,y,z)),_=>Err("bounds vectors require three finite components".into()) }
}
fn spatial_query(params:&Value)->Result<SpatialQuery,String>{
    let bounds=params.get("bounds").ok_or("spatial query requires bounds")?;
    let min=spatial_vector(bounds.get("min").ok_or("bounds requires min")?)?;
    let max=spatial_vector(bounds.get("max").ok_or("bounds requires max")?)?;
    if min.x>max.x||min.y>max.y||min.z>max.z{return Err("bounds min must not exceed max".into());}
    let class_id=params.get("class_id").map(|v|nonempty_string(v,"class_id").map(ClassId::new)).transpose()?;
    let level_id=params.get("level_id").map(|v|v.as_u64().map(LevelId::new).ok_or("level_id must be an unsigned integer".to_owned())).transpose()?;
    Ok(SpatialQuery{bounds:Aabb{min,max},class_id,level_id})
}

fn unsigned_field(params:&Value,key:&str)->Result<u64,String>{
    params.get(key).and_then(Value::as_u64).ok_or_else(||format!("{key} must be an unsigned integer"))
}
fn geometry_element_id(params:&Value)->Result<ElementId,String>{
    unsigned_field(params,"element_id").map(ElementId::new)
}
pub(super) fn vector3(value:&Value)->Result<Vec3,String>{
    let array=value.as_array().filter(|a|a.len()==3).ok_or("vector must contain three finite numbers")?;
    let mut components=[0.0;3];
    for (index,value) in array.iter().enumerate(){components[index]=value.as_f64().filter(|v|v.is_finite()).ok_or("vector must contain three finite numbers")?;}
    Ok(Vec3::new(components[0],components[1],components[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plane_vectors_require_three_finite_numbers() {
        assert!(vector3(&json!([0,1,2])).is_ok());
        assert!(vector3(&json!([0,1])).is_err());
        assert!(vector3(&json!([0,1,"2"])).is_err());
        assert!(vector3(&Value::Null).is_err());
    }
    #[test]
    fn geometry_ids_do_not_round_large_integers() {
        let id=9_007_199_254_740_993u64;
        assert_eq!(geometry_element_id(&json!({"element_id":id})).unwrap().get(),id);
        assert!(geometry_element_id(&json!({"element_id":-1})).is_err());
        assert!(geometry_element_id(&json!({"element_id":"7"})).is_err());
    }
    #[test]
    fn quantity_batch_ids_are_bounded_sorted_and_lossless() {
        let large=9_007_199_254_740_993u64;
        assert_eq!(observation_ids(&json!({"element_ids":[large,7,7]}),3).unwrap(),vec![ElementId::new(7),ElementId::new(large)]);
        assert!(observation_ids(&json!({"element_ids":[]}),20_000).is_err());
        assert!(observation_ids(&json!({"element_ids":[1,2]}),1).is_err());
        assert!(observation_ids(&json!({"element_ids":["7"]}),2).is_err());
    }
    #[test]
    fn spatial_bounds_support_both_vector_encodings_without_invalid_boxes() {
        assert!(spatial_query(&json!({"bounds":{"min":[0,0,0],"max":{"x":1,"y":2,"z":3}},"class_id":"core.wall","level_id":1})).is_ok());
        assert!(spatial_query(&json!({"bounds":{"min":[2,0,0],"max":[1,1,1]}})).is_err());
        assert!(spatial_query(&json!({"bounds":{"min":[0,0,0],"max":[1,1,"2"]}})).is_err());
        assert!(spatial_query(&json!({"bounds":{"min":[0,0,0],"max":[1,1,1]},"level_id":"1"})).is_err());
    }
}
