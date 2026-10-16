//! Durable host pairing credentials and peer authorization coordination.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Two-store enrollment preserves each request's credential generation. The
/// OS slot is durable before authorization; failed SQL never grants access.
pub(super) struct DurableHostPairing {
    database: Arc<SqliteDatabase>,
    gate: Mutex<()>,
}

impl DurableHostPairing {
    pub(super) const fn new(database: Arc<SqliteDatabase>) -> Self {
        Self {
            database,
            gate: Mutex::new(()),
        }
    }

    fn load(
        &self,
        request: &PendingPairingRequest,
        handle: &SecretHandle,
    ) -> Result<PairingGrantDto, PairingError> {
        if !self
            .database
            .sync_peer_has_credential(
                request.binding,
                request.device_id,
                handle.as_str().to_owned(),
            )
            .map_err(|_| provisioning_failure("authorization_lookup"))?
        {
            return Err(PairingError::Unavailable);
        }
        let secret = OsSecureStore
            .get(handle)
            .map_err(|_| provisioning_failure("credential_read"))?;
        let bearer_token = std::str::from_utf8(secret.expose())
            .map_err(|_| provisioning_failure("credential_decode"))?
            .to_owned();
        if !(32..=256).contains(&bearer_token.len())
            || !bearer_token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(provisioning_failure("credential_validation"));
        }
        Ok(PairingGrantDto {
            binding: request.binding,
            device_id: request.device_id,
            bearer_token,
        })
    }
}

impl PairingPort for DurableHostPairing {
    fn approve(&self, request: &PendingPairingRequest) -> Result<PairingGrantDto, PairingError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| provisioning_failure("pairing_store_lock"))?;
        let handle = pairing_handle(request)?;
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| provisioning_failure("host_configuration_read"))?
            .ok_or(PairingError::Unavailable)?;
        if !configuration.host
            || configuration.binding != request.binding
            || configuration.local_device == request.device_id
        {
            return Err(PairingError::Unavailable);
        }
        let peers = self
            .database
            .authorized_sync_peer_handles()
            .map_err(|_| provisioning_failure("authorized_peers_read"))?;
        if peers.iter().any(|(device, _)| *device == request.device_id) {
            return Err(PairingError::Unavailable);
        }
        if peers.len() >= 32 {
            return Err(PairingError::Capacity);
        }
        // Three independent OS-random v4 UUIDs carry 366 random bits. Store
        // only in the vault; the library receives this request's opaque handle.
        let bearer_token = format!(
            "{}{}{}",
            MutationId::new().as_uuid().simple(),
            MutationId::new().as_uuid().simple(),
            MutationId::new().as_uuid().simple()
        );
        OsSecureStore
            .set(&handle, &SecretBytes::new(bearer_token.as_bytes().to_vec()))
            .map_err(|_| provisioning_failure("credential_write"))?;
        if self
            .database
            .reauthorize_sync_peer(
                request.binding,
                request.device_id,
                request.device_name.clone(),
                handle.as_str().to_owned(),
            )
            .is_err()
        {
            OsSecureStore
                .delete(&handle)
                .map_err(|_| provisioning_failure("credential_compensation"))?;
            return Err(provisioning_failure("authorization_write"));
        }
        Ok(PairingGrantDto {
            binding: request.binding,
            device_id: request.device_id,
            bearer_token,
        })
    }

    fn cancel_approval(&self, request: &PendingPairingRequest) -> Result<(), PairingError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| provisioning_failure("pairing_store_lock"))?;
        let handle = pairing_handle(request)?;
        let revoked = self
            .database
            .revoke_sync_peer_credential(
                request.binding,
                request.device_id,
                handle.as_str().to_owned(),
            )
            .map_err(|_| provisioning_failure("authorization_revoke"))?;
        if revoked {
            OsSecureStore
                .delete(&handle)
                .map_err(|_| provisioning_failure("credential_delete"))?;
        }
        Ok(())
    }

    fn load_approved_grant(
        &self,
        request: &PendingPairingRequest,
    ) -> Result<PairingGrantDto, PairingError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| provisioning_failure("pairing_store_lock"))?;
        self.load(request, &pairing_handle(request)?)
    }
}

fn provisioning_failure(stage: &'static str) -> PairingError {
    tracing::warn!(target: "echo_desktop::sync", stage, "LAN pairing provisioning failed");
    PairingError::ProvisioningUnavailable
}

fn pairing_handle(request: &PendingPairingRequest) -> Result<SecretHandle, PairingError> {
    if request.request_id.len() != 64
        || !request
            .request_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(PairingError::InvalidRequest);
    }
    SecretHandle::new(format!(
        "pairing-{}-{}",
        request.device_id.as_uuid(),
        request.request_id
    ))
    .map_err(|_| PairingError::InvalidRequest)
}
