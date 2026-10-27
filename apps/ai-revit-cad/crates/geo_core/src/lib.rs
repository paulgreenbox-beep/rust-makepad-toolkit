//! Pure authoritative geology records and bounded immutable snapshot queries.
//! CHEAPEST_VERIFICATION_PATH: Root L1 cargo test -p geo_core -j1; then Core adapter checks.
mod model;
mod unique_map;
mod state;
mod validation;
pub mod geometry;
mod query;
mod fixtures;
pub use model::*;
pub use state::{apply_geo_command,make_geo_snapshot,resolve_geo_scenario,snapshot_currentness,validate_snapshot_binding};
pub use validation::validate_geo_state;
pub use query::query_ground;
pub use geometry::query_columns;
pub use fixtures::{synthetic_demo_state, SYNTHETIC_DEMO_MODEL_ID, SYNTHETIC_DEMO_SCENARIO_ID};

