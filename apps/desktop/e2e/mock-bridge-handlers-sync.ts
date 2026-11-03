/**
 * The mock's LAN-synchronization command handlers.
 *
 * Host/peer lifecycle, pairing codes, the sync status the UI polls, and the
 * storage-space / recovery surfaces. The mock keeps a small in-memory
 * `SyncStatus` so the host→peer transitions a scenario walks through are
 * observable; the real transport is Core's and is covered by the Rust suite.
 */
import type {
  SyncRole,
  SyncSessionIntent,
  SyncState,
  SyncStatus,
} from "../src/ipc/ipc-types.generated";
import type { E2EState } from "./mock-bridge-state";
import type { Command, Handler } from "./mock-bridge-commands";

/** Build the LAN-sync handler table for the mock backend. */
export function buildSyncHandlers(state: E2EState): Partial<Record<Command, Handler>> {
  let syncStatus: SyncStatus = {
    enabled: false,
    role: "none" as SyncRole,
    state: "disabled" as SyncState,
    sessionId: 0,
    eventSequence: 0,
    catalogChanged: false,
    libraryId: null as string | null,
    localRoot: null,
    address: null as string | null,
    lastError: null as string | null,
    pendingChanges: null as number | null,
    pendingUploads: null as number | null,
    pendingDownloads: null as number | null,
    uploadedBytes: null as number | null,
    totalUploadBytes: null as number | null,
    downloadedBytes: null as number | null,
    totalDownloadBytes: null as number | null,
    mediaReady: null as boolean | null,
    sessionIntent: "stopped" as SyncSessionIntent,
    restoreAttempts: null as number | null,
    restoreExhausted: false,
  };
  let pairingCode: string | null = null;
  let pairingCounter = 0;
  const syncError = (messageKey: string) => ({ code: "unavailable", messageKey, retryable: true });

  return {
    sync_status: () => syncStatus,
    sync_storage_space: ({ libraryRoot }) => ({
      libraryRoot: String(libraryRoot),
      availableBytes: null,
    }),
    recover_restored_host: () => ({
      recoveryId: "recovery-1",
      localRoot: "root-1",
      previousBinding: { libraryId: "library-1", authorityId: "host-1", epoch: 1 },
      replacementBinding: { libraryId: "library-1", authorityId: "host-2", epoch: 2 },
      pendingCount: 0,
      conflictCount: 0,
    }),
    sync_authorized_peers: () => [],
    local_lan_addresses: () => ["192.168.1.20"],
    host_start: ({ address }) => {
      syncStatus = {
        ...syncStatus,
        enabled: true,
        role: "host",
        state: "hosting",
        sessionId: syncStatus.sessionId + 1,
        eventSequence: syncStatus.eventSequence + 1,
        address: String(address),
        libraryId: state.activeRoot || null,
        localRoot: state.activeRoot || null,
      };
      return syncStatus;
    },
    host_stop: () => {
      syncStatus = {
        ...syncStatus,
        enabled: false,
        role: "none",
        state: "disabled",
        eventSequence: syncStatus.eventSequence + 1,
        address: null,
      };
      pairingCode = null;
      return syncStatus;
    },
    sync_leave: () => {
      syncStatus = {
        ...syncStatus,
        enabled: false,
        role: "none",
        state: "disabled",
        eventSequence: syncStatus.eventSequence + 1,
        address: null,
        libraryId: null,
      };
      pairingCode = null;
      return syncStatus;
    },
    sync_peer_start: () => syncError("sync.unconfigured"),
    sync_pause: () => {
      syncStatus = { ...syncStatus, state: "paused", eventSequence: syncStatus.eventSequence + 1 };
      return syncStatus;
    },
    sync_resume: () => {
      syncStatus = {
        ...syncStatus,
        state: syncStatus.role === "host" ? "hosting" : "connected",
        eventSequence: syncStatus.eventSequence + 1,
      };
      return syncStatus;
    },
    sync_now: () => {
      syncStatus = { ...syncStatus, state: "syncing", eventSequence: syncStatus.eventSequence + 1 };
      return syncStatus;
    },
    sync_restore_retry: () => {
      syncStatus = {
        ...syncStatus,
        restoreAttempts: null,
        restoreExhausted: false,
        state: syncStatus.enabled ? syncStatus.state : "disabled",
        eventSequence: syncStatus.eventSequence + 1,
      };
      return syncStatus;
    },
    revoke_device: () => ({
      code: "unavailable",
      messageKey: "sync.unconfigured",
      retryable: true,
    }),
    discover_hosts: () => [],
    create_sync_pairing_code: () => {
      if (!syncStatus.enabled || syncStatus.role !== "host") return syncError("sync.unconfigured");
      pairingCode = String(++pairingCounter).padStart(6, "0");
      return {
        code: pairingCode,
        expiresAtUnixMs: Date.now() + 5 * 60_000,
        hostName: "Echo Mock",
      };
    },
    request_sync_pairing: ({ code, hostAddress }) => {
      if (!pairingCode || code !== pairingCode) return syncError("sync.pairingCodeInvalid");
      pairingCode = null;
      syncStatus = {
        ...syncStatus,
        enabled: true,
        role: "peer",
        state: "connected",
        sessionId: syncStatus.sessionId + 1,
        eventSequence: syncStatus.eventSequence + 1,
        address: `https://${String(hostAddress)}:50826`,
        localRoot: state.activeRoot || null,
      };
      return syncStatus;
    },
  };
}
