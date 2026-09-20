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

use crate::application::ports::SongRepository;
use crate::application::ports::{ControlPlanePort, ManifestState};
use crate::domain::entities::Song;
use crate::domain::ids::{LibraryRootId, Revision};
use crate::domain::library::{LibraryId, LibraryManifest, PortableRecord};
use crate::error::Error;

/// The version stamp written into `echo/manifest.json`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What [`EnsureControlPlane`] did to make the control surface usable.
///
/// The variant matters: a healed root must be reported (and must never be
/// mistaken for a brand-new library), and a root whose manifest this build
/// cannot read is a refusal, not a blank slate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlPlaneStatus {
    /// A compatible manifest was already present.
    Ready { library_id: LibraryId },
    /// First enablement of a writable root: the manifest was created.
    Initialized { library_id: LibraryId },
    /// `echo/records/` held object records but the manifest was missing: the
    /// manifest was rebuilt from the surviving records (design D3 case 2).
    /// No object or UUID was touched.
    Healed { library_id: LibraryId },
    /// `echo/` cannot be created or updated: the root stays read-only and no
    /// control-plane file was written (design D3 / spec 控制面不可写).
    NotUsable,
}

impl ControlPlaneStatus {
    /// The library identity, when the control surface is usable.
    #[must_use]
    pub const fn library_id(self) -> Option<LibraryId> {
        match self {
            Self::Ready { library_id }
            | Self::Initialized { library_id }
            | Self::Healed { library_id } => Some(library_id),
            Self::NotUsable => None,
        }
    }

    /// Whether this pass had to rebuild a missing manifest.
    #[must_use]
    pub const fn healed(self) -> bool {
        matches!(self, Self::Healed { .. })
    }
}

/// Guarantee `echo/manifest.json` exists for an enabled library (design D3).
///
/// Three distinct situations, in order:
///
/// 1. The manifest exists and this build understands it → no write.
/// 2. The manifest is missing but `echo/records/` holds object records →
///    **self-heal**: write a manifest, keep every object and UUID, and report
///    [`ControlPlaneStatus::Healed`]. This is the real-world case behind
///    issue #1, where a manifest-less directory would otherwise be scanned as
///    a brand-new library and every song would be minted a fresh UUID.
/// 3. The manifest is missing and there are no records → first enablement,
///    create it.
///
/// A manifest this build cannot read (`Incompatible`/`Malformed`) is **never**
/// overwritten: continuation is refused so a newer library is not silently
/// downgraded. A control surface that cannot be written reports
/// [`ControlPlaneStatus::NotUsable`] without touching any file.
pub struct EnsureControlPlane<'a> {
    control: &'a dyn ControlPlanePort,
}

impl<'a> EnsureControlPlane<'a> {
    #[must_use]
    pub const fn new(control: &'a dyn ControlPlanePort) -> Self {
        Self { control }
    }

    /// Ensure the control surface of `root` is initialized and report what it
    /// took to get there.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedMedia`] when a manifest exists but its
    /// format is newer than this build supports, or is unparseable — the file
    /// is left untouched. Propagates control-surface read/write failures.
    pub fn run(&self, root: LibraryRootId) -> Result<ControlPlaneStatus, Error> {
        if !self.control.control_plane_usable(root)? {
            // The root stays read-only: no logic change may be committed to a
            // database that cannot materialize it into `echo/`.
            return Ok(ControlPlaneStatus::NotUsable);
        }
        match self.control.manifest_state(root)? {
            ManifestState::Compatible(manifest) => Ok(ControlPlaneStatus::Ready {
                library_id: manifest.library_id,
            }),
            ManifestState::Incompatible { format_version } => Err(Error::UnsupportedMedia {
                operation: "open library".to_owned(),
                reason: format!(
                    "portable manifest format version {format_version} is newer than this build supports"
                ),
            }),
            ManifestState::Malformed => Err(Error::UnsupportedMedia {
                operation: "open library".to_owned(),
                reason: "portable manifest is unreadable".to_owned(),
            }),
            ManifestState::Absent => {
                let library_id = LibraryId::from_uuid(uuid::Uuid::new_v4());
                let healed = self.control.records_present(root)?;
                self.control
                    .write_manifest(root, &LibraryManifest::new(library_id, APP_VERSION))?;
                Ok(if healed {
                    ControlPlaneStatus::Healed { library_id }
                } else {
                    ControlPlaneStatus::Initialized { library_id }
                })
            }
        }
    }
}

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
        EnsureControlPlane::new(self.control)
            .run(root)?
            .library_id()
            .ok_or_else(|| Error::unavailable("library", "control surface is not writable"))
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
            added_at: 1_700_001_002_003,
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
    fn ensure_control_plane_initializes_a_fresh_writable_root() {
        let fixture = ScanFixture::new();
        assert!(!fixture.control.records_present(fixture.root).unwrap());

        let status = EnsureControlPlane::new(&fixture.control)
            .run(fixture.root)
            .expect("first ensure");
        assert!(matches!(status, ControlPlaneStatus::Initialized { .. }));
        // The manifest now exists and a second pass is a no-op.
        let again = EnsureControlPlane::new(&fixture.control)
            .run(fixture.root)
            .expect("second ensure");
        assert!(matches!(again, ControlPlaneStatus::Ready { .. }));
        assert_eq!(status.library_id(), again.library_id());
    }

    #[test]
    fn ensure_control_plane_heals_a_manifest_less_directory_with_records() {
        let fixture = ScanFixture::new();
        // The real-world issue #1 shape: object records survived, the manifest
        // did not (the directory was later rescanned and every song minted a
        // fresh UUID).
        let song_uuid = uuid::Uuid::new_v4();
        seed_record(&fixture.control, fixture.root, song_uuid);
        assert!(fixture.control.records_present(fixture.root).unwrap());
        assert!(fixture.control.manifest_of(fixture.root).is_none());

        let status = EnsureControlPlane::new(&fixture.control)
            .run(fixture.root)
            .expect("heal");
        assert!(
            matches!(status, ControlPlaneStatus::Healed { .. }),
            "a records-bearing directory is healed, never treated as new"
        );
        assert!(status.healed());
        // Self-heal preserves every object and UUID: nothing was rewritten.
        let records = fixture.control.records_of(fixture.root);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].object_uuid(), song_uuid);
    }

    #[test]
    fn ensure_control_plane_refuses_a_newer_manifest_without_touching_it() {
        let fixture = ScanFixture::new();
        let future = LibraryManifest {
            format_version: crate::domain::library::CURRENT_FORMAT_VERSION + 7,
            library_id: LibraryId::from_uuid(uuid::Uuid::new_v4()),
            written_by_app_version: "9.9.9".to_owned(),
        };
        fixture.control.set_manifest(fixture.root, future.clone());

        let error = EnsureControlPlane::new(&fixture.control)
            .run(fixture.root)
            .expect_err("a newer manifest is a refusal");
        assert_eq!(error.code(), "unsupported_media");
        // The self-heal path must not have overwritten the file.
        assert_eq!(fixture.control.manifest_of(fixture.root), Some(future));
        // And the adapter no longer even reports it as readable.
        assert!(fixture
            .control
            .read_manifest(fixture.root)
            .unwrap()
            .is_none());
    }

    #[test]
    fn ensure_control_plane_reports_an_unusable_surface_without_writing() {
        let fixture = ScanFixture::new();
        fixture.control.set_usable(false);
        let status = EnsureControlPlane::new(&fixture.control)
            .run(fixture.root)
            .expect("reported, not failed");
        assert_eq!(status, ControlPlaneStatus::NotUsable);
        assert!(fixture.control.manifest_of(fixture.root).is_none());
        // The higher-level initializer refuses a logic change outright.
        let error = InitPortableLibrary::new(&fixture.control)
            .run(fixture.root)
            .expect_err("no manifest without a writable control surface");
        assert_eq!(error.code(), "unavailable");
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
