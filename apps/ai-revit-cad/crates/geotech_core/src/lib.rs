//! GEO-SLOPE v0.1: dry, homogeneous, static planar translational failure.
//! Authoritative geometry uses metres and f64; output meshes are derived artifacts.
//! Numerical execution never grants engineering or independent numerical approval.
mod geometry;
mod mechanics;
mod model;
mod report;
mod search;
pub mod circular;

pub use geometry::{extrude, geometry, polygon_area_m2, triangular_section};
pub use mechanics::{
    analyze, analyze_cancellable, input_fingerprint, source_fingerprint, validate_input,
};
pub use model::*;
pub use report::report;
pub use search::{search, search_cancellable};

pub const SOLVER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const METHOD: &str = "planar_mohr_coulomb_dry";
