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

use crate::application::ports::PlaylistRepository;
use crate::domain::ids::{LibraryRootId, PlaylistId};
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
