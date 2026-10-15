import { bridge } from "../../bridge";
import type {
  DiscoveredHost,
  SyncInvitation,
  SyncPairingReceipt,
  SyncPairingRequest,
  SyncStatus,
} from "../../ipc/ipc-types.generated";

export type LanSyncRole = "host" | "peer";

export interface LanSyncAdapter {
  status(): Promise<SyncStatus>;
  subscribeStatus(handler: (status: SyncStatus) => void): Promise<() => void>;
  hostStart(address: string): Promise<SyncStatus>;
  hostStop(): Promise<SyncStatus>;
  peerStart(): Promise<SyncStatus>;
  pause(): Promise<SyncStatus>;
  resume(): Promise<SyncStatus>;
  syncNow(): Promise<SyncStatus>;
  discoverHosts(): Promise<readonly DiscoveredHost[]>;
  createInvitation(): Promise<SyncInvitation>;
  requestPairing(invitation: string, deviceName: string): Promise<SyncPairingReceipt>;
  listPairingRequests(): Promise<readonly SyncPairingRequest[]>;
  approvePairing(requestId: string): Promise<void>;
  rejectPairing(requestId: string): Promise<void>;
  claimPairing(requestId: string, receipt: string, invitation: string): Promise<SyncStatus>;
}

export const lanSyncAdapter: LanSyncAdapter = {
  status: () => bridge.call("sync_status"),
  subscribeStatus: (handler) => bridge.subscribe("sync://status", handler),
  hostStart: (address) => bridge.call("host_start", { address }),
  hostStop: () => bridge.call("host_stop"),
  peerStart: () => bridge.call("sync_peer_start"),
  pause: () => bridge.call("sync_pause"),
  resume: () => bridge.call("sync_resume"),
  syncNow: () => bridge.call("sync_now"),
  discoverHosts: () => bridge.call("discover_hosts"),
  createInvitation: () => bridge.call("create_sync_invitation"),
  requestPairing: (invitation, deviceName) =>
    bridge.call("request_sync_pairing", { invitation, deviceName }),
  listPairingRequests: () => bridge.call("list_sync_pairing_requests"),
  approvePairing: (requestId) => bridge.call("approve_sync_pairing", { requestId }),
  rejectPairing: (requestId) => bridge.call("reject_sync_pairing", { requestId }),
  claimPairing: (requestId, receipt, invitation) =>
    bridge.call("claim_sync_pairing", { requestId, receipt, invitation }),
};
