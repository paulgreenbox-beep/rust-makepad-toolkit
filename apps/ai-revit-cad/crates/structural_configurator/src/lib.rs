//! Parametric steel BIM generation; panels and meshes never supply stiffness.
//! CHEAPEST_VERIFICATION_PATH: Root runs L1 cargo test -p structural_configurator -j1,
//! followed by the Core consumer boundary checks. No GUI/engineering acceptance.
mod adapter;
mod binding;
mod cache;
mod fixtures;
mod generator;
pub mod geometry;
mod model;
mod unique_map;
mod validation;
pub use adapter::{dense_id_map, to_structural_model, to_structural_model_for_scope};
pub use cache::{GenerationDiagnostics, GeneratorCache, GeneratorCacheLimits, GeneratorCacheStats};
pub use fixtures::{FixtureKind, fixture};
pub type GeneratorDiagnostics = GenerationDiagnostics;
pub use generator::{
    GENERATOR_VERSION, connected_analysis_scope, diff_generated, generate, generate_building,
    generate_building_with_cache, set_section_eave, validate_generated_binding,
};
pub use model::*;
pub use validation::validate_config;
