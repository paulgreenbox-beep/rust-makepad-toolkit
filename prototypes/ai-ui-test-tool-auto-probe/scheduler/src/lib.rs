//! Local scheduler and worker-pool contracts for the AI UI Test Tool.
//!
//! The crate intentionally depends only on the public script/catalog surface of
//! `ai-ui-test-tool-auto-probe`. Production execution crosses the same boundary
//! as an external user: `ai-ui-test run <script.aiui.json>`.

pub mod cli;
pub mod container;
pub mod container_runtime;
pub mod container_verification;
pub mod container_worker_loss_e2e;
pub mod e2e;
pub mod environment;
pub mod execution;
pub mod lease;
pub mod mixed_e2e;
pub mod model;
pub mod persistence;
pub mod pool;
pub mod queue;
pub mod report;
pub mod resources;
pub mod scheduler;
pub mod state;
pub mod submission;
pub mod verification;
pub mod worker;

pub use model::*;
