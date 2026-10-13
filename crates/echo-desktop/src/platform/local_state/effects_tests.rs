//! Effects slot recovery is independent of the desktop legacy preference parser.
use super::*;
use crate::effects::{EffectsDocument, EffectsState, EqCurve, PresetId, Selection, UserPreset};
use crate::effects_service::{EffectsPreferencesPort, PreferencesRecovery, PreferencesWrite};

#[test]
fn legacy_document_defaults_missing_effects_and_atomic_write_preserves_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-state.json");
    fs::write(
        &path,
        br#"{"theme":"cobalt","playbackSession":{"queue":[8]},"unknown":42}"#,
    )
    .unwrap();
    let store = DesktopStateStore::new(path.clone(), PlatformCloseDefault::Other);
    assert!(matches!(
        EffectsPreferencesPort::load(&store).unwrap(),
        PreferencesRecovery::Missing
    ));
    EffectsPreferencesPort::write(
        &store,
        &EffectsDocument::default(),
        PreferencesWrite::Normal,
    )
    .unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(raw["theme"], "cobalt");
    assert_eq!(raw["unknown"], 42);
    assert_eq!(raw["playbackSession"]["queue"][0], 8);
    assert_eq!(raw["effects"]["requestedEnabled"], false);
}
#[test]
fn corrupt_json_protected_until_deliberate_repair_keeps_original_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-state.json");
    let original = b"{ corrupt JSON ";
    fs::write(&path, original).unwrap();
    let store = DesktopStateStore::new(path.clone(), PlatformCloseDefault::Other);
    assert!(matches!(
        EffectsPreferencesPort::load(&store).unwrap(),
        PreferencesRecovery::Protected { .. }
    ));
    assert!(EffectsPreferencesPort::write(
        &store,
        &EffectsDocument::default(),
        PreferencesWrite::Normal
    )
    .is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    EffectsPreferencesPort::write(
        &store,
        &EffectsDocument::default(),
        PreferencesWrite::Repair,
    )
    .unwrap();
    assert_eq!(
        fs::read(path.with_extension("json.effects-recovery")).unwrap(),
        original
    );
}
#[test]
fn future_schema_and_registry_are_protected_across_legacy_writes() {
    for field in ["schemaVersion", "registryVersion"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop-state.json");
        let mut effects = serde_json::to_value(EffectsDocument::default()).unwrap();
        effects[field] = serde_json::json!(999);
        let original =
            serde_json::to_vec(&serde_json::json!({"theme":"cobalt", "effects":effects})).unwrap();
        fs::write(&path, &original).unwrap();
        let store = DesktopStateStore::new(path.clone(), PlatformCloseDefault::Other);
        assert!(matches!(
            EffectsPreferencesPort::load(&store).unwrap(),
            PreferencesRecovery::Protected { .. }
        ));
        store.set_theme(DesktopTheme::Turquoise).unwrap();
        assert!(EffectsPreferencesPort::write(
            &store,
            &EffectsDocument::default(),
            PreferencesWrite::Normal
        )
        .is_err());
        let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["effects"], effects);
        assert_eq!(
            fs::read(path.with_extension("json.effects-recovery")).unwrap(),
            original
        );
    }
}
#[test]
fn invalid_payload_and_dangling_selection_are_not_enabled_or_overwritten() {
    for dangling in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop-state.json");
        let document = EffectsDocument {
            requested_enabled: true,
            selection: Selection::Preset(PresetId::new_user()),
            ..EffectsDocument::default()
        };
        let mut raw = serde_json::to_value(document).unwrap();
        if !dangling {
            raw["retainedPayload"]["gainsDb"][0] = serde_json::json!(13.0);
        }
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"effects":raw})).unwrap(),
        )
        .unwrap();
        let original = fs::read(&path).unwrap();
        let store = DesktopStateStore::new(path.clone(), PlatformCloseDefault::Other);
        assert!(matches!(
            EffectsPreferencesPort::load(&store).unwrap(),
            PreferencesRecovery::Protected { .. }
        ));
        assert!(EffectsPreferencesPort::write(
            &store,
            &EffectsDocument::default(),
            PreferencesWrite::Normal
        )
        .is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}
#[test]
fn legal_draft_roundtrips_without_applied_runtime_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-state.json");
    let store = DesktopStateStore::new(path.clone(), PlatformCloseDefault::Other);
    let mut state = EffectsState::default();
    state.edit(EqCurve::default()).unwrap();
    EffectsPreferencesPort::write(&store, &state.document, PreferencesWrite::Normal).unwrap();
    let PreferencesRecovery::Ready(document) = EffectsPreferencesPort::load(&store).unwrap() else {
        panic!("valid document")
    };
    assert_eq!(document, state.document);
    let text = fs::read_to_string(path).unwrap();
    assert!(!text.contains("effectivePreampDb"));
    assert!(!text.contains("playbackEpoch"));
}
#[test]
fn protected_document_retains_valid_user_curve_for_recovery() {
    let preset = UserPreset {
        id: PresetId::new_user(),
        name: "Recover me".into(),
        curve: EqCurve::default(),
    };
    let mut raw = serde_json::to_value(EffectsDocument::default()).unwrap();
    raw["schemaVersion"] = serde_json::json!(999);
    raw["userPresets"] = serde_json::json!([preset]);
    let (state, protected) =
        crate::effects_service::recovery::restore(Ok(PreferencesRecovery::Protected {
            raw: Some(raw),
            reason: "future".into(),
        }));
    assert!(protected.is_some());
    assert!(!state.document.requested_enabled);
    assert_eq!(state.document.user_presets.len(), 1);
    assert_eq!(state.document.user_presets[0].name, "Recover me");
}
