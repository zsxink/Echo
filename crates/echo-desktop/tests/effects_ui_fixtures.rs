//! Backend-generated UI regressions. Update explicitly with
//! `UPDATE_EFFECTS_UI_FIXTURES=1 cargo test -p echo-desktop --test effects_ui_fixtures`.
use echo_desktop::effects::{
    AppliedState, ChannelLayout, EffectsState, EqCurve, PreampMode, PresetId, ProcessingEnvironment,
};
use echo_desktop::ipc::effects::EffectsSnapshotDto;
use std::collections::BTreeMap;

const fn environment(rate: u32, layout: ChannelLayout) -> ProcessingEnvironment {
    ProcessingEnvironment {
        sample_rate: rate,
        channel_layout: layout,
    }
}

fn applied(state: &mut EffectsState) {
    let env = environment(
        state.runtime.processing_rate.unwrap(),
        state.runtime.channel_layout.unwrap(),
    );
    let analysis =
        echo_desktop::effects::math::analyze(&state.document.requested_payload(), env).unwrap();
    let mut receipt = state.runtime.clone();
    receipt.applied = AppliedState::Applied;
    receipt.active_bands = analysis.active_bands;
    receipt.effective_preamp_db = Some(analysis.effective_preamp_db);
    assert!(state.confirm(receipt));
}

fn fixtures() -> BTreeMap<&'static str, EffectsSnapshotDto> {
    let mut result = BTreeMap::new();
    let mut low = EffectsState::default();
    low.select(&PresetId::builtin("pop")).unwrap();
    low.environment_changed(Some(environment(22_050, ChannelLayout::Stereo)))
        .unwrap();
    result.insert("pendingLowRate", low.snapshot().into());
    let mut failed = low.clone();
    let mut receipt = failed.runtime.clone();
    receipt.applied = AppliedState::Failed;
    receipt.reason = Some("injected filter failure".into());
    assert!(failed.confirm(receipt));
    result.insert("failedLowRate", failed.snapshot().into());
    low.set_enabled(false).unwrap();
    result.insert("bypassedLowRate", low.snapshot().into());
    let mut spatial = EffectsState::default();
    spatial.select(&PresetId::builtin("surround")).unwrap();
    spatial
        .environment_changed(Some(environment(48_000, ChannelLayout::Stereo)))
        .unwrap();
    applied(&mut spatial);
    result.insert("appliedSpatial", spatial.snapshot().into());
    for (name, mode) in [
        ("manualProtected", PreampMode::Manual),
        ("autoProtected", PreampMode::Auto),
    ] {
        let mut state = EffectsState::default();
        state
            .edit(EqCurve {
                gains_db: [12.0; 10],
                preamp_mode: mode,
                requested_preamp_db: 0.0,
            })
            .unwrap();
        state
            .environment_changed(Some(environment(48_000, ChannelLayout::Stereo)))
            .unwrap();
        applied(&mut state);
        result.insert(name, state.snapshot().into());
    }
    let mut no_track = EffectsState::default();
    no_track.select(&PresetId::builtin("pop")).unwrap();
    result.insert("noTrack", no_track.snapshot().into());
    let mut mono = EffectsState::default();
    mono.select(&PresetId::builtin("surround")).unwrap();
    mono.environment_changed(Some(environment(48_000, ChannelLayout::Mono)))
        .unwrap();
    result.insert("mono", mono.snapshot().into());
    result
}

#[test]
fn ui_snapshots_match_real_backend_projection() {
    let generated = serde_json::to_string_pretty(&fixtures()).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../apps/desktop/src/features/player/__fixtures__/effects-snapshots.generated.json",
    );
    if std::env::var_os("UPDATE_EFFECTS_UI_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &generated).unwrap();
    }
    let actual: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let expected: serde_json::Value = serde_json::from_str(&generated).unwrap();
    assert_eq!(actual, expected,
        "Regenerate the fixture with UPDATE_EFFECTS_UI_FIXTURES=1; never synthesize runtime facts in UI tests");
}
