pub mod catalog;
pub mod cli;
pub mod contract;
pub mod coverage;
pub mod fixtures;
pub mod generator;
#[cfg(windows)]
#[path = "../../ai-ui-test-tool/src/host.rs"]
pub mod host;
pub mod inventory;

pub mod observation;

#[cfg(windows)]
pub mod advanced;

#[cfg(windows)]
pub mod probe;

#[cfg(windows)]
pub mod reliability;

pub mod spatial;

#[allow(clippy::module_inception)]
pub mod api;
pub mod script;
pub mod script_controls;

pub mod runner;

pub mod sdk;

pub mod suite;
pub mod templates;
pub mod workflow;
