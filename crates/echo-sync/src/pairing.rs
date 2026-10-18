//! Short-lived LAN pairing codes for pinned peers.
//!
//! The registry retains only hashes of invitation and receipt secrets. Approved
//! credentials belong to the desktop's durable authorization and OS vault port.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use echo_core::domain::sync::{DeviceId, MutationId, SyncBinding};
use serde::{Deserialize, Serialize};

const MAX_PENDING: usize = 32;
const MAX_TTL: Duration = Duration::from_secs(600);

/// Transfer this entire object through a trusted out-of-band channel. The DER
/// certificate is public pin material; discovery never supplies its trust.
#[derive(Clone, Serialize, Deserialize)]
pub struct PairingInvitation {
    pub binding: SyncBinding,
    pub host_device_id: DeviceId,
    pub invitation_id: String,
    pub invitation_secret: String,
    pub host_certificate_der: Vec<u8>,
    pub expires_in_secs: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PairingRequestDto {
    pub binding: SyncBinding,
    pub invitation_id: String,
    pub invitation_secret: String,
    pub device_id: DeviceId,
    pub device_name: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PairingCodeRequest {
    pub code: String,
    pub device_id: DeviceId,
    pub device_name: String,
}

#[derive(Serialize, Deserialize)]
pub struct PairingBootstrapResponse {
    pub invitation: PairingInvitation,
    pub grant: PairingGrantDto,
}

/// Possession of this receipt permits polling/claiming only this request.
#[derive(Clone, Serialize, Deserialize)]
pub struct PairingReceipt {
    pub binding: SyncBinding,
    pub request_id: String,
    pub device_id: DeviceId,
    pub claim_secret: String,
    pub expires_in_secs: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PairingClaimDto {
    pub binding: SyncBinding,
    pub request_id: String,
    pub device_id: DeviceId,
    pub claim_secret: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingPairingRequest {
    pub binding: SyncBinding,
    pub request_id: String,
    pub device_id: DeviceId,
    pub device_name: String,
    pub expires_in_secs: u64,
}

/// Never log this value. The bearer is delivered only over the pinned TLS link.
#[derive(Serialize, Deserialize)]
pub struct PairingGrantDto {
    pub binding: SyncBinding,
    pub device_id: DeviceId,
    pub bearer_token: String,
}

/// Desktop implementation must durably provision the OS vault credential and
/// SQLite authorization before returning success; no default approval exists.
pub trait PairingPort: Send + Sync {
    /// Approve the named request and return its provisioned or safely reused credential.
    ///
    /// # Errors
    /// Fails closed when either durable store is unavailable. A valid retry for
    /// an already-authorized device may return that device's existing grant.
    fn approve(&self, request: &PendingPairingRequest) -> Result<PairingGrantDto, PairingError>;

    /// Compensate only this request's newly provisioned credential generation.
    ///
    /// # Errors
    /// Reports a failure to revoke/remove this exact approval; never revoke a prior peer.
    fn cancel_approval(&self, request: &PendingPairingRequest) -> Result<(), PairingError>;

    /// Read this request's exact credential generation from the OS vault and
    /// recheck current SQLite authorization; do not load by device alone.
    ///
    /// # Errors
    /// Fails closed for missing credentials or revoked/invalid authorization.
    fn load_approved_grant(
        &self,
        request: &PendingPairingRequest,
    ) -> Result<PairingGrantDto, PairingError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingError {
    InvalidRequest,
    Unavailable,
    Capacity,
    Pending,
    ProvisioningUnavailable,
}

impl std::fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for PairingError {}

struct Invitation {
    host_device_id: DeviceId,
    secret_hash: [u8; 32],
    certificate_der: Vec<u8>,
    expires: Instant,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RequestState {
    Pending,
    Approving,
    Approved,
    Claiming,
}

#[derive(Clone)]
struct Request {
    metadata: PendingPairingRequest,
    receipt_hash: [u8; 32],
    expires: Instant,
    state: RequestState,
}

#[derive(Default)]
struct RegistryState {
    invitations: HashMap<String, Invitation>,
    requests: HashMap<String, Request>,
}

pub struct PairingRegistry {
    binding: SyncBinding,
    state: Mutex<RegistryState>,
}

impl PairingRegistry {
    #[must_use]
    pub fn new(binding: SyncBinding) -> Self {
        Self {
            binding,
            state: Mutex::default(),
        }
    }

    /// Revoke expired, unclaimed approvals and discard their invitation state.
    ///
    /// # Errors
    /// Reports a failed conditional revocation/removal in the desktop port.
    pub fn cleanup_expired(&self, port: &dyn PairingPort) -> Result<(), PairingError> {
        let expired = {
            let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
            let now = Instant::now();
            state
                .invitations
                .retain(|_, invitation| invitation.expires > now);
            let mut expired = Vec::new();
            state.requests.retain(|_, request| {
                if request.expires <= now && request.state == RequestState::Approved {
                    expired.push(request.clone());
                    false
                } else {
                    true
                }
            });
            prune(&mut state);
            drop(state);
            expired
        };
        let mut result = Ok(());
        for request in expired {
            if let Err(error) = port.cancel_approval(&request.metadata) {
                self.state
                    .lock()
                    .map_err(|_| PairingError::Unavailable)?
                    .requests
                    .entry(request.metadata.request_id.clone())
                    .or_insert(request);
                result = Err(error);
            }
        }
        result
    }

    /// Invalidate all unfinished enrollment when the host stops or changes root.
    ///
    /// # Errors
    /// Reports failure to conditionally revoke unclaimed approval generations.
    pub fn cancel_all(&self, port: &dyn PairingPort) -> Result<(), PairingError> {
        let requests = {
            let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
            state.invitations.clear();
            state
                .requests
                .drain()
                .map(|(_, request)| request)
                .collect::<Vec<_>>()
        };
        let mut result = Ok(());
        for mut request in requests {
            if let Err(error) = port.cancel_approval(&request.metadata) {
                request.expires = Instant::now();
                request.state = RequestState::Approved;
                self.state
                    .lock()
                    .map_err(|_| PairingError::Unavailable)?
                    .requests
                    .entry(request.metadata.request_id.clone())
                    .or_insert(request);
                result = Err(error);
            }
        }
        result
    }

    fn compensate(
        &self,
        metadata: &PendingPairingRequest,
        port: &dyn PairingPort,
    ) -> Result<(), PairingError> {
        if let Err(error) = port.cancel_approval(metadata) {
            // Preserve a non-claimable generation so a later cleanup retries it.
            self.state
                .lock()
                .map_err(|_| PairingError::Unavailable)?
                .requests
                .entry(metadata.request_id.clone())
                .or_insert_with(|| Request {
                    metadata: metadata.clone(),
                    receipt_hash: [0; 32],
                    expires: Instant::now(),
                    state: RequestState::Approved,
                });
            return Err(error);
        }
        Ok(())
    }

    /// Mint a single-use invitation with a maximum lifetime of ten minutes.
    ///
    /// # Errors
    /// Rejects invalid lifetimes/certificates and a full invitation registry.
    pub fn create_invitation(
        &self,
        host_device_id: DeviceId,
        host_certificate_der: Vec<u8>,
        ttl: Duration,
    ) -> Result<PairingInvitation, PairingError> {
        if ttl.is_zero()
            || ttl > MAX_TTL
            || host_certificate_der.is_empty()
            || host_certificate_der.len() > 16 * 1024
        {
            return Err(PairingError::InvalidRequest);
        }
        let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
        prune(&mut state);
        state.invitations.clear();
        let invitation_id = random_secret();
        let mut invitation_secret = random_pairing_code();
        while state
            .invitations
            .values()
            .any(|invite| secret_matches(&invitation_secret, &invite.secret_hash))
        {
            invitation_secret = random_pairing_code();
        }
        state.invitations.insert(
            invitation_id.clone(),
            Invitation {
                host_device_id,
                secret_hash: *blake3::hash(invitation_secret.as_bytes()).as_bytes(),
                certificate_der: host_certificate_der.clone(),
                expires: Instant::now() + ttl,
            },
        );
        drop(state);
        Ok(PairingInvitation {
            binding: self.binding,
            host_device_id,
            invitation_id,
            invitation_secret,
            host_certificate_der,
            expires_in_secs: ttl.as_secs(),
        })
    }

    /// Resolve a short code to the host's temporary TLS pin, then create the
    /// same one-use receipt used by the existing credential lifecycle.
    ///
    /// # Errors
    /// Rejects a malformed or expired code, a code already claimed by another
    /// request, and registry or provisioning failures.
    pub fn request_with_code(
        &self,
        dto: &PairingCodeRequest,
    ) -> Result<(PairingReceipt, PairingInvitation), PairingError> {
        if dto.code.len() != 6
            || !dto.code.bytes().all(|byte| byte.is_ascii_digit())
            || !valid_name(&dto.device_name)
        {
            return Err(PairingError::InvalidRequest);
        }
        let (invitation_id, host_device_id, certificate_der, expires_in_secs) = {
            let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
            prune(&mut state);
            let found = state
                .invitations
                .iter()
                .find(|(_, invite)| secret_matches(&dto.code, &invite.secret_hash))
                .map(|(id, invite)| {
                    (
                        id.clone(),
                        invite.host_device_id,
                        invite.certificate_der.clone(),
                        invite
                            .expires
                            .saturating_duration_since(Instant::now())
                            .as_secs(),
                    )
                })
                .ok_or(PairingError::Unavailable)?;
            found
        };
        let invitation_secret = dto.code.clone();
        let receipt = self.request(&PairingRequestDto {
            binding: self.binding,
            invitation_id: invitation_id.clone(),
            invitation_secret: invitation_secret.clone(),
            device_id: dto.device_id,
            device_name: dto.device_name.clone(),
        })?;
        Ok((
            receipt,
            PairingInvitation {
                binding: self.binding,
                host_device_id,
                invitation_id,
                invitation_secret,
                host_certificate_der: certificate_der,
                expires_in_secs,
            },
        ))
    }

    /// Consume an invitation and create a request requiring explicit approval.
    ///
    /// # Errors
    /// Rejects wrong bindings, invalid metadata, expired/used secrets, or capacity.
    pub fn request(&self, dto: &PairingRequestDto) -> Result<PairingReceipt, PairingError> {
        if dto.binding != self.binding
            || !valid_secret(&dto.invitation_id)
            || !valid_invitation_secret(&dto.invitation_secret)
            || !valid_name(&dto.device_name)
        {
            return Err(PairingError::InvalidRequest);
        }
        let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
        prune(&mut state);
        if state.requests.len() >= MAX_PENDING {
            return Err(PairingError::Capacity);
        }
        let invite = state
            .invitations
            .get(&dto.invitation_id)
            .ok_or(PairingError::Unavailable)?;
        if dto.device_id == invite.host_device_id {
            return Err(PairingError::InvalidRequest);
        }
        if !secret_matches(&dto.invitation_secret, &invite.secret_hash) {
            return Err(PairingError::Unavailable);
        }
        let expires = invite.expires;
        state.invitations.remove(&dto.invitation_id);
        let request_id = random_secret();
        let claim_secret = random_secret();
        let expires_in_secs = expires.saturating_duration_since(Instant::now()).as_secs();
        state.requests.insert(
            request_id.clone(),
            Request {
                metadata: PendingPairingRequest {
                    binding: self.binding,
                    request_id: request_id.clone(),
                    device_id: dto.device_id,
                    device_name: dto.device_name.clone(),
                    expires_in_secs,
                },
                receipt_hash: *blake3::hash(claim_secret.as_bytes()).as_bytes(),
                expires,
                state: RequestState::Pending,
            },
        );
        drop(state);
        Ok(PairingReceipt {
            binding: self.binding,
            request_id,
            device_id: dto.device_id,
            claim_secret,
            expires_in_secs,
        })
    }

    /// List bounded, non-secret metadata for local host approval UI.
    ///
    /// # Errors
    /// Fails closed if the registry lock is poisoned.
    pub fn pending_requests(&self) -> Result<Vec<PendingPairingRequest>, PairingError> {
        let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
        prune(&mut state);
        let mut requests: Vec<_> = state
            .requests
            .values()
            .filter(|request| request.state == RequestState::Pending)
            .map(|request| {
                let mut metadata = request.metadata.clone();
                metadata.expires_in_secs = request
                    .expires
                    .saturating_duration_since(Instant::now())
                    .as_secs();
                metadata
            })
            .collect();
        drop(state);
        requests.sort_by(|left, right| left.request_id.cmp(&right.request_id));
        Ok(requests)
    }

    /// Persist authorization, activate the live credential, then enable claiming.
    ///
    /// # Errors
    /// Rejects absent/expired requests or failed durable provisioning/activation.
    pub fn approve(
        &self,
        request_id: &str,
        port: &dyn PairingPort,
        activate: impl FnOnce(&PairingGrantDto) -> Result<(), PairingError>,
    ) -> Result<(), PairingError> {
        let metadata = {
            let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
            prune(&mut state);
            let request = state
                .requests
                .get_mut(request_id)
                .ok_or(PairingError::Unavailable)?;
            if request.state != RequestState::Pending {
                return Err(PairingError::Unavailable);
            }
            request.state = RequestState::Approving;
            let mut metadata = request.metadata.clone();
            metadata.expires_in_secs = request
                .expires
                .saturating_duration_since(Instant::now())
                .as_secs();
            drop(state);
            metadata
        };
        let result = port.approve(&metadata).and_then(|grant| {
            validate_grant(&grant, &metadata)?;
            activate(&grant)
        });
        let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
        let Some(request) = state.requests.get_mut(request_id) else {
            drop(state);
            self.compensate(&metadata, port)?;
            return Err(PairingError::Unavailable);
        };
        if result.is_err() || request.expires <= Instant::now() {
            state.requests.remove(request_id);
            drop(state);
            self.compensate(&metadata, port)?;
            return result.and(Err(PairingError::Unavailable));
        }
        request.state = RequestState::Approved;
        Ok(())
    }

    /// Reject a pending request without granting any permission.
    ///
    /// # Errors
    /// Rejects absent, expired, or approved requests. An in-progress approval
    /// is cancelled and conditionally compensated when its port call returns.
    pub fn deny(&self, request_id: &str) -> Result<(), PairingError> {
        let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
        prune(&mut state);
        if state.requests.get(request_id).is_none_or(|request| {
            !matches!(
                request.state,
                RequestState::Pending | RequestState::Approving
            )
        }) {
            return Err(PairingError::Unavailable);
        }
        state.requests.remove(request_id);
        drop(state);
        Ok(())
    }

    /// Deliver the explicitly approved credential once; replay always fails.
    ///
    /// # Errors
    /// Rejects invalid receipts, expiry, unapproved requests, and vault failures.
    pub fn claim(
        &self,
        dto: &PairingClaimDto,
        port: &dyn PairingPort,
    ) -> Result<PairingGrantDto, PairingError> {
        self.cleanup_expired(port)?;
        if dto.binding != self.binding
            || !valid_secret(&dto.request_id)
            || !valid_secret(&dto.claim_secret)
        {
            return Err(PairingError::InvalidRequest);
        }
        let metadata = {
            let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
            prune(&mut state);
            let request = state
                .requests
                .get_mut(&dto.request_id)
                .ok_or(PairingError::Unavailable)?;
            if request.metadata.device_id != dto.device_id
                || !secret_matches(&dto.claim_secret, &request.receipt_hash)
            {
                return Err(PairingError::Unavailable);
            }
            if request.state == RequestState::Pending || request.state == RequestState::Approving {
                return Err(PairingError::Pending);
            }
            if request.state != RequestState::Approved {
                return Err(PairingError::Unavailable);
            }
            request.state = RequestState::Claiming;
            let metadata = request.metadata.clone();
            drop(state);
            metadata
        };
        let loaded = port.load_approved_grant(&metadata);
        let transient_read_failure = matches!(&loaded, Err(PairingError::ProvisioningUnavailable));
        let result = loaded.and_then(|grant| validate_grant(&grant, &metadata).map(|()| grant));
        let mut state = self.state.lock().map_err(|_| PairingError::Unavailable)?;
        let Some(request) = state.requests.get_mut(&dto.request_id) else {
            // Cancellation owns compensation when it removed the request while
            // the vault read was in flight.
            return result.and(Err(PairingError::Unavailable));
        };
        if request.expires <= Instant::now() {
            state.requests.remove(&dto.request_id);
            drop(state);
            self.compensate(&metadata, port)?;
            return Err(PairingError::Unavailable);
        }
        match result {
            Ok(grant) => {
                state.requests.remove(&dto.request_id);
                Ok(grant)
            }
            Err(error) if transient_read_failure => {
                // A vault/authorization-store read can fail transiently. Keep
                // this exact approved generation retryable; no bearer escaped.
                request.state = RequestState::Approved;
                Err(error)
            }
            Err(error) => {
                state.requests.remove(&dto.request_id);
                drop(state);
                self.compensate(&metadata, port)?;
                Err(error)
            }
        }
    }
}

fn validate_grant(
    grant: &PairingGrantDto,
    request: &PendingPairingRequest,
) -> Result<(), PairingError> {
    if grant.binding != request.binding
        || grant.device_id != request.device_id
        || grant.bearer_token.len() < 32
        || grant.bearer_token.len() > 256
        || !grant
            .bearer_token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(PairingError::ProvisioningUnavailable);
    }
    Ok(())
}

fn prune(state: &mut RegistryState) {
    let now = Instant::now();
    state.invitations.retain(|_, invite| invite.expires > now);
    state
        .requests
        .retain(|_, request| request.expires > now || request.state != RequestState::Pending);
}

fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn valid_secret(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_invitation_secret(value: &str) -> bool {
    (value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())) || valid_secret(value)
}

fn secret_matches(secret: &str, hash: &[u8; 32]) -> bool {
    let digest = blake3::hash(secret.as_bytes());
    digest
        .as_bytes()
        .iter()
        .zip(hash)
        .fold(0u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

fn random_secret() -> String {
    // Three OS-random v4 UUIDs provide 366 bits of entropy before extraction.
    let mut entropy = [0; 48];
    for chunk in entropy.as_chunks_mut::<16>().0 {
        chunk.copy_from_slice(MutationId::new().as_uuid().as_bytes());
    }
    blake3::hash(&entropy).to_hex().to_string()
}

fn random_pairing_code() -> String {
    let random = MutationId::new().as_uuid().as_u128();
    format!("{:06}", random % 1_000_000)
}
