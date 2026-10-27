//! Bounded derived XY mesh sections. No drafting authority or persistent edge IDs.
use super::*;
use ai_revit_core::{DrawingId, ViewDefinitionId};
use ai_revit_geometry::SectionPlane;
use drafting_core::Point2 as DraftPoint;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const MAX_REFERENCE_ELEMENTS: usize = 20_000;
const MAX_REFERENCE_SEGMENTS: usize = 100_000;

#[derive(Clone, Debug)]
pub(super) struct UnderlaySegment {
    pub element_id: ElementId,
    pub start: DraftPoint,
    pub end: DraftPoint,
}

#[derive(Clone, Debug)]
pub(super) struct UnderlayFrame {
    pub segments: Arc<Vec<UnderlaySegment>>,
    pub source_revision: u64,
    pub generation: u64,
    pub status: Value,
}

#[derive(Clone)]
struct CachedSource {
    fingerprint: String,
    elevation_bits: u64,
    segments: Arc<Vec<UnderlaySegment>>,
    status: Value,
}

#[derive(Default)]
pub(super) struct BimUnderlayCache {
    identity: Option<(String, String, u64, DrawingId)>,
    frame: Option<UnderlayFrame>,
    sources: BTreeMap<(ViewDefinitionId, ElementId), CachedSource>,
    generation: u64,
    pub source_checks: u64,
    pub source_regenerations: u64,
    pub reused_sources: u64,
    pub frame_hits: u64,
}

impl BimUnderlayCache {
    pub(super) fn metrics(&self) -> Value {
        json!({"source_checks":self.source_checks,"source_regenerations":self.source_regenerations,
            "reused_sources":self.reused_sources,"frame_hits":self.frame_hits,
            "retained_sources":self.sources.len(),"generation":self.generation,
            "max_sources":MAX_REFERENCE_ELEMENTS,"max_segments":MAX_REFERENCE_SEGMENTS})
    }

    pub(super) fn refresh(
        &mut self,
        document: &Document,
        geometry: &GeometryCache,
        epoch: u64,
        drawing_id: DrawingId,
    ) -> Result<UnderlayFrame, String> {
        let identity = (
            document.document_uuid.to_string(),
            document.runtime_instance_id(),
            epoch,
            drawing_id,
        );
        let unchanged_identity = self.identity.as_ref() == Some(&identity);
        if unchanged_identity {
            if let Some(frame) = self
                .frame
                .as_ref()
                .filter(|f| f.source_revision == document.revision())
            {
                self.frame_hits = self.frame_hits.saturating_add(1);
                return Ok(frame.clone());
            }
        }
        let drawing = document.drawing(drawing_id).ok_or("drawing is absent")?;
        // Only explicitly supported global XY coordinates are projected in v0.5.
        let xy_plane = drafting_core::DrawingPlane::default();
        let supported_plane = drawing.plane == xy_plane;
        let mut proposed = if unchanged_identity {
            self.sources.clone()
        } else {
            BTreeMap::new()
        };
        let mut retained = BTreeSet::new();
        let mut statuses = Vec::new();
        let mut changed = !unchanged_identity;
        let mut count = 0usize;
        for view in document
            .view_definitions()
            .values()
            .filter(|v| v.drawing_id == drawing_id)
        {
            let elevation = view.source.elevation_m.unwrap_or(0.0);
            if !elevation.is_finite() {
                return Err("reference elevation must be finite".into());
            }
            for id in &view.source.element_ids {
                count = count.checked_add(1).ok_or("reference count overflow")?;
                if count > MAX_REFERENCE_ELEMENTS {
                    return Err("BIM reference source budget exceeded".into());
                }
                let key = (view.id, *id);
                retained.insert(key);
                self.source_checks = self.source_checks.saturating_add(1);
                let record = geometry.record(*id);
                let fingerprint = if !supported_plane {
                    "unsupported_drawing_plane".into()
                } else if document.element(*id).is_none() {
                    "missing_bim_source".into()
                } else if let Some(record) = record.filter(|r| r.valid) {
                    record.geometry_fingerprint.clone()
                } else {
                    "geometry_unavailable".into()
                };
                if let Some(cached) = proposed.get(&key).filter(|c| {
                    c.fingerprint == fingerprint && c.elevation_bits == elevation.to_bits()
                }) {
                    self.reused_sources = self.reused_sources.saturating_add(1);
                    statuses.push(cached.status.clone());
                    continue;
                }
                changed = true;
                let mut segments = Vec::new();
                let (state, reason) = if !supported_plane {
                    (
                        "unsupported",
                        Some("drawing plane is not global XY".to_owned()),
                    )
                } else if document.element(*id).is_none() {
                    (
                        "missing",
                        Some("referenced BIM ElementId no longer exists".to_owned()),
                    )
                } else if record.is_none_or(|r| !r.valid) {
                    (
                        "unavailable",
                        Some("current BIM geometry is unavailable".to_owned()),
                    )
                } else {
                    match geometry.section(
                        &[*id],
                        SectionPlane {
                            origin: Vec3::new(0.0, 0.0, elevation),
                            normal: Vec3::new(0.0, 0.0, 1.0),
                        },
                    ) {
                        Ok(section) => {
                            for polyline in section.loops {
                                for pair in polyline.points.windows(2) {
                                    push_segment(&mut segments, *id, pair[0], pair[1])?;
                                }
                                if polyline.closed && polyline.points.len() > 2 {
                                    push_segment(
                                        &mut segments,
                                        *id,
                                        *polyline.points.last().unwrap(),
                                        polyline.points[0],
                                    )?;
                                }
                            }
                            ("available", None)
                        }
                        Err(error) => ("unavailable", Some(error.to_string())),
                    }
                };
                self.source_regenerations = self.source_regenerations.saturating_add(1);
                let status = json!({"domain":"bim","element_id":id.get().to_string(),"view_definition_id":view.id,
                    "state":state,"reason":reason,"quality":"approximate_mesh_projection","method":"mesh_section_xy",
                    "coordinate_system":"xy","elevation_m":elevation,"segment_count":segments.len(),
                    "geometry_fingerprint":record.map(|r|r.geometry_fingerprint.as_str()),
                    "geometry_revision":record.map(|r|r.geometry_revision),"mesh_revision":record.map(|r|r.mesh_revision)});
                statuses.push(status.clone());
                proposed.insert(
                    key,
                    CachedSource {
                        fingerprint,
                        elevation_bits: elevation.to_bits(),
                        segments: Arc::new(segments),
                        status,
                    },
                );
            }
        }
        if proposed.len() != retained.len() {
            changed = true;
        }
        proposed.retain(|key, _| retained.contains(key));
        let total: usize = proposed
            .values()
            .map(|v| v.segments.len())
            .try_fold(0usize, |a, b| a.checked_add(b))
            .ok_or("reference segment count overflow")?;
        if total > MAX_REFERENCE_SEGMENTS {
            return Err("BIM reference display segment budget exceeded".into());
        }
        let segments = if !changed {
            self.frame
                .as_ref()
                .map(|f| f.segments.clone())
                .unwrap_or_else(|| Arc::new(Vec::new()))
        } else {
            let mut combined = Vec::with_capacity(total);
            for source in proposed.values() {
                combined.extend(source.segments.iter().cloned());
            }
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or("BIM projection generation exhausted")?;
            Arc::new(combined)
        };
        let frame = UnderlayFrame {
            segments,
            source_revision: document.revision(),
            generation: self.generation,
            status: json!({"quality":"approximate_mesh_projection","coordinate_system":"xy","default_elevation_m":0.0,
                "document_id":document.document_uuid,"runtime_instance_id":document.runtime_instance_id(),"document_epoch":epoch,
                "source_revision":document.revision(),"generation":self.generation,"source_count":count,
                "segment_count":total,"sources":statuses,"limits":{"sources":MAX_REFERENCE_ELEMENTS,"segments":MAX_REFERENCE_SEGMENTS}}),
        };
        self.identity = Some(identity);
        self.sources = proposed;
        self.frame = Some(frame.clone());
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_revit_core::{
        BUILTIN_WALL_CLASS_ID, DraftCommand, DraftCoordinateSystem, DraftGeometry, DraftPoint2,
        DraftSourceReference, DraftStyle, WallSpec,
    };

    fn commit(d: &mut Document, id: &str, commands: Vec<Command>) -> ai_revit_core::CommitReceipt {
        let mut transaction = d.begin_transaction(id, d.revision());
        for command in commands {
            transaction.push(command);
        }
        d.commit(transaction).unwrap()
    }
    fn wall(length: f64) -> ElementKind {
        ElementKind::Wall(WallSpec {
            start: Point2::new(0., 0.),
            end: Point2::new(length, 0.),
            base_elevation_m: 0.,
            height_m: 3.,
            thickness_m: 0.2,
        })
    }

    #[test]
    fn bim_source_edit_regenerates_only_dependency_and_preserves_manual_on_undo() {
        let mut d = Document::new("underlay").unwrap();
        let a = commit(
            &mut d,
            "a",
            vec![Command::CreateElement {
                name: "A".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                kind: wall(5.),
                properties: BTreeMap::new(),
            }],
        )
        .created_elements[0];
        let b = commit(
            &mut d,
            "b",
            vec![Command::CreateElement {
                name: "B".into(),
                class_id: BUILTIN_WALL_CLASS_ID.into(),
                kind: wall(3.),
                properties: BTreeMap::new(),
            }],
        )
        .created_elements[0];
        let drawing = commit(
            &mut d,
            "drawing",
            vec![Command::Draft(DraftCommand::CreateDrawing {
                name: "XY".into(),
                plane: Default::default(),
            })],
        )
        .created_drawings[0];
        let layer = *d.drawing(drawing).unwrap().layers.keys().next().unwrap();
        let manual = commit(
            &mut d,
            "manual",
            vec![Command::Draft(DraftCommand::CreateDraftEntity {
                drawing_id: drawing,
                layer_id: layer,
                geometry: DraftGeometry::Line {
                    start: DraftPoint2::new(9., 1.),
                    end: DraftPoint2::new(10., 2.),
                },
                style: DraftStyle::default(),
            })],
        )
        .created_draft_entities[0];
        commit(
            &mut d,
            "reference",
            vec![Command::Draft(DraftCommand::CreateViewDefinition {
                drawing_id: drawing,
                name: "section".into(),
                source: DraftSourceReference {
                    element_ids: vec![a, b],
                    elevation_m: Some(1.),
                    coordinate_system: DraftCoordinateSystem::Xy,
                },
            })],
        );
        let manual_before = d.draft_entity(drawing, manual).unwrap().clone();
        let mut geometry = GeometryCache::new();
        geometry.rebuild(&d);
        let mut cache = BimUnderlayCache::default();
        let first = cache.refresh(&d, &geometry, 1, drawing).unwrap();
        assert!(!first.segments.is_empty());
        assert_eq!(cache.source_regenerations, 2);
        let hit = cache.refresh(&d, &geometry, 1, drawing).unwrap();
        assert!(Arc::ptr_eq(&first.segments, &hit.segments));
        assert_eq!(cache.source_checks, 2);
        commit(
            &mut d,
            "edit-a",
            vec![Command::UpdateElement {
                element_id: a,
                kind: wall(7.),
            }],
        );
        geometry.rebuild(&d);
        let changed = cache.refresh(&d, &geometry, 1, drawing).unwrap();
        assert_eq!(cache.source_regenerations, 3);
        assert_eq!(cache.reused_sources, 1);
        assert_ne!(first.generation, changed.generation);
        assert_eq!(d.draft_entity(drawing, manual), Some(&manual_before));
        d.undo().unwrap();
        geometry.rebuild(&d);
        let undone = cache.refresh(&d, &geometry, 1, drawing).unwrap();
        assert_eq!(cache.source_regenerations, 4);
        assert_eq!(d.draft_entity(drawing, manual), Some(&manual_before));
        assert_eq!(first.segments.len(), undone.segments.len());
        for (x, y) in first.segments.iter().zip(undone.segments.iter()) {
            assert_eq!(
                (x.element_id, x.start, x.end),
                (y.element_id, y.start, y.end)
            );
        }
        let before = cache.source_regenerations;
        let cloned = d.clone();
        cache.refresh(&cloned, &geometry, 1, drawing).unwrap();
        assert_eq!(
            cache.source_regenerations,
            before + 2,
            "different runtime owner cannot reuse stale frame"
        );
    }
}

fn push_segment(
    out: &mut Vec<UnderlaySegment>,
    element_id: ElementId,
    a: Vec3,
    b: Vec3,
) -> Result<(), String> {
    if !a.is_finite() || !b.is_finite() {
        return Err("nonfinite BIM section output".into());
    }
    if a.x == b.x && a.y == b.y {
        return Ok(());
    }
    if out.len() >= MAX_REFERENCE_SEGMENTS {
        return Err("BIM reference segment budget exceeded".into());
    }
    out.push(UnderlaySegment {
        element_id,
        start: DraftPoint::new(a.x, a.y),
        end: DraftPoint::new(b.x, b.y),
    });
    Ok(())
}

impl App {
    pub(super) fn bim_underlay_frame(
        &mut self,
        drawing_id: DrawingId,
    ) -> Result<UnderlayFrame, String> {
        self.bim_underlay_cache.refresh(
            self.document.as_ref().ok_or("no document is open")?,
            &self.geometry_cache,
            self.api_document_generation,
            drawing_id,
        )
    }
}
