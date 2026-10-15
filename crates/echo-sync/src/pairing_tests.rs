use super::pairing::*;
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
