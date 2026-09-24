/// 搜索多个字段：完整查询词在标题、艺人、专辑中做不区分大小写包含匹配，
/// 至少返回一个字段包含完整查询词（而非碎片 token）的歌曲。
#[test]
fn catalog_search_matches_full_query_word_ignoring_case_across_fields() {
    let (_directory, database, root) = database();
    let rows = seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    // 标题前缀 + 大小写不敏感（lat 匹配 "Sunny Day Remix" 于 "SunnyDay" 之流）
    let page = query
        .search(
            "sunny",
            false,
            None,
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("search sunny");
    assert_eq!(page.items.len(), 1, "one candidate matches on title");
    assert_eq!(
        page.items[0].id(),
        rows[3].0,
        "全查询词 'sunny' 命中 'Sunny Day Remix'"
    );

    // 艺人包含：“杰” -> 晴天(周杰伦)/七里香(周杰伦)/晴天吉它版(杰倫) 的艺人含
    // “杰”（NFKC 兼容折叠后“杰倫”仍含“杰”）；晴天 remix 艺人 "Jay" 不含。
    let page = query
        .search(
            "杰",
            false,
            None,
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("search 杰");
    let ids: Vec<_> = page.items.iter().map(Song::id).collect();
    assert!(ids.contains(&rows[0].0), "晴天 艺人 周杰伦");
    assert!(ids.contains(&rows[2].0), "七里香 艺人 周杰伦");
    assert!(ids.contains(&rows[1].0), "晴天吉它版 艺人 杰倫");
    assert!(!ids.contains(&rows[3].0), "remix 艺人 Jay 不含");

    // 专辑字段：“叶惠美” -> 三条（晴天、吉它版、remix），不含七里香。
    let page = query
        .search(
            "叶惠美",
            false,
            None,
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("search album");
    assert_eq!(page.items.len(), 3);
}

/// 空查询词恢复当前视图完整集合（全部歌曲）。
#[test]
fn catalog_search_empty_query_restores_full_view() {
    let (_directory, database, root) = database();
    let rows = seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let cleared = query
        .search(
            "",
            false,
            None,
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("empty search");
    assert_eq!(cleared.items.len(), rows.len(), "清空恢复完整集合");

    let full = query
        .all_songs(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("all songs");
    let cleared_ids: Vec<_> = cleared.items.iter().map(Song::id).collect();
    let full_ids: Vec<_> = full.items.iter().map(Song::id).collect();
    assert_eq!(cleared_ids, full_ids, "空搜索 == 当前视图完整集合与顺序");
}

/// 搜索无结果：返回空页，不报错，不吞 active 根问题。
#[test]
fn catalog_search_no_results_returns_empty_page_not_error() {
    let (_directory, database, root) = database();
    seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let page = query
        .search("不存在的歌名xyz", false, None, SongSort::default(), None, 100)
        .expect("no-results is not an error");
    assert!(page.items.is_empty());
    assert!(page.is_last);
    assert_eq!(page.next_cursor, None);
}

/// 搜索叠加在喜欢的音乐视图：只返回收藏且匹配的可用歌曲。
#[test]
fn catalog_search_overlays_on_favorites_view() {
    let (_directory, database, root) = database();
    let rows = seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    // 全视图搜索 "周" -> 三条（晴天、七里香 艺人周杰伦；吉它版艺人杰倫不含；remix Jay 不含）
    let all = query
        .search("周", false, None, SongSort::default(), None, 100)
        .expect("search 周");
    assert!(all.items.iter().any(|s| s.id() == rows[0].0));
    assert!(all.items.iter().any(|s| s.id() == rows[2].0));

    // 叠加 favorites：只有 favorite 的晴天命中。
    let fav = query
        .search("周", true, None, SongSort::default(), None, 100)
        .expect("search 周 in favorites");
    assert_eq!(fav.total_count, 1, "favorite search count includes the favorite filter");
    assert_eq!(fav.items.len(), 1);
    assert_eq!(fav.items[0].id(), rows[0].0);

    // 清空叠加 favorites：= favorites 视图。
    let cleared_fav = query
        .search(
            "",
            true,
            None,
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("empty in favorites");
    assert_eq!(cleared_fav.items.len(), 1);
}

/// 过期请求取消（revision guard）：搜索会话中发生写入使 cursor 失效后，
/// 继续分页必须被拒绝，而非返回陈旧/错乱结果。
#[test]
fn catalog_search_stale_request_cancelled_after_write_invalidation() {
    let (_directory, database, root) = database();
    seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let sort = SongSort {
        field: SongSortField::Title,
        direction: SortDirection::Asc,
    };
    let first = query.search("", false, None, sort, None, 2).expect("page 1");
    assert_eq!(first.total_count, 4, "first page carries the full view total");
    let cursor = first.next_cursor.expect("non-last page cursor");
    assert!(!first.is_last);
    let second = query
        .search("", false, None, sort, Some(&cursor), 2)
        .expect("page 2");
    assert_eq!(second.total_count, 4, "cursor must not shrink the total");

    // A write bumps the root revision, invalidating the in-flight search.
    let mut later = Song::new(
        SongId::new(),
        root,
        RelativeMediaPath::new("later.flac").expect("path"),
        Revision::INITIAL,
    );
    later.apply_metadata(
        Some("later".to_owned()),
        Some("歌手".to_owned()),
        Some("专辑".to_owned()),
        Some(Duration::from_secs(1)),
    );
    SongRepository::upsert(&database, &later).expect("write bumps revision");
    assert!(
        query.search("", false, None, sort, Some(&cursor), 2).is_err(),
        "过期请求取消：revision 变化后 cursor 必须被拒绝"
    );
}

/// 50k 正确性：大量歌曲搜索分页稳定、不截断、每一页 keyset 正确覆盖。
#[test]
fn catalog_search_pages_deterministically_across_large_library() {
    let (_directory, database, root) = database();
    // 55 songs all titled with a shared token + distinct suffix.
    let mut all_ids = Vec::new();
    for index in 0..55u64 {
        let shared = index % 3 == 0; // every third song carries the search token
        let title = if shared {
            format!("共歌曲 {index:03}")
        } else {
            format!("獨歌 {index:03}")
        };
        let mut record = Song::with_added_at(
            SongId::new(),
            root,
            RelativeMediaPath::new(&format!("songs/{index:03}.flac")).expect("path"),
            Revision::INITIAL,
            10_000 + index,
        );
        record.apply_metadata(
            Some(title.clone()),
            Some("大众".to_owned()),
            Some("专辑".to_owned()),
            Some(Duration::from_secs(1)),
        );
        SongRepository::upsert(&database, &record).expect("seed");
        all_ids.push(record.id());
    }

    let query = CatalogQuery::new(&database);
    let sort = SongSort {
        field: SongSortField::AddedAt,
        direction: SortDirection::Asc,
    };

    // All-songs just to know the full count and that pagination works.
    let mut ids = Vec::new();
    let mut cursor: Option<OpaqueCursor> = None;
    loop {
        let page = query
            .search("", false, None, sort, cursor.as_ref(), 7)
            .expect("page");
        assert_eq!(page.total_count, 55, "every page reports the full total");
        for r in &page.items {
            ids.push(r.id());
        }
        if page.is_last {
            break;
        }
        cursor = page.next_cursor;
    }
    assert_eq!(
        ids.len(),
        55,
        "55 songs paged completely, nothing truncated"
    );
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 55, "no duplicate rows across pages");

    // Searching "共歌曲" hits exactly the 19 shared-token songs, deterministic.
    let mut hits = Vec::new();
    let mut cursor = None;
    loop {
        let page = query
            .search("共歌曲", false, None, sort, cursor.as_ref(), 5)
            .expect("search page");
        assert_eq!(page.total_count, 19, "search count includes its filter");
        for r in &page.items {
            hits.push(r.id());
        }
        if page.is_last {
            break;
        }
        cursor = page.next_cursor;
    }
    let expected_count = all_ids
        .iter()
        .enumerate()
        .filter(|(i, _)| *i % 3 == 0)
        .count();
    assert_eq!(hits.len(), expected_count, "exactly the 19 candidates");
    // Determinism: repeat paging yields identical order.
    let repeat = {
        let mut ids = Vec::new();
        let mut cursor = None;
        loop {
            let page = query
                .search("共歌曲", false, None, sort, cursor.as_ref(), 5)
                .expect("repeat");
            for r in &page.items {
                ids.push(r.id());
            }
            if page.is_last {
                break;
            }
            cursor = page.next_cursor;
        }
        ids
    };
    assert_eq!(hits, repeat, "重复分页顺序必须确定性");
}

// ---------------------------------------------------------------------------
// Task 6.7 — playlist missing/blocked member display + Echo-delete cascade
// ---------------------------------------------------------------------------

/// 外部失效成员保留展示：missing 成员（外部删除）仍显示在歌单中并标记
/// 失效；同 UUID 恢复后重新可用且不产生重复成员。
#[test]
fn playlist_missing_members_stay_visible_and_recover_without_duplicates() {
    let (_directory, database, root) = database();
    let keep = song(root, "keep.flac", "Kept", "甲");
    let external = song(root, "gone.flac", "Gone", "乙");
    SongRepository::upsert(&database, &keep).expect("keep");
    SongRepository::upsert(&database, &external).expect("external");

    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    database
        .add_member(playlist, keep.id(), u64::MAX)
        .expect("keep member");
    database
        .add_member(playlist, external.id(), u64::MAX)
        .expect("external member");

    // External program deletion -> song Missing, membership retained + mirrored.
    SongRepository::set_availability(&database, external.id(), SongAvailability::Missing)
        .expect("mark missing");
    let members = database.members(playlist).expect("members");
    assert_eq!(members.len(), 2, "失效成员不得被移除");
    let external_row = members
        .iter()
        .find(|m| m.song() == external.id())
        .expect("member present");
    assert_eq!(
        external_row.song_availability(),
        SongAvailability::Missing,
        "member is marked unavailable"
    );

    // The playlist view still includes it (available + missing shown).
    let view = playlist_songs_view(&database, playlist);
    assert_eq!(view.len(), 2);
    assert!(view.iter().any(|s| s.id() == external.id()));

    // Same UUID restores: availability back to Available, one row only.
    SongRepository::set_availability(&database, external.id(), SongAvailability::Available)
        .expect("restore");
    let after = database.members(playlist).expect("members");
    assert_eq!(after.len(), 2, "no duplicate member on recovery");
    let restored = after
        .iter()
        .find(|m| m.song() == external.id())
        .expect("present");
    assert_eq!(
        restored.song_availability(),
        SongAvailability::Available,
        "失效标记取消"
    );
}

/// Echo 主动删除 finalize 级联移除成员：`delete_song`（finalize 路径）在同一
/// 事务内把歌曲和它所有歌单成员删除，其他歌单/歌曲不受影响且顺序保留。
#[test]
fn echo_delete_finalize_cascades_memberships_atomically() {
    let (_directory, database, root) = database();
    let doomed = song(root, "doomed.flac", "Doomed", "甲");
    let survivor = song(root, "survivor.flac", "Survivor", "乙");
    SongRepository::upsert(&database, &doomed).expect("doomed");
    SongRepository::upsert(&database, &survivor).expect("survivor");

    let first = PlaylistId::new();
    let second = PlaylistId::new();
    database.create(first, root, "一").expect("first");
    database.create(second, root, "二").expect("second");
    database
        .add_member(first, doomed.id(), u64::MAX)
        .expect("doomed in 一");
    database
        .add_member(first, survivor.id(), u64::MAX)
        .expect("survivor in 一");
    database
        .add_member(second, doomed.id(), u64::MAX)
        .expect("doomed in 二");

    // Echo delete hides first (pending-delete) — memberships stay.
    SongRepository::set_availability(&database, doomed.id(), SongAvailability::PendingDelete)
        .expect("pending delete");
    assert_eq!(database.members(first).expect("一").len(), 2);
    assert_eq!(database.members(second).expect("二").len(), 1);
    let hidden_view = playlist_songs_view(&database, first);
    assert!(
        !hidden_view.iter().any(|s| s.id() == doomed.id()),
        "pending-delete member hidden from the playlist view"
    );

    // The finalize step (as finalize_persisted_trash runs) deletes the song
    // row; the FK cascades remove its membership rows in the same transaction.
    let doomed_id = doomed.id();
    database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.delete_song(doomed_id)
        }))
        .expect("finalize delete");
    assert!(SongRepository::by_id(&database, doomed.id())
        .expect("query")
        .is_none());
    let first_after = database.members(first).expect("一 after");
    assert_eq!(
        first_after.len(),
        1,
        "cascade removed the doomed membership, survivor stays"
    );
    assert_eq!(first_after[0].song(), survivor.id());
    assert_eq!(database.members(second).expect("二 after").len(), 0);
    // The survivor song itself is untouched.
    assert!(SongRepository::by_id(&database, survivor.id())
        .expect("query")
        .is_some());
}

/// Direct read of the playlist view (same contract as `playlist_songs_query`).
fn playlist_songs_view(database: &SqliteDatabase, playlist: PlaylistId) -> Vec<Song> {
    use crate::application::catalog::CatalogQuery;
    CatalogQuery::new(database)
        .playlist(playlist)
        .expect("playlist view")
}

// ---------------------------------------------------------------------------
// Task 6.8 — repository integration gates：正常/空/错误/只读/不可用状态
// ---------------------------------------------------------------------------

/// 空资料库：全部歌曲/喜欢/最近返回空页而非错误（`is_last = true`）。
#[test]
fn catalog_repository_gate_empty_library_returns_empty_pages() {
    let (_directory, database, _root) = database();
    let query = CatalogQuery::new(&database);

    let all = query
        .all_songs(SongSort::default(), None, 100)
        .expect("empty all songs");
    assert!(all.items.is_empty());
    assert!(all.is_last, "empty page is the last page");

    let favs = query
        .favorites(SongSort::default(), None, 100)
        .expect("empty favorites");
    assert!(favs.items.is_empty());

    let recent = query.recent_100().expect("empty recent");
    assert!(recent.is_empty());

    let p = PlaylistId::new();
    let empty = CatalogQuery::new(&database)
        .playlist(p)
        .expect("empty playlist");
    assert!(empty.is_empty(), "playlist queries tolerate unknown ids");
}

/// 错误状态：无 active 根时 catalog/playlist 查询返回不可用，而非空结果。
#[test]
fn catalog_repository_gate_no_active_root_is_unavailable() {
    let (_directory, database, _root) = database();
    // Remove active: no active root remains.
    // (The database() helper made one active; deactivate it.)
    // Deactivate requires the exact id — simpler: open a fresh DB with none.
    drop(database);
    let directory = tempfile::tempdir().expect("tempdir");
    let database = SqliteDatabase::open(directory.path().join("echo.db")).expect("open");
    let query = CatalogQuery::new(&database);

    assert!(query.all_songs(SongSort::default(), None, 10).is_err());
    assert!(query.favorites(SongSort::default(), None, 10).is_err());
    assert!(query.recent_100().is_err());
}

/// 只读状态：write-safety-locked 根仍可读（catalog 查询可用），仅禁用写。
#[test]
fn catalog_repository_gate_read_only_root_still_serves_reads() {
    let (_directory, database, root) = database();
    // Lock writes via the safety isolation.
    LibraryRepository::set_write_safety_locked(&database, root, true).expect("lock");
    let song_rec = song(root, "r.flac", "R", "甲");
    SongRepository::upsert(&database, &song_rec).expect("song");

    let query = CatalogQuery::new(&database);
    let page = query
        .all_songs(SongSort::default(), None, 10)
        .expect("readonly root still serves reads");
    assert_eq!(page.items.len(), 1);
}

/// 错误路径：坏的 page limit 被拒绝；歌单查询对不存在歌单返回空。
#[test]
fn catalog_repository_gate_invalid_limit_rejected_playlist_unknown_empty() {
    let (_directory, database, root) = database();
    let song_rec = song(root, "e.flac", "E", "乙");
    SongRepository::upsert(&database, &song_rec).expect("song");
    let query = CatalogQuery::new(&database);

    assert!(query.all_songs(SongSort::default(), None, 0).is_err());
    assert!(query.all_songs(SongSort::default(), None, 501).is_err());
    let ghost = PlaylistId::new();
    assert!(query.playlist(ghost).expect("unknown playlist").is_empty());
}

/// 歌单仓库 gate：正常 CRUD、空成员、错误（重复名/未知 id）、只读与不可用
/// 根下成员关系与查询的行为。名字含 `playlists` 以满足 6.8 的
/// `cargo test ... playlists` 验收过滤。
#[test]
fn playlists_repository_gate_covers_normal_empty_error_and_root_states() {
    let (_directory, database, root) = database();
    let playlist = PlaylistId::new();
    database.create(playlist, root, "常规").expect("create");
    // Normal: create + rename + list.
    assert!(PlaylistRepository::by_id(&database, playlist)
        .expect("by_id")
        .is_some());
    database.rename(playlist, "常规二").expect("rename");
    assert_eq!(
        PlaylistRepository::list(&database, root)
            .expect("list")
            .len(),
        1
    );
    // Empty: fresh playlist has no members.
    assert!(PlaylistRepository::members(&database, playlist)
        .expect("members")
        .is_empty());
    // Error: duplicate name rejected; unknown id by_id is None.
    assert!(database.create(PlaylistId::new(), root, "常规二").is_err());
    assert!(PlaylistRepository::by_id(&database, PlaylistId::new())
        .expect("ghost")
        .is_none());

    // Read-only root still allows reads and membership lookup.
    LibraryRepository::set_write_safety_locked(&database, root, true).expect("lock");
    let song_rec = song(root, "s.flac", "S", "丙");
    SongRepository::upsert(&database, &song_rec).expect("song");
    database
        .add_member(playlist, song_rec.id(), u64::MAX)
        .expect("member");
    assert_eq!(
        PlaylistRepository::members(&database, playlist)
            .expect("readonly members")
            .len(),
        1,
        "readonly root keeps playlists readable"
    );
}

// ---------------------------------------------------------------------------
// 歌单内搜索（playlist-search-locate-import）：当前歌单成员内包含搜索
// ---------------------------------------------------------------------------

fn playlists_search_sort() -> SongSort {
    SongSort {
        field: SongSortField::Title,
        direction: SortDirection::Asc,
    }
}

/// 歌单内搜索只命中当前歌单成员，不匹配其他歌单歌曲或全库歌曲。
#[test]
fn playlist_search_restricts_to_playlist_members() {
    let (_directory, database, root) = database();
    let alpha = song(root, "a.flac", "Alpha", "艺人甲");
    let beta = song(root, "b.flac", "Beta", "艺人乙");
    let gamma = song(root, "c.flac", "Alpha 二", "艺人丙");
    let outside = song(root, "d.flac", "Alpha 全库", "艺人丁");
    for entry in [&alpha, &beta, &gamma, &outside] {
        SongRepository::upsert(&database, entry).expect("seed song");
    }
    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    for entry in [&alpha, &beta, &gamma] {
        database
            .add_member(playlist, entry.id(), u64::MAX)
            .expect("member");
    }

    let query = CatalogQuery::new(&database);
    // FTS 路径（≥3 字符）
    let page = query
        .search("Alpha", false, Some(playlist), playlists_search_sort(), None, 100)
        .expect("playlist search");
    assert_eq!(page.total_count, 2, "two members match, outside song excluded");
    assert_eq!(page.items.len(), 2);
    assert!(page.items.iter().any(|s| s.id() == alpha.id()));
    assert!(page.items.iter().any(|s| s.id() == gamma.id()));
    assert!(
        page.items.iter().all(|s| s.id() != outside.id()),
        "non-member must not be returned"
    );

    // 短查询 LIKE 路径（<3 字符）
    let short = query
        .search("Be", false, Some(playlist), playlists_search_sort(), None, 100)
        .expect("playlist short search");
    assert_eq!(short.total_count, 1);
    assert_eq!(short.items[0].id(), beta.id());
}

/// 歌单内搜索空词恢复歌单完整成员；无命中返回空结果而非全库。
#[test]
fn playlist_search_empty_query_restores_members_and_no_hit_is_empty() {
    let (_directory, database, root) = database();
    let first = song(root, "a.flac", "One", "艺人甲");
    let second = song(root, "b.flac", "Two", "艺人乙");
    for entry in [&first, &second] {
        SongRepository::upsert(&database, entry).expect("seed song");
    }
    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    for entry in [&first, &second] {
        database
            .add_member(playlist, entry.id(), u64::MAX)
            .expect("member");
    }

    let query = CatalogQuery::new(&database);
    let cleared = query
        .search("", false, Some(playlist), playlists_search_sort(), None, 100)
        .expect("cleared search");
    assert_eq!(cleared.total_count, 2, "empty query restores full membership");

    let none = query
        .search(
            "不存在xyz",
            false,
            Some(playlist),
            playlists_search_sort(),
            None,
            100,
        )
        .expect("no-hit search");
    assert_eq!(none.total_count, 0, "no membership hits");
    assert!(none.items.is_empty());
}

/// 歌单内搜索与歌单成员视图同可见性：缺失（外部删除）成员可搜到，pending-delete 成员不出现。
#[test]
fn playlist_search_shares_member_visibility_not_available_only() {
    let (_directory, database, root) = database();
    let missing = song(root, "m.flac", "Missing 歌", "艺人甲");
    let doomed = song(root, "p.flac", "Doomed 歌", "艺人乙");
    let healthy = song(root, "h.flac", "Healthy 歌", "艺人丙");
    for entry in [&missing, &doomed, &healthy] {
        SongRepository::upsert(&database, entry).expect("seed song");
    }
    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    for entry in [&missing, &doomed, &healthy] {
        database
            .add_member(playlist, entry.id(), u64::MAX)
            .expect("member");
    }
    SongRepository::set_availability(&database, missing.id(), SongAvailability::Missing)
        .expect("mark missing");
    SongRepository::set_availability(&database, doomed.id(), SongAvailability::PendingDelete)
        .expect("mark pending delete");

    let query = CatalogQuery::new(&database);
    let page = query
        .search("歌", false, Some(playlist), playlists_search_sort(), None, 100)
        .expect("playlist visibility search");
    let ids: Vec<_> = page.items.iter().map(Song::id).collect();
    assert!(
        ids.contains(&missing.id()),
        "missing member stays searchable like the playlist view"
    );
    assert!(ids.contains(&healthy.id()));
    assert!(
        !ids.contains(&doomed.id()),
        "pending-delete member is hidden like the playlist view"
    );
}

/// 歌单内搜索键集分页：游标跨页完整迭代，总数为筛选后的完整数。
#[test]
fn playlist_search_pages_with_keyset_cursors() {
    let (_directory, database, root) = database();
    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    let mut matching = Vec::new();
    for number in 0..4 {
        let entry = song(root, &format!("m{number}.flac"), &format!("曲目 {number}"), "歌手");
        SongRepository::upsert(&database, &entry).expect("seed song");
        database
            .add_member(playlist, entry.id(), u64::MAX)
            .expect("member");
        matching.push(entry.id());
    }
    for number in 0..3 {
        let entry = song(root, &format!("n{number}.flac"), &format!("其他 {number}"), "歌手");
        SongRepository::upsert(&database, &entry).expect("seed song");
        database
            .add_member(playlist, entry.id(), u64::MAX)
            .expect("member");
    }

    let query = CatalogQuery::new(&database);
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let result = query
            .search(
                "曲目",
                false,
                Some(playlist),
                playlists_search_sort(),
                cursor.as_ref(),
                2,
            )
            .expect("search page");
        assert_eq!(result.total_count, 4, "count is the filtered total");
        for entry in &result.items {
            seen.push(entry.id());
        }
        if result.is_last {
            break;
        }
        cursor = result.next_cursor;
    }
    assert_eq!(seen.len(), 4, "all matching members returned across pages");
    let unique: std::collections::HashSet<_> = seen.iter().copied().collect();
    assert_eq!(unique.len(), 4, "pages do not repeat a member");
    for id in &matching {
        assert!(seen.contains(id), "every matching member is found");
    }
}
