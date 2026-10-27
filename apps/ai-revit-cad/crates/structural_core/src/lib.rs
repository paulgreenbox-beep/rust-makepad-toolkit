//! Bounded, small-displacement, linear-elastic structural analysis.
//!
//! Units are metres, kN, kN m, radians and kN/m². Global axes are right handed,
//! z up. Plane formulations operate in XZ: ux, uz and (for frames) ry.
//! Member axes are right handed; x runs from start to end. A positive local
//! y reference fixes roll, z = x × y. End forces are forces *on the member*.
//! Passing a numerical analysis is not a steel/RC, buckling or code check.

pub mod fixtures;
mod ids;
mod model;
mod solver;

pub use ids::*;
pub use model::*;
pub use solver::{
    SOLVER_VERSION, analyze, input_fingerprint, validate, validate_result_binding, validate_target,
};

#[cfg(test)]
mod tests;
