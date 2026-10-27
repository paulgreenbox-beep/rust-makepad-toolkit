//! Immutable GEO-to-GEO-SLOPE projection. Legacy editable GroundSections and
//! solver mathematics are preserved; this adapter creates no second soil owner.
use crate::*;
use geotech_core::circular::{GroundMaterial,GroundSection2D,LayerInterface,SectionCoordinateSystem,SoilRegion};
use sha2::{Digest,Sha256};

#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoSlopeProjection {
    pub source:geo_core::ResolvedGeoSnapshot,
    pub polyline_xy_m:Vec<[f64;2]>,
    pub max_spacing_m:f64,
    pub ground:GroundSection2D,
    pub content_fingerprint:String,
}
impl GeoSlopeProjection {
    pub fn validate(&self)->Result<(),CoreError>{
        let expected=project_geo_section_for_slope(&self.source,&self.polyline_xy_m,self.max_spacing_m)?;
        if &expected!=self{return Err(error("immutable slope projection disagrees with its selected GEO snapshot"));}Ok(())
    }
    pub fn ground_definition(&self)->GroundDefinition{
        GroundDefinition{coordinate_system:self.ground.coordinate_system,terrain_polyline:self.ground.terrain_polyline.clone(),
            layer_interfaces:self.ground.layer_interfaces.clone(),soil_regions:self.ground.soil_regions.clone(),
            bottom_boundary:self.ground.bottom_boundary.clone(),materials:self.ground.materials.clone()}
    }
    pub fn currentness(&self,state:&geo_core::GeoState)->Result<EngineeringSnapshotStatus,CoreError>{
        self.validate()?;
        Ok(match geo_core::snapshot_currentness(state,&self.source.snapshot).map_err(error)?{
            geo_core::SnapshotCurrentness::Current=>EngineeringSnapshotStatus::Current,
            geo_core::SnapshotCurrentness::Stale=>EngineeringSnapshotStatus::Stale,
        })
    }
}

/// Retains TIN facet/unknown-zone section stations supplied by geo_core. Only a
/// complete ordered non-pinched dry stack with known material values is mapped
/// to the existing full-span X/Z solver contract. No analysis is executed.
pub fn project_geo_section_for_slope(source:&geo_core::ResolvedGeoSnapshot,polyline_xy_m:&[[f64;2]],max_spacing_m:f64)->Result<GeoSlopeProjection,CoreError>{
    source.require_explicit_dry().map_err(error)?;
    let queried=geo_core::query_ground(source,&geo_core::GeoQuery::Section{polyline_xy_m:polyline_xy_m.to_vec(),max_spacing_m}).map_err(error)?;
    let section=queried.section.as_ref().ok_or_else(||error("GEO did not return a bounded section"))?;
    if queried.status!=geo_core::QueryStatus::Known||section.samples.len()<2{return Err(error("section is unknown, conflicting or outside coverage"));}
    let first=section.samples[0].result.column.as_ref().ok_or_else(||error("section has no first column"))?;
    let order:Vec<_>=first.layers.iter().map(|l|l.ige_id.clone()).collect();
    if order.is_empty(){return Err(error("slope projection needs an explicitly bounded layer stack"));}
    let mut terrain=Vec::with_capacity(section.samples.len());
    let mut bottoms=vec![Vec::with_capacity(section.samples.len());order.len()];
    for sample in &section.samples{
        let column=sample.result.column.as_ref().ok_or_else(||error("section includes an unknown column"))?;
        if sample.result.status!=geo_core::QueryStatus::Known||column.layers.iter().any(|l|l.pinched)
            ||column.layers.iter().map(|l|&l.ige_id).ne(order.iter()){
            return Err(error("legacy slope adapter does not support missing/pinched or changing layer topology"));
        }
        terrain.push(geotech_core::Point2::new(sample.station_m,column.terrain_abs_m));
        for(index,layer)in column.layers.iter().enumerate(){bottoms[index].push(geotech_core::Point2::new(sample.station_m,layer.bottom_abs_m));}
    }
    let mut materials=BTreeMap::new();
    let mut regions=Vec::new();
    for ige_id in &order{
        let ige=source.snapshot.records.iges.get(ige_id).ok_or_else(||error("section IGE is missing from the immutable closure"))?;
        let soil=source.snapshot.records.soils.get(&ige.soil_id).ok_or_else(||error("section soil is missing from the immutable closure"))?;
        let known=|parameter:&geo_core::Parameter<f64>,name:&str,unit:geo_core::ParameterUnit,effective:bool|{
            let geo_core::Parameter::Known{value,metadata,..}=parameter else{return Err(error(format!("{name} is missing/conflicting; a solver value cannot be invented")));};
            if metadata.unit!=unit||(effective&&metadata.strength_context!=geo_core::StrengthContext::EffectiveDrained){return Err(error(format!("{name} has unsupported units or strength context for the effective drained dry solver")));}
            Ok(*value)
        };
        let material_id=geotech_core::circular::SoilMaterialId(soil.id.as_str().into());
        let provenance=format!("GEO selected immutable snapshot {}; content {}; source IDs {}",source.snapshot.id,source.snapshot.content_fingerprint,
            serde_json::to_string(&source.snapshot.records.sources.keys().map(|id|id.as_str()).collect::<Vec<_>>())?);
        materials.insert(material_id.clone(),GroundMaterial{material_id:material_id.clone(),properties:geotech_core::SoilMaterial{
            name:soil.name.clone(),unit_weight_kn_per_m3:known(&soil.parameters.unit_weight_kn_m3,"unit weight",geo_core::ParameterUnit::KnPerM3,false)?,
            effective_cohesion_kpa:known(&soil.parameters.cohesion_kpa,"cohesion",geo_core::ParameterUnit::Kpa,true)?,
            effective_friction_angle_deg:known(&soil.parameters.friction_angle_deg,"friction",geo_core::ParameterUnit::Degree,true)?,provenance}});
        regions.push(SoilRegion{region_id:geotech_core::circular::SoilRegionId(ige_id.as_str().into()),material_id});
    }
    let bottom_boundary=bottoms.pop().ok_or_else(||error("section lacks a bottom boundary"))?;
    let interfaces=bottoms.into_iter().enumerate().map(|(index,polyline)|LayerInterface{
        interface_id:geotech_core::circular::LayerInterfaceId(format!("geo-bottom-{}",order[index])),polyline}).collect();
    let ground=GroundSection2D{section_id:geotech_core::circular::GroundSectionId(source.snapshot.id.as_str().into()),
        coordinate_system:SectionCoordinateSystem::CartesianXzMetres,terrain_polyline:terrain,layer_interfaces:interfaces,
        soil_regions:regions,bottom_boundary,materials:materials.into_values().collect(),model_revision:source.snapshot.source_revision.operation_id};
    let definition=GroundDefinition{coordinate_system:ground.coordinate_system,terrain_polyline:ground.terrain_polyline.clone(),layer_interfaces:ground.layer_interfaces.clone(),
        soil_regions:ground.soil_regions.clone(),bottom_boundary:ground.bottom_boundary.clone(),materials:ground.materials.clone()};
    let readiness=geotech_core::circular::inspect_ground_readiness(&definition).map_err(error)?;
    if !readiness.is_ready(){return Err(error("mapped legacy slope payload is an incomplete draft"));}
    let content_fingerprint=format!("{:x}",Sha256::digest(serde_json::to_vec(&(&source,polyline_xy_m,max_spacing_m,&ground))?));
    Ok(GeoSlopeProjection{source:source.clone(),polyline_xy_m:polyline_xy_m.to_vec(),max_spacing_m,ground,content_fingerprint})
}
fn error(value:impl fmt::Display)->CoreError{CoreError::InvalidDocument(format!("GEO slope adapter: {value}"))}
