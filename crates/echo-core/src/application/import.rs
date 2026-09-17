//! Import use case public surface.
//!
//! Planning, execution, reporting and lyrics handling live in sibling
//! modules; this root preserves the historic `application::import::*` API.

mod logic;

pub use logic::{ImportBatchReport, ImportOutcome, LyricsImportResult, PlanImport};
