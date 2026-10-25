//! Import crash recovery against real SQLite + directory commits (task 5.1):
//! the recovered row must reuse the directory claim's reserved identity, and
//! a wiped database must rebuild that exact identity from the replay alone.
use super::*;
use crate::application::ports::{
    ImportSource, LibraryProjectionPort, OperationJournalRepository, SongRepository,
};
use crate::application::recover::RecoverOperations;
use crate::domain::library::durability::StateKind;
use crate::domain::state::OperationState;
use crate::infrastructure::sqlite::SqliteDatabase;

/// A journal that dies exactly before the import's first `publish_applied`
/// item write: the final file is already on disk, the journal is still
/// pre-commit — the state a crash between publish and journal-update leaves.
struct CrashBeforePublishApplied {
    inner: std::sync::Arc<SqliteDatabase>,
}

impl OperationJournalRepository for CrashBeforePublishApplied {
    fn ensure_operation(
        &self,
        operation: crate::domain::ids::OperationId,
        root: LibraryRootId,
        kind: &str,
        reserved_song: Option<crate::domain::ids::SongId>,
    ) -> Result<(), crate::error::Error> {
        self.inner
            .ensure_operation(operation, root, kind, reserved_song)
    }

    fn item_state(
        &self,
        operation: crate::domain::ids::OperationId,
        item: &str,
    ) -> Result<Option<crate::application::ports::OperationItem>, crate::error::Error> {
        self.inner.item_state(operation, item)
    }

    fn upsert_item(
        &self,
        operation: crate::domain::ids::OperationId,
        item: crate::application::ports::OperationItem,
    ) -> Result<(), crate::error::Error> {
        assert!(
            item.state != OperationState::PublishApplied,
            "injected crash: publish_applied before the journal write"
        );
        self.inner.upsert_item(operation, item)
    }

    fn items(
        &self,
        operation: crate::domain::ids::OperationId,
    ) -> Result<Vec<crate::application::ports::OperationItem>, crate::error::Error> {
        self.inner.items(operation)
    }

    fn incomplete_items(
        &self,
        root: LibraryRootId,
    ) -> Result<
        Vec<(
            crate::domain::ids::OperationId,
            String,
            crate::application::ports::OperationItem,
        )>,
        crate::error::Error,
    > {
        self.inner.incomplete_items(root)
    }

    fn set_undo_deadline(
        &self,
        operation: crate::domain::ids::OperationId,
        deadline_ms: i64,
    ) -> Result<(), crate::error::Error> {
        self.inner.set_undo_deadline(operation, deadline_ms)
    }

    fn undo_deadline(
        &self,
        operation: crate::domain::ids::OperationId,
    ) -> Result<Option<i64>, crate::error::Error> {
        self.inner.undo_deadline(operation)
    }

    fn release_claims(
        &self,
        operation: crate::domain::ids::OperationId,
    ) -> Result<(), crate::error::Error> {
        self.inner.release_claims(operation)
    }
}

#[test]
fn library_durability_import_recovery_rebuilds_reserved_identity_in_a_fresh_database() {
    let stack = ScanStack::new();
    let (_candidate, control, deps) = configured_roots(&stack);
    // The scan stack's filesystem only maps `stack.root`; the candidate root
    // exists for activation tests, not for imports.
    let root = stack.root;
    let audio = b"sqlite recovery rebuilds me";
    let published = "media/Artist/Artist - Rebuild.flac";
    let tags = |artist: &str, title: &str| crate::domain::media::ParsedMetadata {
        artist: Some(artist.to_owned()),
        title: Some(title.to_owned()),
        ..crate::domain::media::ParsedMetadata::default()
    };
    // Content-keyed tags for the copied bytes + path-keyed probe/tags for the
    // published target (the same wiring the scan stack's import seeds use).
    stack.metadata.set_bytes(audio, tags("Artist", "Rebuild"));
    stack.probe.set(
        published,
        crate::application::ports::ProbeOutcome::Audio {
            format: crate::domain::media::AudioFormat::Flac,
            duration: Some(std::time::Duration::from_secs(1)),
        },
    );
    stack.metadata.set(published, tags("Artist", "Rebuild"));
    let sources = crate::application::testing::FakeImportSources::new();
    sources.add("rebuild", "Rebuild.flac", audio);

    let crashing = ScanDeps {
        journal: std::sync::Arc::new(CrashBeforePublishApplied {
            inner: stack.database.clone(),
        }),
        ..deps.clone()
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::application::import::PlanImport::new(&crashing, &sources)
            .run(root, &[ImportSource::new("rebuild").unwrap()])
            .unwrap()
    }));
    assert!(
        outcome.is_err(),
        "the injected crash interrupted the import"
    );
    assert!(
        stack.library_dir.join(published).exists(),
        "the final file was published before the crash"
    );

    // Recovery on the clean dependency set converges through the real
    // directory commit boundary and the real SQLite transaction.
    RecoverOperations::new(&deps).run(root).unwrap();
    let songs = SongRepository::all_in_root(stack.database.as_ref(), root).unwrap();
    assert_eq!(songs.len(), 1, "recovery converges to one song");
    let reserved = songs[0].id();

    // The directory claim owns the ingestion identity: its reserved
    // `added_at` is the value the recovered row carries.
    let replay = control.durability().unwrap().recover(root).unwrap();
    let claim = replay
        .states()
        .find(|state| {
            state.kind == StateKind::Claim
                && state.payload.get("song_uuid").and_then(|v| v.as_str())
                    == Some(&reserved.as_uuid().to_string())
        })
        .expect("the ingestion claim is in the replay");
    let claim_added_at = claim.payload["added_at"].as_u64().unwrap();
    assert_eq!(
        songs[0].added_at(),
        claim_added_at,
        "the recovered row reuses the claim-reserved added_at"
    );

    // A wiped projection rebuilds the same identity from the directory alone.
    let (_keep, rebuilt) = fresh_database(&stack);
    rebuilt.project_library_commits(root, &replay).unwrap();
    let rebuilt_songs = SongRepository::all_in_root(&rebuilt, root).unwrap();
    assert_eq!(rebuilt_songs.len(), 1, "the rebuild projects one song");
    assert_eq!(
        rebuilt_songs[0].id(),
        reserved,
        "the rebuild restores the reserved UUID"
    );
    assert_eq!(
        rebuilt_songs[0].added_at(),
        claim_added_at,
        "the rebuild restores the claim-reserved added_at"
    );
}

#[test]
fn library_durability_claim_alone_never_becomes_a_song_in_a_fresh_database() {
    // The crash lands BEFORE the directory commit: the reservation claim is
    // durable (with its `added_at`) but no Song record was ever written. A
    // wiped projection replaying that claim alone MUST NOT fabricate a song —
    // 未提交操作不冒充成功 (safe-file-ingestion 崩溃后恢复).
    let stack = ScanStack::new();
    let (_candidate, _control, deps) = configured_roots(&stack);
    let root = stack.root;
    let audio = b"claim-only bytes";
    let published = "media/Artist/Artist - Claim.flac";
    stack.metadata.set_bytes(
        audio,
        crate::domain::media::ParsedMetadata {
            artist: Some("Artist".to_owned()),
            title: Some("Claim".to_owned()),
            ..crate::domain::media::ParsedMetadata::default()
        },
    );
    stack.probe.set(
        published,
        crate::application::ports::ProbeOutcome::Audio {
            format: crate::domain::media::AudioFormat::Flac,
            duration: Some(std::time::Duration::from_secs(1)),
        },
    );
    stack.metadata.set(
        published,
        crate::domain::media::ParsedMetadata {
            artist: Some("Artist".to_owned()),
            title: Some("Claim".to_owned()),
            ..crate::domain::media::ParsedMetadata::default()
        },
    );
    let sources = crate::application::testing::FakeImportSources::new();
    sources.add("claim", "Claim.flac", audio);
    let crashing = ScanDeps {
        journal: std::sync::Arc::new(CrashBeforePublishApplied {
            inner: stack.database.clone(),
        }),
        ..deps.clone()
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::application::import::PlanImport::new(&crashing, &sources)
            .run(root, &[ImportSource::new("claim").unwrap()])
            .unwrap()
    }));
    assert!(
        outcome.is_err(),
        "the injected crash interrupted the import"
    );
    assert!(
        stack.library_dir.join(published).exists(),
        "the audio is published but the record was never committed"
    );

    // Only the claim exists in the replay — no Song record, no commit that
    // carries one.
    let replay = deps.control.durability().unwrap().recover(root).unwrap();
    let has_claim = replay.states().any(|state| state.kind == StateKind::Claim);
    assert!(has_claim, "the reservation claim survived the crash");
    let has_record =
        replay.commits.iter().any(|commit| {
            commit.batch.records.iter().any(|envelope| {
                envelope.record().kind() == crate::domain::library::RecordKind::Song
            })
        });
    assert!(!has_record, "no committed Song record exists");

    let (_keep, rebuilt) = fresh_database(&stack);
    rebuilt.project_library_commits(root, &replay).unwrap();
    assert!(
        SongRepository::all_in_root(&rebuilt, root)
            .unwrap()
            .is_empty(),
        "a claim alone never becomes a song in the rebuilt projection"
    );
}
