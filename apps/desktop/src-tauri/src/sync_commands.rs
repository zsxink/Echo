//! Typed Tauri boundary for native opt-in synchronization lifecycle.
#![allow(clippy::needless_pass_by_value)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use echo_core::application::ports::{
    HostSyncConflictPort, SyncConflictResolution, SyncConflictView,
};
use echo_core::domain::ids::LibraryRootId;
use echo_core::infrastructure::sqlite::SqliteDatabase;
use echo_desktop::ipc::{
    sync::{SyncConflictResolution as IpcSyncConflictResolution, SyncPairingCode},
    IpcErrorDto,
};
use echo_sync::client::LanClient;
use reqwest::Url;
use tauri::{Emitter, State};

use crate::sync_runtime::{
    self, DiscoveredHost, SyncPeer, SyncRuntime, SyncRuntimeError, SyncStatus,
};

fn error(error: SyncRuntimeError) -> IpcErrorDto {
    IpcErrorDto::new(
        if error == SyncRuntimeError::InvalidAddress {
            "validation"
        } else {
            "unavailable"
        },
        error.message_key(),
        error != SyncRuntimeError::InvalidAddress,
        None,
        None,
    )
}

#[tauri::command]
pub fn local_lan_addresses() -> Vec<String> {
    sync_runtime::available_local_lan_addresses()
        .into_iter()
        .map(|address| address.to_string())
        .collect()
}

async fn transition(
    app: tauri::AppHandle,
    runtime: Arc<SyncRuntime>,
    action: impl FnOnce(&SyncRuntime) -> Result<SyncStatus, SyncRuntimeError> + Send + 'static,
) -> Result<SyncStatus, IpcErrorDto> {
    let worker_runtime = Arc::clone(&runtime);
    let outcome = tauri::async_runtime::spawn_blocking(move || action(&worker_runtime))
        .await
        .unwrap_or(Err(SyncRuntimeError::WorkerFailed));
    let status = match outcome {
        Ok(status) => status,
        Err(failure) => {
            let _ = app.emit("sync://status", runtime.report_error(failure));
            return Err(error(failure));
        }
    };
    let _ = app.emit("sync://status", &status);
    Ok(status)
}

#[tauri::command]
pub fn sync_status(runtime: State<'_, Arc<SyncRuntime>>) -> SyncStatus {
    runtime.status()
}

#[tauri::command]
pub fn sync_storage_space(
    services: State<'_, echo_desktop::runtime::services::AppServices>,
    library_root: String,
) -> Result<echo_desktop::ipc::sync::SyncStorageSpace, IpcErrorDto> {
    let root = library_root.parse().map_err(|_| {
        IpcErrorDto::new("validation", "sync.libraryUnavailable", false, None, None)
    })?;
    services.sync_storage_space(root).map_err(IpcErrorDto::from)
}

fn host_conflict_scope(
    database: &SqliteDatabase,
    library_root: &str,
) -> Result<(LibraryRootId, echo_core::domain::sync::SyncBinding), IpcErrorDto> {
    let root = library_root.parse::<LibraryRootId>().map_err(|_| {
        IpcErrorDto::new("validation", "sync.libraryUnavailable", false, None, None)
    })?;
    let configuration = database
        .current_sync_configuration()
        .map_err(IpcErrorDto::from)?
        .filter(|configuration| configuration.local_root == root && configuration.host)
        .ok_or_else(|| {
            IpcErrorDto::new(
                "validation",
                "sync.hostConflictsUnavailable",
                false,
                None,
                None,
            )
        })?;
    Ok((root, configuration.binding))
}

#[tauri::command]
pub fn sync_conflicts(
    database: State<'_, Arc<SqliteDatabase>>,
    library_root: String,
    limit: Option<usize>,
) -> Result<Vec<SyncConflictView>, IpcErrorDto> {
    let (root, binding) = host_conflict_scope(&database, &library_root)?;
    database
        .host_sync_conflicts(root, binding, limit.unwrap_or(100))
        .map_err(IpcErrorDto::from)
}

#[tauri::command]
pub fn resolve_sync_conflict(
    database: State<'_, Arc<SqliteDatabase>>,
    library_root: String,
    conflict_id: String,
    recover_replaced: bool,
) -> Result<IpcSyncConflictResolution, IpcErrorDto> {
    let (root, binding) = host_conflict_scope(&database, &library_root)?;
    let conflict = uuid::Uuid::parse_str(&conflict_id).map_err(|_| {
        IpcErrorDto::new("validation", "sync.conflictUnavailable", false, None, None)
    })?;
    let resolution: SyncConflictResolution = database
        .resolve_host_sync_conflict(root, binding, conflict, recover_replaced)
        .map_err(IpcErrorDto::from)?;
    Ok(resolution)
}

/// Explicitly archives a restored host ledger and starts a new authority epoch.
#[tauri::command]
pub async fn recover_restored_host(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<echo_core::application::ports::HostRecoveryArchive, IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    let worker_runtime = Arc::clone(&runtime);
    let outcome =
        tauri::async_runtime::spawn_blocking(move || worker_runtime.recover_restored_host())
            .await
            .unwrap_or(Err(SyncRuntimeError::WorkerFailed));
    match outcome {
        Ok(archive) => {
            let _ = app.emit("sync://status", runtime.status());
            Ok(archive)
        }
        Err(failure) => {
            let _ = app.emit("sync://status", runtime.report_error(failure));
            Err(error(failure))
        }
    }
}

#[tauri::command]
pub async fn sync_authorized_peers(
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<Vec<SyncPeer>, IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    tauri::async_runtime::spawn_blocking(move || runtime.authorized_sync_peers())
        .await
        .map_err(|_| error(SyncRuntimeError::WorkerFailed))?
        .map_err(error)
}

#[tauri::command]
pub async fn host_start(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
    address: String,
) -> Result<SyncStatus, IpcErrorDto> {
    transition(app, Arc::clone(&runtime), move |runtime| {
        runtime.host_start(&address)
    })
    .await
}

#[tauri::command]
pub async fn host_stop(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<SyncStatus, IpcErrorDto> {
    transition(app, Arc::clone(&runtime), SyncRuntime::stop).await
}

#[tauri::command]
pub async fn sync_leave(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<SyncStatus, IpcErrorDto> {
    transition(app, Arc::clone(&runtime), SyncRuntime::leave_sync).await
}

#[tauri::command]
pub async fn sync_peer_start(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<SyncStatus, IpcErrorDto> {
    transition(app, Arc::clone(&runtime), SyncRuntime::peer_start).await
}

#[tauri::command]
pub async fn sync_pause(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<SyncStatus, IpcErrorDto> {
    transition(app, Arc::clone(&runtime), SyncRuntime::pause).await
}

#[tauri::command]
pub async fn sync_resume(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<SyncStatus, IpcErrorDto> {
    transition(app, Arc::clone(&runtime), SyncRuntime::resume).await
}

#[tauri::command]
pub async fn sync_now(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<SyncStatus, IpcErrorDto> {
    transition(app, Arc::clone(&runtime), SyncRuntime::sync_now).await
}

/// Explicitly retry a startup restoration that already gave up. Automatic
/// restoration is finite, so the terminal failure state must have a user path.
#[tauri::command]
pub async fn sync_restore_retry(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<SyncStatus, IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    let worker_runtime = Arc::clone(&runtime);
    let outcome = tauri::async_runtime::spawn_blocking(move || worker_runtime.retry_restore())
        .await
        .unwrap_or(Err(SyncRuntimeError::WorkerFailed));
    let status = match outcome {
        Ok(status) => status,
        Err(failure) => {
            let _ = app.emit("sync://status", runtime.report_error(failure));
            return Err(error(failure));
        }
    };
    let _ = app.emit("sync://status", &status);
    // Restart the bounded worker only while the persisted intent still allows
    // it; a stopped or paused library stays stopped.
    if let Err(failure) = runtime.start_restore_retry() {
        if failure != SyncRuntimeError::Unconfigured {
            let _ = app.emit("sync://status", runtime.report_error(failure));
            return Err(error(failure));
        }
    }
    Ok(runtime.status())
}

#[tauri::command]
pub async fn revoke_device(
    runtime: State<'_, Arc<SyncRuntime>>,
    device_id: String,
) -> Result<(), IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    tauri::async_runtime::spawn_blocking(move || runtime.revoke_device(&device_id))
        .await
        .map_err(|_| error(SyncRuntimeError::WorkerFailed))?
        .map_err(error)
}

#[tauri::command]
pub async fn discover_hosts() -> Result<Vec<DiscoveredHost>, IpcErrorDto> {
    tauri::async_runtime::spawn_blocking(sync_runtime::discover_hosts)
        .await
        .map_err(|_| error(SyncRuntimeError::WorkerFailed))?
        .map_err(error)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn local_device_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .filter(|name| !name.trim().is_empty())
        })
        .map(|name| {
            let mut safe_name = String::new();
            for character in name
                .trim()
                .chars()
                .filter(|character| !character.is_control())
            {
                if safe_name.len() + character.len_utf8() > 128 {
                    break;
                }
                safe_name.push(character);
            }
            safe_name
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Echo 设备".to_owned())
}

#[tauri::command]
pub async fn create_sync_pairing_code(
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<SyncPairingCode, IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    tauri::async_runtime::spawn_blocking(move || {
        let pairing = runtime.create_pairing_invitation()?;
        Ok(SyncPairingCode {
            code: pairing.invitation_secret,
            expires_at_unix_ms: unix_ms().saturating_add(600_000),
        })
    })
    .await
    .map_err(|_| error(SyncRuntimeError::WorkerFailed))?
    .map_err(error)
}

#[tauri::command]
pub async fn request_sync_pairing(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
    code: String,
    host_address: String,
) -> Result<SyncStatus, IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    let worker_runtime = Arc::clone(&runtime);
    let result = tauri::async_runtime::spawn_blocking(move || {
        let address = host_address.trim();
        let formatted = if address.contains(':') && !address.starts_with('[') {
            format!("[{address}]")
        } else {
            address.to_owned()
        };
        let endpoint = pairing_endpoint(
            &format!("https://{formatted}:{}", crate::sync_runtime::SYNC_PORT),
            None,
        )?;
        let device_id = worker_runtime.local_sync_device_id();
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let pairing = executor
            .block_on(LanClient::pair_with_code(
                &endpoint,
                &code,
                device_id,
                local_device_name(),
            ))
            .map_err(map_pairing_client_error)?;
        worker_runtime.complete_peer_pairing(&pairing.invitation, endpoint.as_str(), &pairing.grant)
    })
    .await
    .map_err(|_| error(SyncRuntimeError::WorkerFailed))?;
    match result {
        Ok(status) => {
            let _ = app.emit("sync://status", &status);
            Ok(status)
        }
        Err(failure) => {
            let _ = app.emit("sync://status", runtime.report_error(failure));
            Err(error(failure))
        }
    }
}

fn map_pairing_client_error(error: echo_sync::client::ClientError) -> SyncRuntimeError {
    use echo_sync::client::ClientError;
    use reqwest::StatusCode;

    match error {
        ClientError::HostRejected(StatusCode::NOT_FOUND) => SyncRuntimeError::PairingFailed,
        ClientError::HostRejected(StatusCode::TOO_MANY_REQUESTS) => {
            SyncRuntimeError::PairingRateLimited
        }
        ClientError::HostRejected(StatusCode::SERVICE_UNAVAILABLE) => {
            SyncRuntimeError::PairingHostUnavailable
        }
        ClientError::HostRejectedCode(status, _) if status == StatusCode::NOT_FOUND => {
            SyncRuntimeError::PairingFailed
        }
        ClientError::HostRejectedCode(status, _) if status == StatusCode::TOO_MANY_REQUESTS => {
            SyncRuntimeError::PairingRateLimited
        }
        ClientError::HostRejectedCode(status, _) if status == StatusCode::SERVICE_UNAVAILABLE => {
            SyncRuntimeError::PairingHostUnavailable
        }
        ClientError::HostRejected(_) | ClientError::HostRejectedCode(_, _) => {
            SyncRuntimeError::PairingRequestRejected
        }
        ClientError::Transport(transport) => {
            // Keep the user-facing error stable, but retain the reqwest source
            // chain in the local diagnostic log. Without this, TCP refusal,
            // Local Network privacy denial, timeout, and TLS handshake errors
            // all collapse into the same message and cannot be distinguished.
            tracing::warn!(error = ?transport, "LAN pairing transport failed");
            SyncRuntimeError::PairingConnectionFailed
        }
        ClientError::InvalidResponse(_)
        | ClientError::InvalidResponseBody
        | ClientError::InvalidEndpoint
        | ClientError::InvalidTlsConfiguration(_)
        | ClientError::InvalidMutation(_)
        | ClientError::InvalidSnapshot(_)
        | ClientError::InvalidCommitChain
        | ClientError::RemoteApply(_) => SyncRuntimeError::PairingProtocolFailed,
    }
}

/// Apply a user-selected local network address to the pairing endpoint.
fn pairing_endpoint(endpoint: &str, host_address: Option<&str>) -> Result<Url, SyncRuntimeError> {
    let mut endpoint = Url::parse(endpoint).map_err(|_| SyncRuntimeError::InvalidAddress)?;
    if endpoint.scheme() != "https" || endpoint.host().is_none() {
        return Err(SyncRuntimeError::InvalidAddress);
    }
    let Some(host_address) = host_address
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(endpoint);
    };

    let (address, port) = match host_address.parse::<SocketAddr>() {
        Ok(socket) => (socket.ip(), Some(socket.port())),
        Err(_) => (
            host_address
                .parse::<IpAddr>()
                .map_err(|_| SyncRuntimeError::InvalidAddress)?,
            None,
        ),
    };
    let address = crate::sync_runtime::selected_lan_address(&address.to_string())?;
    let host = match address {
        IpAddr::V4(_) => address.to_string(),
        IpAddr::V6(_) => format!("[{address}]"),
    };
    endpoint
        .set_host(Some(&host))
        .map_err(|_| SyncRuntimeError::InvalidAddress)?;
    if let Some(port) = port {
        endpoint
            .set_port(Some(port))
            .map_err(|()| SyncRuntimeError::InvalidAddress)?;
    }
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_endpoint_accepts_manual_lan_host_and_keeps_invite_port() {
        let endpoint = pairing_endpoint("https://192.168.1.5:43000", Some("10.0.0.8"))
            .expect("valid LAN override");
        assert_eq!(endpoint.as_str(), "https://10.0.0.8:43000/");
    }

    #[test]
    fn pairing_endpoint_uses_discovered_socket_and_rejects_non_lan_hosts() {
        let endpoint = pairing_endpoint("https://192.168.1.5:43000", Some("[fd00::2]:44000"))
            .expect("valid discovered socket");
        assert_eq!(endpoint.as_str(), "https://[fd00::2]:44000/");
        assert_eq!(
            pairing_endpoint("https://192.168.1.5:43000", Some("8.8.8.8")),
            Err(SyncRuntimeError::InvalidAddress)
        );
        assert_eq!(
            pairing_endpoint("https://192.168.1.5:43000", Some("host.example")),
            Err(SyncRuntimeError::InvalidAddress)
        );
    }
}
