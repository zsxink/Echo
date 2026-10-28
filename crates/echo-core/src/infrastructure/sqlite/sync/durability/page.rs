//! Bounded capture for favorite-only replica pages with a proven directory base.
//!
//! Other object projectors can replay deferred state or rewrite relationships.
//! Keep those pages on complete rescue until they expose an explicit write set.
use super::{publication, CONFIRMED_PAYLOAD, PEER_CURSOR_PAYLOAD, RECEIPT_PAYLOAD};
use crate::application::ports::RemoteCommit;
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    ChangeOrigin, DurableStateChange, LibraryBatch, LibraryReplay, StateNamespace, StateTarget,
};
use crate::domain::library::{
    DeviceId as RecordDeviceId, FavoriteRecord, HybridLogicalClock, LibraryId, PortableRecord,
    PortableRecordEnvelope, RecordKind, SongRecord,
};
use crate::domain::sync::{CommitCheckpoint, DeviceId, ObjectKey, SyncBinding};
use crate::error::Error;
use crate::infrastructure::sqlite::{
    conversion::song_from_row, query::SONG_SELECT, support::storage,
};
use rusqlite::{params, Connection, OptionalExtension, Params};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub(crate) struct RemotePage<'a> {
    pub binding: SyncBinding,
    pub peer: DeviceId,
    pub anchor: CommitCheckpoint,
    pub commits: &'a [RemoteCommit],
}

impl RemotePage<'_> {
    pub(super) fn capture(
        &self,
        connection: &Connection,
        root: LibraryRootId,
        library: LibraryId,
        operation: Uuid,
        replay: &LibraryReplay,
    ) -> Result<Option<LibraryBatch>, Error> {
        let namespace = StateNamespace::Bound {
            binding: self.binding,
        };
        if self.commits.is_empty()
            || super::sync_namespace(connection, &root.to_string())? != Some(namespace.clone())
            || !watermark_matches(connection, root, library, replay)?
            || !self.anchor_is_covered(replay, &namespace)
            || !self.is_favorite_only(connection)?
        {
            return Ok(None);
        }
        let songs = self
            .commits
            .iter()
            .filter_map(|commit| match commit.object {
                ObjectKey::Favorite(song) => Some(song.as_uuid()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let Some(covered) = covered_songs(replay, &songs) else {
            return Ok(None);
        };
        let raw_device: String = connection
            .query_row(
                "SELECT value FROM device_state WHERE key='device_id'",
                [],
                |r| r.get(0),
            )
            .map_err(storage)?;
        let device = RecordDeviceId::from_uuid(Uuid::parse_str(&raw_device).map_err(storage)?);
        let mut batch = LibraryBatch::new(operation, library, ChangeOrigin::Remote, Vec::new());
        for id in songs {
            let Some((song, favorite)) = song_after_image(connection, root, id, device)? else {
                return Ok(None);
            };
            if covered.get(&id).copied() != Some(&song) {
                return Ok(None);
            }
            batch.records.push(PortableRecordEnvelope::new(
                PortableRecord::Favorite(favorite),
                None,
            )?);
        }
        let Some(states) = self.capture_state(connection, namespace)? else {
            return Ok(None);
        };
        batch.state = states;
        Ok(Some(batch))
    }

    fn anchor_is_covered(&self, replay: &LibraryReplay, namespace: &StateNamespace) -> bool {
        replay
            .states()
            .filter(|state| {
                state.key.as_ref().is_some_and(|key| {
                    &key.namespace == namespace
                        && key.target == StateTarget::PeerCursor { peer: self.peer }
                })
            })
            .last()
            .is_some_and(|state| {
                state.payload["schema"] == "echo.sync.peer-cursor.v1"
                    && state.payload["sequence"].as_u64() == Some(self.anchor.sequence.as_u64())
                    && state.payload["digest"].as_str()
                        == Some(
                            blake3::Hash::from_bytes(self.anchor.digest)
                                .to_hex()
                                .as_str(),
                        )
            })
    }

    fn is_favorite_only(&self, connection: &Connection) -> Result<bool, Error> {
        for commit in self.commits {
            let ObjectKey::Favorite(song) = commit.object else {
                return Ok(false);
            };
            let body: Value = serde_json::from_str(&commit.canonical_json).map_err(storage)?;
            let aliases_empty = match body.get("aliases") {
                None => true,
                Some(Value::Array(aliases)) => aliases.is_empty(),
                Some(_) => false,
            };
            if commit.is_tombstone
                || !aliases_empty
                || !super::super::manifests::from_commit_json(&commit.canonical_json)?.is_empty()
                || super::super::projection::is_tombstoned(connection, "song", &song.to_string())?
            {
                return Ok(false);
            }
            // Matching mutations may have changed disposition in this page.
            // Existing aliases also expand pending-object lookup in the projector.
            let related: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE origin_device_id=?1 AND mutation_id=?2) OR EXISTS(SELECT 1 FROM sync_song_aliases WHERE alias_song_uuid=?3 OR canonical_song_uuid=?3)",
                params![commit.origin_device_id.as_uuid().to_string(), commit.mutation_id.as_uuid().to_string(), song.to_string()],
                |row| row.get(0),
            ).map_err(storage)?;
            if related {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn capture_state(
        &self,
        connection: &Connection,
        namespace: StateNamespace,
    ) -> Result<Option<Vec<DurableStateChange>>, Error> {
        let mut states = Vec::new();
        let mut objects = BTreeSet::new();
        for commit in self.commits {
            let Some(ledger) = read_payload(
                connection,
                &format!(
                    "SELECT {} FROM sync_commits WHERE commit_seq=?1",
                    publication::LEDGER_PAYLOAD
                ),
                [commit.sequence.as_u64()],
            )?
            else {
                return Ok(None);
            };
            let ledger_namespace = StateNamespace::Bound {
                binding: super::facts::binding(&ledger)?,
            };
            if ledger_namespace != namespace {
                return Ok(None);
            }
            states.push(super::scoped_fact(ledger_namespace, "commit", ledger)?);
            let Some(receipt) = read_payload(
                connection,
                &format!(
                    "SELECT {RECEIPT_PAYLOAD} FROM sync_inbox WHERE origin_device_id=?1 AND mutation_id=?2"
                ),
                params![
                    commit.origin_device_id.as_uuid().to_string(),
                    commit.mutation_id.as_uuid().to_string()
                ],
            )?
            else {
                return Ok(None);
            };
            states.push(super::scoped_fact(namespace.clone(), "receipt", receipt)?);
            if let ObjectKey::Favorite(song) = commit.object {
                objects.insert(song.to_string());
            }
        }
        for song in objects {
            let Some(confirmed) = read_payload(
                connection,
                &format!(
                    "SELECT {CONFIRMED_PAYLOAD} FROM sync_object_versions WHERE object_type='favorite' AND object_uuid=?1"
                ),
                [song],
            )?
            else {
                return Ok(None);
            };
            states.push(super::scoped_fact(namespace.clone(), "object", confirmed)?);
        }
        let Some(cursor) = read_payload(
            connection,
            &format!("SELECT {PEER_CURSOR_PAYLOAD} FROM sync_peer_cursors WHERE peer_device_id=?1"),
            [self.peer.as_uuid().to_string()],
        )?
        else {
            return Ok(None);
        };
        states.push(super::scoped_fact(namespace, "peer", cursor)?);
        super::complete_confirmations(&mut states)?;
        if states.iter().any(|state| {
            state.kind == crate::domain::library::durability::StateKind::Confirmed
                && state.payload["schema"] != "echo.sync.confirmed.v2"
        }) {
            return Ok(None);
        }
        Ok(Some(states))
    }
}

fn watermark_matches(
    connection: &Connection,
    root: LibraryRootId,
    library: LibraryId,
    replay: &LibraryReplay,
) -> Result<bool, Error> {
    let current: Option<(String, u64, Option<String>)> = connection.query_row(
        "SELECT library_id,commit_seq,commit_digest FROM library_projection_watermarks WHERE library_root_uuid=?1",
        [root.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional().map_err(storage)?;
    Ok(match (current, replay.head()) {
        (Some((id, seq, Some(digest))), Some((head_seq, head_digest))) => {
            id == library.to_string() && seq == head_seq && digest == head_digest
        }
        _ => false,
    })
}

fn covered_songs<'a>(
    replay: &'a LibraryReplay,
    wanted: &BTreeSet<Uuid>,
) -> Option<BTreeMap<Uuid, &'a SongRecord>> {
    let mut songs = BTreeMap::new();
    for envelope in replay
        .checkpoint
        .iter()
        .flat_map(|image| &image.records)
        .chain(
            replay
                .commits
                .iter()
                .flat_map(|commit| &commit.batch.records),
        )
    {
        match envelope.record() {
            PortableRecord::Song(song) if wanted.contains(&song.song_uuid) => {
                songs.insert(song.song_uuid, song);
            }
            PortableRecord::Tombstone(tombstone)
                if wanted.contains(&tombstone.object_uuid)
                    && matches!(
                        tombstone.deleted_kind,
                        RecordKind::Song | RecordKind::Favorite
                    ) =>
            {
                return None;
            }
            _ => {}
        }
    }
    (songs.len() == wanted.len()).then_some(songs)
}

fn song_after_image(
    connection: &Connection,
    root: LibraryRootId,
    id: Uuid,
    device: RecordDeviceId,
) -> Result<Option<(SongRecord, FavoriteRecord)>, Error> {
    let Some(mut song) = connection
        .query_row(
            &format!("{SONG_SELECT} WHERE s.uuid=?1 AND s.library_root_uuid=?2"),
            params![id.to_string(), root.to_string()],
            song_from_row,
        )
        .optional()
        .map_err(storage)?
    else {
        return Ok(None);
    };
    if let Some(logical) = crate::infrastructure::sqlite::sync_manifests::published_logical_path(
        connection,
        root,
        song.path(),
    )? {
        song.relink(logical);
    }
    let (wall, counter, favorited_at): (u64, u32, Option<u64>) = connection
        .query_row(
            "SELECT hlc_wall_secs,hlc_counter,favorited_at FROM songs WHERE uuid=?1",
            [id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(storage)?;
    let hlc = HybridLogicalClock::new(wall, counter);
    let favorite = FavoriteRecord {
        song_uuid: id,
        revision: song.revision(),
        updated_by_device_id: device,
        hlc,
        is_favorite: song.favorite(),
        favorited_at,
    };
    Ok(Some((
        crate::application::portable_materialize::song_record(device, hlc, song.revision(), &song)?,
        favorite,
    )))
}

fn read_payload(
    connection: &Connection,
    sql: &str,
    parameters: impl Params,
) -> Result<Option<Value>, Error> {
    connection
        .query_row(sql, parameters, |row| row.get::<_, String>(0))
        .optional()
        .map_err(storage)?
        .map(|raw| serde_json::from_str(&raw).map_err(storage))
        .transpose()
}
