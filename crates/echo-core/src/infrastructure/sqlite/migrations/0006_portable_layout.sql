-- Echo 0.1.0 portable-library-layout schema (方向 B: 可携带资料库的本地持久化)。
--
-- This migration lands after 0005 (sync foundation). It adds the *local*
-- persistence halves of the portable library layout:
--
--   1. `device_id` — the single, stable identity of this machine. It is
--      generated once, stored in the application data directory, and is
--      **never** regenerated. It is NOT part of the `echo/` control surface
--      (which is portable and must not carry machine-local metadata).
--   2. `hlc_wall_secs` / `hlc_counter` — the Hybrid Logical Clock timestamp
--      attached to every syncable object so two devices can order mutations
--      deterministically (LWW in 二期) without relying on wall-clock alone.
--   3. `playlist_songs.member_uuid` — a stable, independent UUID per playlist
--      member. Previously membership was keyed only by `(playlist_uuid,
--      song_uuid)`; a stable member UUID allows member-level add/remove/reorder
--      to propagate across devices without operations collapsing onto the whole
--      playlist.
--
-- This migration only adds columns/tables for the *shape*: 0.1.0 application
-- code populates them (see portable broad statement paths). It does not shape
-- the remote protocol — that is 二期 work.
--
-- Migration 0005 created tombstones as a shape. This migration does not alter
-- released migrations (0001..0005 are immutable).

-- Stable device identity (one row, single value). 1 = initialized.
CREATE TABLE IF NOT EXISTS device_state (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

-- Per-syncable-object HLC (wall seconds + counter).
-- Added to the objects that will carry cross-device ordering.
ALTER TABLE songs ADD COLUMN hlc_wall_secs INTEGER NOT NULL DEFAULT 0;
ALTER TABLE songs ADD COLUMN hlc_counter INTEGER NOT NULL DEFAULT 0;

ALTER TABLE song_overrides ADD COLUMN hlc_wall_secs INTEGER NOT NULL DEFAULT 0;
ALTER TABLE song_overrides ADD COLUMN hlc_counter INTEGER NOT NULL DEFAULT 0;

ALTER TABLE playlists ADD COLUMN hlc_wall_secs INTEGER NOT NULL DEFAULT 0;
ALTER TABLE playlists ADD COLUMN hlc_counter INTEGER NOT NULL DEFAULT 0;

-- Playlist members: stable per-member UUID (populated by the application on
-- add; back-filled below for rows that predate this migration).
ALTER TABLE playlist_songs ADD COLUMN member_uuid TEXT;
-- A member may be removed (a tombstone) without dropping the row; the member
-- tombstone is written to `tombstones` by the application. This column records
-- the fact at the member level for local ordering.
ALTER TABLE playlist_songs ADD COLUMN member_revision INTEGER NOT NULL DEFAULT 0;

-- Backfill a stable member UUID for pre-existing rows. These rows were created
-- before the portable layout existed, so they have no cross-device identity
-- yet: giving each a fresh UUID now lets a future sync treat them as normal
-- members instead of crashing on NULL.
UPDATE playlist_songs SET member_uuid = lower(hex(randomblob(16))) WHERE member_uuid IS NULL;

-- Mark the device identity as initialized (idempotent; the application reads
-- the value and generates it first if absent).
INSERT INTO device_state (key, value)
SELECT 'device_id', 'uninitialized'
WHERE NOT EXISTS (SELECT 1 FROM device_state WHERE key = 'device_id');