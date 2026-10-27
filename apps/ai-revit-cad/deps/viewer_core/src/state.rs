use crate::{Camera, CameraError, CameraState, ObjectId, PickHit, Scene, SectionBox, Vec3};
use serde::{Deserialize, Serialize};

/// Current world-space navigation pivot, optionally attached to a scene object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NavigationPivot {
    /// Stable object ID for a surface-attached pivot, or none for a free pivot.
    pub object_id: Option<ObjectId>,
    /// Pivot point in world coordinates.
    pub world_point: Vec3,
}

/// Serializable camera and scene interaction state, excluding model geometry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewerState {
    /// Schema version for forward-compatible saved viewer state.
    pub schema_version: u32,
    /// Camera pose and perspective lens.
    pub camera: CameraState,
    /// Object-aware navigation pivot used by orbit and surface-centered dolly.
    #[serde(default)]
    pub navigation_pivot: NavigationPivot,
    /// Most recent surface hit beneath the pointer.
    #[serde(default)]
    pub hover_hit: Option<PickHit>,
    /// Count of registered visible scene objects at capture time.
    #[serde(default)]
    pub visible_count: usize,
    /// Count of registered hidden scene objects at capture time.
    #[serde(default)]
    pub hidden_count: usize,
    /// IDs kept visible by the most recent isolate operation.
    #[serde(default)]
    pub isolated_object_ids: Vec<ObjectId>,
    /// IDs carrying a highlight flag at capture time.
    #[serde(default)]
    pub highlighted_object_ids: Vec<ObjectId>,
    /// Visualization-only Section Box. Missing values in v0.1 snapshots disable clipping.
    #[serde(default)]
    pub section_box: SectionBox,
    /// Selected object IDs in stable ascending order.
    pub selected_object_ids: Vec<ObjectId>,
    /// Hidden object IDs in stable ascending order.
    pub hidden_object_ids: Vec<ObjectId>,
    /// Transient highlighted object ID, if any.
    #[serde(default)]
    pub highlighted_object_id: Option<ObjectId>,
}

impl ViewerState {
    /// Captures the camera and scene's serializable interaction state.
    pub fn capture(camera: &Camera, scene: &Scene) -> Self {
        Self::capture_full(
            camera,
            scene,
            NavigationPivot {
                object_id: None,
                world_point: camera.pivot,
            },
            None,
            &[],
            SectionBox::default(),
        )
    }

    /// Captures complete viewer interaction state, including hover and isolation state.
    pub fn capture_full(
        camera: &Camera,
        scene: &Scene,
        navigation_pivot: NavigationPivot,
        hover_hit: Option<PickHit>,
        isolated_object_ids: &[ObjectId],
        section_box: SectionBox,
    ) -> Self {
        Self {
            schema_version: 2,
            camera: camera.state(),
            navigation_pivot,
            hover_hit,
            visible_count: scene.visible_count(),
            hidden_count: scene.hidden_count(),
            isolated_object_ids: isolated_object_ids.to_vec(),
            highlighted_object_ids: scene.highlighted_ids(),
            section_box,
            selected_object_ids: scene.selected_ids(),
            hidden_object_ids: scene
                .objects()
                .filter(|object| !object.visible)
                .map(|object| object.object_id)
                .collect(),
            highlighted_object_id: scene.highlighted_ids().first().copied(),
        }
    }

    /// Applies this state to a camera and scene, ignoring object IDs absent from the current scene.
    pub fn apply(&self, camera: &mut Camera, scene: &mut Scene) -> Result<(), CameraError> {
        *camera = Camera::from_state(self.camera)?;
        camera.pivot = self.navigation_pivot.world_point;
        scene.restore_visibility(&self.hidden_object_ids);
        scene.restore_selection(&self.selected_object_ids);
        let highlights = if self.highlighted_object_ids.is_empty() {
            self.highlighted_object_id
                .iter()
                .copied()
                .collect::<Vec<_>>()
        } else {
            self.highlighted_object_ids.clone()
        };
        scene.restore_highlights(&highlights);
        Ok(())
    }

    /// Encodes this state as JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Decodes viewer state from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}
