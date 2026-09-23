#[test]
fn scan_runs_progress_issues_round_trip_through_sqlite() {
    let stack = ScanStack::new();
    stack.write("good.mp3", b"good");
    stack.probe.set(
        "good.mp3",
        crate::application::ports::ProbeOutcome::Audio {
            format: crate::domain::media::AudioFormat::Flac,
            duration: Some(Duration::from_secs(1)),
        },
    );
    stack.metadata.set(
        "good.mp3",
        crate::domain::media::ParsedMetadata {
            title: Some("Good".to_owned()),
            duration: Some(Duration::from_secs(1)),
            format: crate::domain::media::AudioFormat::Flac,
            ..crate::domain::media::ParsedMetadata::default()
        },
    );
    // One bad file forms a persistent issue row.
    stack.write("bad.mp3", b"bad");
    stack.probe.set(
        "bad.mp3",
        crate::application::ports::ProbeOutcome::Unsupported,
    );

    // A stepping clock makes the throttle deterministic (60 ms steps vs a
    // 100 ms interval: batch snapshots are suppressed, terminal persists).
    let mut deps = stack.deps();
    deps.clock = Arc::new(SteppingClock::new(60));
    let summary = StartScan::new(&deps, &ScanSupervisor::new())
        .run(stack.root)
        .expect("scan ok");
    assert_eq!(summary.progress.failed, 1);

    let (state, progress, finished) = stack
        .database
        .scan_run_snapshot(stack.root, 1)
        .expect("run row")
        .expect("row");
    assert_eq!(state, ScanRunState::Completed);
    assert_eq!(progress.discovered, 2);
    assert_eq!(progress.failed, 1);
    assert!(finished, "terminal state never lost");
    let issues = stack.database.scan_issues(stack.root, 1).expect("issues");
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code(), "unsupported_media");
    // The bad file did not block the good one.
    assert_eq!(
        stack
            .database
            .query_active_songs("", SongSort::default(), None, 100)
            .expect("query")
            .items
            .len(),
        1
    );
}

#[test]
fn activation_commits_active_root_and_epoch_atomically() {
    let stack = ScanStack::new();
    let second_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();
    let root_two =
        crate::application::root_switch::derive_root_id(&second_dir.path().canonicalize().unwrap());
    stack
        .fs
        .add_root_at(root_two, second_dir.path().to_path_buf());

    let deps = stack.deps();
    let supervisor = ScanSupervisor::new();
    // Prepare + activate the empty second root.
    let prepared = PrepareLibraryCandidate::new(&deps, stack.database.as_ref(), &supervisor)
        .prepare(second_dir.path())
        .expect("prepare");
    let outcome = ActivateLibrary::new(
        &deps,
        stack.database.as_ref(),
        &supervisor,
        stack.database.as_ref(),
        &Blockers::new(),
    )
    .activate(prepared.root_id)
    .expect("activate");
    assert_eq!(outcome.epoch.as_u64(), 1);
    // One transaction flipped active + epoch: both are visible now.
    let active = stack.database.active_root().expect("active").expect("root");
    assert_eq!(active.id(), root_two);
    assert_eq!(
        stack
            .database
            .load(ROOT_EPOCH_KEY)
            .expect("epoch")
            .as_deref(),
        Some("1")
    );
    // And the previous root record is kept but inactive.
    assert!(
        !LibraryRepository::by_id(stack.database.as_ref(), stack.root)
            .expect("by id")
            .unwrap()
            .is_active()
    );
}

// ---------------------------------------------------------------------------
// Task 6.1 — catalog view scenarios (全部歌曲 / 最近添加 / 喜欢的音乐 / 歌单)
// ---------------------------------------------------------------------------

/// Seed the active root with songs covering every view-relevant state.
/// Returns `(available, missing, pending_delete, favorite)` so tests can assert
/// membership of each view without re-deriving ids from the database.
fn seed_view_fixture(
    database: &SqliteDatabase,
    root: LibraryRootId,
) -> (Vec<Song>, Vec<Song>, Vec<Song>, Vec<Song>) {
    let mut available = Vec::new();
    let mut missing = Vec::new();
    let mut pending_delete = Vec::new();
    let mut favorite = Vec::new();

    // Deterministic insertion order key (`added_at`) — the recent-view ladder.
    for (stamp, index) in (1000u64..).zip(0..6) {
        let mut record = Song::with_added_at(
            SongId::new(),
            root,
            RelativeMediaPath::new(&format!("songs/tune-{index}.flac")).expect("path"),
            Revision::INITIAL,
            stamp,
        );
        record.apply_metadata(
            Some(format!("title {index}")),
            Some(format!("artist {index}")),
            Some(format!("album {}", index % 3)),
            Some(Duration::from_secs(180)),
        );
        if index == 2 {
            record.mark_missing();
            missing.push(record.clone());
        } else if index == 4 {
            record.set_favorite(true);
            available.push(record.clone());
            favorite.push(record.clone());
        } else if index == 5 {
            record.begin_pending_delete();
            pending_delete.push(record.clone());
        } else {
            available.push(record.clone());
        }
        SongRepository::upsert(database, &record).expect("seed song");
    }

    // A second, inactive root must never leak into any view.
    let other_root = LibraryRootId::new();
    let mut foreign = Song::new(
        SongId::new(),
        other_root,
        RelativeMediaPath::new("songs/foreign.flac").expect("path"),
        Revision::INITIAL,
    );
    foreign.apply_metadata(
        Some("foreign".to_owned()),
        Some("outsider".to_owned()),
        Some("album".to_owned()),
        Some(Duration::from_secs(180)),
    );
    // The inactive root's record must exist for the foreign song to be valid.
    let dir = tempfile::tempdir().expect("tempdir");
    LibraryRepository::upsert(
        database,
        &LibraryRoot::new(other_root, dir.path().join("other"), false, true),
    )
    .expect("insert inactive root");
    SongRepository::upsert(database, &foreign).expect("seed foreign song");

    (available, missing, pending_delete, favorite)
}

/// 全部歌曲: only the active root's available songs, pending-delete hidden,
/// other roots never visible.
#[test]
fn catalog_all_songs_view_covers_active_root_available_songs_only() {
    let (_directory, database, root) = database();
    let (expected, missing, pending_delete, favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let page = query
        .all_songs(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("all songs");
    assert!(page.is_last);
    assert_eq!(page.next_cursor, None);

    let mut ids: Vec<_> = page.items.iter().map(Song::id).collect();
    let mut expected_ids: Vec<_> = expected.iter().map(Song::id).collect();
    ids.sort();
    expected_ids.sort();
    assert_eq!(ids, expected_ids, "available songs of the active root only");
    for record in &page.items {
        assert_ne!(record.id(), pending_delete[0].id(), "pending-delete hidden");
        assert_ne!(
            record.id(),
            missing[0].id(),
            "missing hidden from all-songs"
        );
        assert_eq!(record.availability(), SongAvailability::Available);
    }
    // Favorite songs still appear in 全部歌曲 (favorite is orthogonal).
    assert!(page.items.iter().any(|s| s.id() == favorite[0].id()));
}

/// 喜欢的音乐: favorites of the active root, keyset-stable, pending-delete
/// hidden, other roots never visible.
#[test]
fn catalog_favorites_view_is_favorited_available_active_root_songs_only() {
    let (_directory, database, root) = database();
    let (_available, _missing, _pending, favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let page = query
        .favorites(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("favorites");
    assert!(page.is_last);
    let mut ids: Vec<_> = page.items.iter().map(Song::id).collect();
    let mut favored_ids: Vec<_> = favorite.iter().map(Song::id).collect();
    ids.sort();
    favored_ids.sort();
    assert_eq!(
        ids, favored_ids,
        "exactly the favorited, available songs of the active root"
    );
    assert!(page.items.iter().all(Song::favorite));

    // Un-favoriting removes the song from the view immediately.
    SongRepository::set_favorite(&database, favorite[0].id(), false).expect("unfavorite");
    let after = query
        .favorites(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("favorites after unfavorite");
    assert!(after.items.is_empty(), "取消收藏后歌曲立即从该视图移除");
}

#[test]
fn catalog_favorites_orders_by_the_latest_favorite_action() {
    let (_directory, database, root) = database();
    let first = song(root, "songs/first.flac", "先收藏", "艺人");
    let last = song(root, "songs/last.flac", "后收藏", "艺人");
    SongRepository::upsert(&database, &first).expect("seed first");
    SongRepository::upsert(&database, &last).expect("seed last");
    SongRepository::set_favorite(&database, first.id(), true).expect("favorite first");
    // `favorited_at` records the real commit clock. Cross a millisecond here
    // rather than manufacturing a timestamp in production just to make a
    // same-tick test sort differently.
    std::thread::sleep(std::time::Duration::from_millis(2));
    SongRepository::set_favorite(&database, last.id(), true).expect("favorite last");

    // 最近添加 in the favorites view means the favorite action time, not the
    // song's original library insertion time.
    let page = CatalogQuery::new(&database)
        .favorites(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Desc,
            },
            None,
            100,
        )
        .expect("favorites");

    assert_eq!(
        page.items.iter().map(Song::id).collect::<Vec<_>>(),
        vec![last.id(), first.id()],
        "the song liked last is the first favorite row"
    );
}

#[test]
fn catalog_favorites_honors_its_own_manual_sort() {
    let (_directory, database, root) = database();
    let alphabetically_first = song(root, "songs/first.flac", "A song", "艺人");
    let alphabetically_last = song(root, "songs/last.flac", "Z song", "艺人");
    SongRepository::upsert(&database, &alphabetically_first).expect("seed first");
    SongRepository::upsert(&database, &alphabetically_last).expect("seed last");
    SongRepository::set_favorite(&database, alphabetically_first.id(), true)
        .expect("favorite first");
    SongRepository::set_favorite(&database, alphabetically_last.id(), true).expect("favorite last");

    let page = CatalogQuery::new(&database)
        .favorites(
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("favorites by title");

    assert_eq!(
        page.items.iter().map(Song::id).collect::<Vec<_>>(),
        vec![alphabetically_first.id(), alphabetically_last.id()],
        "a favorites-only choice must not inherit the default recent-favorite order"
    );
}

/// 资料库导航计数: the SQLite implementation must agree with the views it
/// advertises *and* with the in-memory fake, so a count can never be an
/// artefact of one backend. The fixture is deliberately small enough that the
/// answer is "4 / 1 / 4" by inspection.
#[test]
fn catalog_counts_match_view_membership_over_sqlite() {
    let (_directory, database, root) = database();
    let (available, _missing, _pending_delete, favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let counts = query.counts().expect("counts");
    assert_eq!(
        counts.all,
        available.len(),
        "全部歌曲 counts the active root's available songs"
    );
    assert_eq!(
        counts.favorites,
        favorite.len(),
        "喜欢的音乐 counts favorites"
    );
    assert_eq!(counts.recent, counts.all, "recent is uncapped below 100");
    assert_eq!(counts.artists, query.collections(CatalogCollectionKind::Artist, "").unwrap().len());
    assert_eq!(counts.albums, query.collections(CatalogCollectionKind::Album, "").unwrap().len());

    // Cross-check against the views themselves, then against a mutation.
    assert_eq!(
        query
            .all_songs(SongSort::default(), None, 100)
            .expect("all")
            .items
            .len(),
        counts.all,
        "count agrees with the rendered 全部歌曲 rows"
    );

    // Toggle a favorite: the count must follow the commit.
    let target = available
        .iter()
        .find(|song| !song.favorite())
        .expect("a non-favorited available song");
    SongRepository::set_favorite(&database, target.id(), true).expect("favorite");
    let after = query.counts().expect("counts after favorite");
    assert_eq!(after.favorites, counts.favorites + 1, "收藏后计数 +1");
    assert_eq!(after.all, counts.all, "收藏不改变全部歌曲总数");
}

/// No active root is an absent library, not an empty one: returning
/// `{0, 0, 0}` would have the sidebar print "0" next to views it cannot open.
#[test]
fn catalog_counts_are_unavailable_without_an_active_root() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = SqliteDatabase::open(directory.path().join("echo.db")).expect("open");
    assert!(matches!(
        CatalogQuery::new(&database).counts(),
        Err(Error::Unavailable { .. })
    ));
}

/// 最近添加: the newest 100 available songs by added_at desc, stable UUID
/// tie-break, pending-delete + missing hidden, other roots never visible.
#[test]
fn catalog_recent_100_is_newest_available_with_stable_tie_break() {
    let (_directory, database, root) = database();
    let (_available, missing, pending_delete, _favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let recent = query.recent_100().expect("recent 100");
    // All six seeded songs have distinct added_at; the two non-available ones
    // (missing + pending-delete) are dropped, leaving four.
    assert_eq!(recent.len(), 4);
    let ids: Vec<_> = recent.iter().map(Song::id).collect();
    let missing_id = missing[0].id();
    let pending_id = pending_delete[0].id();
    assert!(!ids.contains(&missing_id), "missing hidden");
    assert!(!ids.contains(&pending_id), "pending-delete hidden");
    // Newest first on the added_at ladder.
    let stamps: Vec<_> = recent.iter().map(Song::added_at).collect();
    let mut sorted = stamps.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(stamps, sorted, "最新添加排在前面");

    // Stable tie-break: two records sharing an added_at keep deterministic order.
    let mut tied_a = Song::with_added_at(
        SongId::new(),
        root,
        RelativeMediaPath::new("songs/tied-a.flac").expect("path"),
        Revision::INITIAL,
        999,
    );
    let mut tied_b = Song::with_added_at(
        SongId::new(),
        root,
        RelativeMediaPath::new("songs/tied-b.flac").expect("path"),
        Revision::INITIAL,
        999,
    );
    tied_a.apply_metadata(
        Some("tied a".into()),
        Some("a".into()),
        Some("album".into()),
        Some(Duration::from_secs(1)),
    );
    tied_b.apply_metadata(
        Some("tied b".into()),
        Some("b".into()),
        Some("album".into()),
        Some(Duration::from_secs(2)),
    );
    SongRepository::upsert(&database, &tied_a).expect("tied a");
    SongRepository::upsert(&database, &tied_b).expect("tied b");
    let recent_again = query.recent_100().expect("recent again");
    assert_eq!(recent_again.len(), 6, "the two tied songs join the four");
    assert!(
        recent_again.iter().any(|s| s.id() == tied_a.id()),
        "tied a present"
    );
    assert!(
        recent_again.iter().any(|s| s.id() == tied_b.id()),
        "tied b present"
    );
    // It's deterministic: re-running reproduces the same relative order.
    let recent_thrice = query.recent_100().expect("recent thrice");
    let order_one: Vec<_> = recent_again.iter().map(Song::id).collect();
    let order_two: Vec<_> = recent_thrice.iter().map(Song::id).collect();
    assert_eq!(order_one, order_two, "重复刷新不得随机改变顺序");
}

/// 歌单: members by position over the active root; available + missing shown,
/// pending-delete hidden.
#[test]
fn catalog_playlist_view_shows_available_and_missing_hides_pending_delete() {
    let (_directory, database, root) = database();
    let (available_songs, missing_songs, pending_songs, _favorite) =
        seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    database
        .add_member(playlist, available_songs[0].id(), 0)
        .expect("member available");
    database
        .add_member(playlist, missing_songs[0].id(), 1)
        .expect("member missing");
    database
        .add_member(playlist, pending_songs[0].id(), 2)
        .expect("member pending");

    let songs = query.playlist(playlist).expect("playlist songs");
    let ids: Vec<_> = songs.iter().map(Song::id).collect();
    assert_eq!(ids.len(), 2, "pending-delete member hidden");
    assert_eq!(ids[0], available_songs[0].id(), "position 0 first");
    assert_eq!(ids[1], missing_songs[0].id(), "position 1 second");
    assert!(!ids.contains(&pending_songs[0].id()));

    // A missing member was already visible (missing rows display so a blocked
    // row can be shown); restoring it to available keeps it in place, and the
    // pending-delete member stays hidden throughout.
    SongRepository::set_availability(
        &database,
        missing_songs[0].id(),
        SongAvailability::Available,
    )
    .expect("restore missing");
    let after = query.playlist(playlist).expect("playlist after restore");
    assert_eq!(after.len(), 2, "still two visible members");
    assert_eq!(
        after[0].id(),
        available_songs[0].id(),
        "position order kept"
    );
    assert_eq!(
        after[1].id(),
        missing_songs[0].id(),
        "restored member in place"
    );
}

/// 全部歌曲 keyset pagination is stable and hidden songs never appear, and a
/// stale cursor (revision guarded) is rejected.
#[test]
fn catalog_all_songs_keyset_pages_stably_and_rejects_stale_cursor() {
    let (_directory, database, root) = database();
    let (expected, _missing, _pending, _favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    for field in SongSortField::ALL {
        for direction in [SortDirection::Asc, SortDirection::Desc] {
            let sort = SongSort { field, direction };
            let mut collected: Vec<SongId> = Vec::new();
            let mut cursor: Option<OpaqueCursor> = None;
            loop {
                let page = query.all_songs(sort, cursor.as_ref(), 2).expect("page");
                for record in &page.items {
                    assert_eq!(record.availability(), SongAvailability::Available);
                }
                let page_ids: Vec<_> = page.items.iter().map(Song::id).collect();
                collected.extend(page_ids);
                if page.is_last {
                    break;
                }
                cursor = page.next_cursor;
                assert!(cursor.is_some(), "non-last page must carry a cursor");
            }
            let mut ordered = expected.clone();
            ordered.sort_by(|left, right| sort.compare(left, right));
            assert_eq!(
                collected,
                ordered.iter().map(Song::id).collect::<Vec<_>>(),
                "{field:?} {direction:?} preserves the comparator order across pages"
            );
            let mut expected_ids: Vec<_> = expected.iter().map(Song::id).collect();
            expected_ids.sort();
            collected.sort();
            assert_eq!(
                collected, expected_ids,
                "{field:?} {direction:?} covers active available only"
            );
        }
    }

    // A write after the cursor was minted invalidates it (revision guard).
    let first = query
        .all_songs(
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            1,
        )
        .expect("first page");
    let cursor = first.next_cursor.expect("cursor");
    let mut extra = Song::new(
        SongId::new(),
        root,
        RelativeMediaPath::new("songs/extra.flac").expect("path"),
        Revision::INITIAL,
    );
    extra.apply_metadata(
        Some("extra".into()),
        Some("artist".into()),
        Some("album".into()),
        Some(Duration::from_secs(1)),
    );
    SongRepository::upsert(&database, &extra).expect("insert extra");
    assert!(
        query
            .all_songs(
                SongSort {
                    field: SongSortField::Title,
                    direction: SortDirection::Asc,
                },
                Some(&cursor),
                1,
            )
            .is_err(),
        "stale cursor rejected"
    );
}

// ---------------------------------------------------------------------------
// Task 6.2 — 资料库搜索：标题/艺人/专辑全体词包含、随视图叠加、清空恢复
// ---------------------------------------------------------------------------

/// Seed a deterministically-titled active root for search scenarios. Returns
/// the songs in (title, artist, album) order so expectations are explicit.
fn seed_search_fixture(
    database: &SqliteDatabase,
    root: LibraryRootId,
) -> Vec<(SongId, String, String, String)> {
    let rows = vec![
        ("晴天.mp3", "晴天", "周杰伦", "叶惠美"),
        ("晴天吉它版.flac", "晴天 (吉它版)", "杰倫", "叶惠美"),
        ("七里香.flac", "七里香", "周杰伦", "七里香"),
        ("晴天remix.flac", "Sunny Day Remix", "Jay", "叶惠美"),
    ];
    let mut out = Vec::new();
    for (index, (path, title, artist, album)) in rows.into_iter().enumerate() {
        let mut record = Song::with_added_at(
            SongId::new(),
            root,
            RelativeMediaPath::new(path).expect("path"),
            Revision::INITIAL,
            5000 + index as u64,
        );
        record.apply_metadata(
            Some(title.to_owned()),
            Some(artist.to_owned()),
            Some(album.to_owned()),
            Some(Duration::from_secs(200)),
        );
        if index == 0 {
            record.set_favorite(true);
        }
        SongRepository::upsert(database, &record).expect("seed song");
        out.push((
            record.id(),
            title.to_owned(),
            artist.to_owned(),
            album.to_owned(),
        ));
    }
    out
}
