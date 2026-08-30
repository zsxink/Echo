//! Playlist CRUD use cases (task 6.5, spec "歌单 CRUD").
//!
//! Creating, renaming and deleting a playlist are thin orchestrations over the
//! [`PlaylistRepository`] port, with the name rules enforced here:
//!
//! - trim leading/trailing whitespace;
//! - reject empty and > 40 user-perceived characters (grapheme clusters);
//! - reject a name that collides within the same library root under
//!   NFKC + case-fold (the repository's unique `normalized_name_key`).
//!
//! Rename preserves members and append order (the repository only touches the
//! `playlists` row — `playlist_songs` is untouched). Delete only removes the
//! playlist and its membership rows; songs, their records and memberships in
//! other playlists are never touched.

use crate::application::ports::{PlaylistRepository, SongRepository, TxAccess, UnitOfWork};
use crate::domain::entities::{PlaylistMember, SongAvailability};
use crate::domain::ids::{LibraryRootId, PlaylistId, SongId};
use crate::domain::text::validate_playlist_name;
use crate::error::{Error, Subject};

/// Create an empty playlist with a validated, unoccupied name.
pub struct CreatePlaylist<'a> {
    repository: &'a dyn PlaylistRepository,
}

impl<'a> CreatePlaylist<'a> {
    #[must_use]
    pub const fn new(repository: &'a dyn PlaylistRepository) -> Self {
        Self { repository }
    }

    /// # Errors
    ///
    /// `Validation` when the name is empty or exceeds 40 user-perceived
    /// characters; `Conflict` when a same-root playlist already has that name
    /// (NFKC/case-fold equal); storage errors propagate.
    pub fn execute(&self, root: LibraryRootId, name: &str) -> Result<PlaylistId, Error> {
        let name = name.trim();
        validate_playlist_name(name)
            .map_err(|reason| Error::validation(Subject::Name, "name", reason))?;
        let id = PlaylistId::new();
        self.repository.create(id, root, name)?;
        Ok(id)
    }
}

/// Rename a playlist, preserving members and append order.
pub struct RenamePlaylist<'a> {
    repository: &'a dyn PlaylistRepository,
}

impl<'a> RenamePlaylist<'a> {
    #[must_use]
    pub const fn new(repository: &'a dyn PlaylistRepository) -> Self {
        Self { repository }
    }

    /// # Errors
    ///
    /// `Validation` for an invalid name; `Conflict` for a name collision with a
    /// *different* playlist in the same root; `Unavailable` when the playlist
    /// does not exist; storage errors propagate.
    pub fn execute(&self, id: PlaylistId, name: &str) -> Result<(), Error> {
        let name = name.trim();
        validate_playlist_name(name)
            .map_err(|reason| Error::validation(Subject::Name, "name", reason))?;
        if self.repository.by_id(id)?.is_none() {
            return Err(Error::unavailable("playlist", "not found"));
        }
        // Renaming to the playlist's own current name is allowed (no self
        // collision); a different playlist with the same normalized name is a
        // real conflict surfaced by the unique index.
        self.repository.rename(id, name)
    }
}

/// Delete a playlist and its memberships, never song data.
pub struct DeletePlaylist<'a> {
    repository: &'a dyn PlaylistRepository,
}

impl<'a> DeletePlaylist<'a> {
    #[must_use]
    pub const fn new(repository: &'a dyn PlaylistRepository) -> Self {
        Self { repository }
    }

    /// # Errors
    ///
    /// `Unavailable` when the playlist does not exist; storage errors propagate.
    pub fn execute(&self, id: PlaylistId) -> Result<(), Error> {
        if self.repository.by_id(id)?.is_none() {
            return Err(Error::unavailable("playlist", "not found"));
        }
        self.repository.delete(id)
    }
}

/// List the playlist ids of a root.
pub struct ListPlaylists<'a> {
    repository: &'a dyn PlaylistRepository,
}

impl<'a> ListPlaylists<'a> {
    #[must_use]
    pub const fn new(repository: &'a dyn PlaylistRepository) -> Self {
        Self { repository }
    }

    /// # Errors
    ///
    /// Storage errors propagate.
    pub fn execute(&self, root: LibraryRootId) -> Result<Vec<PlaylistId>, Error> {
        self.repository.list(root)
    }
}

// ---------------------------------------------------------------------------
// Memberships (task 6.6)
// ---------------------------------------------------------------------------

/// Add one song to one or more playlists atomically.
///
/// All memberships commit in a single transaction, so a failure on any target
/// rolls the whole batch back. Adding a song that already belongs to a target
/// is idempotent: no duplicate row, the existing position is preserved. Append
/// (`position = u64::MAX`) takes the next free position, never the wall-clock
/// timestamp, so ordering is deterministic.
pub struct AddToPlaylists<'a> {
    songs: &'a dyn SongRepository,
    playlists: &'a dyn PlaylistRepository,
    unit: &'a dyn UnitOfWork,
}

impl<'a> AddToPlaylists<'a> {
    #[must_use]
    pub const fn new(
        songs: &'a dyn SongRepository,
        playlists: &'a dyn PlaylistRepository,
        unit: &'a dyn UnitOfWork,
    ) -> Self {
        Self {
            songs,
            playlists,
            unit,
        }
    }

    /// # Errors
    ///
    /// `Unavailable` when the song is unknown or not a visible (audio) row;
    /// returns [`Error::Unavailable`] for any missing target playlist;
    /// storage errors propagate. On error none of the memberships are applied.
    pub fn execute(
        &self,
        song: SongId,
        targets: &[PlaylistId],
        position: u64,
    ) -> Result<(), Error> {
        // Validate the song is a visible, available record once.
        let record = self
            .songs
            .by_id(song)?
            .ok_or_else(|| Error::unavailable("song", "not in the library"))?;
        if !record.availability().is_playable() {
            return Err(Error::unavailable("song", "not playable"));
        }
        // Validate every target exists before writing anything.
        for id in targets {
            if self.playlists.by_id(*id)?.is_none() {
                return Err(Error::unavailable("playlist", "not found"));
            }
        }
        if targets.is_empty() {
            return Ok(());
        }
        // Resolve each target's append slot before opening the transaction so
        // the batch is deterministic. `u64::MAX` means "append at the end".
        let slots: Vec<u64> = targets
            .iter()
            .map(|id| {
                let next = self
                    .playlists
                    .members(*id)?
                    .into_iter()
                    .map(|member| member.position())
                    .max()
                    .map_or(0, |current| {
                        if current == u64::MAX {
                            u64::MAX
                        } else {
                            current + 1
                        }
                    });
                Ok(if position == u64::MAX { next } else { position })
            })
            .collect::<Result<_, Error>>()?;
        let targets_owned: Vec<PlaylistId> = targets.to_vec();
        self.unit.with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            for (id, slot) in targets_owned.iter().zip(slots) {
                tx.insert_member(&PlaylistMember::new(
                    *id,
                    song,
                    slot,
                    SongAvailability::Available,
                ))?;
            }
            Ok(())
        }))
    }
}

/// Remove one song from one playlist. Only that membership is affected.
pub struct RemoveFromPlaylist<'a> {
    playlists: &'a dyn PlaylistRepository,
}

impl<'a> RemoveFromPlaylist<'a> {
    #[must_use]
    pub const fn new(playlists: &'a dyn PlaylistRepository) -> Self {
        Self { playlists }
    }

    /// # Errors
    ///
    /// Storage errors propagate. Removing a non-member is a no-op success.
    pub fn execute(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.playlists.remove_member(playlist, song)
    }
}

/// Members of a playlist ordered by append position (task 6.6 "打开歌单"):
/// the display keeps the membership's deterministic position order, never the
/// insertion timestamp.
pub struct PlaylistMembers<'a> {
    playlists: &'a dyn PlaylistRepository,
}

impl<'a> PlaylistMembers<'a> {
    #[must_use]
    pub const fn new(playlists: &'a dyn PlaylistRepository) -> Self {
        Self { playlists }
    }

    /// # Errors
    ///
    /// Storage errors propagate.
    pub fn execute(&self, playlist: PlaylistId) -> Result<Vec<PlaylistMember>, Error> {
        let mut members = self.playlists.members(playlist)?;
        members.sort_by_key(PlaylistMember::position);
        Ok(members)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::testing::memory_database::MemoryDatabase;
    use crate::domain::entities::LibraryRoot;

    fn root(db: &MemoryDatabase) -> LibraryRootId {
        let root = LibraryRootId::new();
        crate::application::ports::LibraryRepository::upsert(
            db,
            &LibraryRoot::new(root, ".".into(), true, true),
        )
        .expect("active root");
        root
    }

    #[test]
    fn create_validates_and_dedups_names() {
        let db = MemoryDatabase::new();
        let root = root(&db);
        let create = CreatePlaylist::new(&db);

        // Empty / whitespace-only rejected.
        assert!(create.execute(root, "   ").is_err());
        // Over-40 graphemes rejected.
        let long = "长".repeat(41);
        assert!(create.execute(root, &long).is_err());
        // Exactly 40 accepted.
        let ok = "长".repeat(40);
        create.execute(root, &ok).expect("40 graphemes ok");

        // Duplicate under NFKC/case-fold rejected.
        assert!(create.execute(root, "  CHILL  ").is_ok());
        assert!(create.execute(root, "chill").is_err(), "case-fold dup");

        // 41 *spaces* are trimmed to empty before the grapheme check.
        assert!(create.execute(root, &" ".repeat(41)).is_err());
    }

    #[test]
    fn rename_keeps_members_and_rejects_collisions() {
        let db = MemoryDatabase::new();
        let root = root(&db);
        // Members: add two songs so rename must leave them ordered.
        let song_a = crate::domain::ids::SongId::new();
        let song_b = crate::domain::ids::SongId::new();
        for (i, song) in [song_a, song_b].into_iter().enumerate() {
            let record = crate::domain::entities::Song::new(
                song,
                root,
                crate::domain::ids::RelativeMediaPath::new(&format!("{i}.flac")).expect("path"),
                crate::domain::ids::Revision::INITIAL,
            );
            crate::application::ports::SongRepository::upsert(&db, &record).expect("song");
        }
        let id = CreatePlaylist::new(&db)
            .execute(root, "Road Trip")
            .expect("create");
        PlaylistRepository::add_member(&db, id, song_a, u64::MAX).expect("member a");
        PlaylistRepository::add_member(&db, id, song_b, u64::MAX).expect("member b");

        let rename = RenamePlaylist::new(&db);
        rename.execute(id, "  Road Trip 2  ").expect("rename");
        // Members and append order untouched.
        let members = PlaylistRepository::members(&db, id).expect("members");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].song(), song_a, "append order kept");
        assert_eq!(members[1].song(), song_b);

        // Another playlist collides with the new name.
        let other = CreatePlaylist::new(&db)
            .execute(root, "other")
            .expect("other");
        assert!(
            rename.execute(other, "ROAD TRIP 2").is_err(),
            "case-fold collision"
        );
        // Renaming to one's own name is a no-op success.
        rename.execute(id, "road trip 2").expect("self-rename ok");
    }

    #[test]
    fn add_to_multiple_playlists_is_atomic_and_idempotent() {
        let db = MemoryDatabase::new();
        let root = root(&db);
        let song = crate::domain::ids::SongId::new();
        let record = crate::domain::entities::Song::new(
            song,
            root,
            crate::domain::ids::RelativeMediaPath::new("song.flac").expect("path"),
            crate::domain::ids::Revision::INITIAL,
        );
        crate::application::ports::SongRepository::upsert(&db, &record).expect("song");
        let a = CreatePlaylist::new(&db).execute(root, "A").expect("a");
        let b = CreatePlaylist::new(&db).execute(root, "B").expect("b");

        let add = AddToPlaylists::new(&db, &db, &db);
        // One atomic batch into two playlists (append).
        add.execute(song, &[a, b], u64::MAX).expect("add both");
        assert_eq!(
            PlaylistRepository::members(&db, a).expect("A").len(),
            1,
            "atomic batch committed every target"
        );
        assert_eq!(PlaylistRepository::members(&db, b).expect("B").len(), 1);

        // Idempotent re-add: no duplicate, position unchanged.
        add.execute(song, &[a], u64::MAX).expect("re-add");
        let members = PlaylistRepository::members(&db, a).expect("A members");
        assert_eq!(members.len(), 1, "no duplicate member");
        assert_eq!(members[0].position(), 0, "existing position preserved");

        // A missing target rolls the whole batch back.
        let ghost = PlaylistId::new();
        assert!(add.execute(song, &[a, ghost], u64::MAX).is_err());
        assert_eq!(
            PlaylistRepository::members(&db, a).expect("A").len(),
            1,
            "rollback keeps A untouched"
        );

        // The song added at an explicit position lands there.
        let c = CreatePlaylist::new(&db).execute(root, "C").expect("c");
        add.execute(song, &[c], 7).expect("explicit position");
        assert_eq!(
            PlaylistRepository::members(&db, c).expect("C")[0].position(),
            7
        );
    }

    #[test]
    fn remove_and_reappend_do_not_reuse_timestamps() {
        let db = MemoryDatabase::new();
        let root = root(&db);
        let song = crate::domain::ids::SongId::new();
        let record = crate::domain::entities::Song::new(
            song,
            root,
            crate::domain::ids::RelativeMediaPath::new("song.flac").expect("path"),
            crate::domain::ids::Revision::INITIAL,
        );
        crate::application::ports::SongRepository::upsert(&db, &record).expect("song");
        let a = CreatePlaylist::new(&db).execute(root, "A").expect("a");

        let add = AddToPlaylists::new(&db, &db, &db);
        add.execute(song, &[a], u64::MAX).expect("add");
        assert_eq!(
            PlaylistRepository::members(&db, a).expect("A")[0].position(),
            0
        );

        // Remove, then re-append.
        RemoveFromPlaylist::new(&db)
            .execute(a, song)
            .expect("remove");
        assert!(PlaylistRepository::members(&db, a).expect("A").is_empty());
        add.execute(song, &[a], u64::MAX).expect("re-append");
        // Deterministic: it goes to position 0 again (fresh append), never
        // depending on a timestamp.
        assert_eq!(
            PlaylistRepository::members(&db, a).expect("A")[0].position(),
            0
        );
        // Removing a non-member is a no-op.
        RemoveFromPlaylist::new(&db)
            .execute(a, song)
            .expect("remove again is a no-op");
    }

    #[test]
    fn delete_owns_playlist_and_members_but_not_songs_or_other_playlists() {
        let db = MemoryDatabase::new();
        let root = root(&db);
        let song = crate::domain::ids::SongId::new();
        let record = crate::domain::entities::Song::new(
            song,
            root,
            crate::domain::ids::RelativeMediaPath::new("keep.flac").expect("path"),
            crate::domain::ids::Revision::INITIAL,
        );
        crate::application::ports::SongRepository::upsert(&db, &record).expect("song");
        let a = CreatePlaylist::new(&db).execute(root, "A").expect("a");
        let b = CreatePlaylist::new(&db).execute(root, "B").expect("b");
        PlaylistRepository::add_member(&db, a, song, u64::MAX).expect("member in A");
        PlaylistRepository::add_member(&db, b, song, u64::MAX).expect("member in B");

        DeletePlaylist::new(&db).execute(a).expect("delete A");
        assert!(PlaylistRepository::by_id(&db, a).expect("by id").is_none());
        // The song record survives (never deleted).
        assert!(crate::application::ports::SongRepository::by_id(&db, song)
            .expect("by id")
            .is_some());
        // Its membership in B survives.
        assert_eq!(
            PlaylistRepository::members(&db, b)
                .expect("B members")
                .len(),
            1
        );
        // Deleting an unknown playlist is unavailable.
        assert!(DeletePlaylist::new(&db).execute(PlaylistId::new()).is_err());
    }
}
