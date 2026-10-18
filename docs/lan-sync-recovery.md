# LAN library sync recovery

This guide covers recovery after a host database or portable library has been restored from backup. The host owns the commit sequence, so a restored copy must not resume publishing under its old authority when its ledger may be behind.

## Before restoring

1. Stop sync on every device and quit Echo on the host.
2. Preserve a copy of the complete library root, including `echo/` and `media/`. Keep any separate Echo application-data backup too; it may contain the host ledger and device credentials.
3. Restore the library and application data from the same backup point whenever possible. Do not delete `echo/tmp/`, `echo/records/`, `echo/manifest.json`, or the sync ledger by hand.
4. Open the restored library on the host and inspect its integrity status. A checkpoint ahead of the local ledger, an unreadable checkpoint, or a same-sequence digest mismatch fences the host. Do not reconnect peers until the recovery action completes.

## Start a new host epoch

In **Settings → 局域网同步 → 从备份恢复主机**, select the intended restored library and confirm the action. It stops an active host session, archives pending and conflicting local mutations, and creates a new authority epoch with an independent genesis checkpoint. The archive receipt contains counts and identifiers, not credentials or filesystem paths. Keep the receipt with the backup record.

Then start hosting again and pair each peer with a fresh six-digit code. Peers must accept the new authority identity; they must not reuse credentials from the old epoch. Review preserved conflicts and pending edits as ordinary new changes. Tombstones, inbox receipts, and conflict evidence are retained; recovery does not garbage-collect them.

If Echo cannot read the checkpoint or portable authority evidence, leave sync disabled and preserve the full backup for diagnosis. Do not copy only `echo/` or only the SQLite database to another machine and treat that as a consistent host restore.

## Interrupted media publication

Remote media is staged and verified before it is atomically published. If the destination file is locked (for example, by a player on Windows), metadata may be present while that media remains unavailable; the status must not claim that all media is ready. Close the player or release the file handle, then resume sync. A restart resumes from the durable publication journal. Cleanup only removes redundant staging data whose journal and content hash/length prove that it belongs to a completed publication; staging with unknown provenance is left untouched for diagnosis.

## What cannot be detected

If the entire machine state, library root, and independently stored checkpoint are all rolled back together to the same older point, local evidence cannot prove that newer commits once existed. Use a backup inventory or another peer's observed checkpoint to identify that case. When in doubt, recover into a new host epoch and re-pair; never resume the old epoch from a potentially stale backup.

## Network checks

Sync is opt-in and stays disabled until a user starts hosting or pairs a peer. It uses HTTPS on the local network, port `50826`, with device identity pinned after pairing. There is no account sign-in or public relay. If discovery fails, verify both devices are on a reachable LAN, allow Echo through the operating-system local-network/firewall prompt, and enter the host's current LAN IP address with the pairing code. An IP change requires reconnecting to the pinned host identity; it does not authorize a different host. Revoke a lost device from the host before pairing a replacement.
