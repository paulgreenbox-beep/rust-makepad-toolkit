//! Pure Rust state and render-planning primitives for complex Makepad applications.
//!
//! This crate intentionally has no Makepad or application-domain dependency. It owns
//! coordinate transforms, zoom, scroll clamping, visibility, page placement, visible
//! ranges, rulers, text layout inputs, and the document frame consumed by renderers.

#![forbid(unsafe_code)]

pub mod content;
pub mod geometry;
pub mod model;
pub mod pages;
pub mod rulers;
pub mod text_layout;
pub mod viewport;

pub use content::*;
pub use geometry::*;
pub use model::*;
pub use pages::*;
pub use rulers::*;
pub use text_layout::*;
pub use viewport::*;
