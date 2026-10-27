//! Preliminary rigid rectangular raft on explicit compression-only Winkler springs.
//!
//! Coordinates are right handed, global z upwards, with the raft centre at (0,0,0).
//! Settlement `w` is positive downwards. Input pressure moments are
//! `mx_pressure = integral(y*q)dA`, `my_pressure = integral(x*q)dA`.
//! Thus physical external moments are `Mx = -mx_pressure`, `My = my_pressure`.
//! The homogeneous full-contact continuum candidate is
//! `q = N/A + mx_pressure*y/Ix + my_pressure*x/Iy`. The numerical model uses
//! `w=w0+a*x+b*y`, `q_i=k_i*max(w_i-gap_i,0)`, `K_i=k_i*A_i`, with
//! k in kN/m^3, K in kN/m and q in kPa; this is midpoint quadrature.
//! An explicit assumed bedding stiffness is required; elastic modulus E is never
//! converted into a universal Winkler coefficient. This crate makes no bearing,
//! reinforcement, normative ULS, groundwater or structural-frame acceptance claim.

mod model;
mod solver;
pub mod foundation_geometry;
pub mod geo_context;

pub use model::*;
pub use foundation_geometry::{FoundationLayout, FoundationPlacement, FoundationTier, validate_layout as validate_foundation_layout, derive_layout_geometry};
pub use geo_context::{
    FoundationBeddingAssumption, ManualIgeBeddingValue, FoundationBeddingReadiness,
    FoundationGeoContext, FoundationGeoBinding, FoundationGeoReadiness,
    FoundationGeoReadinessState, FoundationGeoSampleIssue, FoundationGeoQuerySample,
    FoundationGeoError, GateFoundationBReadiness, FoundationMethodStatus,
    resolve_geo_context, validate_bedding_assumption,
};
pub use solver::{
    input_fingerprint, self_weight_ledger, solve, validate_input, validate_result_binding,
};

#[cfg(test)]
mod tests;
