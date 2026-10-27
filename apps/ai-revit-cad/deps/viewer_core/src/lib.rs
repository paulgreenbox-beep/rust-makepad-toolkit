//! Pure Rust building blocks for a CAD viewer.
//!
//! This crate deliberately has no renderer or Makepad dependency. It provides
//! geometry, a perspective camera, triangle BVH picking, scene state, semantic
//! events, and a small procedural scene that a UI can render independently.

mod camera;
mod collision;
mod collision_demo;
mod demo;
mod events;
mod exact_acceptance;
mod fh87_fixture;
mod large_demo;
mod math;
mod mesh;
mod reduced_3d;
mod scene;
mod section;
mod state;
mod viewer;

pub use camera::{
    Camera, CameraError, CameraProjection, CameraState, ProjectedPoint, SurfaceAnchor, Viewport,
};
pub use collision::{
    detect_aabb_collisions, detect_aabb_collisions_with_clearance, Collision, CollisionMethod,
    CollisionSeverity, CollisionStatus, CollisionSummary,
};
pub use collision_demo::collision_demo_scene;
pub use demo::procedural_test_scene;
pub use events::{ViewerEvent, ViewerEventKind};
pub use exact_acceptance::exact_acceptance_scene;
pub use fh87_fixture::decode_fh87_mesh_scene;
pub use large_demo::large_demo_scene;
pub use math::{Aabb, Quat, Ray, Vec2, Vec3};
pub use mesh::{Mesh, PickHit, Triangle};
pub use reduced_3d::{
    capture_reduced_3d, Reduced3DBoundsReason, Reduced3DBudget, Reduced3DComplexity,
    Reduced3DError, Reduced3DGeometryQuality, Reduced3DObject, Reduced3DObservation,
    Reduced3DQuality, Reduced3DRepresentation, Reduced3DRole,
};
pub use scene::{
    ExactBoxPart, ExactGeometryRef, ExactGeometrySource, ObjectId, RenderTexture, Scene,
    SceneObject, Transform3,
};
pub use section::{SectionBounds, SectionBox, SectionBoxError, SectionMode};
pub use state::{NavigationPivot, ViewerState};
pub use viewer::{RestoreSnapshotError, Viewer};

#[cfg(test)]
mod tests;
