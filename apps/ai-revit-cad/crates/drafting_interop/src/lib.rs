//! Bounded ASCII DXF adapter for Drafting Lab.
//!
//! CADCraft is used only as a pinned DXF tag parser/writer. It does not own the
//! Lab document, entity identity, commands, or history.
#![forbid(unsafe_code)]

mod ascii;
mod export;
mod import;
mod loss;
mod units;

pub use export::{ExportOptions, ExportResult, export_dxf, export_dxf_preview};
pub use import::{ImportOptions, ImportPreview, import_preview};
pub use loss::{LossItem, LossPolicy, LossReport, LossSeverity};
pub use units::{DxfUnitChoice, OutputUnit};
