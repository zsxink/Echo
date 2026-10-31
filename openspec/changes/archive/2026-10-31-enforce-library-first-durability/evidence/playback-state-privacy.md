# Playback state privacy (2026-10-24)

Task 2.5 is verified independently of the unfinished production recorder/error
flows in 6.3 and the Application coordinator in 4.2.

`PlayStatsRecord` contains per-device monotone buckets and its portable version
metadata. Session idempotence is a separate `PlaybackEvent` in directory-private
state. The native filesystem test
`library_durability_playback_event_is_materialized_outside_shared_records`
reads the actual `echo/state/playback-events/<id>.json` and checks its complete
value, then reads and round-trips the shared statistics without that identity.

The existing real SQLite producer/host test
`recording_a_play_enqueues_a_complete_play_stats_record_the_host_accepts`
now checks the actual queued cross-device payload for absence of the session UUID
and session fields, round-trips the `PortableRecord`, and verifies that private
playback export preserves the original event ID and the playback-events
namespace. The host still accepts the unchanged complete statistics payload.

Actual main-agent runs:

- `cargo test -p echo-core library_durability --lib`: 60 passed, 0 failed,
  29.04 s, including native event-file verification and the existing Core
  saved-event/retry privacy test.
- `cargo test -p echo-core recording_a_play_enqueues_a_complete_play_stats_record_the_host_accepts --lib`:
  1 passed, 0 failed, 0.07 s.
- Before the final fence/privacy additions, `cargo test -p echo-core --lib`:
  726 passed, 0 failed, 1 ignored, 33.08 s. Final whole-workspace validation is
  recorded separately; this earlier full result is not substituted for it.

No session fields were added to a shared or network DTO. These tests do not
certify complete production playback retry, ACK/reinstall behavior or trusted
session/adoption recovery.
