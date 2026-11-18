use super::pairing::*;
use crate::pairing_pake::{DevicePake, PakeContext};
use crate::protocol::PROTOCOL_VERSION;
use echo_core::domain::sync::{AuthorityId, DeviceId, Epoch, LibraryId, SyncBinding};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

fn binding() -> SyncBinding {
    SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    }
}

#[derive(Default)]
struct Port {
    approved: Mutex<Vec<PendingPairingRequest>>,
    cancelled: Mutex<Vec<PendingPairingRequest>>,
    load_count: Mutex<usize>,
    load_errors: Mutex<Vec<PairingError>>,
    load_grant_override: Mutex<Option<PairingGrantDto>>,
    grant_override: Mutex<Option<PairingGrantDto>>,
    approve_barrier: Mutex<Option<Arc<Barrier>>>,
}

impl Port {
    fn grant(request: &PendingPairingRequest) -> PairingGrantDto {
        PairingGrantDto {
            binding: request.binding,
            device_id: request.device_id,
            bearer_token: "a".repeat(64),
        }
    }
}

impl PairingPort for Port {
    fn approve(&self, request: &PendingPairingRequest) -> Result<PairingGrantDto, PairingError> {
        self.approved.lock().unwrap().push(request.clone());
        let approve_barrier = self.approve_barrier.lock().unwrap().clone();
        if let Some(barrier) = approve_barrier {
            barrier.wait();
            barrier.wait();
        }
        Ok(self
            .grant_override
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Self::grant(request)))
    }

    fn cancel_approval(&self, request: &PendingPairingRequest) -> Result<(), PairingError> {
        self.cancelled.lock().unwrap().push(request.clone());
        Ok(())
    }

    fn load_approved_grant(
        &self,
        request: &PendingPairingRequest,
    ) -> Result<PairingGrantDto, PairingError> {
        *self.load_count.lock().unwrap() += 1;
        if !self.load_errors.lock().unwrap().is_empty() {
            return Err(self.load_errors.lock().unwrap().remove(0));
        }
        Ok(self
            .load_grant_override
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Self::grant(request)))
    }
}

fn invite(registry: &PairingRegistry, host: DeviceId) -> PairingInvitation {
    registry
        .create_invitation(host, vec![1, 2, 3], Duration::from_secs(60))
        .unwrap()
}

fn request_dto(invitation: &PairingInvitation, device: DeviceId) -> PairingRequestDto {
    PairingRequestDto {
        binding: invitation.binding,
        invitation_id: invitation.invitation_id.clone(),
        invitation_secret: invitation.invitation_secret.clone(),
        device_id: device,
        device_name: "Windows PC".into(),
    }
}

#[test]
fn invitation_rejects_wrong_binding_secret_self_pair_and_replay() {
    let expected_binding = binding();
    let registry = PairingRegistry::new(expected_binding);
    let host = DeviceId::new();
    let invitation = invite(&registry, host);

    let mut wrong_binding = request_dto(&invitation, DeviceId::new());
    wrong_binding.binding = binding();
    assert_eq!(
        registry.request(&wrong_binding).err().unwrap(),
        PairingError::InvalidRequest
    );

    let mut wrong_secret = request_dto(&invitation, DeviceId::new());
    let replacement = if wrong_secret.invitation_secret.starts_with('0') {
        "1"
    } else {
        "0"
    };
    wrong_secret
        .invitation_secret
        .replace_range(0..1, replacement);
    assert_eq!(
        registry.request(&wrong_secret).err().unwrap(),
        PairingError::Unavailable
    );

    assert_eq!(
        registry
            .request(&request_dto(&invitation, host))
            .err()
            .unwrap(),
        PairingError::InvalidRequest
    );

    let request = request_dto(&invitation, DeviceId::new());
    let receipt = registry.request(&request).unwrap();
    assert_eq!(receipt.device_id, request.device_id);
    assert_eq!(
        registry.request(&request).err().unwrap(),
        PairingError::Unavailable
    );
}

#[test]
fn expired_invitation_cannot_be_consumed() {
    let registry = PairingRegistry::new(binding());
    let invitation = registry
        .create_invitation(DeviceId::new(), vec![1], Duration::from_nanos(1))
        .unwrap();
    thread::sleep(Duration::from_millis(1));
    assert_eq!(
        registry
            .request(&request_dto(&invitation, DeviceId::new()))
            .err()
            .unwrap(),
        PairingError::Unavailable
    );
}

#[test]
fn deny_removes_pending_request_without_provisioning() {
    let registry = PairingRegistry::new(binding());
    let invitation = invite(&registry, DeviceId::new());
    let receipt = registry
        .request(&request_dto(&invitation, DeviceId::new()))
        .unwrap();
    let request = registry.pending_requests().unwrap().remove(0);
    registry.deny(&request.request_id).unwrap();
    assert!(registry.pending_requests().unwrap().is_empty());
    assert_eq!(
        registry.deny(&request.request_id),
        Err(PairingError::Unavailable)
    );
    assert_eq!(
        registry
            .claim(
                &PairingClaimDto {
                    binding: receipt.binding,
                    request_id: receipt.request_id,
                    device_id: receipt.device_id,
                    claim_secret: receipt.claim_secret,
                },
                &Port::default()
            )
            .err()
            .unwrap(),
        PairingError::Unavailable
    );
}

#[test]
fn claim_is_pending_until_approved_then_succeeds_once() {
    let registry = PairingRegistry::new(binding());
    let port = Port::default();
    let invitation = invite(&registry, DeviceId::new());
    let receipt = registry
        .request(&request_dto(&invitation, DeviceId::new()))
        .unwrap();
    let claim = PairingClaimDto {
        binding: receipt.binding,
        request_id: receipt.request_id.clone(),
        device_id: receipt.device_id,
        claim_secret: receipt.claim_secret.clone(),
    };
    assert_eq!(
        registry.claim(&claim, &port).err().unwrap(),
        PairingError::Pending
    );
    registry
        .approve(&receipt.request_id, &port, |_| Ok(()))
        .unwrap();
    assert_eq!(
        registry.claim(&claim, &port).unwrap().device_id,
        receipt.device_id
    );
    assert_eq!(
        registry.claim(&claim, &port).err().unwrap(),
        PairingError::Unavailable
    );
    assert_eq!(*port.load_count.lock().unwrap(), 1);
}

#[test]
fn transient_vault_read_failure_keeps_approval_claimable() {
    let registry = PairingRegistry::new(binding());
    let port = Port::default();
    let invitation = invite(&registry, DeviceId::new());
    let receipt = registry
        .request(&request_dto(&invitation, DeviceId::new()))
        .unwrap();
    registry
        .approve(&receipt.request_id, &port, |_| Ok(()))
        .unwrap();
    let claim = PairingClaimDto {
        binding: receipt.binding,
        request_id: receipt.request_id.clone(),
        device_id: receipt.device_id,
        claim_secret: receipt.claim_secret,
    };
    port.load_errors
        .lock()
        .unwrap()
        .push(PairingError::ProvisioningUnavailable);

    assert_eq!(
        registry.claim(&claim, &port).err().unwrap(),
        PairingError::ProvisioningUnavailable
    );
    assert_eq!(registry.pending_requests().unwrap().len(), 0);
    assert!(port.cancelled.lock().unwrap().is_empty());
    assert_eq!(
        registry.claim(&claim, &port).unwrap().device_id,
        receipt.device_id
    );
    assert_eq!(*port.load_count.lock().unwrap(), 2);
    assert!(port.cancelled.lock().unwrap().is_empty());
}

#[test]
fn revoked_or_invalid_claim_grant_is_compensated_and_consumed() {
    for invalid_grant in [false, true] {
        let registry = PairingRegistry::new(binding());
        let port = Port::default();
        let invitation = invite(&registry, DeviceId::new());
        let receipt = registry
            .request(&request_dto(&invitation, DeviceId::new()))
            .unwrap();
        let request = registry.pending_requests().unwrap().remove(0);
        registry
            .approve(&receipt.request_id, &port, |_| Ok(()))
            .unwrap();
        let claim = PairingClaimDto {
            binding: receipt.binding,
            request_id: receipt.request_id.clone(),
            device_id: receipt.device_id,
            claim_secret: receipt.claim_secret,
        };
        if invalid_grant {
            let mut grant = Port::grant(&request);
            grant.device_id = DeviceId::new();
            *port.load_grant_override.lock().unwrap() = Some(grant);
        } else {
            port.load_errors
                .lock()
                .unwrap()
                .push(PairingError::Unavailable);
        }

        assert_eq!(
            registry.claim(&claim, &port).err().unwrap(),
            if invalid_grant {
                PairingError::ProvisioningUnavailable
            } else {
                PairingError::Unavailable
            }
        );
        assert_eq!(port.cancelled.lock().unwrap().len(), 1);
        assert_eq!(
            registry.claim(&claim, &port).err().unwrap(),
            PairingError::Unavailable
        );
        assert_eq!(port.cancelled.lock().unwrap().len(), 1);
    }
}

#[test]
fn malformed_grants_are_compensated_and_never_claimable() {
    for wrong_device in [true, false] {
        let registry = PairingRegistry::new(binding());
        let port = Port::default();
        let invitation = invite(&registry, DeviceId::new());
        let receipt = registry
            .request(&request_dto(&invitation, DeviceId::new()))
            .unwrap();
        let request = registry.pending_requests().unwrap().remove(0);
        let mut grant = Port::grant(&request);
        if wrong_device {
            grant.device_id = DeviceId::new();
        } else {
            grant.binding = binding();
        }
        *port.grant_override.lock().unwrap() = Some(grant);
        assert_eq!(
            registry.approve(&receipt.request_id, &port, |_| Ok(())),
            Err(PairingError::ProvisioningUnavailable)
        );
        assert_eq!(port.cancelled.lock().unwrap().len(), 1);
        assert!(registry.pending_requests().unwrap().is_empty());
    }
}

#[test]
fn deny_during_approval_compensates_the_late_grant() {
    let registry = Arc::new(PairingRegistry::new(binding()));
    let barrier = Arc::new(Barrier::new(2));
    let port = Arc::new(Port::default());
    *port.approve_barrier.lock().unwrap() = Some(barrier.clone());
    let invitation = invite(&registry, DeviceId::new());
    let receipt = registry
        .request(&request_dto(&invitation, DeviceId::new()))
        .unwrap();

    let approving_registry = registry.clone();
    let approving_port = port.clone();
    let request_id = receipt.request_id.clone();
    let approval = thread::spawn(move || {
        approving_registry.approve(&request_id, approving_port.as_ref(), |_| Ok(()))
    });
    barrier.wait();
    registry.deny(&receipt.request_id).unwrap();
    barrier.wait();

    assert_eq!(approval.join().unwrap(), Err(PairingError::Unavailable));
    assert_eq!(port.cancelled.lock().unwrap().len(), 1);
    assert!(registry.pending_requests().unwrap().is_empty());
}

fn code_request(code: &str, device: DeviceId) -> (PairingCodeRequest, DevicePake) {
    let (message, state) = DevicePake::start(&PakeContext {
        code,
        device_id: device,
    })
    .unwrap();
    (
        PairingCodeRequest {
            protocol_version: PROTOCOL_VERSION,
            device_id: device,
            device_name: "Joining device".into(),
            message,
        },
        state,
    )
}

fn confirmation(
    receipt: &PairingReceipt,
    device: DeviceId,
    confirmation: Vec<u8>,
) -> PairingConfirmationDto {
    PairingConfirmationDto {
        protocol_version: PROTOCOL_VERSION,
        binding: receipt.binding,
        request_id: receipt.request_id.clone(),
        device_id: device,
        claim_secret: receipt.claim_secret.clone(),
        confirmation,
    }
}

#[test]
fn pake_pairing_rejects_malformed_and_wrong_codes() {
    let registry = PairingRegistry::new(binding());
    let invitation = invite(&registry, DeviceId::new());
    assert_eq!(invitation.invitation_secret.len(), 6);
    assert!(invitation
        .invitation_secret
        .bytes()
        .all(|byte| byte.is_ascii_digit()));
    let device = DeviceId::new();

    // A malformed first message never reaches the handshake.
    let (mut request, _state) = code_request(&invitation.invitation_secret, device);
    request.message.truncate(64);
    assert_eq!(
        registry.request_with_pake(&request).err(),
        Some(PairingError::InvalidRequest)
    );

    // A wrong code is not detectable at the first message, but the device
    // cannot verify the host's confirmation and never confirms.
    let wrong_code = if invitation.invitation_secret == "000000" {
        "111111"
    } else {
        "000000"
    };
    let (request, state) = code_request(wrong_code, device);
    let bootstrap = registry.request_with_pake(&request).unwrap();
    assert_eq!(bootstrap.host.host_certificate_der, vec![1, 2, 3]);
    assert_eq!(bootstrap.receipt.binding, bootstrap.host.binding);
    assert!(state
        .finish(&bootstrap.message, &bootstrap.confirmation)
        .is_err());
}

#[test]
fn pake_pairing_completes_once_and_rejects_spent_expired_and_rotated_codes() {
    let registry = PairingRegistry::new(binding());
    let invitation = invite(&registry, DeviceId::new());
    let device = DeviceId::new();

    // The correct code completes: the confirmation unlocks approval.
    let (request, state) = code_request(&invitation.invitation_secret, device);
    let bootstrap = registry.request_with_pake(&request).unwrap();
    let device_confirmation = state
        .finish(&bootstrap.message, &bootstrap.confirmation)
        .unwrap();
    assert_eq!(
        registry.confirm(&confirmation(
            &bootstrap.receipt,
            device,
            device_confirmation.confirmation
        )),
        Ok(())
    );
    let port = Port::default();
    registry
        .approve(&bootstrap.receipt.request_id, &port, |_| Ok(()))
        .unwrap();
    let grant = registry
        .claim(
            &PairingClaimDto {
                binding: bootstrap.receipt.binding,
                request_id: bootstrap.receipt.request_id.clone(),
                device_id: device,
                claim_secret: bootstrap.receipt.claim_secret,
            },
            &port,
        )
        .unwrap();
    assert_eq!(grant.device_id, device);
    assert!(registry.pending_requests().unwrap().is_empty());
    // The code is spent: another handshake finds no live invitation.
    let (request, _state) = code_request(&invitation.invitation_secret, device);
    assert_eq!(
        registry.request_with_pake(&request).err(),
        Some(PairingError::Unavailable)
    );

    // An expired invitation is unavailable before any handshake runs.
    let expired = registry
        .create_invitation(DeviceId::new(), vec![1], Duration::from_nanos(1))
        .unwrap();
    thread::sleep(Duration::from_millis(1));
    let (request, _state) = code_request(&expired.invitation_secret, device);
    assert_eq!(
        registry.request_with_pake(&request).err(),
        Some(PairingError::Unavailable)
    );

    // Rotation invalidates the old code immediately — even if an extremely
    // unlikely six-digit collision reuses its displayed code — because the
    // host derives its scalar from the current code only.
    let rotated = invite(&registry, DeviceId::new());
    let mut current = invite(&registry, DeviceId::new());
    while current.invitation_secret == rotated.invitation_secret {
        current = invite(&registry, DeviceId::new());
    }
    let (request, state) = code_request(&rotated.invitation_secret, device);
    let bootstrap = registry.request_with_pake(&request).unwrap();
    assert_eq!(bootstrap.host.invitation_id, current.invitation_id);
    assert!(state
        .finish(&bootstrap.message, &bootstrap.confirmation)
        .is_err());
}

#[test]
fn a_forged_confirmation_is_a_protocol_mismatch_and_consumes_the_handshake() {
    let registry = PairingRegistry::new(binding());
    let invitation = invite(&registry, DeviceId::new());
    let device = DeviceId::new();
    let (message, state) = DevicePake::start(&PakeContext {
        code: &invitation.invitation_secret,
        device_id: device,
    })
    .unwrap();
    let bootstrap = registry
        .request_with_pake(&PairingCodeRequest {
            protocol_version: PROTOCOL_VERSION,
            device_id: device,
            device_name: "Joining device".into(),
            message,
        })
        .unwrap();
    let genuine = state
        .finish(&bootstrap.message, &bootstrap.confirmation)
        .unwrap();
    let dto = |confirmation: Vec<u8>| PairingConfirmationDto {
        protocol_version: PROTOCOL_VERSION,
        binding: bootstrap.receipt.binding,
        request_id: bootstrap.receipt.request_id.clone(),
        device_id: device,
        claim_secret: bootstrap.receipt.claim_secret.clone(),
        confirmation,
    };

    let mut forged = genuine.confirmation.clone();
    forged[0] ^= 0xff;
    assert_eq!(
        registry.confirm(&dto(forged)).err(),
        Some(PairingError::ProtocolMismatch),
        "a wrong code or relay must not be reported as an expired code"
    );
    // Single use even on failure: the genuine MAC no longer has a handshake.
    assert_eq!(
        registry.confirm(&dto(genuine.confirmation)).err(),
        Some(PairingError::Unavailable)
    );
}

#[test]
fn short_code_lifetime_is_bounded_to_ten_minutes() {
    let registry = PairingRegistry::new(binding());
    for ttl in [Duration::ZERO, Duration::from_secs(601)] {
        assert_eq!(
            registry
                .create_invitation(DeviceId::new(), vec![1], ttl)
                .err(),
            Some(PairingError::InvalidRequest)
        );
    }
    assert_eq!(
        registry
            .create_invitation(DeviceId::new(), vec![1], Duration::from_secs(600))
            .unwrap()
            .expires_in_secs,
        600
    );
}
