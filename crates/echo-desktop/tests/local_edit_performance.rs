//! Repeatable latency measurement of completed native library mutations.
//!
//! Run with real directory durability and SQLite, entirely below a temporary
//! directory (the user's library and application data are never opened):
//!
//! ```text
//! cargo test -p echo-desktop --test local_edit_performance --release -- --ignored --nocapture --test-threads=1
//! ECHO_PERF_MAX_P95_MS=50 cargo test -p echo-desktop --test local_edit_performance --release -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `ECHO_PERF_SONGS` (3000), `ECHO_PERF_HISTORY` (64), and
//! `ECHO_PERF_SAMPLES` (100 per command) change the fixture size. The fixture
//! contains synthetic metadata originals, not decoded audio: neither measured
//! command reads audio. The measured interval is the complete production
//! `AppServices` command, including directory commit and any foreground SQLite
//! catch-up needed before planning. The background SQLite projection is flushed
//! after both measured loops, outside the interaction latency interval.
//! `ECHO_PERF_SERVE=1` serves the same fixture through loopback HTTP for a
//! React integration measurement; the production IPC implementation is not
//! changed. The server prints its address and stops after `GET /shutdown`.
//! `ECHO_PERF_FIXTURE_PATH` reuses a synthetic fixture under the OS temporary
//! directory (or `/tmp`) whose name starts `echo-local-edit-perf-`. Reuse is
//! refused unless the fixture marker and requested song count agree.

use echo_core::application::ports::{
    ControlPlanePort, DeviceIdProvider, LibraryProjectionPort, LibraryRepository,
    PlaylistRepository, SongRepository, UnitOfWork,
};
use echo_core::application::root_switch::Blockers;
use echo_core::application::scan::ScanSupervisor;
use echo_core::domain::catalog::{OpaqueCursor, SongSort, SongSortField, SortDirection};
use echo_core::domain::entities::{LibraryRoot, SongAvailability};
use echo_core::domain::ids::{LibraryRootId, PlaylistId, Revision, SongId};
use echo_core::domain::library::durability::{ChangeOrigin, LibraryBatch};
use echo_core::domain::library::{
    HybridLogicalClock, LibraryId, LibraryManifest, LibraryRelativePath, PortableRecord,
    PortableRecordEnvelope, RecordKind, SongRecord,
};
use echo_core::infrastructure::sqlite::SqliteDatabase;
use echo_desktop::platform::dialogs::TestDialogs;
use echo_desktop::platform::trash::DesktopTrash;
use echo_desktop::runtime::app::{assemble, RoutedRuntime};
use echo_desktop::runtime::services::AppServices;
use echo_desktop::runtime::StartupSupervisor;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Fixture {
    _directory: Option<tempfile::TempDir>,
    app: AppServices,
    database: Arc<SqliteDatabase>,
    control: Arc<dyn ControlPlanePort>,
    root: LibraryRootId,
    songs: Vec<SongId>,
}

fn fixture(song_count: usize) -> Fixture {
    let (directory, path) = fixture_location();
    let root_path = path.join("library");
    let marker = path.join(".echo-local-edit-perf-fixture.json");
    let saved = std::fs::read(&marker).ok().map(|raw| {
        let saved: serde_json::Value = serde_json::from_slice(&raw).expect("valid fixture marker");
        assert_eq!(saved["schema"], "echo.local-edit-performance.v1");
        assert_eq!(saved["songCount"], song_count);
        saved
    });
    if saved.is_none() {
        assert!(
            std::fs::read_dir(&path).unwrap().next().is_none(),
            "refuse a nonempty unmarked fixture directory"
        );
        std::fs::create_dir(&root_path).expect("library directory");
    }
    let runtime = assemble(&path.join("echo.sqlite"), &path.join("covers"))
        .expect("production runtime assembly");
    if let Some(saved) = saved {
        let root = saved["root"]
            .as_str()
            .unwrap()
            .parse()
            .expect("saved root id");
        let songs = saved["songs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().parse().expect("saved song id"))
            .collect::<Vec<_>>();
        assert_eq!(songs.len(), song_count);
        let active = runtime
            .database
            .active_root()
            .unwrap()
            .expect("active synthetic root");
        assert_eq!(active.id(), root);
        assert_eq!(active.absolute_path(), root_path);
        return ready_fixture(directory, runtime, root, songs);
    }
    let root = LibraryRootId::new();
    runtime.registry.register(root, &root_path);
    LibraryRepository::upsert(
        runtime.database.as_ref(),
        &LibraryRoot::new(root, root_path, true, true),
    )
    .expect("active fixture root");
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    runtime
        .deps
        .control
        .write_manifest(root, &LibraryManifest::new(library, "performance fixture"))
        .expect("real library manifest");
    runtime
        .deps
        .fs
        .establish_write_capability(root)
        .expect("establish import-capable synthetic root");
    let device = runtime.database.current_device_id();
    let songs: Vec<_> = (0..song_count)
        // Production identities spread records across 256 UUID prefixes.
        // Sequential integer UUIDs would put every record in prefix 00 and
        // turn the containment adapter's sibling check into a false hotspot.
        .map(|_| SongId::new())
        .collect();
    let records = songs
        .iter()
        .enumerate()
        .map(|(index, song)| {
            PortableRecordEnvelope::new(
                PortableRecord::Song(SongRecord {
                    song_uuid: song.as_uuid(),
                    revision: Revision::INITIAL,
                    updated_by_device_id: device,
                    hlc: HybridLogicalClock::new(1_700_000_000, 0),
                    media_path: LibraryRelativePath::new(&format!(
                        "media/artist-{:03}/song-{index:05}.flac",
                        index % 100
                    ))
                    .expect("relative metadata path"),
                    content_hash: format!("{index:064x}"),
                    added_at: 1_700_000_000_000 + index as u64,
                    title: Some(format!("Fixture Song {index:05}")),
                    artist: Some(format!("Fixture Artist {:03}", index % 100)),
                    album: Some(format!("Fixture Album {:03}", index % 200)),
                }),
                None,
            )
            .expect("valid portable song")
        })
        .collect();
    let durable = runtime.deps.control.durability().expect("real durability");
    durable
        .commit(
            root,
            &LibraryBatch::new(uuid::Uuid::new_v4(), library, ChangeOrigin::Scan, records),
        )
        .expect("durably seed metadata originals");
    let replay = durable.recover(root).expect("verify fixture directory");
    runtime
        .database
        .project_library_commits(root, &replay)
        .expect("real SQLite projection");
    let available = songs.clone();
    runtime
        .database
        .with_tx(Box::new(move |tx| {
            for song in available {
                tx.set_song_availability(song, SongAvailability::Available)?;
            }
            Ok(())
        }))
        .expect("seed already scanned availability in one transaction");
    std::fs::write(&marker, serde_json::to_vec(&serde_json::json!({
        "schema": "echo.local-edit-performance.v1", "root": root.to_string(),
        "songCount": song_count, "songs": songs.iter().map(ToString::to_string).collect::<Vec<_>>()
    })).unwrap()).expect("mark completed synthetic fixture");
    ready_fixture(directory, runtime, root, songs)
}

fn fixture_location() -> (Option<tempfile::TempDir>, PathBuf) {
    let Some(path) = std::env::var_os("ECHO_PERF_FIXTURE_PATH") else {
        let directory = tempfile::tempdir().expect("private performance fixture");
        let path = directory.path().to_path_buf();
        return (Some(directory), path);
    };
    let path = PathBuf::from(path);
    assert!(path.is_absolute(), "fixture path must be absolute");
    assert!(
        path.file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with("echo-local-edit-perf-")),
        "fixture path needs the performance prefix"
    );
    let parent = path
        .parent()
        .unwrap()
        .canonicalize()
        .expect("existing temporary parent");
    let temporary = std::env::temp_dir().canonicalize().unwrap();
    let system_tmp = Path::new("/tmp").canonicalize().ok();
    assert!(
        parent == temporary || system_tmp.as_ref() == Some(&parent),
        "fixture must be directly under a temporary directory"
    );
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "fixture must be a real directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&path).expect("create controlled synthetic directory")
        }
        Err(error) => panic!("cannot inspect synthetic fixture: {error}"),
    }
    (None, path)
}

fn ready_fixture(
    directory: Option<tempfile::TempDir>,
    runtime: RoutedRuntime,
    root: LibraryRootId,
    songs: Vec<SongId>,
) -> Fixture {
    let startup = Arc::new(StartupSupervisor::new());
    let supervisor = ScanSupervisor::new();
    startup
        .run_recovery(&runtime.deps, &supervisor, &DesktopTrash::default())
        .expect("real startup recovery");
    startup.on_ready();
    let app = AppServices::with_runtime(
        runtime.deps.clone(),
        supervisor,
        startup,
        Arc::new(TestDialogs::cancelling()),
        runtime.registry,
        runtime.database.clone(),
        Blockers::new(),
    );
    Fixture {
        _directory: directory,
        app,
        database: runtime.database,
        control: runtime.deps.control.clone(),
        root,
        songs,
    }
}

fn setting(name: &str, default: usize) -> usize {
    std::env::var(name).map_or(default, |value| {
        value.parse().unwrap_or_else(|_| panic!("invalid {name}"))
    })
}

fn distribution(samples: &[Duration]) -> serde_json::Value {
    let mut milliseconds: Vec<_> = samples
        .iter()
        .map(|sample| sample.as_secs_f64() * 1_000.0)
        .collect();
    milliseconds.sort_by(f64::total_cmp);
    let percentile = |percentage: usize| {
        let index = (milliseconds.len() * percentage).div_ceil(100) - 1;
        milliseconds[index]
    };
    serde_json::json!({
        "samples": milliseconds.len(),
        "min_ms": milliseconds[0],
        "p50_ms": percentile(50),
        "p95_ms": percentile(95),
        "p99_ms": percentile(99),
        "max_ms": milliseconds[milliseconds.len() - 1],
        "mean_ms": milliseconds.iter().sum::<f64>() / milliseconds.len() as f64,
    })
}

fn argument<'a>(
    args: &'a serde_json::Value,
    name: &'static str,
) -> Result<&'a str, echo_core::error::Error> {
    args[name].as_str().ok_or_else(|| {
        echo_core::error::Error::validation(
            echo_core::error::Subject::Other,
            name,
            "missing performance transport argument",
        )
    })
}

fn parsed_argument<T: std::str::FromStr>(
    args: &serde_json::Value,
    name: &'static str,
) -> Result<T, echo_core::error::Error> {
    argument(args, name)?.parse().map_err(|_| {
        echo_core::error::Error::validation(
            echo_core::error::Subject::Other,
            name,
            "invalid performance transport argument",
        )
    })
}

fn encode(value: impl serde::Serialize) -> Result<serde_json::Value, echo_core::error::Error> {
    serde_json::to_value(value).map_err(|error| {
        echo_core::error::Error::unavailable_with_source(
            "performance transport",
            "encode DTO",
            error,
        )
    })
}

fn sort(args: &serde_json::Value) -> SongSort {
    let token = args["sort"].as_str().unwrap_or("addedAt:desc");
    let (field, direction) = token.split_once(':').unwrap_or(("addedAt", "desc"));
    SongSort {
        field: match field {
            "title" => SongSortField::Title,
            "artist" => SongSortField::Artist,
            "album" => SongSortField::Album,
            "playCount" => SongSortField::PlayCount,
            _ => SongSortField::AddedAt,
        },
        direction: if direction == "asc" {
            SortDirection::Asc
        } else {
            SortDirection::Desc
        },
    }
}

fn dispatch(
    f: &Fixture,
    command: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, echo_core::error::Error> {
    let cursor = args["cursor"]
        .as_str()
        .map(|value| serde_json::from_value::<OpaqueCursor>(serde_json::json!(value)))
        .transpose()
        .map_err(|_| echo_core::error::Error::conflict("invalid performance cursor"))?;
    let limit = args["limit"].as_u64().unwrap_or(100) as usize;
    match command {
        "set_favorite" => encode(f.app.set_favorite(
            parsed_argument(args, "songId")?,
            args["favorite"].as_bool().unwrap_or(false),
        )?),
        "create_playlist" => encode(
            f.app
                .create_playlist(parsed_argument(args, "root")?, argument(args, "name")?)?,
        ),
        "library_counts" => encode(f.app.library_counts()?),
        "get_bootstrap_state" => encode(f.app.bootstrap()?),
        "library_status" => encode(f.app.library_status()?),
        "all_songs" => encode(f.app.all_songs(sort(args), cursor.as_ref(), limit)?),
        "favorites" => encode(f.app.favorites(sort(args), cursor.as_ref(), limit)?),
        "playlists" => encode(f.app.playlists()?),
        "playlist_members" => encode(
            f.app
                .playlist_members(parsed_argument(args, "playlistId")?)?,
        ),
        "song_detail" => encode(f.app.song_detail(parsed_argument(args, "songId")?)?),
        "song_cover_keys" => {
            let ids = args["songIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str()?.parse().ok())
                .collect::<Vec<_>>();
            encode(f.app.cover_keys(&ids)?)
        }
        _ => Err(echo_core::error::Error::unavailable(
            "performance transport",
            format!("unsupported command {command}"),
        )),
    }
}

fn request(stream: &mut TcpStream) -> Result<(String, String, serde_json::Value), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    let (header_end, content_length) = loop {
        let mut block = [0; 4096];
        let read = stream.read(&mut block).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("request ended before its headers".into());
        }
        bytes.extend_from_slice(&block[..read]);
        if bytes.len() > 1_048_576 {
            return Err("request exceeds fixture transport limit".into());
        }
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header =
                std::str::from_utf8(&bytes[..position]).map_err(|error| error.to_string())?;
            let length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            break (position + 4, length);
        }
    };
    if content_length > 1_048_576 {
        return Err("request body exceeds fixture transport limit".into());
    }
    while bytes.len() - header_end < content_length {
        let mut block = [0; 4096];
        let read = stream.read(&mut block).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("request ended before its body".into());
        }
        bytes.extend_from_slice(&block[..read]);
    }
    let header = std::str::from_utf8(&bytes[..header_end]).map_err(|error| error.to_string())?;
    let mut first = header.lines().next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default().to_owned();
    let path = first.next().unwrap_or_default().to_owned();
    let body = if content_length == 0 {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + content_length])
            .map_err(|error| error.to_string())?
    };
    Ok((method, path, body))
}

fn response(
    stream: &mut TcpStream,
    status: &str,
    body: &serde_json::Value,
    elapsed: Option<Duration>,
) {
    let bytes = serde_json::to_vec(body).expect("response JSON");
    let timing = elapsed.map_or_else(String::new, |duration| {
        format!(
            "X-Echo-Service-Ms: {:.6}\r\n",
            duration.as_secs_f64() * 1_000.0
        )
    });
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Expose-Headers: X-Echo-Service-Ms\r\nCache-Control: no-store\r\nConnection: close\r\n{timing}\r\n", bytes.len()).expect("HTTP response headers");
    stream.write_all(&bytes).expect("HTTP response body");
}

fn serve(f: &Fixture, history: usize) {
    let port = setting("ECHO_PERF_PORT", 0);
    let listener = TcpListener::bind(("127.0.0.1", u16::try_from(port).expect("valid port")))
        .expect("loopback fixture listener");
    let fixture = serde_json::json!({
        "url": format!("http://{}", listener.local_addr().unwrap()),
        "root": f.root.to_string(),
        "songs": f.songs.iter().rev().take(100).map(ToString::to_string).collect::<Vec<_>>(),
        "rootId": f.root.to_string(),
        "songId": f.songs.last().unwrap().to_string(),
        "songCount": f.songs.len(),
        "historyOperations": history,
    });
    println!("ECHO_PERF_READY {fixture}");
    std::thread::scope(|scope| {
        for stream in listener.incoming() {
            let mut stream = stream.expect("loopback fixture connection");
            let Ok((method, path, body)) = request(&mut stream) else {
                response(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error":"invalid fixture request"}),
                    None,
                );
                continue;
            };
            if method == "OPTIONS" {
                response(&mut stream, "200 OK", &serde_json::Value::Null, None);
            } else if path == "/fixture" {
                response(&mut stream, "200 OK", &fixture, None);
            } else if path == "/shutdown" {
                response(
                    &mut stream,
                    "200 OK",
                    &serde_json::json!({"stopped":true}),
                    None,
                );
                break;
            } else if path == "/ipc" && method == "POST" {
                scope.spawn(move || {
                    let command = body["command"].as_str().unwrap_or_default();
                    let start = Instant::now();
                    let result = dispatch(f, command, &body["args"]);
                    let elapsed = start.elapsed();
                    match result {
                        Ok(value) => response(&mut stream, "200 OK", &value, Some(elapsed)),
                        Err(error) => response(
                            &mut stream,
                            "500 Internal Server Error",
                            &serde_json::to_value(echo_desktop::ipc::IpcErrorDto::from_mutation(
                                &error,
                            ))
                            .unwrap(),
                            Some(elapsed),
                        ),
                    }
                });
            } else {
                response(&mut stream, "404 Not Found", &serde_json::Value::Null, None);
            }
        }
    });
}

#[test]
#[ignore = "real filesystem latency benchmark; run explicitly with --ignored --nocapture"]
fn completed_local_edit_latency() {
    let song_count = setting("ECHO_PERF_SONGS", 3_000);
    let history = setting("ECHO_PERF_HISTORY", 64);
    let samples = setting("ECHO_PERF_SAMPLES", 100);
    assert!(song_count > 0 && samples > 0);
    let preparation = Instant::now();
    let f = fixture(song_count);
    let run_id = uuid::Uuid::new_v4().simple().to_string();
    let run_id = &run_id[..8];
    let initial_operations = f
        .control
        .durability()
        .unwrap()
        .recover(f.root)
        .unwrap()
        .head()
        .map_or(0, |(sequence, _)| sequence);
    let current_values = SongRepository::all_in_root(f.database.as_ref(), f.root)
        .unwrap()
        .into_iter()
        .map(|song| (song.id(), song.favorite()))
        .collect::<std::collections::HashMap<_, _>>();
    let mut values = f
        .songs
        .iter()
        .map(|song| current_values[song])
        .collect::<Vec<_>>();
    for index in 0..history {
        if index % 2 == 0 {
            let song_index = (index * 37) % song_count;
            values[song_index] = !values[song_index];
            f.app
                .set_favorite(f.songs[song_index], values[song_index])
                .expect("completed history favorite");
        } else {
            f.app
                .create_playlist(f.root, &format!("{run_id}-history-{index:05}"))
                .expect("completed history playlist");
        }
    }
    println!(
        "ECHO_LOCAL_EDIT_PERF fixture songs={song_count} initial_operations={initial_operations} history={history} preparation_ms={:.3}",
        preparation.elapsed().as_secs_f64() * 1_000.0
    );
    if std::env::var("ECHO_PERF_SERVE").is_ok_and(|value| value == "1") {
        serve(&f, history);
        return;
    }
    let mut favorites = Vec::with_capacity(samples);
    for index in 0..samples {
        let song_index = (index * 37) % song_count;
        values[song_index] = !values[song_index];
        let start = Instant::now();
        let song = f
            .app
            .set_favorite(f.songs[song_index], values[song_index])
            .expect("completed favorite command");
        favorites.push(start.elapsed());
        assert_eq!(song.favorite, values[song_index]);
    }
    let mut playlists = Vec::with_capacity(samples);
    let mut last_playlist = None;
    for index in 0..samples {
        let start = Instant::now();
        let id = f
            .app
            .create_playlist(f.root, &format!("{run_id}-measured-{index:05}"))
            .expect("completed playlist command");
        playlists.push(start.elapsed());
        last_playlist = Some(id.parse::<PlaylistId>().expect("playlist identity"));
    }
    // The playlist is a prerequisite fixture for the membership benchmark. Let
    // its independent read-model projection settle before measuring members.
    f.database
        .flush_library_projection(f.root, Duration::from_secs(30))
        .expect("background playlist projection catches up");
    let member_playlist = last_playlist.expect("playlist for membership measurements");
    let member_song = *f.songs.last().expect("at least one fixture song");
    let mut additions = Vec::with_capacity(samples);
    let mut removals = Vec::with_capacity(samples);
    for _ in 0..samples {
        // Each sample represents a human-paced command whose preceding
        // projection has had time to finish. A separate stress run can measure
        // immediately queued edits, which intentionally pay the prior-log
        // catch-up barrier in the next writer.
        f.database
            .flush_library_projection(f.root, Duration::from_secs(30))
            .expect("projection settles before add sample");
        let start = Instant::now();
        f.app
            .add_to_playlists(member_song, &[member_playlist])
            .expect("completed add-to-playlist command");
        additions.push(start.elapsed());

        f.database
            .flush_library_projection(f.root, Duration::from_secs(30))
            .expect("projection settles before remove sample");
        let start = Instant::now();
        f.app
            .remove_playlist_song(member_playlist, member_song)
            .expect("completed remove-from-playlist command");
        removals.push(start.elapsed());
    }
    f.database
        .flush_library_projection(f.root, Duration::from_secs(30))
        .expect("membership projection catches up before import samples");
    let import_inputs = tempfile::tempdir().expect("private import benchmark inputs");
    let audio_fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio");
    let mut imports = Vec::new();
    for extension in ["mp3", "flac", "m4a", "ogg", "opus", "wav"] {
        let source = audio_fixtures.join(format!("tone-short.{extension}"));
        let input = import_inputs
            .path()
            .join(format!("import-{run_id}.{extension}"));
        std::fs::copy(&source, &input).expect("copy real audio fixture to private input");
        let bytes = std::fs::metadata(&input)
            .expect("audio input metadata")
            .len();
        let started = Instant::now();
        let outcome = f
            .app
            .import_single_path(&input, input.file_name().unwrap().to_str().unwrap())
            .expect("complete audio import command");
        let elapsed = started.elapsed();
        assert!(
            matches!(
                &outcome,
                echo_desktop::ipc::dto::ImportResultDto::Imported { .. }
            ),
            "audio fixture should import successfully: {outcome:?}"
        );
        if let echo_desktop::ipc::dto::ImportResultDto::Imported { song_id, .. } = &outcome {
            let imported_song = song_id.parse::<SongId>().expect("imported song identity");
            let imported_row = SongRepository::by_id(f.database.as_ref(), imported_song)
                .expect("read imported song without rescanning")
                .expect("successful import should already be visible in SQLite");
            assert_eq!(imported_row.root(), f.root);
            assert_eq!(imported_row.availability(), SongAvailability::Available);
            let catalog = f
                .app
                .all_songs(
                    SongSort {
                        field: SongSortField::AddedAt,
                        direction: SortDirection::Desc,
                    },
                    None,
                    100,
                )
                .expect("read catalog without rescanning");
            assert_eq!(
                catalog.total_count,
                f.songs.len() + imports.len() + 1,
                "successful import should contribute to the catalog count; row={imported_row:?}, active={:?}",
                f.app.library_status().expect("active library status")
            );
        }
        imports.push(serde_json::json!({
            "extension": extension,
            "size_bytes": bytes,
            "elapsed_ms": elapsed.as_secs_f64() * 1_000.0,
        }));
    }
    // Interactive commands acknowledge the durable directory journal; wait
    // outside the timed interval before asserting eventual SQLite projection.
    f.database
        .flush_library_projection(f.root, Duration::from_secs(30))
        .expect("background projection catches up");
    assert!(
        PlaylistRepository::members(f.database.as_ref(), member_playlist)
            .expect("read projected playlist members")
            .iter()
            .all(|member| member.song() != member_song),
        "the final removal must reach the SQLite read model"
    );
    // Check both durable originals and the SQL view after timing. A successful
    // command must have completed both stores, rather than merely queued work.
    let song_index = ((samples - 1) * 37) % song_count;
    let song_id = f.songs[song_index];
    let favorite = f
        .control
        .read_record(f.root, RecordKind::Favorite, &song_id.to_string())
        .expect("read durable favorite")
        .expect("favorite original exists");
    assert!(
        matches!(favorite, PortableRecord::Favorite(record) if record.is_favorite == values[song_index])
    );
    assert_eq!(
        SongRepository::by_id(f.database.as_ref(), song_id)
            .unwrap()
            .unwrap()
            .favorite(),
        values[song_index]
    );
    let playlist = last_playlist.expect("at least one playlist sample");
    assert!(f
        .control
        .read_record(f.root, RecordKind::Playlist, &playlist.to_string())
        .unwrap()
        .is_some());
    assert!(PlaylistRepository::by_id(f.database.as_ref(), playlist)
        .unwrap()
        .is_some());
    let favorite_distribution = distribution(&favorites);
    let playlist_distribution = distribution(&playlists);
    let result = serde_json::json!({
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "songs": song_count,
        "initial_operations": initial_operations,
        "history_operations": history,
        "favorite": favorite_distribution,
        "create_playlist": playlist_distribution,
        "add_to_playlist": distribution(&additions),
        "remove_from_playlist": distribution(&removals),
        "import_small_audio_fixtures": imports,
    });
    println!("ECHO_LOCAL_EDIT_PERF {result}");
    if let Ok(limit) = std::env::var("ECHO_PERF_MAX_P95_MS") {
        let limit: f64 = limit.parse().expect("numeric P95 limit");
        for command in [
            "favorite",
            "create_playlist",
            "add_to_playlist",
            "remove_from_playlist",
        ] {
            let p95 = result[command]["p95_ms"].as_f64().unwrap();
            assert!(p95 < limit, "{command} P95 {p95:.3}ms exceeds {limit:.3}ms");
        }
    }
}
