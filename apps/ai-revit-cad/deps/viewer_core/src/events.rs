use crate::{CameraState, NavigationPivot, ObjectId, PickHit, SectionBox};
use serde::{Deserialize, Serialize};

/// High-level categories emitted by viewer interactions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerEventKind {
    /// A pointer ray intersected a visible surface.
    ObjectPicked,
    /// The surface currently under the pointer changed.
    HoverChanged,
    /// The selected object ID set changed.
    SelectionChanged,
    /// An object's visibility changed.
    VisibilityChanged,
    /// A scene object became hidden.
    ObjectHidden,
    /// A scene object became visible.
    ObjectShown,
    /// The transient highlight changed.
    HighlightChanged,
    /// The camera pose or lens changed.
    CameraChanged,
    /// The object-aware world-space navigation pivot changed.
    #[serde(rename = "pivot_changed")]
    NavigationPivotChanged,
    /// The set of isolated visible object IDs changed.
    IsolationChanged,
    /// One or more scene objects became isolated.
    ObjectIsolated,
    /// A saved viewer state was applied.
    StateRestored,
    /// The enabled state or bounds of the visualization Section Box changed.
    SectionBoxChanged,
}

/// A serializable semantic event produced by viewer interactions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ViewerEvent {
    /// A visible object surface was picked.
    ObjectPicked { hit: PickHit },
    /// The surface currently under the pointer changed.
    HoverChanged { hit: Option<PickHit> },
    /// The ordered set of selected scene object IDs changed.
    SelectionChanged { selected_object_ids: Vec<ObjectId> },
    /// One scene object's visibility changed.
    VisibilityChanged { object_id: ObjectId, visible: bool },
    /// A scene object became hidden.
    ObjectHidden { object_id: ObjectId },
    /// A scene object became visible.
    ObjectShown { object_id: ObjectId },
    /// The single transiently highlighted scene object changed.
    HighlightChanged { object_id: Option<ObjectId> },
    /// The camera pose or lens settings changed.
    CameraChanged { camera: CameraState },
    /// The object-aware navigation pivot changed.
    #[serde(rename = "pivot_changed")]
    NavigationPivotChanged { pivot: NavigationPivot },
    /// The set of objects retained by isolate mode changed.
    IsolationChanged { object_ids: Vec<ObjectId> },
    /// One or more scene objects became the isolated visible set.
    ObjectIsolated { object_ids: Vec<ObjectId> },
    /// A saved viewer state was restored.
    StateRestored,
    /// The Section Box clipping state changed.
    SectionBoxChanged { section_box: SectionBox },
}

impl ViewerEvent {
    /// Returns the semantic category for this event.
    pub fn kind(&self) -> ViewerEventKind {
        match self {
            Self::ObjectPicked { .. } => ViewerEventKind::ObjectPicked,
            Self::HoverChanged { .. } => ViewerEventKind::HoverChanged,
            Self::SelectionChanged { .. } => ViewerEventKind::SelectionChanged,
            Self::VisibilityChanged { .. } => ViewerEventKind::VisibilityChanged,
            Self::ObjectHidden { .. } => ViewerEventKind::ObjectHidden,
            Self::ObjectShown { .. } => ViewerEventKind::ObjectShown,
            Self::HighlightChanged { .. } => ViewerEventKind::HighlightChanged,
            Self::CameraChanged { .. } => ViewerEventKind::CameraChanged,
            Self::NavigationPivotChanged { .. } => ViewerEventKind::NavigationPivotChanged,
            Self::IsolationChanged { .. } => ViewerEventKind::IsolationChanged,
            Self::ObjectIsolated { .. } => ViewerEventKind::ObjectIsolated,
            Self::StateRestored => ViewerEventKind::StateRestored,
            Self::SectionBoxChanged { .. } => ViewerEventKind::SectionBoxChanged,
        }
    }
}
