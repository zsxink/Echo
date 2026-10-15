//! Typed Tauri boundary for native opt-in synchronization lifecycle.
#![allow(clippy::needless_pass_by_value)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use echo_desktop::ipc::{
    sync::{SyncInvitation, SyncPairingReceipt, SyncPairingRequest},
    IpcErrorDto,
};
use echo_sync::client::LanClient;
use echo_sync::pairing::{PairingGrantDto, PairingInvitation, PairingReceipt};
use reqwest::Url;
use tauri::{Emitter, State};

use crate::sync_runtime::{self, DiscoveredHost, SyncRuntime, SyncRuntimeError, SyncStatus};

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

#[derive(serde::Serialize, serde::Deserialize)]
struct InvitationEnvelope {
    endpoint: String,
    pairing: PairingInvitation,
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[tauri::command]
pub async fn create_sync_invitation(
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<SyncInvitation, IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    tauri::async_runtime::spawn_blocking(move || {
        let pairing = runtime.create_pairing_invitation()?;
        let status = runtime.status();
        let endpoint = status.address.ok_or(SyncRuntimeError::Unconfigured)?;
        let invitation = serde_json::to_string(&InvitationEnvelope {
            endpoint: endpoint.clone(),
            pairing,
        })
        .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        Ok(SyncInvitation {
            invitation,
            expires_at_unix_ms: unix_ms().saturating_add(300_000),
            host_name: endpoint,
        })
    })
    .await
    .map_err(|_| error(SyncRuntimeError::WorkerFailed))?
    .map_err(error)
}

#[tauri::command]
pub async fn list_sync_pairing_requests(
    runtime: State<'_, Arc<SyncRuntime>>,
) -> Result<Vec<SyncPairingRequest>, IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    tauri::async_runtime::spawn_blocking(move || {
        let now = unix_ms();
        runtime.pending_pairing_requests().map(|requests| {
            requests
                .into_iter()
                .map(|request| SyncPairingRequest {
                    request_id: request.request_id,
                    device_name: request.device_name,
                    device_id: request.device_id.as_uuid().to_string(),
                    requested_at_unix_ms: now.saturating_sub(
                        300_u64
                            .saturating_sub(request.expires_in_secs)
                            .saturating_mul(1000),
                    ),
                })
                .collect()
        })
    })
    .await
    .map_err(|_| error(SyncRuntimeError::WorkerFailed))?
    .map_err(error)
}

#[tauri::command]
pub async fn approve_sync_pairing(
    runtime: State<'_, Arc<SyncRuntime>>,
    request_id: String,
) -> Result<(), IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    tauri::async_runtime::spawn_blocking(move || runtime.approve_pairing_request(&request_id))
        .await
        .map_err(|_| error(SyncRuntimeError::WorkerFailed))?
        .map_err(error)
}

#[tauri::command]
pub async fn reject_sync_pairing(
    runtime: State<'_, Arc<SyncRuntime>>,
    request_id: String,
) -> Result<(), IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    tauri::async_runtime::spawn_blocking(move || runtime.deny_pairing_request(&request_id))
        .await
        .map_err(|_| error(SyncRuntimeError::WorkerFailed))?
        .map_err(error)
}

fn parse_invitation(value: &str) -> Result<(String, PairingInvitation), SyncRuntimeError> {
    let envelope: InvitationEnvelope =
        serde_json::from_str(value).map_err(|_| SyncRuntimeError::InvalidAddress)?;
    if !envelope.endpoint.starts_with("https://")
        || envelope.pairing.invitation_secret.len() != 64
        || envelope.pairing.invitation_id.len() != 64
        || envelope.pairing.expires_in_secs > 300
    {
        return Err(SyncRuntimeError::InvalidAddress);
    }
    Ok((envelope.endpoint, envelope.pairing))
}

fn encode_receipt(receipt: PairingReceipt) -> Result<SyncPairingReceipt, SyncRuntimeError> {
    let request_id = receipt.request_id.clone();
    let expires_at_unix_ms = unix_ms().saturating_add(receipt.expires_in_secs.saturating_mul(1000));
    let receipt = serde_json::to_string(&receipt).map_err(|_| SyncRuntimeError::WorkerFailed)?;
    Ok(SyncPairingReceipt {
        request_id,
        receipt,
        expires_at_unix_ms,
    })
}

#[tauri::command]
pub async fn request_sync_pairing(
    runtime: State<'_, Arc<SyncRuntime>>,
    invitation: String,
    device_name: String,
    host_address: Option<String>,
) -> Result<SyncPairingReceipt, IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    tauri::async_runtime::spawn_blocking(move || {
        let (endpoint, invitation) = parse_invitation(&invitation)?;
        let endpoint = pairing_endpoint(&endpoint, host_address.as_deref())?;
        let device_id = runtime.local_sync_device_id();
        let client =
            LanClient::for_pairing(&invitation).map_err(|_| SyncRuntimeError::Unconfigured)?;
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let receipt = executor
            .block_on(client.request_pairing(&endpoint, &invitation, device_id, device_name))
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        encode_receipt(receipt)
    })
    .await
    .map_err(|_| error(SyncRuntimeError::WorkerFailed))?
    .map_err(error)
}

#[tauri::command]
pub async fn claim_sync_pairing(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<SyncRuntime>>,
    request_id: String,
    receipt: String,
    invitation: String,
    host_address: Option<String>,
) -> Result<SyncStatus, IpcErrorDto> {
    let runtime = Arc::clone(&runtime);
    let worker_runtime = Arc::clone(&runtime);
    let result = tauri::async_runtime::spawn_blocking(move || {
        let (endpoint, invitation) = parse_invitation(&invitation)?;
        let receipt: PairingReceipt =
            serde_json::from_str(&receipt).map_err(|_| SyncRuntimeError::InvalidAddress)?;
        if receipt.request_id != request_id
            || receipt.binding != invitation.binding
            || receipt.device_id != worker_runtime.local_sync_device_id()
        {
            return Err(SyncRuntimeError::InvalidAddress);
        }
        let endpoint = pairing_endpoint(&endpoint, host_address.as_deref())?;
        let client =
            LanClient::for_pairing(&invitation).map_err(|_| SyncRuntimeError::Unconfigured)?;
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let grant: PairingGrantDto = executor
            .block_on(client.claim_pairing(&endpoint, &receipt))
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        worker_runtime.complete_peer_pairing(&invitation, endpoint.as_str(), &grant)
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

/// Apply a user-selected LAN address to the invite endpoint while retaining
/// the certificate pin carried by the invitation. Discovery supplies only an
/// address hint; the invitation remains the trust anchor.
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
