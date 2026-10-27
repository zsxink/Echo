//! Freeze page boundaries that fit both transport and replica apply budgets.

use rusqlite::{Connection, OptionalExtension, params};

use crate::application::ports::{HostSnapshotRecord, HostSnapshotSession};
use crate::error::{Error, Subject};

use super::storage;

const MAX_PAGE_ITEMS: usize = 512;
pub const MAX_PAGE_BYTES: usize = 16 * 1024 * 1024;
// The replica envelope and wire envelope contain only fixed-size identities,
// digest and numeric cursors. Reserve for their longest legal representations.
const ENVELOPE_BYTES: usize = 1024;

fn record_budget(record: &HostSnapshotRecord) -> Result<(usize, usize), Error> {
    let resources = match record {
        HostSnapshotRecord::Object { resources, .. } => resources.len(),
        _ => 0,
    };
    let encoded = serde_json::to_vec(record).map_err(storage)?.len();
    // Core's representation already includes the complete payload and resource
    // strings. Wire adds protocol/binding fields to a mutation and uses typed
    // alias wrappers; 512 bytes covers these fixed fields. Resource hashes use
    // hex instead of byte arrays: reserve 64 additional bytes per reference for
    // even an all-zero digest (the shortest array encoding) and renamed keys.
    // The extra byte covers array commas in either representation.
    Ok((1 + resources, encoded + 512 + resources * 64 + 1))
}

pub(super) fn persist_pages(
    connection: &Connection,
    session: &HostSnapshotSession,
    records: &[HostSnapshotRecord],
) -> Result<(), Error> {
    let mut insert = connection.prepare(
        "INSERT INTO sync_snapshot_pages(token,page_index,record_offset,record_count) VALUES(?1,?2,?3,?4)",
    ).map_err(storage)?;
    let mut page_index = 0u32;
    let mut offset = 0usize;
    let mut count = 0usize;
    let mut items = 0usize;
    let mut bytes = ENVELOPE_BYTES;
    for record in records {
        let (record_items, record_bytes) = record_budget(record)?;
        if record_items > MAX_PAGE_ITEMS || ENVELOPE_BYTES + record_bytes > MAX_PAGE_BYTES {
            return Err(Error::conflict("snapshot record exceeds page budget"));
        }
        if count > 0
            && (count == session.page_size as usize
                || items + record_items > MAX_PAGE_ITEMS
                || bytes + record_bytes > MAX_PAGE_BYTES)
        {
            insert
                .execute(params![
                    session.token,
                    page_index,
                    i64::try_from(offset).map_err(storage)?,
                    count
                ])
                .map_err(storage)?;
            page_index = page_index
                .checked_add(1)
                .ok_or_else(|| Error::conflict("snapshot page index exhausted"))?;
            offset += count;
            count = 0;
            items = 0;
            bytes = ENVELOPE_BYTES;
        }
        count += 1;
        items += record_items;
        bytes += record_bytes;
    }
    // Empty libraries still have one terminal page at index zero.
    insert
        .execute(params![
            session.token,
            page_index,
            i64::try_from(offset).map_err(storage)?,
            count
        ])
        .map_err(storage)?;
    Ok(())
}

pub(super) fn load_page(
    connection: &Connection,
    session: &HostSnapshotSession,
    page_index: u32,
) -> Result<(u64, u64), Error> {
    let (offset, count): (u64, u64) = connection.query_row(
        "SELECT record_offset,record_count FROM sync_snapshot_pages WHERE token=?1 AND page_index=?2",
        params![session.token, page_index], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(storage)?.ok_or_else(|| Error::validation(
        Subject::Other, "snapshot page", "page is outside the frozen snapshot",
    ))?;
    if offset > session.record_count
        || count > u64::from(session.page_size)
        || count > session.record_count - offset
        || (count == 0 && (page_index != 0 || session.record_count != 0))
    {
        return Err(Error::conflict("snapshot page boundary is invalid"));
    }
    Ok((offset, count))
}
