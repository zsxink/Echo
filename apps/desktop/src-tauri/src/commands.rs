//! Thin Tauri command layer (task 10.2 / 10.6 / 11.1).
//!
//! Every function here is deliberately thin: it parses the incoming args,
//! calls a real method on [`AppServices`] or the `PlaybackCoordinator`, and
//! maps `Result` to the `IpcErrorDto` envelope. All behaviour is tested one
//! layer down (in `echo-desktop`'s `AppServices` / coordinator / runtime), so
//! this module stays untestable-shape and logic-free.
//!
//! Command names are `snake_case` to match the frontend `BridgeCommandMap`
//! exactly (the frontend invokes these strings verbatim).
//!
//! The Tauri `#[tauri::command]` macro requires owned (by-value) arguments, so
//! `needless_pass_by_value` does not apply to command signatures. Two other
//! pedantic lints are intrinsic to this thin layer:
//! - `significant_drop_tightening`: holding the coordinator `MutexGuard` for
//!   the whole command body is intended (we mutate across the match).
//! - `unnecessary_wraps`: the never-failing player commands still return
//!   `Ok(())` because the frontend bridge types every command as
//!   `T | IpcErrorDto`; a future failure path belongs here.
#![allow(
    clippy::needless_pass_by_value,
    clippy::significant_drop_tightening,
    clippy::unnecessary_wraps
)]

use std::sync::{Arc, Mutex};

use echo_core::domain::catalog::{OpaqueCursor, SongSort, SongSortField, SortDirection};
use echo_core::domain::ids::{LibraryRootId, OperationId, PlaylistId, SongId};
use echo_desktop::ipc::dto::{
    BootstrapSnapshot, ImportBatchDto, ImportResultDto, LibraryCountsDto, LibraryRootStatusDto,
    LibraryStatus, PagedSongs, PlaylistView, RevealResultDto, ScanSnapshot, SongDetailView,
    SongView,
};
use echo_desktop::ipc::IpcErrorDto;
use echo_desktop::player::coordinator::PlaybackCoordinator;
use echo_desktop::player::port::{PlayerCommand, PlayerPort};
use echo_desktop::player::queue::{QueueEntry, QueueItem, ViewContext};
use echo_desktop::player::session::{snapshot_queue, SessionPersistence};
use echo_desktop::runtime::services::AppServices;
use tauri::State;

/// The managed player handle the command layer drives. The coordinator is
/// locked per command (it is not internally synchronized). Direct commands
/// (play/pause/seek/volume) go through the coordinator's player port; the
/// forwarder thread reads the port separately (not via this handle).
pub struct PlayerHandle {
    pub coordinator: Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
    /// The view the current playback context was built from ("allSongs" /
    /// "favorites" / "recent" / "search" / "playlist:<id>") — the 记住当前播
    /// 放的是哪个歌单 half of local persistence. Written by the playback-context
    /// commands, read by the session-saver thread (same shared slot).
    pub source: Arc<std::sync::Mutex<Option<String>>>,
    /// The desktop-local session store is shared with the throttled saver so
    /// lifecycle boundaries can force one final write before hiding or exit.
    pub persistence: Arc<dyn SessionPersistence>,
}

/// Persist the latest authoritative playback snapshot synchronously for a
/// lifecycle boundary. Normal progress writes remain throttled in the runtime;
/// this only closes the gap where a window/tray exit happens between ticks.
pub fn flush_player_session(state: &PlayerHandle) {
    let Ok(coordinator) = state.coordinator.lock() else {
        return;
    };
    let snapshot = coordinator.snapshot();
    let source = state.source.lock().ok().and_then(|value| value.clone());
    let session = snapshot_queue(
        coordinator.queue(),
        coordinator.mode(),
        snapshot.volume,
        snapshot.muted,
        snapshot.position,
        source.as_deref(),
    );
    let _ = state.persistence.save(Some(&session));
}

// ---------------------------------------------------------------------------
// Small parse helpers (arguments arrive as strings; the domain parses them)
// ---------------------------------------------------------------------------

fn parse_sort(token: &str) -> Result<SongSort, IpcErrorDto> {
    let (field, direction) = token.split_once(':').ok_or_else(|| {
        IpcErrorDto::from(&echo_core::error::Error::validation(
            echo_core::error::Subject::Other,
            "sort",
            "sort must be field:direction".to_owned(),
        ))
    })?;
    let field = match field {
        "addedAt" => SongSortField::AddedAt,
        "title" => SongSortField::Title,
        "artist" => SongSortField::Artist,
        "playCount" => SongSortField::PlayCount,
        _ => {
            return Err(IpcErrorDto::from(&echo_core::error::Error::validation(
                echo_core::error::Subject::Other,
                "sort",
                "unknown sort field".to_owned(),
            )))
        }
    };
    let direction = match direction {
        "asc" => SortDirection::Asc,
        "desc" => SortDirection::Desc,
        _ => {
            return Err(IpcErrorDto::from(&echo_core::error::Error::validation(
                echo_core::error::Subject::Other,
                "sort",
                "unknown sort direction".to_owned(),
            )))
        }
    };
    Ok(SongSort { field, direction })
}

fn parse_cursor(token: Option<String>) -> Result<Option<OpaqueCursor>, IpcErrorDto> {
    let Some(token) = token else { return Ok(None) };
    // The frontend forwards the opaque cursor token (a JSON string) verbatim.
    // `OpaqueCursor` deserializes from a String; wrap with quotes so the raw
    // token round-trips.
    let quoted = format!("\"{token}\"");
    serde_json::from_str::<OpaqueCursor>(&quoted)
        .map(Some)
        .map_err(|_| IpcErrorDto::from(&echo_core::error::Error::conflict("stale cursor")))
}

fn parse_id<T: std::str::FromStr>(raw: &str, field: &'static str) -> Result<T, IpcErrorDto> {
    raw.parse::<T>().map_err(|_| {
        IpcErrorDto::from(&echo_core::error::Error::validation(
            echo_core::error::Subject::Other,
            field,
            String::from("invalid id"),
        ))
    })
}

// ---------------------------------------------------------------------------
// Library / query commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_bootstrap_state(
    services: State<'_, AppServices>,
) -> Result<BootstrapSnapshot, IpcErrorDto> {
    services.bootstrap().map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn library_status(services: State<'_, AppServices>) -> Result<LibraryStatus, IpcErrorDto> {
    services.library_status().map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn all_songs(
    services: State<'_, AppServices>,
    sort: String,
    cursor: Option<String>,
    limit: usize,
) -> Result<PagedSongs, IpcErrorDto> {
    let sort = parse_sort(&sort)?;
    let cursor = parse_cursor(cursor)?;
    services
        .all_songs(sort, cursor.as_ref(), limit)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn search(
    services: State<'_, AppServices>,
    query: String,
    in_favorites: bool,
    sort: String,
    cursor: Option<String>,
    limit: usize,
) -> Result<PagedSongs, IpcErrorDto> {
    let sort = parse_sort(&sort)?;
    let cursor = parse_cursor(cursor)?;
    services
        .search(&query, in_favorites, sort, cursor.as_ref(), limit)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn favorites(
    services: State<'_, AppServices>,
    sort: String,
    cursor: Option<String>,
    limit: usize,
) -> Result<PagedSongs, IpcErrorDto> {
    let sort = parse_sort(&sort)?;
    let cursor = parse_cursor(cursor)?;
    services
        .favorites(sort, cursor.as_ref(), limit)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn recent(
    services: State<'_, AppServices>,
    query: String,
) -> Result<Vec<SongView>, IpcErrorDto> {
    services.recent(&query).map_err(IpcErrorDto::from)
}

/// Per-view song totals for the navigation sidebar (counts must be readable
/// before a view is ever opened).
#[tauri::command]
pub fn library_counts(services: State<'_, AppServices>) -> Result<LibraryCountsDto, IpcErrorDto> {
    services.library_counts().map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn playlists(services: State<'_, AppServices>) -> Result<Vec<PlaylistView>, IpcErrorDto> {
    services.playlists().map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn playlist_members(
    services: State<'_, AppServices>,
    playlist_id: String,
) -> Result<Vec<SongView>, IpcErrorDto> {
    let playlist = parse_id::<PlaylistId>(&playlist_id, "playlistId")?;
    services
        .playlist_members(playlist)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn song_detail(
    services: State<'_, AppServices>,
    song_id: String,
) -> Result<SongDetailView, IpcErrorDto> {
    let song = parse_id::<SongId>(&song_id, "songId")?;
    services.song_detail(song).map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn get_lyrics(
    services: State<'_, AppServices>,
    song_id: String,
) -> Result<echo_core::application::detail::SongLyrics, IpcErrorDto> {
    let song = parse_id::<SongId>(&song_id, "songId")?;
    services.get_lyrics(song).map_err(IpcErrorDto::from)
}

/// The opaque cover-asset keys of a batch of songs (design §115 内置优先).
///
/// The library list asks once per rendered window rather than per row: a song
/// whose audio file carries no embedded artwork is **absent** from the map and
/// the UI keeps the prototype's palette placeholder for it. Values are the cover
/// cache's opaque `cv1-…` keys, never a path — the `WebView` composes
/// `cover://<key>` and the protocol resolves it.
///
/// A malformed id is skipped instead of failing the batch, so one stale row on
/// screen cannot cost every other row its artwork.
#[tauri::command]
pub fn song_cover_keys(
    services: State<'_, AppServices>,
    song_ids: Vec<String>,
) -> Result<std::collections::BTreeMap<String, String>, IpcErrorDto> {
    let ids: Vec<SongId> = song_ids
        .iter()
        .filter_map(|id| id.parse::<SongId>().ok())
        .collect();
    services.cover_keys(&ids).map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn set_favorite(
    services: State<'_, AppServices>,
    song_id: String,
    favorite: bool,
) -> Result<SongView, IpcErrorDto> {
    let song = parse_id::<SongId>(&song_id, "songId")?;
    services
        .set_favorite(song, favorite)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn create_playlist(
    services: State<'_, AppServices>,
    root: String,
    name: String,
) -> Result<String, IpcErrorDto> {
    let root = parse_id::<LibraryRootId>(&root, "root")?;
    services
        .create_playlist(root, &name)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn rename_playlist(
    services: State<'_, AppServices>,
    id: String,
    name: String,
) -> Result<(), IpcErrorDto> {
    let playlist = parse_id::<PlaylistId>(&id, "id")?;
    services
        .rename_playlist(playlist, &name)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn set_playlist_cover(
    services: State<'_, AppServices>,
    id: String,
    bytes: Option<Vec<u8>>,
    mime: Option<String>,
) -> Result<(), IpcErrorDto> {
    let playlist = parse_id::<PlaylistId>(&id, "id")?;
    services
        .set_playlist_cover(playlist, bytes, mime.as_deref())
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn delete_playlist(services: State<'_, AppServices>, id: String) -> Result<(), IpcErrorDto> {
    let playlist = parse_id::<PlaylistId>(&id, "id")?;
    services
        .delete_playlist(playlist)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn add_to_playlists(
    services: State<'_, AppServices>,
    song: String,
    targets: Vec<String>,
) -> Result<(), IpcErrorDto> {
    let song = parse_id::<SongId>(&song, "song")?;
    let mut playlists = Vec::with_capacity(targets.len());
    for raw in &targets {
        playlists.push(parse_id::<PlaylistId>(raw, "targets")?);
    }
    services
        .add_to_playlists(song, &playlists)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn remove_playlist_song(
    services: State<'_, AppServices>,
    playlist: String,
    song: String,
) -> Result<(), IpcErrorDto> {
    let playlist = parse_id::<PlaylistId>(&playlist, "playlist")?;
    let song = parse_id::<SongId>(&song, "song")?;
    services
        .remove_playlist_song(playlist, song)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn delete_song(
    services: State<'_, AppServices>,
    player: State<'_, PlayerHandle>,
    root: String,
    song: String,
) -> Result<String, IpcErrorDto> {
    let root = parse_id::<LibraryRootId>(&root, "root")?;
    let song = parse_id::<SongId>(&song, "song")?;
    // Coordinated delete (task 8.11): if the song is in the playback queue the
    // coordinator snapshots/removes it under the same lock, stopping playback
    // first when it is the current entry — never deleting a file the player is
    // showing as current.
    echo_desktop::runtime::player::delete_song_coordinated(
        &player.coordinator,
        song,
        |song| services.delete_song(root, song),
        std::time::Duration::from_secs(3),
    )
    .map_err(|reason| IpcErrorDto::from(&echo_core::error::Error::unavailable("delete", &reason)))
}

#[tauri::command]
pub fn undo_delete(
    services: State<'_, AppServices>,
    root: String,
    operation: String,
) -> Result<String, IpcErrorDto> {
    let root = parse_id::<LibraryRootId>(&root, "root")?;
    let operation = parse_id::<OperationId>(&operation, "operation")?;
    services
        .undo_delete(root, operation)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub async fn choose_library_root(
    services: State<'_, AppServices>,
) -> Result<Option<LibraryRootStatusDto>, IpcErrorDto> {
    // Tauri executes synchronous commands on the event-loop thread. The real
    // dialog adapter deliberately uses the plugin's blocking picker, which
    // requires a worker thread while the native event loop continues pumping
    // dialog events. Making this command async gives it that worker context.
    services.choose_library_root().map_err(IpcErrorDto::from)
}

#[tauri::command]
pub async fn choose_and_import_files(
    services: State<'_, AppServices>,
    state: State<'_, PlayerHandle>,
) -> Result<Option<ImportBatchDto>, IpcErrorDto> {
    // Keep the other blocking native picker off the event loop for the same
    // reason as `choose_library_root` above.
    let result = services
        .choose_and_import_files()
        .map_err(IpcErrorDto::from)?;
    if result.as_ref().is_some_and(|batch| {
        batch
            .results
            .iter()
            .any(|item| matches!(item, ImportResultDto::Imported { .. }))
    }) {
        prime_initial_library_song(&services, &state);
    }
    Ok(result)
}

/// Put the newest available library song into the paused player bar only when
/// no current entry exists. Import completion is deliberately idempotent: it
/// never replaces a user-selected or restored current item.
fn prime_initial_library_song(services: &AppServices, state: &PlayerHandle) {
    let mut coordinator = state.coordinator.lock().expect("player coordinator lock");
    if coordinator.current().is_some() {
        return;
    }
    let Some(song) = services.latest_available_song().ok().flatten() else {
        return;
    };
    let snapshot = coordinator.snapshot();
    let mode = coordinator.mode();
    coordinator.prime_context_paused(
        &ViewContext {
            songs: vec![song],
            selected_index: 0,
        },
        mode,
        snapshot.volume,
        snapshot.muted,
    );
}

#[tauri::command]
pub fn reveal_song(
    services: State<'_, AppServices>,
    song_id: String,
) -> Result<RevealResultDto, IpcErrorDto> {
    let song = parse_id::<SongId>(&song_id, "songId")?;
    services.reveal_song(song).map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn start_scan(
    services: State<'_, AppServices>,
    root: String,
) -> Result<ScanSnapshot, IpcErrorDto> {
    let root = parse_id::<LibraryRootId>(&root, "root")?;
    services.start_scan(root).map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn cancel_scan(services: State<'_, AppServices>, root: String) -> bool {
    let Ok(root) = root.parse::<LibraryRootId>() else {
        return false;
    };
    services.cancel_scan(root)
}

#[tauri::command]
pub fn set_theme(
    local_state: State<'_, Arc<echo_desktop::platform::local_state::DesktopStateStore>>,
    theme: String,
) -> Result<(), IpcErrorDto> {
    let theme = match theme.as_str() {
        "coral" => echo_desktop::platform::local_state::DesktopTheme::Coral,
        "cobalt" => echo_desktop::platform::local_state::DesktopTheme::Cobalt,
        "turquoise" => echo_desktop::platform::local_state::DesktopTheme::Turquoise,
        _ => {
            return Err(IpcErrorDto::from(&echo_core::error::Error::validation(
                echo_core::error::Subject::Other,
                "theme",
                "unknown theme".to_owned(),
            )))
        }
    };
    local_state.set_theme(theme).map_err(|_| {
        IpcErrorDto::from(&echo_core::error::Error::unavailable(
            "preferences",
            "write failed",
        ))
    })
}

#[tauri::command]
pub fn set_close_behavior(
    local_state: State<'_, Arc<echo_desktop::platform::local_state::DesktopStateStore>>,
    behavior: String,
) -> Result<(), IpcErrorDto> {
    let behavior = match behavior.as_str() {
        "exit" => echo_desktop::platform::local_state::CloseBehavior::Exit,
        "background" => echo_desktop::platform::local_state::CloseBehavior::Background,
        _ => {
            return Err(IpcErrorDto::from(&echo_core::error::Error::validation(
                echo_core::error::Subject::Other,
                "behavior",
                "unknown close behavior".to_owned(),
            )))
        }
    };
    local_state.set_close_behavior(behavior).map_err(|_| {
        IpcErrorDto::from(&echo_core::error::Error::unavailable(
            "preferences",
            "write failed",
        ))
    })
}

/// Return the resolved desktop close behavior, including the platform default
/// when the user has never saved a preference.
#[tauri::command]
pub fn get_close_behavior(
    local_state: State<'_, Arc<echo_desktop::platform::local_state::DesktopStateStore>>,
) -> Result<String, IpcErrorDto> {
    local_state.close_behavior().map(String::from).map_err(|_| {
        IpcErrorDto::from(&echo_core::error::Error::unavailable(
            "preferences",
            "read failed",
        ))
    })
}

// ---------------------------------------------------------------------------
// Player commands (task 10.6 / 11.1). The snapshot event is the authority;
// these return `Ok(())` and let the actor's authoritative rollback drive UI.
// ---------------------------------------------------------------------------

fn lib_entry(song: SongId) -> QueueEntry {
    QueueEntry {
        id: echo_core::domain::ids::QueueEntryId::new(),
        item: QueueItem::Library(song),
    }
}

/// Start playback from a playlist, resolving the full member set on the
/// desktop side. The `WebView` supplies only the playlist id and the selected
/// song — never a song-id list — so a paged or partial client-side list cannot
/// truncate the queue (spec: 视图播放重建队列数量). The queue is atomically
/// replaced: any previous queue and manual "play next" lane are discarded.
#[tauri::command]
pub fn play_playlist_context(
    services: State<'_, AppServices>,
    state: State<'_, PlayerHandle>,
    playlist: String,
    selected_song: String,
) -> Result<(), IpcErrorDto> {
    let playlist = parse_id::<PlaylistId>(&playlist, "playlist")?;
    let selected = parse_id::<SongId>(&selected_song, "selectedSong")?;
    let songs = services
        .resolve_playlist_playback_context(playlist, selected)
        .map_err(IpcErrorDto::from)?;
    let selected_index = songs
        .iter()
        .position(|song| *song == selected)
        .ok_or_else(|| {
            IpcErrorDto::from(&echo_core::error::Error::conflict(
                "selected song is no longer a member of the playlist",
            ))
        })?;
    state
        .coordinator
        .lock()
        .expect("player coordinator lock")
        .play_context(&ViewContext {
            songs,
            selected_index,
        });
    if let Ok(mut slot) = state.source.lock() {
        *slot = Some(format!("playlist:{playlist}"));
    }
    Ok(())
}

/// Resolve a library view on desktop before building a queue. Unlike
/// `play_context`, this accepts no client-side page of song IDs.
#[tauri::command]
pub fn play_library_context(
    services: State<'_, AppServices>,
    state: State<'_, PlayerHandle>,
    view: String,
    query: String,
    sort: String,
    selected_song: String,
) -> Result<(), IpcErrorDto> {
    let sort = parse_sort(&sort)?;
    let selected = parse_id::<SongId>(&selected_song, "selectedSong")?;
    let songs = services
        .resolve_library_playback_context(&view, &query, sort, selected)
        .map_err(IpcErrorDto::from)?;
    let selected_index = songs
        .iter()
        .position(|song| *song == selected)
        .ok_or_else(|| {
            IpcErrorDto::from(&echo_core::error::Error::conflict(
                "selected song is no longer in the active library view",
            ))
        })?;
    state
        .coordinator
        .lock()
        .expect("player coordinator lock")
        .play_context(&ViewContext {
            songs,
            selected_index,
        });
    if let Ok(mut slot) = state.source.lock() {
        *slot = Some(view);
    }
    Ok(())
}

/// Cold-start playback restore (task 8.9 + 默认态): a persisted session is
/// rebuilt paused; with nothing persisted, the first song of 全部歌曲 is
/// primed into the 播放控制栏 (paused, 列表循环). Returns what happened:
/// "restored" / "primed" / "empty". The frontend calls this once at boot.
#[tauri::command]
pub fn restore_playback_session(
    services: State<'_, AppServices>,
    state: State<'_, PlayerHandle>,
    local_state: State<'_, Arc<echo_desktop::platform::local_state::DesktopStateStore>>,
) -> Result<String, IpcErrorDto> {
    let persistence = echo_desktop::player::session::StateStoreSession::new((*local_state).clone());
    // Initial playback candidate: Core owns the 最近添加 ordering and returns
    // the newest available song in the active root.
    let default_view_songs = || -> Vec<SongId> {
        services
            .latest_available_song()
            .ok()
            .flatten()
            .into_iter()
            .collect()
    };
    let outcome = echo_desktop::runtime::player::restore_or_prime_playback(
        &state.coordinator,
        &persistence,
        |session| services.playback_restore_verdicts(session),
        default_view_songs,
    );
    if outcome == "primed" {
        if let Ok(mut slot) = state.source.lock() {
            *slot = Some("allSongs".to_owned());
        }
    }
    Ok(outcome.to_owned())
}

#[tauri::command]
pub fn play_temporary_file(
    services: State<'_, AppServices>,
    state: State<'_, PlayerHandle>,
    path: String,
    display_name: String,
) -> Result<(), IpcErrorDto> {
    // The path comes from an OS file-open boundary that already validated it
    // (task 9.1/9.2); the actor re-checks local-path hardness on load.
    let coord = state.coordinator.clone();
    {
        let mut coord = coord.lock().expect("player coordinator lock");
        if let Some(song_id) = services
            .active_song_for_path(std::path::Path::new(&path))
            .map_err(IpcErrorDto::from)?
        {
            coord.play_context(&ViewContext {
                songs: vec![song_id],
                selected_index: 0,
            });
            return Ok(());
        }
        coord.play_temporary(echo_desktop::player::coordinator::TemporaryPlay {
            display_name,
            path: std::path::PathBuf::from(path),
            duration: None,
            on_active_root: false,
        });
    }
    Ok(())
}

/// Import the currently-playing temporary playback item into the active library
/// (task 11.7). The absolute path stays desktop-side — the `WebView` never
/// receives it; the command reads it directly from the coordinator's current
/// queue entry.
#[tauri::command]
pub fn import_current_temporary_file(
    services: State<'_, AppServices>,
    state: State<'_, PlayerHandle>,
) -> Result<echo_desktop::ipc::dto::ImportResultDto, IpcErrorDto> {
    // Read the temp file path from the coordinator under a short lock, then
    // release the lock before calling the (potentially slow) import pipeline.
    let (path, display_name) = {
        let coord = state.coordinator.lock().expect("player coordinator lock");
        match coord.current().map(|e| &e.item) {
            Some(echo_desktop::player::queue::QueueItem::Temporary(t)) => {
                (t.path.clone(), t.display_name.clone())
            }
            _ => {
                return Err(IpcErrorDto::from(&echo_core::error::Error::validation(
                    echo_core::error::Subject::Other,
                    "current",
                    "no temporary item is playing",
                )));
            }
        }
    };
    services
        .import_single_path(&path, &display_name)
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn player_control(state: State<'_, PlayerHandle>, action: String) -> Result<(), IpcErrorDto> {
    let mut coord = state.coordinator.lock().expect("player coordinator lock");
    match action.as_str() {
        "play" => {
            let _ = coord.player().send(PlayerCommand::Play);
        }
        "pause" => {
            let _ = coord.player().send(PlayerCommand::Pause);
        }
        "toggle" => {
            let _ = coord.player().send(PlayerCommand::TogglePlayPause);
        }
        "previous" => coord.previous(),
        "next" => {
            let _ = coord.advance_to_next();
        }
        s if s.starts_with("mode:") => {
            let mode = match s.strip_prefix("mode:") {
                Some("shuffle") => echo_desktop::player::port::PlayMode::Shuffle,
                Some("repeatOne") => echo_desktop::player::port::PlayMode::RepeatOne,
                _ => echo_desktop::player::port::PlayMode::Sequential,
            };
            coord.set_mode(mode);
        }
        _ => {}
    }
    Ok(())
}

#[tauri::command]
pub fn queue_command(
    services: State<'_, AppServices>,
    state: State<'_, PlayerHandle>,
    command: String,
    song_id: Option<String>,
    entry_id: Option<String>,
) -> Result<(), IpcErrorDto> {
    let mut coord = state.coordinator.lock().expect("player coordinator lock");
    match command.as_str() {
        "enqueue" => {
            if let Some(raw) = song_id {
                if let Ok(id) = raw.parse::<SongId>() {
                    coord.enqueue(lib_entry(id));
                }
            }
        }
        "playNext" => {
            if let Some(raw) = song_id {
                if let Ok(id) = raw.parse::<SongId>() {
                    coord.play_next(lib_entry(id));
                }
            }
        }
        "playEntry" => {
            if let Some(raw) = entry_id {
                if let Ok(id) = raw.parse() {
                    coord.play_queue_entry(id);
                }
            }
        }
        "clearPending" => coord.clear_pending(),
        "retryBlocked" => coord.retry_blocked(|song| services.playback_song_is_playable(song)),
        _ => {}
    }
    Ok(())
}

#[tauri::command]
pub fn set_volume(state: State<'_, PlayerHandle>, volume: f64) -> Result<(), IpcErrorDto> {
    let mut coord = state.coordinator.lock().expect("player coordinator lock");
    coord.set_volume(volume.clamp(0.0, 1.0));
    Ok(())
}

#[tauri::command]
pub fn toggle_mute(state: State<'_, PlayerHandle>) -> Result<(), IpcErrorDto> {
    let mut coord = state.coordinator.lock().expect("player coordinator lock");
    coord.toggle_mute();
    Ok(())
}

#[tauri::command]
pub fn seek(state: State<'_, PlayerHandle>, position: f64) -> Result<(), IpcErrorDto> {
    let mut coord = state.coordinator.lock().expect("player coordinator lock");
    coord.seek(position.max(0.0));
    Ok(())
}
