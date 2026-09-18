//! Identity resolution for scanned files (task 4.8 / design §6.7).
//!
//! Given the scan-start snapshot of a root's songs and one freshly parsed
//! file, [`RelinkPlanner`] decides — as a pure, testable computation —
//! whether the file
//!
//! - keeps the record already stored at its path (refresh in place),
//! - *re-links* an existing identity whose path vanished (same BLAKE3 hash,
//!   or a conservative unique music-key match),
//! - is duplicate content of an available record (one record per hash;
//!   duplicates are reported, never merged into new UUIDs), or
//! - mints a new UUID.
//!
//! Ambiguity always loses: any non-unique match leaves the old record
//! missing and creates a new UUID, so no wrong merge can happen. Every
//! decision is folded back into the snapshot so later decisions in the same
//! scan see earlier ones.

use std::collections::HashSet;

use crate::domain::entities::{Song, SongAvailability};
use crate::domain::ids::{LibraryRootId, RelativeMediaPath, Revision, SongId};
use crate::domain::media::ParsedMetadata;
use crate::domain::text::normalized_key;

/// One fully parsed file entering identity resolution. The planner is
/// root-scoped by construction: the scan pipeline only feeds it files of the
/// root whose snapshot it was built from.
#[derive(Clone, Debug)]
pub struct ParsedFile {
    pub path: RelativeMediaPath,
    pub hash: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub meta: ParsedMetadata,
}

/// What should happen to one parsed file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resolution {
    /// A record already lives at this path: refresh it in place.
    Keep { song: SongId },
    /// The file re-identifies an existing record whose old path is gone:
    /// move the identity to the new path.
    Relink { song: SongId },
    /// No existing record matches: create a fresh UUID.
    Create,
    /// The hash already belongs to an *available* record at another path —
    /// duplicate content. One record per hash; the caller reports the
    /// duplicate path as an issue.
    Duplicate { primary: SongId },
}

/// Duration tolerance for the weak music-key re-link (design §6.7: ≤ 2 s).
const MUSIC_KEY_DURATION_TOLERANCE_MS: u64 = 2_000;

/// The mutable decision state over one scan's song snapshot.
#[derive(Debug, Default)]
pub struct RelinkPlanner {
    songs: Vec<Song>,
    /// Records whose identity a file already claimed during this scan — a
    /// second file matching the same missing record is ambiguous.
    claimed: HashSet<SongId>,
}

impl RelinkPlanner {
    /// Build the planner from the scan-start snapshot.
    #[must_use]
    pub fn new(songs: Vec<Song>) -> Self {
        Self {
            songs,
            claimed: HashSet::new(),
        }
    }

    /// Resolve `file` and fold the decision into the snapshot so later
    /// decisions see earlier ones.
    pub fn resolve(&mut self, file: &ParsedFile) -> Resolution {
        // 1. Path identity first: the same path refreshes its record.
        if let Some(song) = self.songs.iter_mut().find(|song| song.path() == &file.path) {
            // A re-parsed file physically present at its record's path proves
            // the file came back, so an externally-missing record is restored
            // here too (task 5.9 mirror image of the fast-skip `restore`).
            if song.availability() == SongAvailability::Missing {
                song.restore_available();
            }
            let id = song.id();
            Self::fold_facts(song, file);
            self.claimed.insert(id);
            return Resolution::Keep { song: id };
        }

        // 2. Hash match: content-identical re-link or duplicate detection.
        if let Some(primary_id) = self
            .primary_hash_holder(&file.hash)
            .map(super::super::domain::entities::Song::id)
        {
            let primary_path = self
                .songs
                .iter()
                .find(|song| song.id() == primary_id)
                .map(|song| song.path().identity_key().to_owned());
            if let Some(primary_path) = primary_path {
                if file.path.identity_key() < primary_path.as_str() {
                    self.relink(primary_id, file);
                    return Resolution::Relink { song: primary_id };
                }
                self.claimed.insert(primary_id);
                return Resolution::Duplicate {
                    primary: primary_id,
                };
            }
        }
        if let Some(missing) = self
            .songs
            .iter()
            .filter(|song| {
                song.blake3_hash() == Some(file.hash.as_str())
                    && song.availability() == SongAvailability::Missing
            })
            .min_by_key(|song| song.path().identity_key())
            .map(Song::id)
        {
            self.relink(missing, file);
            return Resolution::Relink { song: missing };
        }

        // 3. Weak music-key re-link: only a *unique* unclaimed missing
        // candidate with a matching normalized (artist, album, title) and a
        // duration within the tolerance.
        if let Some(song_id) = self.unique_music_key_match(file) {
            self.relink(song_id, file);
            return Resolution::Relink { song: song_id };
        }

        // 4. Nothing matches: a fresh identity (the caller registers it via
        // [`Self::register_created`] so later duplicates resolve against it).
        Resolution::Create
    }

    /// Register a freshly created record in the snapshot (after the caller
    /// minted the UUID), so duplicate detection sees it.
    pub fn register_created(&mut self, song: Song) {
        self.songs.push(song);
    }

    /// The snapshot entity of one record (for the caller to persist).
    #[must_use]
    pub fn song(&self, id: SongId) -> Option<Song> {
        self.songs.iter().find(|song| song.id() == id).cloned()
    }

    /// Restore a fast-skipped record whose file came back (the snapshot must
    /// agree, or the final missing pass could contradict it).
    pub fn restore_available(&mut self, id: SongId) {
        if let Some(song) = self.songs.iter_mut().find(|song| song.id() == id) {
            song.restore_available();
        }
    }

    /// The current snapshot (initial songs + folded decisions).
    #[must_use]
    pub fn snapshot(&self) -> &[Song] {
        &self.songs
    }

    /// The available holder of `hash` with the smallest canonical path key.
    fn primary_hash_holder(&self, hash: &str) -> Option<&Song> {
        self.songs
            .iter()
            .filter(|song| {
                song.blake3_hash() == Some(hash)
                    && song.availability() == SongAvailability::Available
            })
            .min_by_key(|song| song.path().identity_key())
    }

    fn unique_music_key_match(&self, file: &ParsedFile) -> Option<SongId> {
        let key = MusicKey::of(&file.meta)?;
        let mut matches = self.songs.iter().filter(|song| {
            song.availability() == SongAvailability::Missing
                && !self.claimed.contains(&song.id())
                && MusicKey::of_song(song).is_some_and(|candidate| candidate.matches(&key))
        });
        let first = matches.next()?;
        if matches.next().is_some() {
            return None; // ambiguous: more than one missing candidate
        }
        Some(first.id())
    }

    fn relink(&mut self, id: SongId, file: &ParsedFile) {
        self.claimed.insert(id);
        if let Some(song) = self.songs.iter_mut().find(|song| song.id() == id) {
            song.relink(file.path.clone());
            song.restore_available();
            Self::fold_facts(song, file);
        }
    }

    /// Fold parsed metadata + scan facts into a snapshot entity.
    fn fold_facts(song: &mut Song, file: &ParsedFile) {
        song.apply_metadata(
            file.meta.title.clone(),
            file.meta.artist.clone(),
            file.meta.album.clone(),
            file.meta.duration,
        );
        song.apply_scan_facts(
            file.hash.clone(),
            file.size,
            file.mtime_ns,
            file.meta.format,
        );
    }
}

/// The normalized music key for conservative weak re-linking.
#[derive(Clone, Debug, Eq, PartialEq)]
struct MusicKey {
    artist: String,
    album: String,
    title: String,
    duration_ms: i64,
}

impl MusicKey {
    /// From parsed metadata. Requires artist, album AND title to be present
    /// *and non-empty* — an empty key would match every untagged file and is
    /// maximally ambiguous, so it never re-links.
    fn of(meta: &ParsedMetadata) -> Option<Self> {
        let duration_ms = i64::try_from(meta.duration?.as_millis()).ok()?;
        let artist = normalized_key(meta.artist.as_deref()?);
        let album = normalized_key(meta.album.as_deref()?);
        let title = normalized_key(meta.title.as_deref()?);
        Self::checked(artist, album, title, duration_ms)
    }

    fn of_song(song: &Song) -> Option<Self> {
        let duration_ms = i64::try_from(song.duration()?.as_millis()).ok()?;
        let artist = normalized_key(song.artist()?);
        let album = normalized_key(song.album()?);
        let title = normalized_key(song.title()?);
        Self::checked(artist, album, title, duration_ms)
    }

    fn checked(artist: String, album: String, title: String, duration_ms: i64) -> Option<Self> {
        if artist.is_empty() || album.is_empty() || title.is_empty() {
            return None;
        }
        Some(Self {
            artist,
            album,
            title,
            duration_ms,
        })
    }

    fn matches(&self, other: &Self) -> bool {
        self.artist == other.artist
            && self.album == other.album
            && self.title == other.title
            && self.duration_ms.abs_diff(other.duration_ms) <= MUSIC_KEY_DURATION_TOLERANCE_MS
    }
}

/// Build a song entity with parsed facts under a **fixed** identity. The
/// watch reconciliation (journal-reserved records) and the import commit
/// (reserved `SongId`) share this: the parsed file describes facts, the
/// caller owns identity.
#[must_use]
pub(crate) fn song_from_parsed(
    id: SongId,
    root: LibraryRootId,
    file: &ParsedFile,
    added_at: u64,
) -> Song {
    // A revision is an optimistic-concurrency tag, not an event timestamp.
    let mut entity = Song::with_added_at(id, root, file.path.clone(), Revision::INITIAL, added_at);
    entity.apply_metadata(
        file.meta.title.clone(),
        file.meta.artist.clone(),
        file.meta.album.clone(),
        file.meta.duration,
    );
    entity.apply_scan_facts(
        file.hash.clone(),
        file.size,
        file.mtime_ns,
        file.meta.format,
    );
    entity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ids::{LibraryRootId, Revision};
    use crate::domain::media::AudioFormat;
    use std::time::Duration;

    fn song(id: SongId, path: &str, title: &str, duration_secs: u64) -> Song {
        let mut song = Song::new(
            id,
            LibraryRootId::new(),
            RelativeMediaPath::new(path).unwrap(),
            Revision::INITIAL,
        );
        song.apply_metadata(
            Some(title.to_owned()),
            Some("歌手".to_owned()),
            Some("专辑".to_owned()),
            Some(Duration::from_secs(duration_secs)),
        );
        song.apply_scan_facts(format!("hash-{path}"), 100, 1, AudioFormat::Flac);
        song
    }

    fn missing_song(id: SongId, path: &str, title: &str, duration_secs: u64) -> Song {
        let mut song = song(id, path, title, duration_secs);
        song.mark_missing();
        song
    }

    fn file(path: &str, hash: &str, title: &str, duration_secs: u64) -> ParsedFile {
        ParsedFile {
            path: RelativeMediaPath::new(path).unwrap(),
            hash: hash.to_owned(),
            size: 100,
            mtime_ns: 2,
            meta: ParsedMetadata {
                title: Some(title.to_owned()),
                artist: Some("歌手".to_owned()),
                album: Some("专辑".to_owned()),
                duration: Some(Duration::from_secs(duration_secs)),
                format: AudioFormat::Flac,
                ..ParsedMetadata::default()
            },
        }
    }

    #[test]
    fn relink_plans_cover_path_hash_and_music_key_rules() {
        // Same path → Keep.
        let song_a = SongId::new();
        let mut planner = RelinkPlanner::new(vec![song(song_a, "a.flac", "A", 100)]);
        let resolution = planner.resolve(&file("a.flac", "hash-a.flac", "A", 100));
        assert_eq!(resolution, Resolution::Keep { song: song_a });

        // Old path missing, new path with the same hash → Relink (UUID kept).
        let song_b = SongId::new();
        let mut planner = RelinkPlanner::new(vec![song(song_b, "old.flac", "B", 100)]);
        let resolution = planner.resolve(&file("new/moved.flac", "hash-old.flac", "B", 100));
        assert_eq!(resolution, Resolution::Relink { song: song_b });
        assert_eq!(
            planner.song(song_b).unwrap().path().display(),
            "new/moved.flac",
            "rename/move keeps the UUID"
        );
        assert_eq!(
            planner.song(song_b).unwrap().availability(),
            SongAvailability::Available
        );

        // Hash present at an available path, new path sorts *after* →
        // duplicate, no second record.
        let song_c = SongId::new();
        let song_other = SongId::new();
        let mut planner = RelinkPlanner::new(vec![
            song(song_c, "album/z.flac", "C", 100),
            song(song_other, "album/aa.flac", "other", 100),
        ]);
        let resolution = planner.resolve(&file("album/zz.flac", "hash-album/z.flac", "C", 100));
        assert_eq!(resolution, Resolution::Duplicate { primary: song_c });

        // Hash present at an available path, new path sorts *before* → the
        // new path becomes the deterministic primary.
        let resolution = planner.resolve(&file("album/00.flac", "hash-album/z.flac", "C", 100));
        assert_eq!(resolution, Resolution::Relink { song: song_c });
        assert_eq!(
            planner.song(song_c).unwrap().path().display(),
            "album/00.flac"
        );

        // Weak music key: unique missing candidate + unique file + same
        // normalized key + duration within 2 s → conservative re-link.
        let song_d = SongId::new();
        let mut planner = RelinkPlanner::new(vec![missing_song(song_d, "gone.flac", "D", 100)]);
        let resolution = planner.resolve(&file("fresh.flac", "hash-fresh.flac", "D", 101));
        assert_eq!(resolution, Resolution::Relink { song: song_d });

        // Duration beyond the tolerance → no weak re-link, new identity.
        let song_e = SongId::new();
        let mut planner = RelinkPlanner::new(vec![missing_song(song_e, "gone.flac", "E", 100)]);
        let resolution = planner.resolve(&file("fresh.flac", "hash-fresh.flac", "E", 105));
        assert_eq!(resolution, Resolution::Create);

        // Ambiguity: two missing candidates with the same key → never merge.
        let song_f1 = SongId::new();
        let song_f2 = SongId::new();
        let mut planner = RelinkPlanner::new(vec![
            missing_song(song_f1, "gone1.flac", "F", 100),
            missing_song(song_f2, "gone2.flac", "F", 100),
        ]);
        let resolution = planner.resolve(&file("fresh.flac", "hash-fresh.flac", "F", 100));
        assert_eq!(
            resolution,
            Resolution::Create,
            "ambiguous match never merges"
        );

        // Untagged files (no music key) never weak-relink.
        let song_g = SongId::new();
        let mut planner = RelinkPlanner::new(vec![missing_song(song_g, "gone.flac", "", 100)]);
        let untagged = ParsedFile {
            path: RelativeMediaPath::new("fresh.flac").unwrap(),
            hash: "hash-x".to_owned(),
            size: 1,
            mtime_ns: 1,
            meta: ParsedMetadata {
                duration: Some(Duration::from_secs(100)),
                format: AudioFormat::Flac,
                ..ParsedMetadata::default()
            },
        };
        assert_eq!(planner.resolve(&untagged), Resolution::Create);
    }

    #[test]
    fn keep_refreshes_a_missing_record_to_available_at_the_same_path() {
        // A file deleted externally (record Missing), then restored at the
        // SAME path with changed content: the re-parse path (`Keep`) must
        // restore availability, exactly like the fast-skip `restore` branch.
        let song_a = SongId::new();
        let mut planner = RelinkPlanner::new(vec![missing_song(song_a, "same.flac", "A", 100)]);
        let resolution = planner.resolve(&file("same.flac", "hash-changed", "A", 100));
        assert_eq!(resolution, Resolution::Keep { song: song_a });
        let kept = planner.song(song_a).unwrap();
        assert_eq!(
            kept.availability(),
            SongAvailability::Available,
            "re-parsed same-path file restores the missing record"
        );
        // A pending-delete record must NOT be resurrected by file presence.
        let song_b = SongId::new();
        let mut pending = song(song_b, "pending.flac", "B", 100);
        pending.begin_pending_delete();
        let mut planner = RelinkPlanner::new(vec![pending]);
        let resolution = planner.resolve(&file("pending.flac", "hash-changed", "B", 100));
        assert_eq!(resolution, Resolution::Keep { song: song_b });
        assert_eq!(
            planner.song(song_b).unwrap().availability(),
            SongAvailability::PendingDelete,
            "Echo's own delete owns the record; file presence must not resurrect it"
        );
    }
}
