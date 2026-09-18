//! `MemoryDatabase` cases: the transactional commit is visible through
//! every repository view, exactly like one SQLite handle.

use super::*;

#[test]
fn memory_database_commits_transactionally_and_is_visible_everywhere() {
    let database = MemoryDatabase::new();
    let root = LibraryRootId::new();
    let song = Song::new(
        SongId::new(),
        root,
        RelativeMediaPath::new("a.flac").unwrap(),
        Revision::INITIAL,
    );
    database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.upsert_song(&song)?;
            Ok(())
        }))
        .unwrap();
    // Visible through the repository view (shared store).
    assert_eq!(
        SongRepository::all_in_root(&database, root).unwrap().len(),
        1
    );

    database.set_fail_commit(true);
    let second = Song::new(
        SongId::new(),
        root,
        RelativeMediaPath::new("b.flac").unwrap(),
        Revision::INITIAL,
    );
    assert!(database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.upsert_song(&second)?;
            Ok(())
        }))
        .is_err());
    assert_eq!(database.songs().len(), 1, "failed commit rolled back");
}
