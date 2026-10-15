//! Public sync lifecycle DTOs. Credentials and absolute library paths stay native.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRole {
    None,
    Host,
    Peer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    Disabled,
    Hosting,
    Connected,
    Syncing,
    Paused,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub enabled: bool,
    pub role: SyncRole,
    pub state: SyncState,
    pub session_id: u64,
    pub event_sequence: u64,
    pub library_id: Option<String>,
    pub address: Option<String>,
    pub last_error: Option<String>,
}

impl Default for SyncStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            role: SyncRole::None,
            state: SyncState::Disabled,
            session_id: 0,
            event_sequence: 0,
            library_id: None,
            address: None,
            last_error: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredHost {
    pub name: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub device_id: Option<String>,
    pub protocol_version: Option<String>,
}

/// Short-lived bearer material, returned only after the host explicitly
/// creates an invitation. Never include this value in sync status/events.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncInvitation {
    pub invitation: String,
    pub expires_at_unix_ms: u64,
    pub host_name: String,
}

/// Public metadata for a peer waiting for host approval. Contains no secret.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPairingRequest {
    pub request_id: String,
    pub device_name: String,
    pub device_id: String,
    pub requested_at_unix_ms: u64,
}

/// One-time peer claim material. Keep it out of durable status and event DTOs.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPairingReceipt {
    pub request_id: String,
    pub receipt: String,
    pub expires_at_unix_ms: u64,
}

pub(super) const fn typescript() -> &'static str {
    r"export type SyncRole = 'none' | 'host' | 'peer';
export type SyncState = 'disabled' | 'hosting' | 'connected' | 'syncing' | 'paused' | 'failed';
export interface SyncStatus {
  readonly enabled: boolean;
  readonly role: SyncRole;
  readonly state: SyncState;
  readonly sessionId: number;
  readonly eventSequence: number;
  readonly libraryId: string | null;
  readonly address: string | null;
  readonly lastError: string | null;
}
export interface DiscoveredHost {
  readonly name: string;
  readonly addresses: readonly string[];
  readonly port: number;
  readonly deviceId: string | null;
  readonly protocolVersion: string | null;
}
export interface SyncInvitation {
  /** Short-lived bearer invitation; held in component memory only. */
  readonly invitation: string;
  readonly expiresAtUnixMs: number;
  readonly hostName: string;
}
export interface SyncPairingRequest {
  readonly requestId: string;
  readonly deviceName: string;
  readonly deviceId: string;
  readonly requestedAtUnixMs: number;
}
export interface SyncPairingReceipt {
  readonly requestId: string;
  /** One-time claim secret, held in component memory only. */
  readonly receipt: string;
  readonly expiresAtUnixMs: number;
}
export interface SyncCommandArguments {
  readonly sync_status: [args?: Record<string, never>];
  readonly local_lan_addresses: [args?: Record<string, never>];
  readonly host_start: [args: { address: string }];
  readonly host_stop: [args?: Record<string, never>];
  readonly sync_peer_start: [args?: Record<string, never>];
  readonly sync_pause: [args?: Record<string, never>];
  readonly sync_resume: [args?: Record<string, never>];
  readonly sync_now: [args?: Record<string, never>];
  readonly revoke_device: [args: { deviceId: string }];
  readonly discover_hosts: [args?: Record<string, never>];
  readonly create_sync_invitation: [args?: Record<string, never>];
  readonly request_sync_pairing: [args: { invitation: string; deviceName: string; hostAddress?: string }];
  readonly list_sync_pairing_requests: [args?: Record<string, never>];
  readonly approve_sync_pairing: [args: { requestId: string }];
  readonly reject_sync_pairing: [args: { requestId: string }];
  readonly claim_sync_pairing: [args: { requestId: string; receipt: string; invitation: string; hostAddress?: string }];
}

"
}
