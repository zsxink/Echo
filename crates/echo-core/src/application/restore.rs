//! New-device library restore coordinator (task 2.3).
//!
//! Restores a library from its portable records before its media arrives:
//! **records → projection → media scan → hash relink**. The order guarantees
//! a song's UUID is always the one carried by its portable record — never a
//! fresh UUID minted because the media file happened to be scanned first.
//!
//! Steps:
//!
//! 1. **Verify the manifest** (`echo/manifest.json`): format compatibility and,
//!    when a source library id is expected, that the directory actually belongs
//!    to the same logical library. A directory without a compatible manifest is
//!    not a restore target.
//! 2. **Project records** into the local store (stable UUIDs, media kept
//!    `Missing`).
//! 3. **Scan `media/`**: full-file BLAKE3 + `RelinkPlanner` resolves each file
//!    against the projected snapshot. A file whose hash (or a conservative,
//!    unique music key) matches a missing record re-links it — the media
//!    arrived. A file with no match is **rejected**, never minted into a new
//!    UUID that would diverge from the portable records.
//! 4. **Keep missing.** Songs whose media has not arrived stay `Missing`,
//!    retaining every UUID association until the file is transferred.
//!
//! The coordinator is idempotent: re-running against the same records + media
//! preserves every UUID.

use crate::application::continuation::{ContinueFromRecords, ProjectionReport};
use crate::application::ports::{
    ContentHasher, ControlPlanePort, LibraryFileSystem, MediaProbe, MetadataReader,
    PlaylistRepository, ProbeOutcome, SongRepository, UnitOfWork,
};
use crate::application::relink::{ParsedFile, RelinkPlanner, Resolution};
use crate::domain::entities::{Song, SongAvailability};
use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::library::{LibraryId, LibraryManifest};
use crate::error::Error;

/// Outcome of one restore pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RestoreOutcome {
    /// Songs projected from portable song records.
    pub projected_songs: usize,
    /// The full per-kind projection report (songs, favorites, playlists,
    /// members, play stats, tombstones, unplaceable records).
    pub projection: ProjectionReport,
    /// Files found under `media/` and reconciled onto projected identities.
    pub media_reconciled: usize,
    /// Records that remain `Missing` (no media arrived yet).
    pub missing: usize,
    /// `media/` files with no identity match (rejected, not minted).
    pub rejected: usize,
}

/// The restore coordinator for the portable-library-layout "全新设备重建"
/// requirement (design §4, portable-library-layout / local-library specs).
pub struct RestoreLibrary<'a> {
    control: &'a dyn ControlPlanePort,
    songs: &'a dyn SongRepository,
    playlists: &'a dyn PlaylistRepository,
    fs: &'a dyn LibraryFileSystem,
    hasher: &'a dyn ContentHasher,
    probe: &'a dyn MediaProbe,
    metadata: &'a dyn MetadataReader,
    uow: &'a dyn UnitOfWork,
    /// When a source library id is already known (e.g. a configured sync
    /// source), the manifest must match it; otherwise any compatible manifest
    /// is accepted.
    expected_library_id: Option<LibraryId>,
}

impl<'a> RestoreLibrary<'a> {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        control: &'a dyn ControlPlanePort,
        songs: &'a dyn SongRepository,
        playlists: &'a dyn PlaylistRepository,
        fs: &'a dyn LibraryFileSystem,
        hasher: &'a dyn ContentHasher,
        probe: &'a dyn MediaProbe,
        metadata: &'a dyn MetadataReader,
        uow: &'a dyn UnitOfWork,
        expected_library_id: Option<LibraryId>,
    ) -> Self {
        Self {
            control,
            songs,
            playlists,
            fs,
            hasher,
            probe,
            metadata,
            uow,
            expected_library_id,
        }
    }

    /// Restore `root` from its portable records.
    ///
    /// # Errors
    ///
    /// Returns `Unavailable` when no compatible manifest exists, `Conflict`
    /// when the manifest library id does not match the expected source, and
    /// propagates infra failures while reading the control surface or `media/`.
    pub fn run(&self, root: LibraryRootId) -> Result<RestoreOutcome, Error> {
        // 1. Verify the manifest (format + library id).
        let manifest = self
            .control
            .read_manifest(root)?
            .ok_or_else(|| Error::unavailable("library", "no portable manifest present"))?;
        self.verify_manifest(&manifest)?;

        // 2. Project portable records (stable UUIDs, media Missing). This is
        //    the same projection the root-switch continuation runs, so the
        //    new-device path and the "open a local directory again" path
        //    cannot drift apart.
        let projection =
            ContinueFromRecords::new(self.control, self.songs, self.playlists, self.uow)
                .run(root)?
                .projection;
        let snapshot = self.songs.all_in_root(root).unwrap_or_default();
        let mut planner = RelinkPlanner::new(snapshot);
        let projected = projection.songs;

        // 3. Scan `media/` and hash-relink files onto projected identities.
        let mut reconciled = 0;
        let mut rejected = 0;
        for path in self.enumerate_media(root)? {
            if let Some(file) = self.parse_file(root, &path)? {
                match planner.resolve(&file) {
                    Resolution::Keep { song: id } | Resolution::Relink { song: id } => {
                        if let Some(song) = planner.song(id) {
                            self.persist_song(&song)?;
                        }
                        reconciled += 1;
                    }
                    Resolution::Duplicate { .. } => {
                        // Content already held by an available record — the
                        // duplicate is not a new identity.
                        reconciled += 1;
                    }
                    Resolution::Create => {
                        // No projected record matches: reject, never mint.
                        rejected += 1;
                    }
                }
            }
        }

        let missing = planner
            .snapshot()
            .iter()
            .filter(|s| s.availability() == SongAvailability::Missing)
            .count();

        Ok(RestoreOutcome {
            projected_songs: projected,
            projection,
            media_reconciled: reconciled,
            missing,
            rejected,
        })
    }

    /// Verify the manifest's format compatibility and (when expected) its
    /// library id.
    fn verify_manifest(&self, manifest: &LibraryManifest) -> Result<(), Error> {
        if !manifest.is_compatible() {
            return Err(Error::UnsupportedMedia {
                operation: "restore".to_owned(),
                reason: "portable manifest format version unsupported".to_owned(),
            });
        }
        if let Some(expected) = self.expected_library_id {
            if expected != manifest.library_id {
                return Err(Error::conflict(
                    "portable manifest library id does not match the expected source",
                ));
            }
        }
        Ok(())
    }

    /// Enumerate the supported audio files under `media/` only.
    fn enumerate_media(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
        Ok(self
            .fs
            .enumerate(root)?
            .into_iter()
            .filter(|p| p.normalized().starts_with("media/"))
            .collect())
    }

    /// Parse one media file into a [`ParsedFile`], or `Ok(None)` when it is
    /// not a supported audio file (the caller skips it).
    fn parse_file(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<Option<ParsedFile>, Error> {
        match self.probe.probe(root, path)? {
            ProbeOutcome::Audio { .. } => {}
            ProbeOutcome::NoAudioTrack | ProbeOutcome::Unsupported => return Ok(None),
        }
        let hash = self.hasher.hash(root, path)?;
        let meta = self.metadata.read(root, path)?;
        let size = self.fs.file_meta(root, path)?.size;
        let mtime_ns = self.fs.file_meta(root, path)?.modified_ns;
        Ok(Some(ParsedFile {
            path: path.clone(),
            hash,
            size,
            mtime_ns,
            meta,
        }))
    }

    /// Persist a song through the shared [`UnitOfWork`] (same authority as the
    /// import/scan commit paths).
    fn persist_song(&self, song: &Song) -> Result<(), Error> {
        let song = song.clone();
        self.uow.with_tx(Box::new(move |tx| {
            tx.upsert_song(&song)?;
            Ok(())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::SongRepository;
    use crate::application::testing::scan_fixture::ScanFixture;
    use crate::domain::ids::{Revision, SongId};
    use crate::domain::library::{
        DeviceId, HybridLogicalClock, LibraryRelativePath, PortableRecord, SongRecord,
    };

    /// Seed one song record into the fixture's control plane.
    fn seed_song_record(fixture: &ScanFixture, song_uuid: uuid::Uuid) {
        let record = PortableRecord::Song(SongRecord {
            song_uuid,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::from_uuid(uuid::Uuid::new_v4()),
            hlc: HybridLogicalClock::new(1_700_000_000, 0),
            media_path: LibraryRelativePath::new("media/歌手/歌手 - 晴天.flac").unwrap(),
            content_hash: "abc123".to_owned(),
            title: Some("晴天".to_owned()),
            artist: Some("歌手".to_owned()),
            album: Some("专辑".to_owned()),
        });
        fixture
            .control
            .write_record(fixture.root, &record)
            .expect("seed song record");
    }

    #[test]
    fn restore_projects_records_and_keeps_missing_without_media() {
        let fixture = ScanFixture::new();
        let song_uuid = uuid::Uuid::new_v4();
        seed_song_record(&fixture, song_uuid);
        fixture
            .control
            .write_manifest(
                fixture.root,
                &LibraryManifest::new(
                    LibraryId::from_uuid(uuid::Uuid::new_v4()),
                    "0.1.0".to_owned(),
                ),
            )
            .expect("write manifest");

        let restore = RestoreLibrary::new(
            &fixture.control,
            fixture.deps.songs.as_ref(),
            fixture.deps.playlists.as_ref(),
            fixture.deps.fs.as_ref(),
            fixture.deps.hasher.as_ref(),
            fixture.deps.probe.as_ref(),
            fixture.deps.metadata.as_ref(),
            fixture.deps.uow.as_ref(),
            None,
        );
        let outcome = restore.run(fixture.root).expect("restore");

        // One song projected, media absent → Missing, no files reconciled.
        assert_eq!(outcome.projected_songs, 1);
        assert_eq!(outcome.media_reconciled, 0);
    }

    #[test]
    fn restore_is_idempotent_across_repeats() {
        let fixture = ScanFixture::new();
        let song_uuid = uuid::Uuid::new_v4();
        seed_song_record(&fixture, song_uuid);
        fixture
            .control
            .write_manifest(
                fixture.root,
                &LibraryManifest::new(
                    LibraryId::from_uuid(uuid::Uuid::new_v4()),
                    "0.1.0".to_owned(),
                ),
            )
            .expect("write manifest");

        let restore = RestoreLibrary::new(
            &fixture.control,
            fixture.deps.songs.as_ref(),
            fixture.deps.playlists.as_ref(),
            fixture.deps.fs.as_ref(),
            fixture.deps.hasher.as_ref(),
            fixture.deps.probe.as_ref(),
            fixture.deps.metadata.as_ref(),
            fixture.deps.uow.as_ref(),
            None,
        );
        let first = restore.run(fixture.root).expect("first restore");
        let second = restore.run(fixture.root).expect("second restore");
        assert_eq!(first.projected_songs, second.projected_songs);
        // The projection did not mint a duplicate UUID.
        let by_id =
            SongRepository::by_id(fixture.deps.songs.as_ref(), SongId::from_uuid(song_uuid))
                .expect("read back")
                .expect("present");
        assert_eq!(by_id.availability(), SongAvailability::Missing);
    }

    #[test]
    fn restore_rejects_a_conflicting_library_id() {
        let fixture = ScanFixture::new();
        fixture
            .control
            .write_manifest(
                fixture.root,
                &LibraryManifest::new(
                    LibraryId::from_uuid(uuid::Uuid::new_v4()),
                    "0.1.0".to_owned(),
                ),
            )
            .expect("write manifest");

        let restore = RestoreLibrary::new(
            &fixture.control,
            fixture.deps.songs.as_ref(),
            fixture.deps.playlists.as_ref(),
            fixture.deps.fs.as_ref(),
            fixture.deps.hasher.as_ref(),
            fixture.deps.probe.as_ref(),
            fixture.deps.metadata.as_ref(),
            fixture.deps.uow.as_ref(),
            Some(LibraryId::from_uuid(uuid::Uuid::new_v4())),
        );
        let error = restore.run(fixture.root).expect_err("conflicting id");
        assert_eq!(error.code(), "conflict");
    }
}
