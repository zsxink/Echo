import { bridge } from "../../bridge";
import type {
  DiscoveredHost,
  SyncPairingCode,
  SyncPeer,
  SyncStatus,
} from "../../ipc/ipc-types.generated";

export type LanSyncRole = "host" | "peer";

export interface LanSyncAdapter {
  status(): Promise<SyncStatus>;
  authorizedPeers(): Promise<readonly SyncPeer[]>;
  subscribeStatus(handler: (status: SyncStatus) => void): Promise<() => void>;
  localLanAddresses(): Promise<readonly string[]>;
  hostStart(address: string): Promise<SyncStatus>;
  hostStop(): Promise<SyncStatus>;
  leaveSync(): Promise<SyncStatus>;
  peerStart(): Promise<SyncStatus>;
  pause(): Promise<SyncStatus>;
  resume(): Promise<SyncStatus>;
  syncNow(): Promise<SyncStatus>;
  discoverHosts(): Promise<readonly DiscoveredHost[]>;
  createPairingCode(): Promise<SyncPairingCode>;
  requestPairing(code: string, hostAddress: string): Promise<SyncStatus>;
}

export const lanSyncAdapter: LanSyncAdapter = {
  status: () => bridge.call("sync_status"),
  authorizedPeers: () => bridge.call("sync_authorized_peers"),
  subscribeStatus: (handler) => bridge.subscribe("sync://status", handler),
  localLanAddresses: () => bridge.call("local_lan_addresses"),
  hostStart: (address) => bridge.call("host_start", { address }),
  hostStop: () => bridge.call("host_stop"),
  leaveSync: () => bridge.call("sync_leave"),
  peerStart: () => bridge.call("sync_peer_start"),
  pause: () => bridge.call("sync_pause"),
  resume: () => bridge.call("sync_resume"),
  syncNow: () => bridge.call("sync_now"),
  discoverHosts: () => bridge.call("discover_hosts"),
  createPairingCode: () => bridge.call("create_sync_pairing_code"),
  requestPairing: (code, hostAddress) => bridge.call("request_sync_pairing", { code, hostAddress }),
};
