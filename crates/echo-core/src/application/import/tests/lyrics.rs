// -----------------------------------------------------------------------
// Task 3.1: the portable `media/` layout. Every imported song (and its
// `.lrc`) is published under `media/<artist>/<artist> - <title>`; the
// safe-naming, BLAKE3 dedup and ` (n)` conflict-numbering behaviours are
// preserved under the new tree.
// -----------------------------------------------------------------------

#[test]
fn batch_import_lands_under_media_and_keeps_dedup_and_numbering() {
    let g = gated();
    // Two tags-torn inputs (same artist/title) would collide under one
    // artist folder, plus an exact-content duplicate inside the same batch.
    g.sources.add("a", "one.flac", b"bytes-a");
    tagged(&g, b"bytes-a", Some("周杰伦"), Some("晴天"));
    g.fixture
        .set_audio("media/周杰伦/周杰伦 - 晴天.flac", "晴天", 1_000);
    g.sources.add("b", "two.flac", b"bytes-b");
    tagged(&g, b"bytes-b", Some("周杰伦"), Some("晴天"));
    g.fixture
        .set_audio("media/周杰伦/周杰伦 - 晴天 (2).flac", "晴天", 2_000);
    g.sources.add("c", "copy.flac", b"bytes-a");

    // First input: lands under media/, file + record at the same target.
    let report_a = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("a")])
        .expect("batch-level success");
    let ImportOutcome::Imported {
        song: song_a,
        target: target_a,
        ..
    } = &report_a.results[0]
    else {
        panic!("the first audio input must import");
    };
    assert!(
        target_a.display().starts_with("media/"),
        "imported target lives under media/: {}",
        target_a.display()
    );
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert_eq!(
        std::fs::read(root_dir.join(target_a.normalized())).expect("published"),
        b"bytes-a"
    );
    let record_a = g.deps.songs.by_id(*song_a).expect("query").expect("record");
    assert_eq!(
        record_a.path(),
        target_a,
        "the record's path is the media/ target"
    );

    // Second input (same tags): minimal ` (n)` under media/.
    // Third input (same content as the first): BLAKE3 dedup returns it.
    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("b"), source("c")])
        .expect("batch-level success");
    let ImportOutcome::Imported { target, .. } = &report.results[0] else {
        panic!(
            "the second tagged input must import: {:?}",
            report.results[0]
        );
    };
    assert_eq!(
        target.display(),
        "media/周杰伦/周杰伦 - 晴天 (2).flac",
        "minimal (n) numbering under media/"
    );
    assert_eq!(
        report.results[1],
        ImportOutcome::Duplicate { existing: *song_a },
        "BLAKE3 dedup holds across the media/ batch"
    );
    // One file per logical song, both under media/.
    assert_eq!(g.fixture.all_songs().len(), 2);
    assert!(root_dir.join("media/周杰伦/周杰伦 - 晴天.flac").is_file());
    assert!(root_dir
        .join("media/周杰伦/周杰伦 - 晴天 (2).flac")
        .is_file());
}

#[test]
fn imported_song_materializes_a_portable_record_with_the_committed_revision() {
    let g = gated();
    g.sources.add("a", "one.flac", b"bytes-a");
    tagged(&g, b"bytes-a", Some("周杰伦"), Some("晴天"));
    g.fixture
        .set_audio("media/周杰伦/周杰伦 - 晴天.flac", "晴天", 1_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("a")])
        .expect("import succeeds");
    let ImportOutcome::Imported { song, .. } = &report.results[0] else {
        panic!("audio must import");
    };

    // The portable record is materialized into the control plane (task 3.2:
    // object-records are the durable form of the outbox full payload).
    let records = g.fixture.control.records_of(g.fixture.root);
    let song_records: Vec<_> = records
        .iter()
        .filter(|rec| matches!(rec, PortableRecord::Song(_)))
        .collect();
    assert_eq!(song_records.len(), 1, "one song record materialized");
    let PortableRecord::Song(song_record) = &song_records[0] else {
        unreachable!("filtered above");
    };
    assert_eq!(song_record.song_uuid, song.as_uuid());
    assert_eq!(
        song_record.media_path.as_str(),
        "media/周杰伦/周杰伦 - 晴天.flac",
        "the record carries the media/ relative path, never an absolute one"
    );
    // Revision/HLC match a committed object (never zero).
    assert!(
        song_record.revision.as_u64() >= 1,
        "record carries the monotone outbox revision: {}",
        song_record.revision
    );
    // The content hash round-trips.
    assert!(song_record.content_hash.len() >= 32);
}
