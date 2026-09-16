//! Portable library initialization and projection use cases (task 2.2).
//!
//! Two entry points:
//!
//! 1. [`InitPortableLibrary`] — first-ever write into a library: creates
//!    `echo/manifest.json` with a fresh [`LibraryId`] and stores the library
//!    root as an active root. Idempotent: calling on an already-initialized
//!    library is a no-op that returns the existing [`LibraryId`].
//!
//! 2. [`ProjectPortableRecords`] — a new device that has received the
//!    `echo/records/` tree (via the sync connector) projects the object records
//!    into the local store. Runs on every startup for a library that has a
//!    manifest. Is projected *before* the scan (`media/`) so songs exist in the
//!    database as soon as possible, enabling playlist restoration and the
//!    "media-not-yet-available" path.

use crate::application::ports::ControlPlanePort;
use crate::application::ports::SongRepository;
use crate::domain::entities::Song;
use crate::domain::ids::{LibraryRootId, Revision};
use crate::domain::library::{LibraryId, LibraryManifest, PortableRecord};
use crate::error::Error;

/// Initialize a new portable library: write the initial `echo/manifest.json`
/// and mark the root as active.
///
/// On repeat calls, the existing manifest is read and the stored
/// [`LibraryId`] is returned without modification (idempotent).
pub struct InitPortableLibrary<'a> {
    control: &'a dyn ControlPlanePort,
}

impl<'a> InitPortableLibrary<'a> {
    #[must_use]
    pub const fn new(control: &'a dyn ControlPlanePort) -> Self {
        Self { control }
    }

    /// Run the initialization for `root`.
    ///
    /// Returns the library's stable identity so the caller can tie this root
    /// to a sync source later.
    ///
    /// # Errors
    ///
    /// Propagates a control-surface write or read failure.
    pub fn run(&self, root: LibraryRootId) -> Result<LibraryId, Error> {
        // Idempotent: if a manifest already exists, return its library id.
        if let Some(existing) = self.control.read_manifest(root)? {
            return Ok(existing.library_id);
        }
        // First write: create a fresh manifest with a stable library id.
        let library_id = LibraryId::from_uuid(uuid::Uuid::new_v4());
        let manifest = LibraryManifest::new(library_id, "0.1.0");
        self.control.write_manifest(root, &manifest)?;
        Ok(library_id)
    }
}

/// Project portable records into the local store for a library that already
/// has a manifest.  Reads every record of the configured kinds, re-creates
/// the matching domain entities, and persists them through the repository.
///
/// Projection is **idempotent**: song/favorite/playlist/member UUIDs are
/// derived from the object UUID in the record, so re-projection overwrites
/// existing rows without changing their identity.
pub struct ProjectPortableRecords<'a> {
    control: &'a dyn ControlPlanePort,
    songs: &'a dyn SongRepository,
}

impl<'a> ProjectPortableRecords<'a> {
    #[must_use]
    pub const fn new(control: &'a dyn ControlPlanePort, songs: &'a dyn SongRepository) -> Self {
        Self { control, songs }
    }

    /// Project every song record from the control plane into the local store.
    ///
    /// Skips tombstones — a tombstone means the object was deleted on another
    /// device and the local store must drop the row (handled by a separate
    /// reconciliation pass, not this projection).
    ///
    /// # Errors
    ///
    /// Propagates a control-surface read failure or a store write failure.
    pub fn project_songs(&self, root: LibraryRootId) -> Result<usize, Error> {
        let raw_records = self
            .control
            .list_records(root, crate::domain::library::RecordKind::Song)?;
        let mut created = 0;
        for raw in raw_records {
            let record: PortableRecord =
                serde_json::from_str(&raw).map_err(|e| Error::Storage {
                    what: "record parse".to_owned(),
                    source: Box::new(e),
                })?;
            if let PortableRecord::Song(song_record) = &record {
                let song_uuid = crate::domain::ids::SongId::from_uuid(song_record.song_uuid);
                let path =
                    crate::domain::ids::RelativeMediaPath::new(song_record.media_path.as_str())?;
                // Upsert with the record's own UUID; the identity is stable.
                let mut song = Song::new(song_uuid, root, path, Revision::INITIAL);
                song.apply_metadata(
                    song_record.title.clone(),
                    song_record.artist.clone(),
                    song_record.album.clone(),
                    None, // duration not in the record
                );
                // Songs whose media hash is not yet resolved are Missing
                // (the media scan will bring them back to Available later).
                // For now, project as Available — the scan reconciles.
                self.songs.upsert(&song)?;
                created += 1;
            }
        }
        Ok(created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::SongRepository;
    use crate::application::testing::scan_fixture::ScanFixture;
    use crate::application::testing::small_fakes::MemoryControlPlane;
    use crate::domain::ids::SongId;
    use crate::domain::library::{DeviceId, HybridLogicalClock, LibraryRelativePath, SongRecord};

    /// Helper: write one song record into the in-memory control plane.
    fn seed_record(control: &MemoryControlPlane, root: LibraryRootId, song_uuid: uuid::Uuid) {
        let record = PortableRecord::Song(SongRecord {
            song_uuid,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::from_uuid(uuid::Uuid::new_v4()),
            hlc: HybridLogicalClock::new(1_700_000_000, 0),
            media_path: LibraryRelativePath::new("media/歌手/歌手 - 晴天.flac").unwrap(),
            content_hash: "abc123".to_owned(),
            title: Some("晴天".to_owned()),
            artist: Some("歌手".to_owned()),
            album: None,
        });
        control.write_record(root, &record).expect("seed record");
    }

    #[test]
    fn init_writes_manifest_and_is_idempotent() {
        let fixture = ScanFixture::new();

        let lib_id = InitPortableLibrary::new(&fixture.control)
            .run(fixture.root)
            .expect("first init");

        // The manifest exists and returns the same library id.
        let read = InitPortableLibrary::new(&fixture.control)
            .run(fixture.root)
            .expect("second init (idempotent)");
        assert_eq!(lib_id, read);
    }

    #[test]
    fn project_songs_upserts_with_stable_uuid() {
        let fixture = ScanFixture::new();
        let songs = fixture.deps.songs.clone();

        let song_uuid = uuid::Uuid::new_v4();
        seed_record(&fixture.control, fixture.root, song_uuid);

        let count = ProjectPortableRecords::new(&fixture.control, songs.as_ref())
            .project_songs(fixture.root)
            .expect("project");
        assert_eq!(count, 1);

        // The song exists in the store with the record's UUID (stable).
        let song = SongRepository::by_id(songs.as_ref(), SongId::from_uuid(song_uuid))
            .expect("read back")
            .expect("present");
        assert_eq!(song.title(), Some("晴天"));
        assert_eq!(song.artist(), Some("歌手"));

        // Re-projecting is idempotent: same UUID, no duplicate.
        let count = ProjectPortableRecords::new(&fixture.control, songs.as_ref())
            .project_songs(fixture.root)
            .expect("project again");
        assert_eq!(count, 1, "idempotent: same song, not duplicated");
        let song = SongRepository::by_id(songs.as_ref(), SongId::from_uuid(song_uuid))
            .expect("read back")
            .expect("present");
        assert_eq!(
            song.title(),
            Some("晴天"),
            "metadata preserved after re-project"
        );
    }
}
