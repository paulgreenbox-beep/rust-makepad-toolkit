//! Reusable analytical drafting types, geometry and derived cache.
//! `DrawingSource` adapts an existing authoritative project; `LabDocument` remains
//! the standalone compatibility authority. No renderer or BIM dependency.
//!
//! Coordinates are f64 metres; camera coordinates are logical pixels. GPU clients
//! cast only the camera-relative, clipped display coordinates to f32.
mod commands;
mod cache;
mod cache_digest;
mod cache_index;
mod cache_tessellation;
mod geometry;
mod ids;
mod model;
mod source;
mod view;

pub use commands::*;
pub use cache::*;
pub use cache_tessellation::TessellatedGeometry;
pub use geometry::*;
pub use ids::*;
pub use model::*;
pub use source::*;
pub use view::*;

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraftError {
    pub code: String,
    pub message: String,
}
impl DraftError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for DraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for DraftError {}
pub type DraftResult<T> = Result<T, DraftError>;

#[cfg(test)]
mod tests;
