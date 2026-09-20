/// Task 12.5 benchmark: a 50,000-song synthetic library, asserting the PRD
/// latency budgets (search p95 ≤ 200 ms, view first-screen p95 ≤ 500 ms) over
/// the real SQLite query path. This is an acceptance *budget*, not a micro
/// bench — it runs a bounded number of iterations through `CatalogQuery` on a
/// temp DB and reports the p95. The budget is deliberately exclusive of scan /
/// fixture creation (only query latency matters for the UI feel).
///
/// On slow CI runners the absolute budgets may be flaky; but the PRD is a hard
/// p95 target, so the test fails loudly rather than being waived. (A synthetic
/// 50k seed on this machine is a few hundred ms.) Devs can run it with
/// `cargo test -p echo-core --all-features -- --ignored` if they only want the
/// other fast tests; it is `#[ignore]`d by default to keep normal `cargo test`
/// fast and deterministic, and the verify:task 12.5 check runs it explicitly.
#[test]
#[ignore = "run explicitly via the 12.5 benchmark check (seeds 50k into temp SQLite)"]
fn bench_50k_search_and_first_screen_p95_meet_prd_budgets() {
    use std::time::Instant;

    const N: u64 = 50_000;
    let (_directory, database, root) = database();
    // Seed exactly 50,000 songs. Every 5th carries the search token "合成" so
    // the trigram search is meaningful; titles/artists are varied for index
    // pressure.
    let now = Instant::now();
    for index in 0..N {
        let shared = index % 5 == 0;
        let title = if shared {
            format!("合成歌曲{index:05}")
        } else {
            format!("普通歌曲{index:05}")
        };
        let artist = format!("艺人{}", index % 97);
        let mut rec = song(root, &format!("audio/{index:05}.flac"), &title, &artist);
        rec.apply_metadata(
            Some(title.clone()),
            Some(artist),
            Some("合成专辑".to_owned()),
            Some(Duration::from_secs(200)),
        );
        SongRepository::upsert(&database, &rec).expect("seed");
    }
    let seed_secs = now.elapsed().as_secs_f64();

    let query = CatalogQuery::new(&database);
    let added = SongSort {
        field: SongSortField::AddedAt,
        direction: SortDirection::Asc,
    };

    // --- Search p95 ≤ 200 ms ---
    let mut search_samples = Vec::new();
    for _ in 0..12 {
        let t = Instant::now();
        let page = query
            .search("合成歌曲", false, added, None, 50)
            .expect("search");
        search_samples.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(!page.items.is_empty(), "search must hit the seeded token");
    }
    let search_p95 = p95(&mut search_samples);
    assert!(
        search_p95 <= 200.0,
        "search p95 {search_p95:.1} ms exceeded the 200 ms budget"
    );

    // --- First-screen (all_songs page 1) p95 ≤ 500 ms ---
    let mut view_samples = Vec::new();
    for _ in 0..12 {
        let t = Instant::now();
        let page = query.all_songs(added, None, 50).expect("first page");
        view_samples.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(page.items.len(), 50, "first screen = one viewport page");
    }
    let view_p95 = p95(&mut view_samples);
    assert!(
        view_p95 <= 500.0,
        "view first-screen p95 {view_p95:.1} ms exceeded the 500 ms budget"
    );

    let _ = std::io::Write::write_fmt(
        &mut std::io::stdout(),
        format_args!(
            "bench 50k: seeded {N} songs in {seed_secs:.1}s; search p95 {search_p95:.1} ms (≤200), first-screen p95 {view_p95:.1} ms (≤500)\n",
        ),
    );
}

// ---------------------------------------------------------------------------
// Task 12.x — residual infrastructure branches: journal deadlines/claims,
// root-state mutations, TxAccess isolation, scan-issue code mapping and the
// query limit gate.
// ---------------------------------------------------------------------------

#[test]
fn journal_deadlines_claims_and_item_state_round_trip() {
    let (_directory, database, root) = database();
    let song = song(root, "jd.flac", "JD", "甲");
    SongRepository::upsert(&database, &song).expect("song");

    let operation = OperationId::new();
    database
        .create_operation(operation, root, "import", Some(song.id()))
        .expect("envelope");
    database
        .upsert_item(
            operation,
            OperationItem {
                kind: OperationResourceKind::Audio,
                state: OperationState::Planned,
                song: Some(song.id()),
                source: Some("hit".to_owned()),
                staging_path: Some(RelativeMediaPath::new("staged.bin").expect("path")),
                target_path: RelativeMediaPath::new("目标/曲.flac").expect("path"),
                expected_hash: "c".repeat(64),
                item_key: "audio".to_owned(),
                claim_key: "目标/曲.flac".to_owned(),
            },
        )
        .expect("item");

    // item_state is the per-item read the recovery journal drives.
    let item = database
        .item_state(operation, "audio")
        .expect("item state")
        .expect("present");
    assert_eq!(item.state, OperationState::Planned);
    assert_eq!(item.item_key, "audio");
    assert_eq!(item.source.as_deref(), Some("hit"));
    assert_eq!(item.claim_key, "目标/曲.flac");

    // Undo deadline set + read, both before and after the claim release.
    assert_eq!(database.undo_deadline(operation).expect("deadline"), None);
    database
        .set_undo_deadline(operation, 123_456_789)
        .expect("deadline");
    assert_eq!(
        database
            .undo_deadline(operation)
            .expect("deadline")
            .unwrap(),
        123_456_789
    );
    // The item_state read reports the exact state the recovery step wrote.
    database
        .upsert_item(
            operation,
            OperationItem {
                kind: OperationResourceKind::Audio,
                state: OperationState::CopyApplied,
                song: Some(song.id()),
                source: Some("hit".to_owned()),
                staging_path: Some(RelativeMediaPath::new("staged.bin").expect("path")),
                target_path: RelativeMediaPath::new("目标/曲.flac").expect("path"),
                expected_hash: "c".repeat(64),
                item_key: "audio".to_owned(),
                claim_key: "目标/曲.flac".to_owned(),
            },
        )
        .expect("state machine advance");
    assert_eq!(
        database
            .item_state(operation, "audio")
            .expect("item")
            .unwrap()
            .state,
        OperationState::CopyApplied
    );
    database.release_claims(operation).expect("release");
    assert_eq!(claim_active(&database, operation), 0);
    // A deadline remains observable after the claim release.
    assert_eq!(
        database
            .undo_deadline(operation)
            .expect("deadline")
            .unwrap(),
        123_456_789
    );
}

#[test]
fn root_state_mutations_flip_active_write_capability_and_safety_lock() {
    let (directory, database, root) = database();
    let other = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(other, directory.path().join("other"), false, true),
    )
    .expect("second inactive root");

    // Deactivate the active root (only the root record changes; the id still
    // resolves to an inactive record).
    LibraryRepository::deactivate(&database, root).expect("deactivate");
    let stored = LibraryRepository::by_id(&database, root)
        .expect("query")
        .expect("present");
    assert!(!stored.is_active(), "the root record is inactive now");
    assert!(
        database.active_root().expect("active query").is_none(),
        "no root is active once the only active one is deactivated"
    );

    // Re-activate the second root and flip its write/availability flags in
    // one transaction.
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(other, directory.path().join("other"), true, true),
    )
    .expect("reactivate second root");
    LibraryRepository::set_write_and_availability(&database, other, false, false).expect("flip");
    let flipped = LibraryRepository::by_id(&database, other)
        .expect("query")
        .expect("present");
    assert!(
        !flipped.observed_write_capable(),
        "write_capable flipped off"
    );
    assert!(
        flipped.availability() == RootAvailability::Unavailable,
        "availability flipped to unavailable"
    );

    // The safety lock can be toggled independently.
    LibraryRepository::set_write_safety_locked(&database, other, true).expect("lock on");
    LibraryRepository::set_write_safety_locked(&database, other, false).expect("lock off");
    assert!(!LibraryRepository::by_id(&database, other)
        .expect("query")
        .expect("present")
        .write_safety_locked());
}

#[test]
fn tx_access_isolates_root_writes_and_exposes_every_write_surface() {
    let (_directory, database, root) = database();
    let song = song(root, "tx.flac", "TX", "甲");
    SongRepository::upsert(&database, &song).expect("song");

    // isolate_root_writes flips the safety lock and availability atomically.
    database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.isolate_root_writes(root, false)
        }))
        .expect("isolate");
    let frozen = LibraryRepository::by_id(&database, root)
        .expect("query")
        .expect("present");
    assert!(frozen.write_safety_locked(), "writes isolated");
    assert_eq!(frozen.availability(), RootAvailability::Unavailable);

    // The remaining write surface works inside a UnitOfWork transaction:
    // roots, playlists, members, journal claims, lyrics and runtime state.
    let other_root = LibraryRootId::new();
    let other = LibraryRoot::new(other_root, database.path().join("other"), false, true);
    let playlist = PlaylistId::new();
    let staging = RelativeMediaPath::new("trash/op-audio").expect("path");
    let target = RelativeMediaPath::new("目标/曲.flac").expect("path");
    let operation = OperationId::new();
    let song_id = song.id();
    // The journal envelope must exist before an item can attach to it.
    database
        .create_operation(operation, root, "import", None)
        .expect("envelope");
    let candidate = LyricsCandidate::new(
        crate::domain::entities::LyricsSource::Embedded,
        vec![LyricsLine {
            timestamp_ms: 0,
            text: "a".to_owned(),
            original_index: 0,
        }],
        false,
    );
    let result: Result<(), Error> = database.with_tx(Box::new(move |tx: &mut dyn TxAccess| {
        tx.upsert_root(&other)?;
        tx.create_playlist(playlist, root, "存在")?;
        tx.insert_member(&PlaylistMember::new(
            playlist,
            song.id(),
            0,
            SongAvailability::Available,
        ))?;
        tx.upsert_operation_item(
            operation,
            OperationItem {
                kind: OperationResourceKind::Lyrics,
                state: OperationState::Planned,
                song: Some(song_id),
                source: None,
                staging_path: Some(staging.clone()),
                target_path: target.clone(),
                expected_hash: "d".repeat(64),
                item_key: "lyrics".to_owned(),
                claim_key: target.display().to_owned(),
            },
        )?;
        tx.set_lyrics_candidate(song_id, &candidate)?;
        tx.clear_lyrics_candidate(song_id, crate::domain::entities::LyricsSource::Embedded)?;
        tx.set_runtime_state("test-key", "test-value")
    }));
    // Clear the just-written candidate in the same transaction proves the
    // clear path is atomic with the write it removes.
    result.expect("tx write surface");

    assert!(LibraryRepository::by_id(&database, other_root)
        .expect("query")
        .expect("present")
        .absolute_path()
        .ends_with("other"));
    assert_eq!(
        PlaylistRepository::by_id(&database, playlist).unwrap(),
        Some(playlist)
    );
    assert_eq!(database.members(playlist).expect("members").len(), 1);
    assert_eq!(
        database
            .item_state(operation, "lyrics")
            .expect("item")
            .unwrap()
            .kind,
        OperationResourceKind::Lyrics
    );
    assert_eq!(
        database.load("test-key").expect("runtime state").as_deref(),
        Some("test-value")
    );
    // The lyrics candidate was cleared by the same transaction.
    assert!(database.candidates(song_id).expect("candidates").is_empty());
}

#[test]
fn scan_issue_codes_map_deterministically_and_unknown_codes_are_scan_file_error() {
    let (_directory, database, root) = database();
    crate::application::ports::ScanRunRepository::begin_run(&database, root, 1)
        .expect("run row exists for the FK");
    for code in [
        "unsupported_media",
        "no_audio_track",
        "corrupt_media",
        "duplicate_content",
        "tag_limit",
        "unknown_thing",
    ] {
        let diagnostic = MediaDiagnostic::new(
            RelativeMediaPath::new(&format!("issues/{code}.mp3")).expect("path"),
            code,
            format!("detail {code}"),
            false,
        );
        database
            .record_issue(root, 1, &diagnostic)
            .expect("record issue");
    }

    let issues = database.scan_issues(root, 1).expect("issues");
    // record_issue stores the code unchanged; scan_issues maps unknown codes
    // to the stable `scan_file_error`.
    let mapped: Vec<(&str, &str)> = issues
        .iter()
        .map(|issue| (issue.code(), issue.path().display()))
        .collect();
    assert!(mapped.contains(&("unsupported_media", "issues/unsupported_media.mp3")));
    assert!(mapped.contains(&("no_audio_track", "issues/no_audio_track.mp3")));
    assert!(mapped.contains(&("corrupt_media", "issues/corrupt_media.mp3")));
    assert!(mapped.contains(&("duplicate_content", "issues/duplicate_content.mp3")));
    assert!(mapped.contains(&("tag_limit", "issues/tag_limit.mp3")));
    // The unknown stored code normalizes to the generic scan-file-error.
    assert!(
        mapped.iter().any(|(code, path)| {
            *code == "scan_file_error" && *path == "issues/unknown_thing.mp3"
        }),
        "unknown stored issue codes map to scan_file_error: {mapped:?}"
    );
}

#[test]
fn page_limit_gate_rejects_zero_and_oversized_limits() {
    let (_directory, database, _root) = database();
    let sort = SongSort::default();
    for limit in [0usize, 501] {
        let error = database
            .query_active_songs("", sort, None, limit)
            .expect_err("limit rejected");
        assert_eq!(error.code(), "validation", "limit {limit}");
        let error = database
            .search_active_songs("", sort, None, limit)
            .expect_err("limit rejected");
        assert_eq!(error.code(), "validation", "search limit {limit}");
    }
    // Exact boundary values are accepted (empty result, not an error).
    assert!(database
        .query_active_songs("", sort, None, 1)
        .expect("limit 1")
        .items
        .is_empty());
    assert!(database
        .query_active_songs("", sort, None, 500)
        .expect("limit 500")
        .items
        .is_empty());
}
