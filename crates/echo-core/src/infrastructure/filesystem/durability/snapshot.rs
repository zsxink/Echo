//! Content-addressed snapshot leaves keep durable roots independent of history.
//! Only obsolete leaves are collected; user resources and operation receipts stay held.
use super::{decode, invalid, io, portable, Hook, LIMIT};
use crate::domain::ids::LibraryRootId;
use crate::error::Error;
use crate::infrastructure::filesystem::RootRegistry;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const DIRECTORY: &str = "echo/state/checkpoint-chunks";
const LEAF_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    shards: BTreeMap<String, String>,
    entries: usize,
}

fn bucket(key: &str) -> String {
    blake3::hash(key.as_bytes()).to_hex()[..2].to_owned()
}
fn relative(digest: &str) -> Result<String, Error> {
    if !crate::domain::library::durability::valid_digest(digest) {
        return Err(invalid("invalid snapshot leaf digest"));
    }
    Ok(format!("{DIRECTORY}/{digest}.json"))
}

fn owned_temporary(name: &str) -> bool {
    name.split_once(".tmp-").is_some_and(|(digest, suffix)| {
        crate::domain::library::durability::valid_digest(digest)
            && suffix
                .parse::<uuid::Uuid>()
                .is_ok_and(|id| id.to_string() == suffix)
    })
}

fn leaves<T: Serialize>(values: &BTreeMap<String, T>) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let mut shards = BTreeMap::<String, BTreeMap<&str, &T>>::new();
    for (key, value) in values {
        shards.entry(bucket(key)).or_default().insert(key, value);
    }
    let mut total = 0_usize;
    let mut result = Vec::with_capacity(shards.len());
    for (prefix, values) in shards {
        let bytes = portable::encode(&values, LEAF_LIMIT)
            .map_err(|_| invalid("snapshot leaf safety limit exceeded"))?;
        total = total.saturating_add(bytes.len());
        if total > LIMIT {
            return Err(invalid("snapshot aggregate safety limit exceeded"));
        }
        result.push((prefix, bytes));
    }
    Ok(result)
}

pub(super) fn preflight<T: Serialize>(values: &BTreeMap<String, T>) -> Result<(), Error> {
    leaves(values).map(|_| ())
}

pub(super) fn publish<T: Serialize>(
    registry: &RootRegistry,
    root: LibraryRootId,
    values: &BTreeMap<String, T>,
    hook: &mut Hook<'_>,
) -> Result<Snapshot, Error> {
    publish_reusing(registry, root, values, None, hook)
}

/// Reuse only leaves referenced by a verified materialization checkpoint, or a
/// checkpoint published successfully in this writer call. Those references can
/// appear only after each leaf completed its file and ancestor-directory syncs.
/// Merely finding an immutable leaf on disk is insufficient after interruption.
pub(super) fn publish_reusing<T: Serialize>(
    registry: &RootRegistry,
    root: LibraryRootId,
    values: &BTreeMap<String, T>,
    durable: Option<&Snapshot>,
    hook: &mut Hook<'_>,
) -> Result<Snapshot, Error> {
    let mut snapshot = Snapshot {
        shards: BTreeMap::new(),
        entries: values.len(),
    };
    for (prefix, bytes) in leaves(values)? {
        let digest = blake3::hash(&bytes).to_hex().to_string();
        let path = relative(&digest)?;
        if let Some(existing) = portable::read(registry, root, &path, LEAF_LIMIT)? {
            if existing.as_bytes() != bytes {
                return Err(invalid("immutable snapshot leaf collision"));
            }
            if durable.and_then(|snapshot| snapshot.shards.get(&prefix)) != Some(&digest) {
                io::flush_existing(registry, root, &path, "snapshot_retry", hook)?;
            }
        } else {
            io::publish(registry, root, &path, &bytes, true, "snapshot", hook)?;
        }
        snapshot.shards.insert(prefix, digest);
    }
    Ok(snapshot)
}

pub(super) fn load<T: DeserializeOwned>(
    registry: &RootRegistry,
    root: LibraryRootId,
    snapshot: &Snapshot,
) -> Result<BTreeMap<String, T>, Error> {
    if snapshot.shards.len() > 256 {
        return Err(invalid("snapshot fanout safety limit exceeded"));
    }
    let mut result = BTreeMap::new();
    let mut total = 0_usize;
    for (prefix, digest) in &snapshot.shards {
        if prefix.len() != 2 || !prefix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid("invalid snapshot shard prefix"));
        }
        let bytes = portable::read(registry, root, &relative(digest)?, LEAF_LIMIT)?
            .ok_or_else(|| invalid("snapshot leaf is missing"))?;
        total = total.saturating_add(bytes.len());
        if total > LIMIT || blake3::hash(bytes.as_bytes()).to_hex().as_str() != digest {
            return Err(invalid(
                "snapshot leaf checksum or aggregate limit mismatch",
            ));
        }
        let values: BTreeMap<String, T> = decode(&bytes)?;
        if values.is_empty() || values.keys().any(|key| bucket(key) != *prefix) {
            return Err(invalid("snapshot leaf has an invalid key partition"));
        }
        for (key, value) in values {
            if result.insert(key, value).is_some() {
                return Err(invalid("duplicate snapshot key"));
            }
        }
    }
    if result.len() != snapshot.entries {
        return Err(invalid("snapshot entry count mismatch"));
    }
    Ok(result)
}

/// The two current durable roots are the complete graph for snapshot leaves.
/// Their full historical facts continue to hold all user resources and receipts.
pub(super) fn collect(
    registry: &RootRegistry,
    root: LibraryRootId,
    roots: &[(&str, &[&Snapshot])],
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let mut protected = BTreeSet::new();
    let mut directories = io::DirectorySyncSet::default();
    for (path, snapshots) in roots {
        let path = io::flush_file(registry, root, path, "snapshot_root_gc", hook)?;
        directories.include_ancestors(
            registry,
            root,
            path.parent()
                .ok_or_else(|| invalid("snapshot root parent missing"))?,
        )?;
        for snapshot in *snapshots {
            for digest in snapshot.shards.values() {
                if protected.insert(digest.clone()) {
                    let path = io::flush_file(
                        registry,
                        root,
                        &relative(digest)?,
                        "snapshot_leaf_gc",
                        hook,
                    )?;
                    directories.include_ancestors(
                        registry,
                        root,
                        path.parent()
                            .ok_or_else(|| invalid("snapshot leaf parent missing"))?,
                    )?;
                }
            }
        }
    }
    directories.sync(registry, root, "snapshot_proof_gc", hook)?;
    let folder = portable::path(registry, root, DIRECTORY)?;
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::io("list snapshot leaves", error, &folder)),
    };
    // Validate every name before deleting anything; unknown evidence is retained.
    let mut obsolete = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| Error::io("read snapshot leaf", error, &folder))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("invalid snapshot leaf name"))?;
        let Some(digest) = name.strip_suffix(".json") else {
            // A real crash bypasses publish's temporary cleanup. This exact
            // owned name may contain the only prepared bytes: retain it without
            // letting it block later committed snapshot collection.
            if owned_temporary(&name) {
                continue;
            }
            return Err(invalid("unknown snapshot evidence"));
        };
        let path = portable::path(registry, root, &relative(digest)?)?;
        if !protected.contains(digest) {
            let bytes = portable::read(registry, root, &relative(digest)?, LEAF_LIMIT)?
                .ok_or_else(|| invalid("snapshot evidence disappeared"))?;
            if blake3::hash(bytes.as_bytes()).to_hex().as_str() != digest {
                return Err(invalid("corrupt unreferenced snapshot evidence"));
            }
            obsolete.push(path);
        }
    }
    for (index, path) in obsolete.iter().enumerate() {
        std::fs::remove_file(path)
            .map_err(|error| Error::io("collect obsolete snapshot leaf", error, path))?;
        hook("snapshot_gc:remove", index)?;
    }
    if !obsolete.is_empty() {
        io::sync_ancestors(registry, root, &folder, "snapshot_gc", hook)?;
    }
    Ok(())
}
