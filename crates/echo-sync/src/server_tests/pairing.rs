//! Pairing protocol and code-failure contracts at the HTTP boundary.

use super::*;

/// Pairing port that never provisions: the version test never approves.
struct NoProvisioning;

impl PairingPort for NoProvisioning {
    fn approve(&self, _request: &PendingPairingRequest) -> Result<PairingGrantDto, PairingError> {
        Err(PairingError::ProvisioningUnavailable)
    }

    fn cancel_approval(&self, _request: &PendingPairingRequest) -> Result<(), PairingError> {
        Ok(())
    }

    fn load_approved_grant(
        &self,
        _request: &PendingPairingRequest,
    ) -> Result<PairingGrantDto, PairingError> {
        Err(PairingError::ProvisioningUnavailable)
    }
}

#[tokio::test]
async fn protocol_v1_pairing_is_rejected_with_a_distinguishable_error() {
    let mut fixture = Fixture::new();
    let binding = fixture.mutation.binding;
    fixture.state = fixture.state.clone().with_pairing(
        Arc::new(PairingRegistry::new(binding)),
        Arc::new(NoProvisioning),
    );
    let request = |version: u16| {
        serde_json::to_string(&PairingCodeRequest {
            protocol_version: version,
            device_id: DeviceId::new(),
            device_name: "Joining device".into(),
            message: vec![0; crate::pairing_pake::PAKE_MESSAGE_BYTES],
        })
        .unwrap()
    };

    // A version 1 peer is rejected before any handshake state exists, and the
    // rejection is a version error — not the code failure a wrong code yields.
    assert_error(
        fixture
            .request(
                "POST",
                "/v1/pairing/requests",
                None,
                request(PROTOCOL_VERSION - 1),
            )
            .await,
        StatusCode::BAD_REQUEST,
        "pairing_protocol_version",
    )
    .await;
    // The same body at the current version reaches the registry, which reports
    // the absent invitation as the code-failure category instead.
    assert_error(
        fixture
            .request(
                "POST",
                "/v1/pairing/requests",
                None,
                request(PROTOCOL_VERSION),
            )
            .await,
        StatusCode::NOT_FOUND,
        "pairing_unavailable",
    )
    .await;
    // The confirmation route applies the same gate.
    assert_error(
        fixture
            .request(
                "POST",
                "/v1/pairing/confirmations",
                None,
                serde_json::to_string(&PairingConfirmationDto {
                    protocol_version: PROTOCOL_VERSION - 1,
                    binding,
                    request_id: "request".into(),
                    device_id: DeviceId::new(),
                    claim_secret: "secret".into(),
                    confirmation: vec![0; crate::pairing_pake::PAKE_CONFIRMATION_BYTES],
                })
                .unwrap(),
            )
            .await,
        StatusCode::BAD_REQUEST,
        "pairing_protocol_version",
    )
    .await;
}

#[tokio::test]
async fn expired_or_used_code_still_reports_code_failure() {
    let mut fixture = Fixture::new();
    let binding = fixture.mutation.binding;
    let registry = Arc::new(PairingRegistry::new(binding));
    fixture.state = fixture
        .state
        .clone()
        .with_pairing(registry.clone(), Arc::new(NoProvisioning));
    let host = DeviceId::new();
    let device = DeviceId::new();
    let message = |code: &str| {
        let (message, _) =
            crate::pairing_pake::DevicePake::start(&crate::pairing_pake::PakeContext {
                code,
                device_id: device,
            })
            .unwrap();
        serde_json::to_string(&PairingCodeRequest {
            protocol_version: PROTOCOL_VERSION,
            device_id: device,
            device_name: "Joining device".into(),
            message,
        })
        .unwrap()
    };

    // An expired code is the code-failure category, not a protocol failure.
    let expired = registry
        .create_invitation(host, vec![1, 2, 3], Duration::from_nanos(1))
        .unwrap();
    std::thread::sleep(Duration::from_millis(1));
    assert_error(
        fixture
            .request(
                "POST",
                "/v1/pairing/requests",
                None,
                message(&expired.invitation_secret),
            )
            .await,
        StatusCode::NOT_FOUND,
        "pairing_unavailable",
    )
    .await;

    // A spent code is the same category: a successful confirmation consumes
    // the invitation, and the next handshake finds none.
    let invitation = registry
        .create_invitation(host, vec![1, 2, 3], Duration::from_secs(60))
        .unwrap();
    let (pake_message, pake) =
        crate::pairing_pake::DevicePake::start(&crate::pairing_pake::PakeContext {
            code: &invitation.invitation_secret,
            device_id: device,
        })
        .unwrap();
    let bootstrap = registry
        .request_with_pake(&PairingCodeRequest {
            protocol_version: PROTOCOL_VERSION,
            device_id: device,
            device_name: "Joining device".into(),
            message: pake_message,
        })
        .unwrap();
    let confirmation = pake
        .finish(&bootstrap.message, &bootstrap.confirmation)
        .unwrap();
    registry
        .confirm(&PairingConfirmationDto {
            protocol_version: PROTOCOL_VERSION,
            binding: bootstrap.receipt.binding,
            request_id: bootstrap.receipt.request_id,
            device_id: device,
            claim_secret: bootstrap.receipt.claim_secret,
            confirmation: confirmation.confirmation,
        })
        .unwrap();
    assert_error(
        fixture
            .request(
                "POST",
                "/v1/pairing/requests",
                None,
                message(&invitation.invitation_secret),
            )
            .await,
        StatusCode::NOT_FOUND,
        "pairing_unavailable",
    )
    .await;
}

#[tokio::test]
async fn pake_mismatch_reports_protocol_failure_not_code_failure() {
    let mut fixture = Fixture::new();
    let binding = fixture.mutation.binding;
    let registry = Arc::new(PairingRegistry::new(binding));
    fixture.state = fixture
        .state
        .clone()
        .with_pairing(registry.clone(), Arc::new(NoProvisioning));
    let device = DeviceId::new();
    let invitation = registry
        .create_invitation(DeviceId::new(), vec![1, 2, 3], Duration::from_secs(60))
        .unwrap();
    // A device holding the wrong code cannot verify the host's confirmation,
    // so a relay that terminated the bootstrap can at best forge one.
    let wrong_code = if invitation.invitation_secret == "000000" {
        "111111"
    } else {
        "000000"
    };
    let (pake_message, pake) =
        crate::pairing_pake::DevicePake::start(&crate::pairing_pake::PakeContext {
            code: wrong_code,
            device_id: device,
        })
        .unwrap();
    let bootstrap: PairingBootstrapResponse = serde_json::from_slice(
        &axum::body::to_bytes(
            fixture
                .request(
                    "POST",
                    "/v1/pairing/requests",
                    None,
                    serde_json::to_string(&PairingCodeRequest {
                        protocol_version: PROTOCOL_VERSION,
                        device_id: device,
                        device_name: "Joining device".into(),
                        message: pake_message,
                    })
                    .unwrap(),
                )
                .await
                .into_body(),
            64 * 1024,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert!(
        pake.finish(&bootstrap.message, &bootstrap.confirmation)
            .is_err(),
        "the wrong code must not verify the host confirmation"
    );
    assert_error(
        fixture
            .request(
                "POST",
                "/v1/pairing/confirmations",
                None,
                serde_json::to_string(&PairingConfirmationDto {
                    protocol_version: PROTOCOL_VERSION,
                    binding: bootstrap.receipt.binding,
                    request_id: bootstrap.receipt.request_id,
                    device_id: device,
                    claim_secret: bootstrap.receipt.claim_secret,
                    confirmation: vec![0; crate::pairing_pake::PAKE_CONFIRMATION_BYTES],
                })
                .unwrap(),
            )
            .await,
        StatusCode::BAD_REQUEST,
        "pairing_protocol_mismatch",
    )
    .await;
}
