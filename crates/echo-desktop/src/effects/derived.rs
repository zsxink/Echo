//! Derived views of the effects document, memoized on the inputs that decide them.
//!
//! An effects snapshot mixes facts (the document, the runtime confirmation)
//! with derived values (the frequency-response analysis, the preset list, the
//! response curve and the editable bands). The derived values are fully
//! determined by the document and the processing environment, and the analysis
//! in particular runs a 32768-point peak sweep. Recomputing that sweep for
//! every playback position frame burned the CPU the position stream needs
//! (issue #112), so the derived view is memoized here — in the effects module,
//! not in the IPC layer, so every caller benefits from one implementation.

use super::{
    math::{self, EffectsAnalysis, ResponsePoint},
    presets::{self, Preset, PresetSource},
    EffectsDocument, Payload, ProcessingEnvironment, BAND_FREQUENCIES,
};
use std::sync::{Arc, Mutex, PoisonError};

/// The derived parts of an effects snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedView {
    /// The frequency-response analysis of the requested payload.
    pub analysis: Option<EffectsAnalysis>,
    /// The built-in and user presets the panel offers.
    pub presets: Vec<Preset>,
    /// The response curve of the requested payload, at the preamp the
    /// analysis derived. A confirmed preamp shifts the whole curve; the
    /// caller recomputes the points for that shift, because they are cheap
    /// while the analysis behind them is not.
    pub response_points: Vec<ResponsePoint>,
    /// Whether the requested payload is an equalizer curve. Only an
    /// equalizer has a response curve.
    pub eq_payload: bool,
    /// Which bands the processing rate can carry.
    pub editable_bands: Option<[bool; 10]>,
}

/// Memoizes the derived view for one (document, environment) pair.
///
/// The memo is shared by every clone of the state that owns it, so a snapshot
/// taken for a playback position frame reuses the analysis of the last real
/// change instead of repeating the sweep.
#[derive(Clone, Debug, Default)]
pub struct DerivedMemo {
    slot: Arc<Mutex<Option<MemoEntry>>>,
}

#[derive(Clone, Debug)]
struct MemoEntry {
    document: EffectsDocument,
    environment: ProcessingEnvironment,
    view: DerivedView,
}

impl DerivedMemo {
    /// The derived view for `document` in `environment`.
    ///
    /// The first call for a distinct input pair computes the view; later calls
    /// with the same inputs reuse it. A changed document or environment
    /// recomputes with the new inputs.
    #[must_use]
    pub fn view(
        &self,
        document: &EffectsDocument,
        environment: ProcessingEnvironment,
    ) -> DerivedView {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = slot.as_ref() {
            if entry.environment == environment && entry.document == *document {
                return entry.view.clone();
            }
        }
        let view = derive(document, environment);
        *slot = Some(MemoEntry {
            document: document.clone(),
            environment,
            view: view.clone(),
        });
        view
    }
}

/// Compute the derived view from the document and the environment.
fn derive(document: &EffectsDocument, environment: ProcessingEnvironment) -> DerivedView {
    let payload = document.requested_payload();
    let eq_payload = matches!(payload, Payload::Eq(_));
    let analysis = math::analyze(&payload, environment).ok();
    let mut presets = presets::builtin_presets();
    presets.extend(document.user_presets.iter().map(|user| Preset {
        id: user.id.clone(),
        name: user.name.clone(),
        description: "已保存的用户曲线".to_owned(),
        payload: Payload::Eq(user.curve.clone()),
        source: PresetSource::User,
    }));
    let response_points = if eq_payload {
        analysis.as_ref().map_or_else(Vec::new, |analysis| {
            math::response_points(analysis, environment.sample_rate)
        })
    } else {
        Vec::new()
    };
    DerivedView {
        analysis,
        presets,
        response_points,
        eq_payload,
        editable_bands: Some(
            BAND_FREQUENCIES
                .map(|frequency| frequency <= 0.45 * f64::from(environment.sample_rate)),
        ),
    }
}
