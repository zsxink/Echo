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
    BootstrapSnapshot, ImportBatchDto, LibraryRootStatusDto, LibraryStatus, PagedSongs,
    PlaylistView, RevealResultDto, ScanSnapshot, SongDetailView, SongView,
};
use echo_desktop::ipc::IpcErrorDto;
use echo_desktop::player::coordinator::PlaybackCoordinator;
use echo_desktop::player::port::{PlayerCommand, PlayerPort};
use echo_desktop::player::queue::{QueueEntry, QueueItem, ViewContext};
use echo_desktop::runtime::services::AppServices;
use tauri::State;

/// The managed player handle the command layer drives. The coordinator is
/// locked per command (it is not internally synchronized). Direct commands
/// (play/pause/seek/volume) go through the coordinator's player port; the
/// forwarder thread reads the port separately (not via this handle).
pub struct PlayerHandle {
    pub coordinator: Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
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
pub fn recent(services: State<'_, AppServices>) -> Result<Vec<SongView>, IpcErrorDto> {
    services.recent().map_err(IpcErrorDto::from)
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
    root: String,
    song: String,
) -> Result<String, IpcErrorDto> {
    let root = parse_id::<LibraryRootId>(&root, "root")?;
    let song = parse_id::<SongId>(&song, "song")?;
    services.delete_song(root, song).map_err(IpcErrorDto::from)
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
pub fn choose_library_root(
    services: State<'_, AppServices>,
) -> Result<Option<LibraryRootStatusDto>, IpcErrorDto> {
    services.choose_library_root().map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn choose_and_import_files(
    services: State<'_, AppServices>,
) -> Result<Option<ImportBatchDto>, IpcErrorDto> {
    services
        .choose_and_import_files()
        .map_err(IpcErrorDto::from)
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
    local_state: State<'_, echo_desktop::platform::local_state::DesktopStateStore>,
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
    local_state: State<'_, echo_desktop::platform::local_state::DesktopStateStore>,
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

#[tauri::command]
pub fn play_context(
    state: State<'_, PlayerHandle>,
    songs: Vec<String>,
    selected_index: usize,
) -> Result<(), IpcErrorDto> {
    let ids: Vec<SongId> = songs
        .iter()
        .filter_map(|s| s.parse::<SongId>().ok())
        .collect();
    if ids.is_empty() {
        return Err(IpcErrorDto::from(&echo_core::error::Error::validation(
            echo_core::error::Subject::Other,
            "songs",
            "no valid songs".to_owned(),
        )));
    }
    let ctx = ViewContext {
        songs: ids,
        selected_index,
    };
    let mut coord = state.coordinator.lock().expect("player coordinator lock");
    coord.play_context(&ctx);
    Ok(())
}

#[tauri::command]
pub fn play_temporary_file(
    state: State<'_, PlayerHandle>,
    path: String,
    display_name: String,
) -> Result<(), IpcErrorDto> {
    // The path comes from an OS file-open boundary that already validated it
    // (task 9.1/9.2); the actor re-checks local-path hardness on load.
    let coord = state.coordinator.clone();
    {
        let mut coord = coord.lock().expect("player coordinator lock");
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
                return Err(IpcErrorDto::from(
                    &echo_core::error::Error::validation(
                        echo_core::error::Subject::Other,
                        "current",
                        "no temporary item is playing",
                    ),
                ));
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
    state: State<'_, PlayerHandle>,
    command: String,
    song_id: Option<String>,
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
        "clearPending" => coord.clear_pending(),
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
