//! Real host HTTP and scheduler; only mDNS and fixed-port startup are bypassed.

#[cfg(test)]
mod cases {
    use super::super::*;
    use crate::sync_provisioner::NativeSyncProvisioner;
    use echo_core::application::ports::{
        CoverRepository, LibraryRepository, LocalSyncQueuePort, LyricsRepository, SongRepository,
    };
    use echo_core::application::ports::{FileEvent, FileEventKind};
    use echo_core::application::remote_resources::RemoteResourcePublishDeps;
    use echo_core::application::root_switch::derive_root_id;
    use echo_core::application::scan::{ScanSupervisor, StartScan};
    use echo_core::application::watch::{EventOutcome, WatchCallbacks, WatchCoordinator};
    use echo_core::domain::entities::LibraryRoot;
    use echo_core::domain::ids::RelativeMediaPath;
    use echo_core::domain::ids::SongId;
    use echo_core::domain::sync::{
        AuthorityId, DeviceId, Epoch, LibraryId, MutationId, ObjectKey, SharedResourceKind,
        SyncBinding,
    };
    use echo_core::infrastructure::filesystem::staging::StagingMarker;
    use echo_core::infrastructure::filesystem::{Blake3ContentHasher, RootConstrainedFileSystem};
    use echo_desktop::platform::sync_security::{
        configure_local_vault, OsSecureStore, SecretBytes, SecretHandle,
    };
    use echo_sync::client::LanClient;
    use echo_sync::protocol::{MutationDto, ResourceRefDto, PROTOCOL_VERSION};
    use echo_sync::resource_transfer::{FileResourceStore, PrepareResource, ResourceTransferPort};
    use reqwest::Url;

    /// The credential vault root is a process-wide `OnceLock`, so every test in
    /// this binary must share one live path: a per-test `TempDir` would be deleted
    /// while a parallel test still writes identity and checkpoint files.
    fn configure_test_vault() {
        static VAULT: std::sync::Once = std::sync::Once::new();
        VAULT.call_once(|| {
            let root =
                std::env::temp_dir().join(format!("echo-sync-test-vault-{}", std::process::id()));
            configure_local_vault(root).unwrap();
        });
    }

    struct RunningHost {
        runtime: SyncRuntime,
        server: tokio::task::JoinHandle<std::io::Result<()>>,
    }

    impl Drop for RunningHost {
        fn drop(&mut self) {
            let _ = self.runtime.stop();
            self.server.abort();
        }
    }

    /// Preserve the fixture's actual audio frames while changing embedded tags.
    fn tagged_flac(album: &str, lyrics: &str, with_picture: bool) -> Vec<u8> {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/audio/tone-short.flac"
        ));
        let mut output = source[..4].to_vec();
        let mut offset = 4;
        loop {
            let flags = source[offset];
            let length = (usize::from(source[offset + 1]) << 16)
                | (usize::from(source[offset + 2]) << 8)
                | usize::from(source[offset + 3]);
            if !matches!(flags & 0x7f, 4 | 6) {
                output.push(flags & 0x7f);
                output.extend_from_slice(&source[offset + 1..offset + 4 + length]);
            }
            offset += 4 + length;
            if flags & 0x80 != 0 {
                break;
            }
        }
        let mut comments = Vec::new();
        comments.extend_from_slice(&0_u32.to_le_bytes()); // Empty vendor.
        comments.extend_from_slice(&4_u32.to_le_bytes());
        for comment in [
            format!("TITLE={album} title"),
            format!("ARTIST={album} artist"),
            format!("ALBUM={album}"),
            format!("LYRICS=[00:00.00]{lyrics}"),
        ] {
            comments.extend_from_slice(&u32::try_from(comment.len()).unwrap().to_le_bytes());
            comments.extend_from_slice(comment.as_bytes());
        }
        output.push(if with_picture { 4 } else { 0x84 });
        let length = u32::try_from(comments.len()).unwrap().to_be_bytes();
        output.extend_from_slice(&length[1..]);
        output.extend_from_slice(&comments);
        if with_picture {
            let png = include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../fixtures/audio/cover-256.png"
            ));
            let mut picture = Vec::new();
            picture.extend_from_slice(&3_u32.to_be_bytes()); // Front cover.
            picture.extend_from_slice(&9_u32.to_be_bytes());
            picture.extend_from_slice(b"image/png");
            for value in [0_u32, 256, 256, 24, 0, u32::try_from(png.len()).unwrap()] {
                picture.extend_from_slice(&value.to_be_bytes());
            }
            picture.extend_from_slice(png);
            output.push(0x86);
            output.extend_from_slice(&u32::try_from(picture.len()).unwrap().to_be_bytes()[1..]);
            output.extend_from_slice(&picture);
        }
        output.extend_from_slice(&source[offset..]);
        output
    }

    async fn wait_for_host_file(
        host: &RunningHost,
        database: &SqliteDatabase,
        song: SongId,
        root_path: &std::path::Path,
        bytes: &[u8],
    ) -> SyncStatus {
        let hash = blake3::hash(bytes).to_hex().to_string();
        let started = Instant::now();
        loop {
            let row = SongRepository::by_id(database, song).unwrap();
            let status = host.runtime.status();
            if let Some(row) = row {
                if row.blake3_hash() == Some(hash.as_str())
                    && row.file_mtime_ns().is_some()
                    && std::fs::read(root_path.join(row.path().normalized()))
                        .ok()
                        .as_deref()
                        == Some(bytes)
                    && status.media_ready == Some(true)
                    && status.state == SyncState::Hosting
                {
                    return status;
                }
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "accepted HTTP mutation was not published by the host loop: {:?}",
                host.runtime.status()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Role switch: a library that joined another machine's host (replica
    /// binding) must become host again without a manual unbind. This used to
    /// fail with `LibraryUnavailable` — "当前音乐目录不可用" — for a library
    /// whose directory was never the problem.
    #[test]
    fn a_replica_bound_library_can_become_host_without_a_manual_unbind() {
        let directory = tempfile::tempdir().unwrap();
        configure_test_vault();
        let app = echo_desktop::runtime::app::assemble(
            &directory.path().join("switch.sqlite"),
            &directory.path().join("covers"),
        )
        .unwrap();
        let database = app.database.clone();
        let root_path = directory.path().join("library");
        std::fs::create_dir_all(root_path.join("echo/tmp")).unwrap();
        std::fs::create_dir_all(root_path.join("media")).unwrap();
        let root = derive_root_id(&root_path.canonicalize().unwrap());
        std::fs::write(
            root_path.join("echo/tmp/.echo-ownership-marker"),
            StagingMarker::new(root).body(),
        )
        .unwrap();
        app.registry.register(root, &root_path);
        LibraryRepository::upsert(
            database.as_ref(),
            &LibraryRoot::new(root, root_path, true, true),
        )
        .unwrap();
        let replica_binding = SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        };
        database
            .bind_sync_library(replica_binding, false, root)
            .unwrap();

        let provisioner = NativeSyncProvisioner::new(
            database.clone(),
            directory.path().canonicalize().unwrap().join("resources"),
            RemoteResourcePublishDeps {
                fs: Arc::new(RootConstrainedFileSystem::new(app.registry.clone())),
                hasher: Arc::new(Blake3ContentHasher::new(app.registry)),
                journal: database.clone(),
            },
            app.deps,
            ScanSupervisor::new(),
        );
        let host = provisioner
            .host(root)
            .expect("a replica-bound library must switch to host without a manual unbind");
        assert_ne!(
            host.binding, replica_binding,
            "the role switch creates a fresh host identity"
        );
        let configuration = database.current_sync_configuration().unwrap().unwrap();
        assert!(
            configuration.host,
            "the library must be bound as host after the switch"
        );
        assert_eq!(configuration.binding, host.binding);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn http_peer_tag_update_is_published_by_the_host_loop_without_manual_sync() {
        let directory = tempfile::tempdir().unwrap();
        // The app normally configures this before constructing sync. The shared
        // test vault keeps every identity, authorization and checkpoint file
        // under one process-local path instead of this TempDir.
        configure_test_vault();
        let app = echo_desktop::runtime::app::assemble(
            &directory.path().join("host.sqlite"),
            &directory.path().join("covers"),
        )
        .unwrap();
        let database = app.database.clone();
        let root_path = directory.path().join("library");
        std::fs::create_dir_all(root_path.join("echo/tmp")).unwrap();
        std::fs::create_dir_all(root_path.join("media")).unwrap();
        let root = derive_root_id(&root_path.canonicalize().unwrap());
        std::fs::write(
            root_path.join("echo/tmp/.echo-ownership-marker"),
            StagingMarker::new(root).body(),
        )
        .unwrap();
        app.registry.register(root, &root_path);
        LibraryRepository::upsert(
            database.as_ref(),
            &LibraryRoot::new(root, root_path.clone(), true, true),
        )
        .unwrap();
        let logical_path = "media/薛之谦/薛之谦 - 骆驼.flac";
        std::fs::create_dir_all(root_path.join("media/薛之谦")).unwrap();
        std::fs::write(
            root_path.join(logical_path),
            tagged_flac("Old album", "old lyrics", false),
        )
        .unwrap();
        StartScan::new(app.deps.as_ref(), &ScanSupervisor::new())
            .run(root)
            .unwrap();
        let imported = SongRepository::all_in_root(database.as_ref(), root).unwrap();
        assert_eq!(imported.len(), 1);
        let song = imported[0].id();
        assert_eq!(imported[0].album(), Some("Old album"));
        assert!(CoverRepository::cover_of(database.as_ref(), song)
            .unwrap()
            .is_none());
        let scan_deps = app.deps.clone();
        let provisioner = Arc::new(NativeSyncProvisioner::new(
            database.clone(),
            directory.path().canonicalize().unwrap().join("resources"),
            RemoteResourcePublishDeps {
                fs: Arc::new(RootConstrainedFileSystem::new(app.registry.clone())),
                hasher: Arc::new(Blake3ContentHasher::new(app.registry)),
                journal: database.clone(),
            },
            app.deps,
            ScanSupervisor::new(),
        ));
        let initial_host = provisioner.host(root).unwrap();
        let binding = initial_host.binding;
        drop(initial_host);
        let peer = DeviceId::new();
        let token = "host-runtime-test-token-0123456789abcdef";
        let handle = SecretHandle::new(format!("test-peer-{}", peer.as_uuid())).unwrap();
        OsSecureStore
            .set(&handle, &SecretBytes::new(token.as_bytes().to_vec()))
            .unwrap();
        database
            .authorize_sync_peer(binding, peer, "Peer".into(), handle.as_str().into())
            .unwrap();
        let host = provisioner.host(root).unwrap();
        let client = LanClient::new(&host.certificate_der, token.into()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!("https://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(serve_tls(listener, host.tls, host_router(host.api)));
        let runtime = SyncRuntime::new(database.clone(), Some(provisioner));
        runtime.set_started(root, binding, SyncRole::Host, endpoint.to_string());
        let (notifier, notifications) = mpsc::channel();
        runtime.set_status_notifier(Arc::new(move |status| {
            let _ = notifier.send(status);
        }));
        runtime.start_sync_loop(SyncRole::Host).unwrap();
        let host = RunningHost { runtime, server };
        let snapshot = client.begin_snapshot(&endpoint, 64).await.unwrap();
        let remote_song = snapshot
            .objects
            .iter()
            .find(|object| object.mutation.object == ObjectKey::Song(song))
            .expect("peer receives the host-imported UUID in its snapshot");
        assert_eq!(remote_song.mutation.payload["album"], "Old album");
        let peer_previous;
        let album = "New album";
        let lyrics = "new lyrics";
        {
            let bytes = tagged_flac(album, lyrics, true);
            let resource = ResourceRefDto {
                kind: SharedResourceKind::Audio,
                content_hash: blake3::hash(&bytes).to_hex().to_string(),
                length: bytes.len() as u64,
                media_type: "audio/flac".into(),
                relative_path: logical_path.into(),
            };
            let prepared = client
                .prepare_resource(
                    &endpoint,
                    &PrepareResource {
                        binding,
                        resource: resource.clone(),
                    },
                )
                .await
                .unwrap();
            let uploaded = client
                .upload_resource_chunk(&endpoint, binding, &prepared, 0, bytes.clone())
                .await
                .unwrap();
            client
                .complete_resource(&endpoint, binding, &uploaded)
                .await
                .unwrap();
            let mutation = MutationDto {
                protocol_version: PROTOCOL_VERSION,
                binding,
                origin_device_id: peer,
                mutation_id: MutationId::new(),
                origin_seq: 1,
                previous_mutation_id: None,
                object: ObjectKey::Song(song),
                base_object_version: Some(remote_song.version),
                changed_at_ms: Some(
                    u64::try_from(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis(),
                    )
                    .unwrap(),
                ),
                resource_refs: vec![resource.clone()],
                payload: serde_json::json!({
                    "song_uuid": song,
                    "relative_path": resource.relative_path,
                    "title": format!("{album} title"),
                    "artist": format!("{album} artist"),
                    "album": album,
                    "blake3_hash": resource.content_hash,
                    "file_size": resource.length,
                    "format": "flac",
                    "added_at": 1
                }),
            };
            let ack = client.submit_mutation(&endpoint, &mutation).await.unwrap();
            peer_previous = Some(mutation.mutation_id);
            let checkpoint = ack
                .confirmation
                .checkpoint
                .expect("host accepted HTTP mutation");
            let commits = client
                .pull_commits(
                    &endpoint,
                    snapshot.checkpoint.commit_seq.as_u64(),
                    snapshot.checkpoint.digest,
                    64,
                    Duration::ZERO,
                )
                .await
                .unwrap();
            let canonical = commits
                .entries
                .iter()
                .find(|entry| entry.commit.mutation.mutation_id == mutation.mutation_id)
                .expect("accepted update is visible in the canonical host log");
            assert_eq!(canonical.commit.commit_seq, checkpoint.sequence);
            assert_eq!(canonical.commit.mutation.payload["album"], album);
            assert_eq!(
                canonical.commit.mutation.payload["title"],
                format!("{album} title")
            );
            assert_eq!(
                canonical.commit.mutation.payload["artist"],
                format!("{album} artist")
            );
            assert_eq!(
                canonical.commit.mutation.payload["blake3_hash"],
                resource.content_hash
            );
            let status = wait_for_host_file(&host, &database, song, &root_path, &bytes).await;
            std::fs::create_dir_all(directory.path().join("new-peer-cache")).unwrap();
            let download_store = Arc::new(
                FileResourceStore::new(
                    directory
                        .path()
                        .join("new-peer-cache")
                        .canonicalize()
                        .unwrap(),
                )
                .unwrap(),
            );
            let downloaded = client
                .download_resource(
                    &endpoint,
                    DeviceId::new(),
                    &PrepareResource {
                        binding,
                        resource: resource.clone(),
                    },
                    download_store.clone(),
                )
                .await
                .unwrap();
            assert!(downloaded.complete);
            download_store.verify_complete(binding, &resource).unwrap();
            assert_eq!(
                download_store
                    .download(binding, &resource.content_hash, 0, 64)
                    .unwrap()
                    .bytes,
                bytes[..64]
            );

            assert_eq!(status.last_error, None);
            assert_eq!(status.state, SyncState::Hosting);
            let updated_song = SongRepository::by_id(database.as_ref(), song)
                .unwrap()
                .unwrap();
            assert_eq!(
                updated_song.title(),
                Some(format!("{album} title").as_str())
            );
            assert_eq!(
                updated_song.artist(),
                Some(format!("{album} artist").as_str())
            );
            assert_eq!(
                updated_song.album(),
                Some(album),
                "the host applies the replica's shared metadata"
            );
            let candidates = LyricsRepository::candidates(database.as_ref(), song).unwrap();
            assert!(
                candidates
                    .iter()
                    .any(|candidate| candidate.raw_text().contains(lyrics)),
                "{candidates:?}"
            );
            assert!(
                CoverRepository::cover_of(database.as_ref(), song)
                    .unwrap()
                    .is_some(),
                "new embedded cover is indexed after automatic publication"
            );
        }
        StartScan::new(scan_deps.as_ref(), &ScanSupervisor::new())
            .run(root)
            .unwrap();
        assert_eq!(
            SongRepository::by_id(database.as_ref(), song)
                .unwrap()
                .unwrap()
                .availability(),
            echo_core::domain::entities::SongAvailability::Available,
            "a full scan must retain the published current version"
        );
        assert_eq!(
            SongRepository::all_in_root(database.as_ref(), root)
                .unwrap()
                .len(),
            1
        );
        // A later external tag edit to the active version follows the same UUID
        // and shared media path; retained originals and historical Echo versions
        // do not become new imports.
        host.runtime.stop().unwrap();
        let current = SongRepository::by_id(database.as_ref(), song)
            .unwrap()
            .unwrap();
        let edited = tagged_flac("Edited again", "edited again lyrics", true);
        std::fs::write(root_path.join(current.path().normalized()), &edited).unwrap();
        let rescanned = StartScan::new(scan_deps.as_ref(), &ScanSupervisor::new())
            .run(root)
            .unwrap();
        assert_eq!(rescanned.progress.created, 0);
        assert_eq!(rescanned.progress.missing, 0);
        assert_eq!(rescanned.progress.failed, 0);
        let updated = SongRepository::by_id(database.as_ref(), song)
            .unwrap()
            .unwrap();
        assert_eq!(updated.album(), Some("Edited again"));
        assert_eq!(
            updated.blake3_hash(),
            Some(blake3::hash(&edited).to_hex().as_str())
        );
        assert_eq!(updated.path(), current.path());
        let pending = LocalSyncQueuePort::pending_local_mutations(database.as_ref(), 64).unwrap();
        let updated_mutation = pending
            .iter()
            .find(|mutation| {
                mutation.object == ObjectKey::Song(song)
                    && mutation.payload["album"] == "Edited again"
            })
            .expect("external edit queues same UUID");
        assert_eq!(updated_mutation.payload["relative_path"], logical_path);
        assert_eq!(
            updated_mutation.resource_refs[0].relative_path.as_str(),
            logical_path
        );
        assert!(LyricsRepository::candidates(database.as_ref(), song)
            .unwrap()
            .iter()
            .any(|candidate| candidate.raw_text().contains("edited again lyrics")));
        let final_scan = StartScan::new(scan_deps.as_ref(), &ScanSupervisor::new())
            .run(root)
            .unwrap();
        assert_eq!(final_scan.progress.created, 0);
        assert_eq!(final_scan.progress.missing, 0);
        assert_eq!(
            SongRepository::all_in_root(database.as_ref(), root)
                .unwrap()
                .len(),
            1
        );
        let watcher = WatchCoordinator::new(
            scan_deps.clone(),
            Arc::new(ScanSupervisor::new()),
            WatchCallbacks::noop(),
        );
        let watched_bytes = tagged_flac("Watcher edit", "watcher lyrics", true);
        std::fs::write(root_path.join(current.path().normalized()), &watched_bytes).unwrap();
        let outcome = watcher
            .handle_event(FileEvent {
                root,
                path: current.path().clone(),
                kind: FileEventKind::Modified,
            })
            .unwrap();
        assert_eq!(
            outcome,
            EventOutcome::Applied {
                song,
                created: false
            }
        );
        assert_eq!(
            SongRepository::by_id(database.as_ref(), song)
                .unwrap()
                .unwrap()
                .album(),
            Some("Watcher edit")
        );
        let pending = LocalSyncQueuePort::pending_local_mutations(database.as_ref(), 64).unwrap();
        let queued_edit = pending
            .iter()
            .find(|mutation| {
                mutation.object == ObjectKey::Song(song)
                    && mutation.payload["album"] == "Watcher edit"
            })
            .unwrap();
        assert_eq!(queued_edit.payload["relative_path"], logical_path);
        assert_eq!(
            queued_edit.resource_refs[0].relative_path.as_str(),
            logical_path
        );
        assert_eq!(
            watcher
                .handle_event(FileEvent {
                    root,
                    path: RelativeMediaPath::new(logical_path).unwrap(),
                    kind: FileEventKind::Modified
                })
                .unwrap(),
            EventOutcome::Ignored
        );
        let arbitrary = "echo/media/unassociated.flac";
        std::fs::write(root_path.join(arbitrary), &watched_bytes).unwrap();
        assert_eq!(
            watcher
                .handle_event(FileEvent {
                    root,
                    path: RelativeMediaPath::new(arbitrary).unwrap(),
                    kind: FileEventKind::Created
                })
                .unwrap(),
            EventOutcome::Ignored
        );
        assert_eq!(
            SongRepository::all_in_root(database.as_ref(), root)
                .unwrap()
                .len(),
            1
        );
        let host_manifest = queued_edit.resource_refs[0].clone();
        host.runtime
            .set_started(root, binding, SyncRole::Host, endpoint.to_string());
        host.runtime.start_sync_loop(SyncRole::Host).unwrap();
        let started = Instant::now();
        while database
            .local_remote_resource_path(root, ObjectKey::Song(song), &host_manifest)
            .unwrap()
            .is_none()
        {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "host local edit did not advance its publication association: {:?}",
                host.runtime.status()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let latest = client.begin_snapshot(&endpoint, 64).await.unwrap();
        let version = latest
            .objects
            .iter()
            .find(|object| object.mutation.object == ObjectKey::Song(song))
            .unwrap();
        assert_eq!(version.mutation.payload["album"], "Watcher edit");
        let final_bytes = tagged_flac("Peer returns again", "peer returned lyrics", true);
        let resource = ResourceRefDto {
            kind: SharedResourceKind::Audio,
            content_hash: blake3::hash(&final_bytes).to_hex().to_string(),
            length: final_bytes.len() as u64,
            media_type: "audio/flac".into(),
            relative_path: logical_path.into(),
        };
        let prepared = client
            .prepare_resource(
                &endpoint,
                &PrepareResource {
                    binding,
                    resource: resource.clone(),
                },
            )
            .await
            .unwrap();
        let uploaded = client
            .upload_resource_chunk(&endpoint, binding, &prepared, 0, final_bytes.clone())
            .await
            .unwrap();
        client
            .complete_resource(&endpoint, binding, &uploaded)
            .await
            .unwrap();
        let mut payload = version.mutation.payload.clone();
        payload["album"] = "Peer returns again".into();
        payload["blake3_hash"] = resource.content_hash.clone().into();
        payload["file_size"] = resource.length.into();
        let mutation = MutationDto {
            protocol_version: PROTOCOL_VERSION,
            binding,
            origin_device_id: peer,
            mutation_id: MutationId::new(),
            origin_seq: 2,
            previous_mutation_id: peer_previous,
            object: ObjectKey::Song(song),
            base_object_version: Some(version.version),
            changed_at_ms: Some(
                u64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis(),
                )
                .unwrap()
                    + 1,
            ),
            resource_refs: vec![resource],
            payload,
        };
        client.submit_mutation(&endpoint, &mutation).await.unwrap();
        wait_for_host_file(&host, &database, song, &root_path, &final_bytes).await;
        assert_eq!(
            SongRepository::by_id(database.as_ref(), song)
                .unwrap()
                .unwrap()
                .album(),
            Some("Peer returns again")
        );
        assert!(LyricsRepository::candidates(database.as_ref(), song)
            .unwrap()
            .iter()
            .any(|candidate| candidate.raw_text().contains("peer returned lyrics")));

        // A distinct local song can legitimately own the same shared media path
        // as an isolated remote song; retaining it must never hide that owner.
        host.runtime.stop().unwrap();
        let independent_id = SongId::new();
        let independent = echo_core::domain::entities::Song::new(
            independent_id,
            root,
            RelativeMediaPath::new(logical_path).unwrap(),
            echo_core::domain::ids::Revision::INITIAL,
        );
        SongRepository::upsert(database.as_ref(), &independent).unwrap();
        let preserved = StartScan::new(scan_deps.as_ref(), &ScanSupervisor::new())
            .run(root)
            .unwrap();
        assert_eq!(preserved.progress.created, 0);
        assert_eq!(preserved.progress.missing, 0);
        let songs = SongRepository::all_in_root(database.as_ref(), root).unwrap();
        assert_eq!(songs.len(), 2);
        assert!(songs
            .iter()
            .all(|song| song.availability()
                == echo_core::domain::entities::SongAvailability::Available));
        assert!(notifications
            .try_iter()
            .any(|status| status.catalog_changed));
        assert!(!LyricsRepository::candidates(database.as_ref(), song)
            .unwrap()
            .is_empty());
    }
}
