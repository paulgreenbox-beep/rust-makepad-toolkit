use crate::{
    collision_demo_scene, exact_acceptance_scene, large_demo_scene,
    procedural_test_scene, Camera, CameraError, NavigationPivot, ObjectId, PickHit, Ray, Scene,
    SectionBounds, SectionBox, SectionBoxError, SectionMode, SurfaceAnchor, Vec2, Vec3,
    ViewerEvent, ViewerState, Viewport,
};

/// Coordinates camera navigation, scene picking, selection, and semantic events.
#[derive(Debug)]
pub struct Viewer {
    camera: Camera,
    scene: Scene,
    viewport: Viewport,
    navigation_pivot: NavigationPivot,
    hover_hit: Option<PickHit>,
    section_box: SectionBox,
    isolated_object_ids: Vec<ObjectId>,
    pending_events: Vec<ViewerEvent>,
}

impl Default for Viewer {
    fn default() -> Self {
        let camera = Camera::default();
        Self {
            camera,
            scene: Scene::new(),
            viewport: Viewport::new(1.0, 1.0),
            navigation_pivot: NavigationPivot {
                object_id: None,
                world_point: camera.pivot,
            },
            hover_hit: None,
            section_box: SectionBox::default(),
            isolated_object_ids: Vec::new(),
            pending_events: Vec::new(),
        }
    }
}

impl Viewer {
    /// Creates a viewer after validating camera and viewport values.
    pub fn new(camera: Camera, scene: Scene, viewport: Viewport) -> Result<Self, CameraError> {
        camera.validate()?;
        if !viewport.is_valid() {
            return Err(CameraError::InvalidViewport);
        }
        let section_box = scene
            .visible_bounds()
            .and_then(|bounds| SectionBounds::from_aabb(bounds).ok())
            .map(|bounds| SectionBox {
                bounds,
                ..SectionBox::default()
            })
            .unwrap_or_default();
        Ok(Self {
            navigation_pivot: NavigationPivot {
                object_id: None,
                world_point: camera.pivot,
            },
            camera,
            scene,
            viewport,
            hover_hit: None,
            section_box,
            isolated_object_ids: Vec::new(),
            pending_events: Vec::new(),
        })
    }

    /// Creates a viewer with the deterministic procedural scene and a 1280 by 720 viewport.
    pub fn demo() -> Self {
        let camera = Camera::default();
        let scene = procedural_test_scene();
        let section_box = scene
            .visible_bounds()
            .and_then(|bounds| SectionBounds::from_aabb(bounds).ok())
            .map(|bounds| SectionBox {
                bounds,
                ..SectionBox::default()
            })
            .unwrap_or_default();
        Self {
            camera,
            scene,
            viewport: Viewport::new(1280.0, 720.0),
            navigation_pivot: NavigationPivot {
                object_id: None,
                world_point: camera.pivot,
            },
            hover_hit: None,
            section_box,
            isolated_object_ids: Vec::new(),
            pending_events: Vec::new(),
        }
    }

    /// Creates a fitted meter-scale engineering-rack fixture with 132 stable scene objects.
    pub fn large_demo() -> Self {
        let camera = Camera::default();
        let scene = large_demo_scene();
        let viewport = Viewport::new(1280.0, 720.0);
        let mut viewer = Self::new(camera, scene, viewport)
            .expect("the built-in large fixture uses a valid camera and viewport");
        if let Some(bounds) = viewer.scene.visible_bounds() {
            viewer
                .camera
                .fit_bounds(bounds, viewport, 1.15)
                .expect("the built-in large fixture has valid bounds");
            viewer.navigation_pivot.world_point = viewer.camera.pivot;
        }
        viewer
    }

    /// Creates a fitted viewer for collision and minimal-context acceptance checks.
    pub fn collision_demo() -> Self {
        let camera = Camera::default();
        let scene = collision_demo_scene();
        let viewport = Viewport::new(1280.0, 720.0);
        let mut viewer = Self::new(camera, scene, viewport)
            .expect("the built-in collision fixture uses a valid camera and viewport");
        if let Some(bounds) = viewer.scene.visible_bounds() {
            viewer
                .camera
                .fit_bounds(bounds, viewport, 1.15)
                .expect("the built-in collision fixture has valid bounds");
            viewer.navigation_pivot.world_point = viewer.camera.pivot;
        }
        viewer
    }
    /// Creates four separated pairs for exact collision acceptance.
    pub fn exact_acceptance() -> Self {
        let camera = Camera::default();
        let scene = exact_acceptance_scene();
        let viewport = Viewport::new(1280.0, 720.0);
        let mut viewer = Self::new(camera, scene, viewport)
            .expect("the exact acceptance fixture uses a valid camera and viewport");
        if let Some(bounds) = viewer.scene.visible_bounds() {
            viewer
                .camera
                .fit_bounds(bounds, viewport, 1.15)
                .expect("the exact acceptance fixture has valid bounds");
            viewer.navigation_pivot.world_point = viewer.camera.pivot;
        }
        viewer
    }
    /// Returns an immutable reference to the active camera.
    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// Returns a mutable reference to the active camera for application-specific updates.
    pub fn camera_mut(&mut self) -> &mut Camera {
        &mut self.camera
    }

    /// Returns an immutable reference to the scene registry.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Returns a mutable reference to the scene registry.
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// Returns the current viewport dimensions.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Returns the current object-aware world-space navigation pivot.
    pub fn navigation_pivot(&self) -> NavigationPivot {
        self.navigation_pivot
    }

    /// Returns the last visible surface hit under the pointer, if any.
    pub fn hover_hit(&self) -> Option<PickHit> {
        self.hover_hit
    }

    /// Returns the object IDs retained by the current isolate operation.
    pub fn isolated_object_ids(&self) -> &[ObjectId] {
        &self.isolated_object_ids
    }

    /// Returns the current visualization-only Section Box by value.
    pub fn section_box(&self) -> SectionBox {
        self.section_box
    }

    /// Returns the current Section Box by value for state-oriented API clients.
    pub fn get_section_box(&self) -> SectionBox {
        self.section_box()
    }

    /// Replaces the Section Box after validating all six bounds.
    pub fn set_section_box(
        &mut self,
        section_box: SectionBox,
    ) -> Result<Vec<ViewerEvent>, SectionBoxError> {
        if !section_box.is_valid() {
            return Err(SectionBoxError::InvalidBounds);
        }
        Ok(self.replace_section_box(section_box))
    }

    /// Replaces all six Section Box planes and emits a semantic change event.
    pub fn set_section_box_bounds(
        &mut self,
        xmin: f64,
        xmax: f64,
        ymin: f64,
        ymax: f64,
        zmin: f64,
        zmax: f64,
    ) -> Result<Vec<ViewerEvent>, SectionBoxError> {
        let bounds = SectionBounds::new(xmin, xmax, ymin, ymax, zmin, zmax)?;
        let mut section_box = self.section_box;
        section_box.bounds = bounds;
        self.set_section_box(section_box)
    }

    /// Enables Section Box clipping and emits an event when its state changes.
    pub fn enable_section_box(&mut self) -> Vec<ViewerEvent> {
        let mut section_box = self.section_box;
        section_box.enabled = true;
        self.replace_section_box(section_box)
    }

    /// Disables Section Box clipping and emits an event when its state changes.
    pub fn disable_section_box(&mut self) -> Vec<ViewerEvent> {
        let mut section_box = self.section_box;
        section_box.enabled = false;
        self.replace_section_box(section_box)
    }

    /// Expands every Section Box plane by `distance` scene units.
    pub fn expand_section_box(
        &mut self,
        distance: f64,
    ) -> Result<Vec<ViewerEvent>, SectionBoxError> {
        let mut section_box = self.section_box;
        section_box.expand(distance)?;
        self.set_section_box(section_box)
    }

    /// Fits the Section Box around all currently visible scene geometry and enables it.
    pub fn fit_section_box_to_model(
        &mut self,
        margin: f64,
    ) -> Result<Vec<ViewerEvent>, SectionBoxError> {
        if !margin.is_finite() || margin < 0.0 {
            return Err(SectionBoxError::InvalidMargin);
        }
        let Some(bounds) = self.scene.visible_bounds() else {
            return Ok(Vec::new());
        };
        let bounds = SectionBounds::from_aabb(bounds)?.expanded(margin)?;
        let mut section_box = self.section_box;
        section_box.bounds = bounds;
        section_box.enabled = true;
        self.set_section_box(section_box)
    }

    /// Fits the Section Box around the selected objects and enables it.
    pub fn fit_section_box_to_selection(
        &mut self,
        margin: f64,
    ) -> Result<Vec<ViewerEvent>, SectionBoxError> {
        let selected = self.scene.selected_ids();
        self.fit_section_box_to_objects(&selected, margin)
    }

    /// Fits the Section Box around the listed objects and enables it.
    /// Unknown and geometry-free IDs are ignored; hidden objects retain bounds.
    pub fn fit_section_box_to_objects(
        &mut self,
        object_ids: &[ObjectId],
        margin: f64,
    ) -> Result<Vec<ViewerEvent>, SectionBoxError> {
        if !margin.is_finite() || margin < 0.0 {
            return Err(SectionBoxError::InvalidMargin);
        }
        let bounds = object_ids
            .iter()
            .filter_map(|id| self.scene.get(*id))
            .filter_map(|object| object.world_bounds())
            .reduce(|left, right| left.union(right));
        let Some(bounds) = bounds else {
            return Ok(Vec::new());
        };
        let bounds = SectionBounds::from_aabb(bounds)?.expanded(margin)?;
        let mut section_box = self.section_box;
        section_box.bounds = bounds;
        section_box.enabled = true;
        self.set_section_box(section_box)
    }

    /// Resets the box to the current visible model extent and disables clipping.
    pub fn reset_section_box(&mut self) -> Vec<ViewerEvent> {
        let mut section_box = self.section_box;
        section_box.bounds = self
            .scene
            .visible_bounds()
            .and_then(|bounds| SectionBounds::from_aabb(bounds).ok())
            .unwrap_or_else(|| SectionBox::default().bounds);
        section_box.enabled = false;
        section_box.mode = SectionMode::Inside;
        self.replace_section_box(section_box)
    }

    /// Changes viewport dimensions after validating them.
    pub fn set_viewport(&mut self, viewport: Viewport) -> Result<(), CameraError> {
        if !viewport.is_valid() {
            return Err(CameraError::InvalidViewport);
        }
        self.viewport = viewport;
        Ok(())
    }

    /// Returns and clears semantic events produced since the previous drain.
    pub fn drain_events(&mut self) -> Vec<ViewerEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Finds the closest visible surface hit by the pointer pixel.
    pub fn pick_screen(&self, screen: Vec2) -> Result<Option<PickHit>, CameraError> {
        let ray = self.camera.ray_from_screen(screen, self.viewport)?;
        Ok(self.pick_ray(ray))
    }

    /// Updates the hover hit for a pointer pixel and returns a hover event on change.
    pub fn update_hover(&mut self, screen: Vec2) -> Result<Vec<ViewerEvent>, CameraError> {
        let hit = self.pick_screen(screen)?;
        let events = self.update_hover_hit(hit);
        self.record(&events);
        Ok(events)
    }

    /// Clears the current pointer hover hit and returns a hover event when state changes.
    pub fn clear_hover(&mut self) -> Vec<ViewerEvent> {
        let events = self.update_hover_hit(None);
        self.record(&events);
        events
    }

    /// Raycasts the pointer pixel and records an object-picked event when there is a hit.
    pub fn raycast(&mut self, screen: Vec2) -> Result<Option<PickHit>, CameraError> {
        let hit = self.pick_screen(screen)?;
        let mut events = self.update_hover_hit(hit);
        if let Some(hit) = hit {
            events.push(ViewerEvent::ObjectPicked { hit });
        }
        self.record(&events);
        Ok(hit)
    }

    /// Raycasts an explicit world-space ray against visible scene geometry.
    pub fn raycast_ray(&self, ray: Ray) -> Option<PickHit> {
        self.pick_ray(ray)
    }

    /// Picks at a pixel and updates selection, returning and queuing semantic events caused by the click.
    pub fn select_at(
        &mut self,
        screen: Vec2,
        additive: bool,
    ) -> Result<Vec<ViewerEvent>, CameraError> {
        let hit = self.pick_screen(screen)?;
        let mut events = self.update_hover_hit(hit);
        let selected_before = self.scene.selected_ids();
        self.scene.select(hit.map(|item| item.object_id), additive);
        let selected_after = self.scene.selected_ids();
        if let Some(hit) = hit {
            events.push(ViewerEvent::ObjectPicked { hit });
        }
        if selected_before != selected_after {
            events.push(ViewerEvent::SelectionChanged {
                selected_object_ids: selected_after,
            });
        }
        self.record(&events);
        Ok(events)
    }

    /// Selects or clears an object and returns and queues a selection event when the set changes.
    pub fn select(&mut self, object_id: Option<ObjectId>, additive: bool) -> Vec<ViewerEvent> {
        let before = self.scene.selected_ids();
        self.scene.select(object_id, additive);
        let after = self.scene.selected_ids();
        let events = if before == after {
            Vec::new()
        } else {
            vec![ViewerEvent::SelectionChanged {
                selected_object_ids: after,
            }]
        };
        self.record(&events);
        events
    }

    /// Replaces the selection with the registered visible IDs and emits one selection event.
    pub fn select_many(&mut self, object_ids: &[ObjectId]) -> Vec<ViewerEvent> {
        let before = self.scene.selected_ids();
        self.scene.restore_selection(object_ids);
        let after = self.scene.selected_ids();
        let events = if before == after {
            Vec::new()
        } else {
            vec![ViewerEvent::SelectionChanged {
                selected_object_ids: after,
            }]
        };
        self.record(&events);
        events
    }

    /// Toggles one object's selection and returns and queues a selection event when it changes.
    pub fn toggle_selection(&mut self, object_id: ObjectId) -> Vec<ViewerEvent> {
        let before = self.scene.selected_ids();
        self.scene.toggle_selection(object_id);
        let after = self.scene.selected_ids();
        let events = if before == after {
            Vec::new()
        } else {
            vec![ViewerEvent::SelectionChanged {
                selected_object_ids: after,
            }]
        };
        self.record(&events);
        events
    }

    /// Changes one object's visibility and reports visibility, selection, and highlight changes.
    pub fn set_visible(&mut self, object_id: ObjectId, visible: bool) -> Vec<ViewerEvent> {
        let Some(old_object) = self.scene.get(object_id) else {
            return Vec::new();
        };
        if old_object.visible == visible {
            return Vec::new();
        }
        let selected_before = self.scene.selected_ids();
        let highlighted_before = self.scene.highlighted_id();
        self.scene.set_visible(object_id, visible);
        let mut events = vec![
            ViewerEvent::VisibilityChanged { object_id, visible },
            if visible {
                ViewerEvent::ObjectShown { object_id }
            } else {
                ViewerEvent::ObjectHidden { object_id }
            },
        ];
        let selected_after = self.scene.selected_ids();
        if selected_before != selected_after {
            events.push(ViewerEvent::SelectionChanged {
                selected_object_ids: selected_after,
            });
        }
        let highlighted_after = self.scene.highlighted_id();
        if highlighted_before != highlighted_after {
            events.push(ViewerEvent::HighlightChanged {
                object_id: highlighted_after,
            });
        }
        if !self.isolated_object_ids.is_empty()
            && visible != self.isolated_object_ids.contains(&object_id)
        {
            self.isolated_object_ids.clear();
            events.push(ViewerEvent::IsolationChanged {
                object_ids: Vec::new(),
            });
        }
        if !visible && self.navigation_pivot.object_id == Some(object_id) {
            let pivot = NavigationPivot {
                object_id: None,
                world_point: self.camera.target,
            };
            if self.navigation_pivot != pivot {
                self.set_navigation_pivot(pivot);
                events.push(ViewerEvent::NavigationPivotChanged { pivot });
            }
        }
        self.record(&events);
        events
    }

    /// Hides one object and reports the semantic state changes.
    pub fn hide(&mut self, object_id: ObjectId) -> Vec<ViewerEvent> {
        self.set_visible(object_id, false)
    }

    /// Shows one object and reports the semantic state changes.
    pub fn show(&mut self, object_id: ObjectId) -> Vec<ViewerEvent> {
        self.set_visible(object_id, true)
    }

    /// Hides every other object while keeping the requested registered object visible.
    pub fn isolate(&mut self, object_id: ObjectId) -> Vec<ViewerEvent> {
        self.isolate_many(&[object_id])
    }

    /// Keeps the listed registered objects visible and hides every other scene object.
    pub fn isolate_many(&mut self, object_ids: &[ObjectId]) -> Vec<ViewerEvent> {
        let mut isolated: Vec<ObjectId> = object_ids
            .iter()
            .copied()
            .filter(|id| self.scene.get(*id).is_some())
            .collect();
        isolated.sort_unstable();
        isolated.dedup();
        if isolated.is_empty() {
            return self.unisolate();
        }
        let selected_before = self.scene.selected_ids();
        let highlighted_before = self.scene.highlighted_id();
        let before_isolated = self.isolated_object_ids.clone();
        let mut events = Vec::new();
        let ids: Vec<ObjectId> = self.scene.object_ids().collect();
        for id in ids {
            let visible = isolated.contains(&id);
            if self
                .scene
                .get(id)
                .map_or(false, |object| object.visible != visible)
            {
                self.scene.set_visible(id, visible);
                events.push(ViewerEvent::VisibilityChanged {
                    object_id: id,
                    visible,
                });
            }
        }
        let selected_after = self.scene.selected_ids();
        if selected_before != selected_after {
            events.push(ViewerEvent::SelectionChanged {
                selected_object_ids: selected_after,
            });
        }
        let highlighted_after = self.scene.highlighted_id();
        if highlighted_before != highlighted_after {
            events.push(ViewerEvent::HighlightChanged {
                object_id: highlighted_after,
            });
        }
        self.isolated_object_ids = isolated.clone();
        if before_isolated != isolated {
            events.push(ViewerEvent::IsolationChanged {
                object_ids: isolated.clone(),
            });
            events.push(ViewerEvent::ObjectIsolated {
                object_ids: isolated.clone(),
            });
        }
        if self
            .navigation_pivot
            .object_id
            .map_or(false, |id| !isolated.contains(&id))
        {
            let pivot = NavigationPivot {
                object_id: None,
                world_point: self.camera.target,
            };
            self.set_navigation_pivot(pivot);
            events.push(ViewerEvent::NavigationPivotChanged { pivot });
        }
        self.record(&events);
        events
    }

    /// Makes all scene objects visible and clears isolate state.
    pub fn unisolate(&mut self) -> Vec<ViewerEvent> {
        self.show_all()
    }

    /// Makes every registered scene object visible and clears isolate state.
    pub fn show_all(&mut self) -> Vec<ViewerEvent> {
        let ids: Vec<ObjectId> = self.scene.object_ids().collect();
        let mut events = Vec::new();
        for id in ids {
            events.extend(self.set_visible(id, true));
        }
        if !self.isolated_object_ids.is_empty() {
            self.isolated_object_ids.clear();
            let isolation = ViewerEvent::IsolationChanged {
                object_ids: Vec::new(),
            };
            self.record(std::slice::from_ref(&isolation));
            events.push(isolation);
        }
        events
    }

    /// Sets or clears the transient highlighted object and reports the change.
    pub fn set_highlighted(&mut self, object_id: Option<ObjectId>) -> Vec<ViewerEvent> {
        let before = self.scene.highlighted_id();
        let after = self.scene.set_highlighted(object_id);
        let events = if before == after {
            Vec::new()
        } else {
            vec![ViewerEvent::HighlightChanged { object_id: after }]
        };
        self.record(&events);
        events
    }

    /// Orbits the camera and returns and queues its updated serializable state.
    pub fn orbit(
        &mut self,
        yaw_radians: f64,
        pitch_radians: f64,
    ) -> Result<ViewerEvent, CameraError> {
        self.camera.orbit_about(
            self.navigation_pivot.world_point,
            yaw_radians,
            pitch_radians,
        )?;
        let event = ViewerEvent::CameraChanged {
            camera: self.camera.state(),
        };
        self.record(std::slice::from_ref(&event));
        Ok(event)
    }

    /// Pans the camera by a screen-space drag and returns and queues its updated state.
    pub fn pan_pixels(&mut self, delta: Vec2) -> Result<ViewerEvent, CameraError> {
        let pivot_before = self.navigation_pivot;
        self.camera.pan_pixels(delta, self.viewport)?;
        if self.navigation_pivot.object_id.is_some() {
            self.camera.pivot = self.navigation_pivot.world_point;
        } else {
            self.navigation_pivot.world_point = self.camera.pivot;
        }
        if pivot_before != self.navigation_pivot {
            let pivot_event = ViewerEvent::NavigationPivotChanged {
                pivot: self.navigation_pivot,
            };
            self.record(std::slice::from_ref(&pivot_event));
        }
        let event = ViewerEvent::CameraChanged {
            camera: self.camera.state(),
        };
        self.record(std::slice::from_ref(&event));
        Ok(event)
    }

    /// Pans at a measured surface depth and returns and queues all resulting semantic events.
    pub fn pan_pixels_at_depth(
        &mut self,
        delta: Vec2,
        depth: f64,
    ) -> Result<Vec<ViewerEvent>, CameraError> {
        let pivot_before = self.navigation_pivot;
        self.camera
            .pan_pixels_at_depth(delta, depth, self.viewport)?;
        if self.navigation_pivot.object_id.is_some() {
            self.camera.pivot = self.navigation_pivot.world_point;
        } else {
            self.navigation_pivot.world_point = self.camera.pivot;
        }
        let mut events = Vec::new();
        if pivot_before != self.navigation_pivot {
            events.push(ViewerEvent::NavigationPivotChanged {
                pivot: self.navigation_pivot,
            });
        }
        events.push(ViewerEvent::CameraChanged {
            camera: self.camera.state(),
        });
        self.record(&events);
        Ok(events)
    }

    /// Sets the navigation pivot to the pointer's visible hit, or the current camera target.
    pub fn set_pivot_at_screen(&mut self, screen: Vec2) -> Result<Vec<ViewerEvent>, CameraError> {
        let hit = self.pick_screen(screen)?;
        let mut events = self.update_hover_hit(hit);
        let pivot = hit.map_or(
            NavigationPivot {
                object_id: None,
                world_point: self.camera.target,
            },
            |hit| NavigationPivot {
                object_id: Some(hit.object_id),
                world_point: hit.world_point,
            },
        );
        let before = self.navigation_pivot;
        self.set_navigation_pivot(pivot);
        if before != self.navigation_pivot {
            events.push(ViewerEvent::NavigationPivotChanged {
                pivot: self.navigation_pivot,
            });
        }
        self.record(&events);
        Ok(events)
    }

    /// Dollies around a surface hit beneath a pixel, falling back to the target pivot when empty.
    pub fn dolly_at_screen(
        &mut self,
        screen: Vec2,
        factor: f64,
    ) -> Result<ViewerEvent, CameraError> {
        let hit = self.pick_screen(screen)?;
        let mut events = self.update_hover_hit(hit);
        let pivot_before = self.navigation_pivot;
        if let Some(hit) = hit {
            self.set_navigation_pivot(NavigationPivot {
                object_id: Some(hit.object_id),
                world_point: hit.world_point,
            });
        }
        let anchor = SurfaceAnchor {
            point: self.navigation_pivot.world_point,
        };
        self.camera.dolly_at(factor, Some(anchor))?;
        if pivot_before != self.navigation_pivot {
            events.push(ViewerEvent::NavigationPivotChanged {
                pivot: self.navigation_pivot,
            });
        }
        let event = ViewerEvent::CameraChanged {
            camera: self.camera.state(),
        };
        events.push(event.clone());
        self.record(&events);
        Ok(event)
    }

    /// Fits all visible scene geometry in the current viewport.
    pub fn fit_scene(&mut self, margin: f64) -> Result<Option<ViewerEvent>, CameraError> {
        let Some(bounds) = self.scene.visible_bounds() else {
            return Ok(None);
        };
        self.camera.fit_bounds(bounds, self.viewport, margin)?;
        let pivot = NavigationPivot {
            object_id: None,
            world_point: bounds.center(),
        };
        let pivot_changed = self.navigation_pivot != pivot;
        self.set_navigation_pivot(pivot);
        let mut events = Vec::new();
        if pivot_changed {
            events.push(ViewerEvent::NavigationPivotChanged { pivot });
        }
        let event = ViewerEvent::CameraChanged {
            camera: self.camera.state(),
        };
        events.push(event.clone());
        self.record(&events);
        Ok(Some(event))
    }

    /// Fits one visible object's world bound and attaches the navigation pivot to its center.
    pub fn fit_object(
        &mut self,
        object_id: ObjectId,
        margin: f64,
    ) -> Result<Option<ViewerEvent>, CameraError> {
        let Some(bounds) = self
            .scene
            .get(object_id)
            .filter(|object| object.visible)
            .and_then(|object| object.world_bounds())
        else {
            return Ok(None);
        };
        self.camera.fit_bounds(bounds, self.viewport, margin)?;
        let pivot = NavigationPivot {
            object_id: Some(object_id),
            world_point: bounds.center(),
        };
        let pivot_changed = self.navigation_pivot != pivot;
        self.set_navigation_pivot(pivot);
        let mut events = Vec::new();
        if pivot_changed {
            events.push(ViewerEvent::NavigationPivotChanged { pivot });
        }
        let event = ViewerEvent::CameraChanged {
            camera: self.camera.state(),
        };
        events.push(event.clone());
        self.record(&events);
        Ok(Some(event))
    }

    /// Fits the selected objects and uses their combined center as the navigation pivot.
    pub fn fit_selection(&mut self, margin: f64) -> Result<Option<ViewerEvent>, CameraError> {
        let selected = self.scene.selected_ids();
        if selected.is_empty() {
            return Ok(None);
        }
        let bounds = selected
            .iter()
            .filter_map(|id| self.scene.get(*id).and_then(|object| object.world_bounds()))
            .reduce(|left, right| left.union(right));
        let Some(bounds) = bounds else {
            return Ok(None);
        };
        self.camera.fit_bounds(bounds, self.viewport, margin)?;
        let object_id = if selected.len() == 1 {
            selected.first().copied()
        } else {
            None
        };
        let pivot = NavigationPivot {
            object_id,
            world_point: bounds.center(),
        };
        let pivot_changed = self.navigation_pivot != pivot;
        self.set_navigation_pivot(pivot);
        let mut events = Vec::new();
        if pivot_changed {
            events.push(ViewerEvent::NavigationPivotChanged { pivot });
        }
        let event = ViewerEvent::CameraChanged {
            camera: self.camera.state(),
        };
        events.push(event.clone());
        self.record(&events);
        Ok(Some(event))
    }

    /// Fits visible geometry using the default 15 percent clearance.
    pub fn fit(&mut self) -> Result<Option<ViewerEvent>, CameraError> {
        self.fit_scene(1.15)
    }

    /// Captures the current camera and scene interaction state.
    pub fn capture_state(&self) -> ViewerState {
        ViewerState::capture_full(
            &self.camera,
            &self.scene,
            self.navigation_pivot,
            self.hover_hit,
            &self.isolated_object_ids,
            self.section_box,
        )
    }

    /// Returns the current serializable camera and scene interaction state.
    pub fn get_viewer_state(&self) -> ViewerState {
        self.capture_state()
    }

    /// Encodes the current viewer state as pretty-printed JSON.
    pub fn snapshot_json(&self) -> Result<String, serde_json::Error> {
        self.capture_state().to_json()
    }

    /// Restores a previously captured state and returns and queues semantic change events.
    pub fn restore_state(&mut self, state: &ViewerState) -> Result<Vec<ViewerEvent>, CameraError> {
        let before_selection = self.scene.selected_ids();
        let before_camera = self.camera.state();
        let before_pivot = self.navigation_pivot;
        let before_hover = self.hover_hit;
        let before_isolated = self.isolated_object_ids.clone();
        let before_visibility: Vec<(ObjectId, bool)> = self
            .scene
            .objects()
            .map(|object| (object.object_id, object.visible))
            .collect();
        let before_highlight = self.scene.highlighted_ids();
        let before_section_box = self.section_box;
        state.apply(&mut self.camera, &mut self.scene)?;
        self.section_box = state.section_box;
        let mut restored_pivot = state.navigation_pivot;
        if restored_pivot == NavigationPivot::default() && state.camera.pivot != Vec3::ZERO {
            restored_pivot.world_point = state.camera.pivot;
        }
        if restored_pivot.object_id.map_or(false, |id| {
            self.scene.get(id).map_or(true, |object| !object.visible)
        }) {
            restored_pivot.object_id = None;
        }
        self.set_navigation_pivot(restored_pivot);
        self.hover_hit = state.hover_hit.filter(|hit| {
            self.scene
                .get(hit.object_id)
                .map_or(false, |object| object.visible)
                && (!self.section_box.is_active()
                    || self.section_box.bounds.contains(hit.world_point))
        });
        self.isolated_object_ids = state
            .isolated_object_ids
            .iter()
            .copied()
            .filter(|id| self.scene.get(*id).is_some())
            .collect();
        let mut events = vec![ViewerEvent::StateRestored];
        if before_camera != self.camera.state() {
            events.push(ViewerEvent::CameraChanged {
                camera: self.camera.state(),
            });
        }
        if before_selection != self.scene.selected_ids() {
            events.push(ViewerEvent::SelectionChanged {
                selected_object_ids: self.scene.selected_ids(),
            });
        }
        if before_pivot != self.navigation_pivot {
            events.push(ViewerEvent::NavigationPivotChanged {
                pivot: self.navigation_pivot,
            });
        }
        if before_hover != self.hover_hit {
            events.push(ViewerEvent::HoverChanged {
                hit: self.hover_hit,
            });
        }
        if before_isolated != self.isolated_object_ids {
            events.push(ViewerEvent::IsolationChanged {
                object_ids: self.isolated_object_ids.clone(),
            });
            if !self.isolated_object_ids.is_empty() {
                events.push(ViewerEvent::ObjectIsolated {
                    object_ids: self.isolated_object_ids.clone(),
                });
            }
        }
        for (object_id, was_visible) in before_visibility {
            if let Some(object) = self.scene.get(object_id) {
                if was_visible != object.visible {
                    events.push(ViewerEvent::VisibilityChanged {
                        object_id,
                        visible: object.visible,
                    });
                    events.push(if object.visible {
                        ViewerEvent::ObjectShown { object_id }
                    } else {
                        ViewerEvent::ObjectHidden { object_id }
                    });
                }
            }
        }
        if before_highlight != self.scene.highlighted_ids() {
            events.push(ViewerEvent::HighlightChanged {
                object_id: self.scene.highlighted_ids().first().copied(),
            });
        }
        if before_section_box != self.section_box {
            events.push(ViewerEvent::SectionBoxChanged {
                section_box: self.section_box,
            });
        }
        self.record(&events);
        Ok(events)
    }

    /// Applies a serializable viewer state and returns and queues semantic change events.
    pub fn set_viewer_state(
        &mut self,
        state: &ViewerState,
    ) -> Result<Vec<ViewerEvent>, CameraError> {
        self.restore_state(state)
    }

    /// Parses and applies saved JSON viewer state.
    pub fn restore_snapshot_json(
        &mut self,
        json: &str,
    ) -> Result<Vec<ViewerEvent>, RestoreSnapshotError> {
        let state = ViewerState::from_json(json).map_err(RestoreSnapshotError::Json)?;
        self.restore_state(&state)
            .map_err(RestoreSnapshotError::Camera)
    }

    fn record(&mut self, events: &[ViewerEvent]) {
        self.pending_events.extend_from_slice(events);
    }

    fn pick_ray(&self, ray: Ray) -> Option<PickHit> {
        if self.section_box.is_active() {
            self.scene
                .pick_with_section_bounds(ray, self.camera.far, self.section_box.bounds)
        } else {
            self.scene.pick(ray, self.camera.far)
        }
    }

    fn replace_section_box(&mut self, section_box: SectionBox) -> Vec<ViewerEvent> {
        if self.section_box == section_box {
            return Vec::new();
        }
        let clipping_changed = self.section_box.is_active() || section_box.is_active();
        self.section_box = section_box;
        let mut events = vec![ViewerEvent::SectionBoxChanged { section_box }];
        if clipping_changed {
            events.extend(self.update_hover_hit(None));
        }
        self.record(&events);
        events
    }

    fn update_hover_hit(&mut self, hit: Option<PickHit>) -> Vec<ViewerEvent> {
        if self.hover_hit == hit {
            return Vec::new();
        }
        self.hover_hit = hit;
        vec![ViewerEvent::HoverChanged { hit }]
    }

    fn set_navigation_pivot(&mut self, pivot: NavigationPivot) {
        self.navigation_pivot = pivot;
        self.camera.pivot = pivot.world_point;
    }
}

/// Errors raised while parsing or applying a saved viewer JSON snapshot.
#[derive(Debug)]
pub enum RestoreSnapshotError {
    /// The input is not valid viewer-state JSON.
    Json(serde_json::Error),
    /// The JSON decoded, but its camera values are invalid.
    Camera(CameraError),
}

impl std::fmt::Display for RestoreSnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(f, "invalid viewer snapshot JSON: {error}"),
            Self::Camera(error) => write!(f, "invalid camera state in viewer snapshot: {error}"),
        }
    }
}

impl std::error::Error for RestoreSnapshotError {}
