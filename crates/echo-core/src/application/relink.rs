//! Identity resolution for scanned files (task 4.8 / design §6.7).
//!
//! Given the scan-start snapshot of a root's songs and one freshly parsed
//! file, [`RelinkPlanner`] decides — as a pure, testable computation —
//! whether the file
//!
//! - keeps the record already stored at its path (refresh in place),
//! - *re-links* an existing identity whose path vanished and whose content
//!   hash matches the new file, or
//! - is duplicate content of an available record (one record per hash;
//!   duplicates are reported, never merged into new UUIDs), or
//! - mints a new UUID.
//!
//! Similar tags alone never re-link: LD-05 forbids merging or rewriting
//! relations on a music-key heuristic, so a file whose content hash changed
//! gets a fresh UUID and the vanished record stays `Missing` (the published
//! `local-library` requirement permits such a re-link but does not require
//! it). Only the content hash — evidence of the same bytes — carries an
//! identity across a move.
//!
//! Ambiguity always loses: any non-unique match leaves the old record
//! missing and creates a new UUID, so no wrong merge can happen. Every
//! decision is folded back into the snapshot so later decisions in the same
//! scan see earlier ones.

use std::collections::HashSet;

use crate::domain::entities::{Song, SongAvailability};
use crate::domain::ids::{LibraryRootId, RelativeMediaPath, Revision, SongId};
use crate::domain::media::ParsedMetadata;

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

/// The mutable decision state over one scan's song snapshot.
#[derive(Debug, Default)]
pub struct RelinkPlanner {
    songs: Vec<Song>,
    /// Records whose identity a file already claimed during this scan — a
    /// second file matching the same missing record is ambiguous.
    claimed: HashSet<SongId>,
    /// Records this scan proved present after they were `Missing`
    /// (revivals). `upsert_song` deliberately never writes `availability`
    /// (a stale snapshot must not roll user/availability state backward), so
    /// the caller persists each revival through the dedicated mutation
    /// instead. Without this the flip lives in memory only and a
    /// record-projected library stays invisible forever.
    revived: HashSet<SongId>,
}

impl RelinkPlanner {
    /// Build the planner from the scan-start snapshot.
    #[must_use]
    pub fn new(songs: Vec<Song>) -> Self {
        Self {
            songs,
            claimed: HashSet::new(),
            revived: HashSet::new(),
        }
    }

    /// Align the snapshot with what this run's enumeration actually saw.
    ///
    /// A completed run persists exactly this flip in its final missing pass
    /// (available records whose path the enumeration did not see become
    /// `Missing`). Applying it up front makes identity resolution agree with
    /// the post-scan truth: without it a record whose file was moved is still
    /// an *available* primary hash holder, so the same-content file at the new
    /// path is misreported as duplicate content and stays unindexed instead of
    /// re-linking the identity. Only an enumeration that covered the whole
    /// root may call this — a partial view would claim absence it never
    /// observed.
    #[must_use]
    pub fn align_with_enumeration(mut self, seen: &HashSet<String>) -> Self {
        for song in &mut self.songs {
            if song.availability() == SongAvailability::Available
                && !seen.contains(song.path().identity_key())
            {
                song.mark_missing();
            }
        }
        self
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
                let id = song.id();
                self.revived.insert(id);
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

        // 3. Nothing matches: a fresh identity (the caller registers it via
        // [`Self::register_created`] so later duplicates resolve against it).
        //
        // Similar tags never reach a re-link here: LD-05 forbids a music-key
        // merge, so the vanished record keeps its `Missing` state and the new
        // file gets its own UUID (no relation is rewritten).
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
            if song.availability() == SongAvailability::Missing {
                song.restore_available();
                self.revived.insert(id);
            }
        }
    }

    /// Whether this scan proved `id` present while its snapshot row was
    /// `Missing`. The caller persists the flip: a revival is a fact the file
    /// itself established, so it belongs in the dedicated mutation, not in
    /// the metadata upsert.
    #[must_use]
    pub fn revived(&self, id: SongId) -> bool {
        self.revived.contains(&id)
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

    fn relink(&mut self, id: SongId, file: &ParsedFile) {
        self.claimed.insert(id);
        if let Some(song) = self.songs.iter_mut().find(|song| song.id() == id) {
            if song.availability() == SongAvailability::Missing {
                self.revived.insert(id);
            }
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

        // LD-05: a vanished record plus a new file with a matching normalized
        // music key MUST NOT re-link — the old record stays missing and the
        // new file gets its own UUID. Only the content hash carries identity.
        let song_d = SongId::new();
        let mut planner = RelinkPlanner::new(vec![missing_song(song_d, "gone.flac", "D", 100)]);
        let resolution = planner.resolve(&file("fresh.flac", "hash-fresh.flac", "D", 100));
        assert_eq!(
            resolution,
            Resolution::Create,
            "a music-key match never merges identities (LD-05)"
        );
        assert_eq!(
            planner.song(song_d).unwrap().availability(),
            SongAvailability::Missing,
            "the vanished record keeps its state and path"
        );
        assert_eq!(planner.song(song_d).unwrap().path().display(), "gone.flac");

        // Duration beyond the tolerance used to matter; with no heuristic at
        // all the outcome is the same fresh identity.
        let song_e = SongId::new();
        let mut planner = RelinkPlanner::new(vec![missing_song(song_e, "gone.flac", "E", 100)]);
        let resolution = planner.resolve(&file("fresh.flac", "hash-fresh.flac", "E", 105));
        assert_eq!(resolution, Resolution::Create);

        // Two missing candidates with the same key → still no merge.
        let song_f1 = SongId::new();
        let song_f2 = SongId::new();
        let mut planner = RelinkPlanner::new(vec![
            missing_song(song_f1, "gone1.flac", "F", 100),
            missing_song(song_f2, "gone2.flac", "F", 100),
        ]);
        let resolution = planner.resolve(&file("fresh.flac", "hash-fresh.flac", "F", 100));
        assert_eq!(resolution, Resolution::Create, "no heuristic merge");

        // Untagged files likewise never re-link.
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
    fn enumeration_alignment_lets_a_same_hash_move_relink_instead_of_duplicating() {
        // A managed path no longer present in the enumeration is not an
        // available primary: the moved copy re-links the identity.
        let song_a = SongId::new();
        let songs = vec![song(song_a, "echo/media/a.flac", "A", 100)];
        let seen = std::iter::once("media/moved.flac".to_owned()).collect();
        let mut planner = RelinkPlanner::new(songs).align_with_enumeration(&seen);
        let resolution = planner.resolve(&file(
            "media/moved.flac",
            "hash-echo/media/a.flac",
            "A",
            100,
        ));
        assert_eq!(resolution, Resolution::Relink { song: song_a });
        assert_eq!(
            planner.song(song_a).unwrap().path().display(),
            "media/moved.flac"
        );

        // The same content still present at its recorded path stays primary,
        // so a second copy is reported as duplicate rather than stealing it.
        let song_b = SongId::new();
        let songs = vec![song(song_b, "album/z.flac", "B", 100)];
        let seen = ["album/z.flac".to_owned(), "album/zz.flac".to_owned()]
            .into_iter()
            .collect();
        let mut planner = RelinkPlanner::new(songs).align_with_enumeration(&seen);
        assert_eq!(
            planner.resolve(&file("album/zz.flac", "hash-album/z.flac", "B", 100)),
            Resolution::Duplicate { primary: song_b }
        );
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
