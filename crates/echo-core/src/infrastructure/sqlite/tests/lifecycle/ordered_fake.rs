//! A controllable-order authority: every wired entrypoint must leave the
//! projection untouched until the directory commit has succeeded.
use super::*;
use crate::application::playback_recording::record_qualified_playback;
use crate::application::ports::{
    ControlPlaneInventoryPort, ControlPlanePort, LibraryAssetPort, LibraryDurabilityPort,
    LibraryWriteSessionPort, ManifestState,
};
use crate::domain::ids::PlaybackSessionId;
use crate::domain::library::durability::{
    CommitOutcome, LibraryBatch, LibraryCommit, LibraryReplay,
};
use crate::domain::library::{
    LibraryManifest, PortableRecord, PortableRecordDocument, PortableRecordEnvelope, RecordKind,
};
use crate::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};

/// Wraps the real control plane and refuses directory commits while armed, so
/// tests can hold the directory still at any point of the commit order.
struct RefusingAuthority {
    inner: Arc<RootControlPlane>,
    refuse_commits: AtomicBool,
}

impl RefusingAuthority {
    fn directory(&self) -> &dyn LibraryDurabilityPort {
        self.inner
            .durability()
            .expect("the wrapped control plane exposes durability")
    }
}

impl ControlPlaneInventoryPort for RefusingAuthority {
    fn records_present(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.records_present(root)
    }
}

impl ControlPlanePort for RefusingAuthority {
    fn durability(&self) -> Option<&dyn LibraryDurabilityPort> {
        Some(self)
    }

    fn write_manifest(&self, root: LibraryRootId, manifest: &LibraryManifest) -> Result<(), Error> {
        self.inner.write_manifest(root, manifest)
    }

    fn read_manifest(&self, root: LibraryRootId) -> Result<Option<LibraryManifest>, Error> {
        self.inner.read_manifest(root)
    }

    fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error> {
        self.inner.write_record(root, record)
    }

    fn read_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecord>, Error> {
        self.inner.read_record(root, kind, object_uuid)
    }

    fn delete_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        self.inner.delete_record(root, kind, object_uuid)
    }

    fn list_records(&self, root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error> {
        self.inner.list_records(root, kind)
    }

    fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.control_plane_usable(root)
    }

    fn manifest_state(&self, root: LibraryRootId) -> Result<ManifestState, Error> {
        self.inner.manifest_state(root)
    }

    fn write_record_envelope(
        &self,
        root: LibraryRootId,
        record: &PortableRecordEnvelope,
    ) -> Result<(), Error> {
        self.inner.write_record_envelope(root, record)
    }

    fn read_record_document(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        self.inner.read_record_document(root, kind, object_uuid)
    }

    fn list_record_documents(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
    ) -> Result<Vec<PortableRecordDocument>, Error> {
        self.inner.list_record_documents(root, kind)
    }

    fn read_tombstone_document(
        &self,
        root: LibraryRootId,
        deleted_kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        self.inner
            .read_tombstone_document(root, deleted_kind, object_uuid)
    }

    fn delete_tombstone(
        &self,
        root: LibraryRootId,
        deleted_kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        self.inner.delete_tombstone(root, deleted_kind, object_uuid)
    }
}

impl LibraryAssetPort for RefusingAuthority {
    fn write_asset(&self, root: LibraryRootId, bytes: &[u8]) -> Result<String, Error> {
        self.directory().write_asset(root, bytes)
    }

    fn read_asset(&self, root: LibraryRootId, hash: &str) -> Result<Option<Vec<u8>>, Error> {
        self.directory().read_asset(root, hash)
    }
}

impl LibraryWriteSessionPort for RefusingAuthority {}

impl LibraryDurabilityPort for RefusingAuthority {
    fn commit(&self, root: LibraryRootId, batch: &LibraryBatch) -> Result<CommitOutcome, Error> {
        if self.refuse_commits.load(Ordering::Relaxed) {
            return Err(Error::unavailable(
                "test directory",
                "simulated commit refusal",
            ));
        }
        self.directory().commit(root, batch)
    }

    fn recover(&self, root: LibraryRootId) -> Result<LibraryReplay, Error> {
        self.directory().recover(root)
    }

    fn read_operation(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<LibraryCommit>, Error> {
        self.directory().read_operation(root, id)
    }

    fn operation_status(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<CommitOutcome>, Error> {
        self.directory().operation_status(root, id)
    }
}

/// Every user-facing table the projection owns: a failed entrypoint must not
/// change a single row count anywhere in the database.
const FINGERPRINT_TABLES: &[&str] = &[
    "songs",
    "playlists",
    "playlist_songs",
    "recorded_play_sessions",
    "sync_device_play_counts",
    "song_overrides",
    "song_lyrics",
    "sync_mutations",
    "sync_outbox",
    "sync_local_highwater",
    "sync_local_origin_cursors",
    "sync_object_versions",
    "library_recovery_records",
    "library_applied_operations",
    "library_projection_watermarks",
];

fn fingerprint(stack: &ScanStack) -> String {
    stack
        .database
        .with_reader(|c| {
            let mut out = String::new();
            for table in FINGERPRINT_TABLES {
                let count: u64 = c
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .map_err(storage)?;
                out.push_str(table);
                out.push(':');
                out.push_str(&count.to_string());
                out.push(';');
            }
            Ok(out)
        })
        .unwrap()
}

/// Assert one refused attempt surfaces the injected failure and leaves both
/// the projection tables and the directory byte-identical.
fn refuse(
    label: &str,
    stack: &ScanStack,
    control: &RootControlPlane,
    before: &str,
    directory_before: &LibraryReplay,
    attempt: impl FnOnce() -> Result<(), Error>,
) {
    let error = attempt().unwrap_err();
    assert!(
        matches!(&error, Error::Unavailable { resource, .. } if resource == "test directory"),
        "{label}: the refused directory commit must surface directly: {error:?}"
    );
    assert_eq!(
        fingerprint(stack),
        before,
        "{label} must not touch any projection table before the directory commit"
    );
    assert_eq!(
        control.durability().unwrap().recover(stack.root).unwrap(),
        *directory_before,
        "{label} must not save anything into the directory"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn library_durability_refused_commit_leaves_every_wired_entrypoint_untouched() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let authority = Arc::new(RefusingAuthority {
        inner: control.clone(),
        refuse_commits: AtomicBool::new(false),
    });
    stack.database.configure_library_control(authority.clone());
    let deps = ScanDeps {
        control: authority.clone(),
        ..deps
    };
    let device = deps.device_id.current_device_id();
    let edits = DurableLibraryEdits {
        control: authority.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device,
    };
    // Seed one song and one playlist member through the healthy authority.
    let song = super::favorite::seed_song(&stack, &control, &deps);
    stack
        .database
        .writer
        .run(move |c| {
            c.execute(
                "UPDATE songs SET availability='available' WHERE uuid=?1",
                [song.to_string()],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let playlist = edits.create_playlist(stack.root, "fake target").unwrap();
    let empty_target = edits.create_playlist(stack.root, "empty target").unwrap();
    edits
        .add_to_playlists(stack.root, song, &[playlist])
        .unwrap();
    let cover = super::playlist_cover::original_png();
    let before = fingerprint(&stack);
    let directory_before = control.durability().unwrap().recover(stack.root).unwrap();
    authority.refuse_commits.store(true, Ordering::Relaxed);
    refuse(
        "favorite",
        &stack,
        &control,
        &before,
        &directory_before,
        || edits.set_favorite(stack.root, song, true).map(|_| ()),
    );
    refuse(
        "playlist create",
        &stack,
        &control,
        &before,
        &directory_before,
        || {
            edits
                .create_playlist(stack.root, "refused creation")
                .map(|_| ())
        },
    );
    refuse(
        "playlist rename",
        &stack,
        &control,
        &before,
        &directory_before,
        || edits.rename_playlist(stack.root, playlist, "refused rename"),
    );
    refuse(
        "playlist delete",
        &stack,
        &control,
        &before,
        &directory_before,
        || edits.delete_playlist(stack.root, playlist),
    );
    refuse(
        "member append",
        &stack,
        &control,
        &before,
        &directory_before,
        || edits.add_to_playlists(stack.root, song, &[empty_target]),
    );
    refuse(
        "member removal",
        &stack,
        &control,
        &before,
        &directory_before,
        || edits.remove_playlist_song(stack.root, playlist, song),
    );
    refuse(
        "playlist cover",
        &stack,
        &control,
        &before,
        &directory_before,
        || edits.set_playlist_cover(stack.root, playlist, Some(&cover), Some("image/png")),
    );
    refuse(
        "playback recording",
        &stack,
        &control,
        &before,
        &directory_before,
        || {
            record_qualified_playback(
                authority.as_ref(),
                deps.songs.as_ref(),
                deps.uow.as_ref(),
                device,
                PlaybackSessionId::new(),
                song,
                1_700_000_000_000,
            )
            .map(|_| ())
        },
    );
    // Disarming the refusal lets the same authority commit and project again.
    authority.refuse_commits.store(false, Ordering::Relaxed);
    let created = edits
        .create_playlist(stack.root, "healthy after refusal")
        .unwrap();
    assert_eq!(
        deps.playlists.name(created).unwrap().as_deref(),
        Some("healthy after refusal")
    );
    assert_ne!(fingerprint(&stack), before);
}
