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
    /// True only when the completed cycle changed local catalog projections.
    pub catalog_changed: bool,
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
            catalog_changed: false,
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

/// Six-digit code with a short lifetime. Never include it in sync status/events.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPairingCode {
    pub code: String,
    pub expires_at_unix_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPeer {
    pub device_id: String,
    pub display_name: String,
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
  readonly catalogChanged: boolean;
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
export interface SyncPairingCode {
  /** Short-lived six-digit code; held in component memory only. */
  readonly code: string;
  readonly expiresAtUnixMs: number;
}
export interface SyncPeer {
  readonly deviceId: string;
  readonly displayName: string;
}
export interface SyncCommandArguments {
  readonly sync_status: [args?: Record<string, never>];
  readonly local_lan_addresses: [args?: Record<string, never>];
  readonly host_start: [args: { address: string }];
  readonly host_stop: [args?: Record<string, never>];
  readonly sync_leave: [args?: Record<string, never>];
  readonly sync_peer_start: [args?: Record<string, never>];
  readonly sync_pause: [args?: Record<string, never>];
  readonly sync_resume: [args?: Record<string, never>];
  readonly sync_now: [args?: Record<string, never>];
  readonly sync_authorized_peers: [args?: Record<string, never>];
  readonly revoke_device: [args: { deviceId: string }];
  readonly discover_hosts: [args?: Record<string, never>];
  readonly create_sync_pairing_code: [args?: Record<string, never>];
  readonly request_sync_pairing: [args: { code: string; hostAddress: string }];
}

"
}
