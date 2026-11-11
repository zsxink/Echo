//! Desktop-only audio effect rules. Storage and native playback implement ports
//! outside this module; Core supplies only its established Unicode text rules.

mod derived;
mod document;
pub mod loudness;
pub mod math;
mod model;
pub mod naming;
pub mod presets;
mod state;

pub use derived::{DerivedMemo, DerivedView};
pub use document::{EffectsDocument, Selection, UserPreset};
pub use model::*;
pub use state::{EffectsSnapshot, EffectsState};
