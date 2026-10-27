use super::*;

#[test]
fn confirmed_tags_update_while_local_audio_waits_for_safe_publication() {
    let (_directory, database, root) = database();
    let mut local = song(root, "media/original.flac", "Old title", "Old artist");
    local.apply_metadata(None, None, Some("Old album".into()), None);
    local.apply_scan_facts(
        "a".repeat(64),
        12,
        42,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(&database, &local).unwrap();
    let id = local.id();
    let payload = serde_json::json!({
        "song_uuid": id,
        "relative_path": "media/remote.flac",
        "blake3_hash": "b".repeat(64),
        "file_size": 24,
        "title": "New title",
        "artist": "New artist",
        "album": "New album",
        "duration_ms": 3000,
        "added_at": 1
    });
    let project = {
        let payload = payload.clone();
        move |connection: &mut Connection| {
            super::super::sync_projection::project_confirmed_song_state(
                connection,
                "song",
                &id.to_string(),
                &payload,
                Some(&root.to_string()),
            )
        }
    };
    database.writer.run(project).unwrap();
    assert_eq!(
        SongRepository::by_id(&database, id)
            .unwrap()
            .unwrap()
            .title(),
        Some("Old title"),
        "a pending local edit remains the visible overlay"
    );
    database
        .writer
        .run(move |connection| {
            connection
                .execute("UPDATE sync_mutations SET status='confirmed'", [])
                .map_err(storage)?;
            super::super::sync_projection::project_confirmed_song_state(
                connection,
                "song",
                &id.to_string(),
                &payload,
                Some(&root.to_string()),
            )
        })
        .unwrap();
    let updated = SongRepository::by_id(&database, id).unwrap().unwrap();
    assert_eq!(updated.title(), Some("New title"));
    assert_eq!(updated.artist(), Some("New artist"));
    assert_eq!(updated.album(), Some("New album"));
    assert_eq!(updated.path(), local.path());
    assert_eq!(updated.blake3_hash(), local.blake3_hash());
    assert_eq!(updated.file_size(), local.file_size());
    assert_eq!(updated.file_mtime_ns(), local.file_mtime_ns());
    assert_eq!(updated.availability(), SongAvailability::Available);
    assert_eq!(
        database
            .query_active_songs("New album", SongSort::default(), None, 10)
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(
        database
            .query_active_songs("Old album", SongSort::default(), None, 10)
            .unwrap()
            .items
            .is_empty()
    );
}
