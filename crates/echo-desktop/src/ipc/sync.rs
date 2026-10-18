//! Public sync lifecycle DTOs. Credentials and absolute library paths stay native.

use serde::Serialize;

pub use echo_core::application::ports::{
    SyncConflictRecovery, SyncConflictResolution, SyncConflictView,
};

/// A capacity hint scoped to a logical root; absolute paths remain native.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStorageSpace {
    pub library_root: String,
    pub available_bytes: Option<u64>,
}

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
    /// Logical root selected on this device, used to reject stale UI events.
    pub local_root: Option<String>,
    pub address: Option<String>,
    pub last_error: Option<String>,
    /// Queue and transfer counters are absent when native transport telemetry
    /// cannot prove an exact value; the UI must never render estimates as fact.
    pub pending_changes: Option<u64>,
    pub pending_uploads: Option<u64>,
    pub pending_downloads: Option<u64>,
    pub uploaded_bytes: Option<u64>,
    pub total_upload_bytes: Option<u64>,
    pub downloaded_bytes: Option<u64>,
    pub total_download_bytes: Option<u64>,
    pub media_ready: Option<bool>,
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
            local_root: None,
            address: None,
            last_error: None,
            pending_changes: None,
            pending_uploads: None,
            pending_downloads: None,
            uploaded_bytes: None,
            total_upload_bytes: None,
            downloaded_bytes: None,
            total_download_bytes: None,
            media_ready: None,
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
  readonly localRoot: string | null;
  readonly address: string | null;
  readonly lastError: string | null;
  readonly pendingChanges: number | null;
  readonly pendingUploads: number | null;
  readonly pendingDownloads: number | null;
  readonly uploadedBytes: number | null;
  readonly totalUploadBytes: number | null;
  readonly downloadedBytes: number | null;
  readonly totalDownloadBytes: number | null;
  readonly mediaReady: boolean | null;
}
export interface SyncStorageSpace {
  readonly libraryRoot: string;
  readonly availableBytes: number | null;
}
export type SyncConflictRecovery = 'available' | 'pending_edit' | 'final_deleted' | 'unsupported' | 'unavailable';
export type SyncConflictResolution = 'recovered' | 'dismissed' | 'already_resolved' | 'final_deleted' | 'pending_edit' | 'unsupported' | 'unavailable';
export interface SyncConflictView {
  readonly conflictId: string;
  readonly objectType: string;
  readonly objectUuid: string;
  readonly replacedVersion: number;
  readonly acceptedVersion: number;
  readonly replacedFields: unknown;
  readonly acceptedFields: unknown;
  readonly recovery: SyncConflictRecovery;
}
export interface HostRecoveryArchive {
  readonly recoveryId: string;
  readonly localRoot: string;
  readonly previousBinding: SyncBinding;
  readonly replacementBinding: SyncBinding;
  readonly pendingCount: number;
  readonly conflictCount: number;
}
export interface SyncBinding {
  readonly libraryId: string;
  readonly authorityId: string;
  readonly epoch: number;
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
  readonly sync_storage_space: [args: { libraryRoot: string }];
  readonly sync_conflicts: [args: { libraryRoot: string; limit?: number }];
  readonly resolve_sync_conflict: [args: { libraryRoot: string; conflictId: string; recoverReplaced: boolean }];
  readonly recover_restored_host: [args?: Record<string, never>];
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
