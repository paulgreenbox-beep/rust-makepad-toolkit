//! Dry static circular 2D stability, independent of Makepad and render meshes.
//! World inputs remain immutable; numerical operations use local differences.
//! CHEAPEST_VERIFICATION_PATH: L0 named pure-core test -> L1 geotech_core suite.
mod analysis;
mod arithmetic_environment;
mod bishop;
mod circular_geometry;
mod contact;
mod contact_interval;
mod canonical_digest;
mod endpoint;
mod fellenius;
mod ground_definition;
mod ground_model;
mod physical;
mod search;
mod search_contact_filter;
mod search_evaluate;
mod search_grid;
mod search_meter;
mod search_types;
mod slice_generator;
mod types;
mod validation;
pub use analysis::{
    CIRCULAR_SOLVER_VERSION, analyze_specified_circle, analyze_specified_circle_cancellable,
    diagnose_specified_circle, input_fingerprint, source_fingerprint,
};
pub use bishop::{bishop, bishop_cancellable, bishop_diagnostic, bishop_diagnostic_cancellable};
pub use contact::{screen_whole_arc_contact, screen_whole_arc_contact_cancellable};
pub use physical::check_bishop_base_forces;
pub use circular_geometry::{build_failure_mass, render_boundary};
pub use fellenius::fellenius;
pub use ground_model::validate_ground;
pub use canonical_digest::canonical_version_bound_digest;
pub use ground_definition::{
    GroundDefinition, GroundMissingPart, GroundReadinessReport, GroundReadinessState,
    inspect_ground_readiness, validate_ground_structure,
};
pub use slice_generator::{generate_slices, slice_mechanics};
pub use types::*;
pub use validation::validate_config;
pub use arithmetic_environment::{ArithmeticEnvironmentReport, inspect_arithmetic_environment};
pub use search::{search_critical_circle, search_critical_circle_cancellable};
pub use search_evaluate::SearchPassEvaluation;
pub use search_meter::SearchWorkCounters;
pub use search_types::*;
