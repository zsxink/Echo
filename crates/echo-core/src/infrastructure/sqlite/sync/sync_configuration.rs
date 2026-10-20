//! Read-only sync provisioning facts for native host composition.

#![allow(clippy::missing_errors_doc)]

use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::domain::ids::LibraryRootId;
use crate::domain::sync::{
    AuthorityId, CommitCheckpoint, CommitSeq, DeviceId, Epoch, LibraryId, SyncBinding,
};
use crate::error::Error;

use super::super::support::storage;
use super::super::SqliteDatabase;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyncConfiguration {
    pub binding: SyncBinding,
    pub host: bool,
    pub local_root: LibraryRootId,
    pub local_device: DeviceId,
}

impl SqliteDatabase {
    /// Read the current library binding and stable local device identity.
    pub fn current_sync_configuration(&self) -> Result<Option<SyncConfiguration>, Error> {
        self.with_reader(|connection| {
            let row: Option<(String, String, i64, String, Option<String>, String)> = connection
                .query_row(
                    "SELECT b.library_id,b.authority_id,b.epoch,b.role,b.local_root_uuid,d.value FROM sync_binding b JOIN device_state d ON d.key='device_id' WHERE b.singleton=1",
                    [],
                    |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
                )
                .optional()
                .map_err(storage)?;
            row.map(|(library, authority, epoch, role, root, device)| {
                let parse = |value: &str, field: &str| {
                    Uuid::parse_str(value).map_err(|_| Error::InvariantViolation {
                        why: format!("invalid {field} in sync configuration"),
                    })
                };
                let root = root.ok_or_else(|| Error::conflict("sync binding has no local root"))?;
                Ok(SyncConfiguration {
                    binding: SyncBinding {
                        library_id: LibraryId::from_uuid(parse(&library, "library id")?),
                        authority_id: AuthorityId::from_uuid(parse(&authority, "authority id")?),
                        epoch: Epoch::from_u64(u64::try_from(epoch).map_err(storage)?),
                    },
                    host: match role.as_str() {
                        "host" => true,
                        "replica" => false,
                        _ => return Err(Error::InvariantViolation { why: "invalid sync role".to_owned() }),
                    },
                    local_root: LibraryRootId::from_uuid(parse(&root, "local root id")?),
                    local_device: DeviceId::from_uuid(parse(&device, "device id")?),
                })
            })
            .transpose()
        })
    }

    /// Return active peer identities and opaque secure-store handles for a host.
    pub fn authorized_sync_peer_handles(&self) -> Result<Vec<(DeviceId, String)>, Error> {
        self.with_reader(|connection| {
            let binding: Option<String> = connection
                .query_row(
                    "SELECT library_id FROM sync_binding WHERE singleton=1 AND role='host'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            let Some(library) = binding else {
                return Ok(Vec::new());
            };
            let mut statement = connection
                .prepare("SELECT device_id,credential_handle FROM sync_peers WHERE library_id=?1 AND revoked_at IS NULL ORDER BY authorized_at,device_id")
                .map_err(storage)?;
            let rows = statement
                .query_map(params![library], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(storage)?;
            rows.map(|row| {
                let (device, handle) = row.map_err(storage)?;
                let device = Uuid::parse_str(&device).map_err(storage)?;
                Ok((DeviceId::from_uuid(device), handle))
            })
            .collect()
        })
    }

    /// Return display names and identities for currently authorized peers.
    pub fn authorized_sync_peers(&self) -> Result<Vec<(DeviceId, String)>, Error> {
        self.with_reader(|connection| {
            let library: Option<String> = connection
                .query_row(
                    "SELECT library_id FROM sync_binding WHERE singleton=1 AND role='host'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            let Some(library) = library else {
                return Ok(Vec::new());
            };
            let mut statement = connection
                .prepare("SELECT device_id,display_name FROM sync_peers WHERE library_id=?1 AND revoked_at IS NULL ORDER BY authorized_at,device_id")
                .map_err(storage)?;
            let rows = statement
                .query_map(params![library], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(storage)?;
            rows.map(|row| {
                let (device, name) = row.map_err(storage)?;
                let device = Uuid::parse_str(&device).map_err(storage)?;
                Ok((DeviceId::from_uuid(device), name))
            })
            .collect()
        })
    }

    /// Read the newest published log checkpoint for this exact host binding.
    /// Pending commits are intentionally excluded; startup recovery repairs
    /// them before the host starts serving.
    pub fn latest_published_sync_checkpoint(
        &self,
        binding: SyncBinding,
    ) -> Result<CommitCheckpoint, Error> {
        self.with_reader(|connection| {
            let row: Option<(i64, Vec<u8>)> = connection
                .query_row(
                    "SELECT commit_seq,commit_digest FROM sync_commits WHERE library_id=?1 AND authority_id=?2 AND epoch=?3 AND publication_state='published' ORDER BY commit_seq DESC LIMIT 1",
                    params![binding.library_id.as_uuid().to_string(),binding.authority_id.as_uuid().to_string(),i64::try_from(binding.epoch.as_u64()).map_err(storage)?],
                    |row| Ok((row.get(0)?,row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            match row {
                Some((sequence, digest)) => Ok(CommitCheckpoint {
                    binding,
                    sequence: CommitSeq::from_u64(u64::try_from(sequence).map_err(storage)?),
                    digest: digest.try_into().map_err(|_| Error::InvariantViolation {
                        why: "invalid published sync digest length".to_owned(),
                    })?,
                }),
                None => Ok(CommitCheckpoint {
                    binding,
                    sequence: CommitSeq::GENESIS,
                    digest: [0; 32],
                }),
            }
        })
    }
}
