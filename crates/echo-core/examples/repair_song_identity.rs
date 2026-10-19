//! Review first; pass --enqueue only after inspecting the proof. This tool
//! appends the normal outbox and never writes song rows, aliases, or media.
use echo_core::domain::ids::SongId;
use echo_core::infrastructure::sqlite::SqliteDatabase;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !(args.len() == 5 || (args.len() == 6 && args[5] == "--enqueue")) {
        return Err("usage: repair_song_identity DATABASE OLD_UUID OLD_VERSION CANONICAL_UUID CANONICAL_VERSION [--enqueue]".into());
    }
    let db = SqliteDatabase::open(&args[0])?;
    let old = SongId::from_uuid(uuid::Uuid::parse_str(&args[1])?);
    let canonical = SongId::from_uuid(uuid::Uuid::parse_str(&args[3])?);
    let plan =
        db.prepare_song_identity_repair(old, args[2].parse()?, canonical, args[4].parse()?)?;
    println!("{}", serde_json::to_string_pretty(&plan)?);
    if args.len() == 6 {
        println!(
            "queued normal song mutation at local revision {}",
            db.enqueue_song_identity_repair(plan)?
        );
    }
    Ok(())
}
