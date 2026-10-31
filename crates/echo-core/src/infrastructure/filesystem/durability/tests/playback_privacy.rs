//! Playback idempotence lives in private directory state, never shared records.
use super::*;

#[test]
fn library_durability_playback_event_is_materialized_outside_shared_records() {
    let (directory, registry, root, library, plane) = fixture();
    let session = uuid::Uuid::new_v4();
    let song = uuid::Uuid::new_v4();
    let mut change = batch(library, uuid::Uuid::new_v4(), "private playback");
    let event = DurableStateChange::new(
        StateKind::PlaybackEvent,
        session,
        serde_json::to_value(crate::domain::library::durability::PlaybackEvent {
            song_uuid: song,
            recorded_at: 1_700_000_000_000,
        })
        .unwrap(),
    );
    change.state.push(event.clone());
    change.records.push(
        PortableRecordEnvelope::new(
            PortableRecord::PlayStats(crate::domain::library::PlayStatsRecord {
                song_uuid: song,
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HybridLogicalClock::default(),
                by_device: BTreeMap::from([(uuid::Uuid::new_v4(), 7)]),
            }),
            None,
        )
        .unwrap(),
    );
    commit(&registry, root, &change).unwrap();
    let path = directory
        .path()
        .join(format!("echo/state/playback-events/{session}.json"));
    let saved: DurableStateChange = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(saved, event);
    let shared = plane.list_records(root, RecordKind::PlayStats).unwrap();
    assert_eq!(shared.len(), 1);
    assert!(!shared[0].contains(&session.to_string()));
    assert!(!shared[0].contains("recorded_sessions"));
    let parsed: PortableRecordEnvelope = serde_json::from_str(&shared[0]).unwrap();
    assert_eq!(parsed.record(), change.records.last().unwrap().record());
}
