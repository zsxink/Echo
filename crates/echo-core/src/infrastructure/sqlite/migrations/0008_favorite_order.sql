-- Keep the moment a song was last added to "喜欢的音乐" independently from
-- metadata edits and playback updates, so the view can be ordered by the
-- user's actual favorite action.
ALTER TABLE songs ADD COLUMN favorited_at INTEGER;

-- Existing favorites predate this field. Their original library insertion time
-- supplies a stable chronological fallback; new favorite actions replace it.
UPDATE songs SET favorited_at = added_at WHERE is_favorite = 1;

CREATE INDEX songs_favorites_by_time
    ON songs(library_root_uuid, is_favorite, favorited_at DESC, uuid DESC);
