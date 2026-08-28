-- Echo 0.1.0 initial local-library schema.
--
-- This migration is append-only: once released its contents (and therefore
-- checksum) must never be changed. Future schema changes receive a new file.
-- Deliberately absent: tombstones, sync_state and sync_outbox. Sync has no
-- approved protocol in 0.1.0 and must arrive in a later ordered migration.

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    checksum TEXT NOT NULL,
    applied_at INTEGER NOT NULL
);

CREATE TABLE library_roots (
    uuid TEXT PRIMARY KEY NOT NULL,
    absolute_path TEXT NOT NULL,
    normalized_path_key TEXT NOT NULL UNIQUE,
    is_active INTEGER NOT NULL DEFAULT 0 CHECK (is_active IN (0, 1)),
    write_capable INTEGER NOT NULL DEFAULT 0 CHECK (write_capable IN (0, 1)),
    availability TEXT NOT NULL DEFAULT 'available' CHECK (availability IN ('available', 'unavailable')),
    scan_generation INTEGER NOT NULL DEFAULT 0,
    last_scanned_at INTEGER,
    staging_dir_name TEXT,
    staging_marker_version INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX library_roots_one_active ON library_roots(is_active) WHERE is_active = 1;

CREATE TABLE songs (
    uuid TEXT PRIMARY KEY NOT NULL,
    library_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    normalized_relative_path TEXT NOT NULL,
    blake3_hash TEXT,
    file_size INTEGER,
    file_mtime_ns INTEGER,
    format TEXT,
    title TEXT,
    artist TEXT,
    album TEXT,
    title_sort TEXT NOT NULL DEFAULT '',
    artist_sort TEXT NOT NULL DEFAULT '',
    album_sort TEXT NOT NULL DEFAULT '',
    duration_ms INTEGER,
    is_favorite INTEGER NOT NULL DEFAULT 0 CHECK (is_favorite IN (0, 1)),
    play_count INTEGER NOT NULL DEFAULT 0 CHECK (play_count >= 0),
    added_at INTEGER NOT NULL,
    availability TEXT NOT NULL DEFAULT 'available' CHECK (availability IN ('available', 'missing', 'pending_delete')),
    revision INTEGER NOT NULL DEFAULT 0,
    parse_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (library_root_uuid, normalized_relative_path)
);
CREATE UNIQUE INDEX songs_available_hash_unique
    ON songs(library_root_uuid, blake3_hash)
    WHERE blake3_hash IS NOT NULL AND availability = 'available';
CREATE INDEX songs_by_added ON songs(library_root_uuid, added_at, uuid);
CREATE INDEX songs_by_title ON songs(library_root_uuid, title_sort, artist_sort, uuid);
CREATE INDEX songs_by_artist ON songs(library_root_uuid, artist_sort, title_sort, uuid);
CREATE INDEX songs_by_plays ON songs(library_root_uuid, play_count, title_sort, uuid);
CREATE INDEX songs_favorites ON songs(library_root_uuid, is_favorite, title_sort, artist_sort, uuid);

-- One row per (song, source): a song keeps every lyrics candidate it has
-- (override / embedded / sidecar) so selection can fall back from a corrupt
-- higher-priority source to a valid lower-priority one without re-reading the
-- file. Selection order is the domain's `Override > Embedded > Sidecar`.
CREATE TABLE song_lyrics (
    song_uuid TEXT NOT NULL REFERENCES songs(uuid) ON DELETE CASCADE,
    source TEXT NOT NULL CHECK (source IN ('override', 'embedded', 'sidecar')),
    text_kind TEXT NOT NULL CHECK (text_kind IN ('timed', 'plain', 'empty')),
    raw_text TEXT,
    timed_lines_json TEXT,
    source_mtime_ns INTEGER,
    parse_error TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (song_uuid, source)
);

CREATE TABLE song_overrides (
    song_uuid TEXT PRIMARY KEY NOT NULL REFERENCES songs(uuid) ON DELETE CASCADE,
    title TEXT,
    artist TEXT,
    album TEXT,
    lyrics_text TEXT,
    version INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);

CREATE TABLE cover_assets (
    content_hash TEXT PRIMARY KEY NOT NULL,
    mime_type TEXT NOT NULL,
    width INTEGER,
    height INTEGER,
    asset_key TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE playlists (
    uuid TEXT PRIMARY KEY NOT NULL,
    library_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    normalized_name_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (library_root_uuid, normalized_name_key)
);
CREATE INDEX playlists_by_name ON playlists(library_root_uuid, normalized_name_key);

CREATE TABLE playlist_songs (
    playlist_uuid TEXT NOT NULL REFERENCES playlists(uuid) ON DELETE CASCADE,
    song_uuid TEXT NOT NULL REFERENCES songs(uuid) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    added_at INTEGER NOT NULL,
    PRIMARY KEY (playlist_uuid, song_uuid)
);
CREATE UNIQUE INDEX playlist_songs_position ON playlist_songs(playlist_uuid, position);

-- A playlist may only reference songs that live in the same library root:
-- cross-root membership would leak songs from a deactivated root into another
-- root's playlist and break active-root isolation.
CREATE TRIGGER playlist_songs_same_root_on_insert
BEFORE INSERT ON playlist_songs
WHEN (SELECT library_root_uuid FROM playlists WHERE uuid = NEW.playlist_uuid)
     IS NOT (SELECT library_root_uuid FROM songs WHERE uuid = NEW.song_uuid)
BEGIN
    SELECT RAISE(ABORT, 'playlist and song must belong to the same library root');
END;
CREATE TRIGGER playlist_songs_same_root_on_song_reparent
BEFORE UPDATE OF library_root_uuid ON songs
WHEN EXISTS (
    SELECT 1 FROM playlist_songs ps
    JOIN playlists p ON p.uuid = ps.playlist_uuid
    WHERE ps.song_uuid = NEW.uuid AND p.library_root_uuid <> NEW.library_root_uuid
)
BEGIN
    SELECT RAISE(ABORT, 'song with cross-root playlist memberships cannot change library root');
END;

CREATE TABLE operation_journal (
    operation_uuid TEXT PRIMARY KEY NOT NULL,
    library_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE RESTRICT,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    payload_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL DEFAULT '{}',
    reserved_song_uuid TEXT,
    undo_deadline INTEGER,
    error_code TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE operation_items (
    operation_uuid TEXT NOT NULL REFERENCES operation_journal(operation_uuid) ON DELETE CASCADE,
    item_key TEXT NOT NULL,
    library_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('audio', 'lyrics')),
    state TEXT NOT NULL,
    song_uuid TEXT REFERENCES songs(uuid) ON DELETE SET NULL,
    source_locator TEXT,
    staging_relative_path TEXT,
    target_relative_path TEXT NOT NULL,
    normalized_target_path TEXT NOT NULL,
    expected_hash TEXT NOT NULL,
    attempt INTEGER NOT NULL DEFAULT 0,
    error_code TEXT,
    claim_active INTEGER NOT NULL DEFAULT 1 CHECK (claim_active IN (0, 1)),
    PRIMARY KEY (operation_uuid, item_key)
);
CREATE UNIQUE INDEX operation_items_active_target_claim
    ON operation_items(library_root_uuid, normalized_target_path)
    WHERE claim_active = 1;

CREATE TABLE scan_runs (
    uuid TEXT PRIMARY KEY NOT NULL,
    library_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE CASCADE,
    generation INTEGER NOT NULL,
    state TEXT NOT NULL,
    discovered_count INTEGER NOT NULL DEFAULT 0,
    processed_count INTEGER NOT NULL DEFAULT 0,
    created_count INTEGER NOT NULL DEFAULT 0,
    updated_count INTEGER NOT NULL DEFAULT 0,
    missing_count INTEGER NOT NULL DEFAULT 0,
    skipped_count INTEGER NOT NULL DEFAULT 0,
    failed_count INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE UNIQUE INDEX scan_runs_generation ON scan_runs(library_root_uuid, generation);

CREATE TABLE scan_issues (
    id INTEGER PRIMARY KEY,
    scan_run_uuid TEXT NOT NULL REFERENCES scan_runs(uuid) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    code TEXT NOT NULL,
    detail TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX scan_issues_by_run ON scan_issues(scan_run_uuid, id);

CREATE TABLE recorded_play_sessions (
    playback_session_uuid TEXT PRIMARY KEY NOT NULL,
    song_uuid TEXT NOT NULL REFERENCES songs(uuid) ON DELETE CASCADE,
    recorded_at INTEGER NOT NULL
);

CREATE VIRTUAL TABLE song_search USING fts5(
    title,
    artist,
    album,
    song_uuid UNINDEXED,
    tokenize = 'trigram case_sensitive 0 remove_diacritics 0'
);
