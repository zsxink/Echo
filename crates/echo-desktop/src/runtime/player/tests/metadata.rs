use super::*;

#[test]
fn metadata_resolver_resolves_library_entry_fields() {
    // 队列展示信息: library entries resolve to title / artist / duration /
    // cover key — the real presentation fields the queue panel needs, not
    // a generic "歌曲"/"资料库歌曲" placeholder.
    let (db, song_id) = seeded_db();
    let resolver = QueueMetadataResolver::new(db.clone(), db);
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(song_id),
    };
    let meta = resolver.resolve(&[entry]);
    let meta = meta.get(&song_id).expect("the queue song is resolved");
    assert_eq!(meta.title.as_deref(), Some("晴天"));
    assert_eq!(meta.artist.as_deref(), Some("周杰伦"));
    assert_eq!(meta.duration_s, Some(239));
    assert_eq!(meta.cover_key.as_deref(), Some("cv1-seeded"));
}

#[test]
fn metadata_resolver_skips_temporary_items_without_querying() {
    // 临时项兜底: session-only temporary items are never queried — they
    // carry their own display name, and the resolver does not attempt a
    // library lookup that could fail for a non-library file.
    let (db, _song_id) = seeded_db();
    let resolver = QueueMetadataResolver::new(db.clone(), db);
    let temp = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Temporary(crate::player::queue::TemporaryItem {
            display_name: "访谈录音.m4a".to_owned(),
            path: std::path::PathBuf::from("/tmp/interview.m4a"),
            duration: None,
            on_active_root: false,
        }),
    };
    let meta = resolver.resolve(&[temp]);
    assert!(
        meta.is_empty(),
        "temporary items resolve to no metadata map entry"
    );
}

#[test]
fn metadata_resolver_caches_resolved_ids_across_snapshots() {
    // 高频位置事件复用已解析的队列 DTO: once resolved, a repeated resolve
    // of the same song id answers from the cache — no re-query, no error
    // — so the 10 Hz position stream never triggers per-row lookups.
    let (db, song_id) = seeded_db();
    // Remove the song after the first resolve: the second resolve must
    // still answer from cache rather than querying an absent id.
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(song_id),
    };
    let resolver = QueueMetadataResolver::new(db.clone(), db.clone());
    let first = resolver.resolve(std::slice::from_ref(&entry));
    assert!(
        first.contains_key(&song_id),
        "first resolve queries the library"
    );
    let song_id_copy = song_id;
    db.with_tx(Box::new(move |tx| tx.delete_song(song_id_copy)))
        .expect("delete after first resolve");
    let second = resolver.resolve(&[entry]);
    assert_eq!(
        second.get(&song_id).cloned(),
        first.get(&song_id).cloned(),
        "the cached metadata is reused for an unchanged queue id"
    );
}

#[test]
fn metadata_resolver_gives_explicit_nulls_for_unknown_ids() {
    // A queue entry whose id no longer resolves (deletion race / foreign
    // id) gets explicit nulls rather than failing the snapshot — the entry
    // still renders with defined-but-empty presentation fields.
    let (db, _) = seeded_db();
    let resolver = QueueMetadataResolver::new(db.clone(), db);
    let ghost = SongId::new();
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(ghost),
    };
    let meta = resolver.resolve(&[entry]);
    assert_eq!(
        meta.get(&ghost).cloned(),
        Some(QueueEntryMeta::default()),
        "an id that does not resolve maps to explicit null metadata"
    );
}

#[test]
fn map_snapshot_attaches_resolved_metadata_to_each_entry() {
    // The UI snapshot's queue entries carry the resolved metadata — title /
    // artist / duration / cover — so the panel renders real songs, and the
    // current entry's title feeds the player bar.
    let song = SongId::new();
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(song),
    };
    let raw = PlayerSnapshot {
        state: PlaybackState::Playing,
        ..PlayerSnapshot::default()
    };
    let view = CoordinatorView {
        entries: vec![entry.clone()],
        failed_round: Default::default(),
        blocked: Default::default(),
        current: Some(entry),
        mode: PlayMode::Sequential,
    };
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        song,
        QueueEntryMeta {
            title: Some("晴天".to_owned()),
            artist: Some("周杰伦".to_owned()),
            duration_s: Some(239),
            cover_key: Some("cv1-seeded".to_owned()),
        },
    );
    let ui = map_snapshot(&raw, &view, &metadata);
    assert_eq!(ui.current_title.as_deref(), Some("晴天"));
    let row = &ui.queue[0];
    assert_eq!(row.title.as_deref(), Some("晴天"));
    assert_eq!(row.artist.as_deref(), Some("周杰伦"));
    assert_eq!(row.duration_s, Some(239));
    assert_eq!(row.cover_key.as_deref(), Some("cv1-seeded"));
}
