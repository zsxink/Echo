//! Initial short-code credential exchange and validation before persistence.
//!
//! Pairing runs two PAKE rounds. The first request carries only the device's
//! SPAKE2 message; the host answers with its own message, its confirmation MAC
//! and a proof for the certificate under the derived session key. The device
//! pins that certificate only after both verify, then proves possession of the
//! code in a second request over the now-pinned channel, and only then
//! receives the credential.

#[allow(clippy::wildcard_imports)]
use super::*;
use crate::pairing_pake::{verify_certificate_proof, DeviceConfirmation, DevicePake, PakeContext};

/// Validate the host's answer to the first PAKE round and return the device's
/// confirmation MAC for the second round.
///
/// Nothing may be pinned or persisted before this returns `Ok`: the host has
/// to prove possession of the code, and the certificate has to be the one that
/// handshake authenticated.
pub(super) fn validate_pairing_bootstrap(
    pairing: &PairingBootstrapResponse,
    pake: DevicePake,
    device_id: DeviceId,
) -> Result<DeviceConfirmation, ClientError> {
    let host = &pairing.host;
    if host.host_certificate_der.is_empty()
        || host.host_certificate_der.len() > 16 * 1024
        || host.expires_in_secs > 600
        || pairing.receipt.binding != host.binding
        || pairing.receipt.device_id != device_id
    {
        return Err(ClientError::InvalidResponseBody);
    }
    // A relay that terminated the bootstrap TLS read the device's first
    // message, but it cannot derive the session key without the code, so it
    // cannot produce a host confirmation MAC that verifies.
    let confirmation = pake
        .finish(&pairing.message, &pairing.confirmation)
        .map_err(|_| ClientError::InvalidResponseBody)?;
    // The key pinned below must be the one this handshake authenticated: the
    // proof is keyed by the session key, which only a party holding the code
    // can derive. A relay can forward the SPAKE2 messages but cannot prove a
    // substituted certificate.
    verify_certificate_proof(
        &confirmation.session_key,
        &host.host_certificate_der,
        &pairing.certificate_proof,
    )
    .map_err(|_| ClientError::InvalidResponseBody)?;
    // A nonempty DER blob is not necessarily usable as a pin. Reject it at
    // bootstrap so an incompatible host cannot replace the existing binding.
    tls::pinned_config(&host.host_certificate_der).map_err(|_| ClientError::InvalidResponseBody)?;
    Ok(confirmation)
}

/// Validate the credential issued after the device's confirmation verified.
fn validate_pairing_grant(
    grant: &PairingGrantDto,
    host: &PairingHostDescriptor,
    device_id: DeviceId,
) -> Result<(), ClientError> {
    if grant.binding != host.binding
        || grant.device_id != device_id
        || grant.bearer_token.len() < 32
        || grant.bearer_token.len() > 256
    {
        return Err(ClientError::InvalidResponseBody);
    }
    Ok(())
}

impl LanClient {
    /// Pair with a local host using its six-digit code, then return the
    /// authenticated pin material and the issued credential.
    ///
    /// # Errors
    /// Rejects a non-TLS endpoint or malformed code, an unreachable host, a
    /// host-side rejection, a failed PAKE confirmation, and a malformed,
    /// unpinnable, or unproven response.
    pub async fn pair_with_code(
        endpoint: &Url,
        code: &str,
        device_id: DeviceId,
        device_name: String,
    ) -> Result<PairedHost, ClientError> {
        if endpoint.scheme() != "https"
            || code.len() != 6
            || !code.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ClientError::InvalidEndpoint);
        }
        let context = PakeContext { code, device_id };
        let (message, pake) =
            DevicePake::start(&context).map_err(|_| ClientError::PairingHandshake)?;
        // `danger_accept_invalid_certs` covers this one bootstrap request only:
        // no pin exists yet, so the certificate cannot be verified here. The
        // response is authenticated by the PAKE confirmation and certificate
        // proof above, and the confirmation request below is sent over a
        // client pinned to the authenticated certificate.
        let bootstrap = Client::builder()
            .https_only(true)
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .map_err(ClientError::Transport)?;
        let url = pairing_url(endpoint, "/v1/pairing/requests")?;
        let response = bootstrap
            .post(url)
            .json(&PairingCodeRequest {
                protocol_version: PROTOCOL_VERSION,
                device_id,
                device_name,
                message,
            })
            .send()
            .await
            .map_err(ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(host_rejected(response).await);
        }
        let pairing: PairingBootstrapResponse = read_bounded_json(response).await?;
        let confirmation = validate_pairing_bootstrap(&pairing, pake, device_id)?;
        drop(bootstrap);

        let tls = tls::pinned_config(&pairing.host.host_certificate_der)
            .map_err(ClientError::InvalidTlsConfiguration)?;
        let pinned = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .no_proxy()
            .use_preconfigured_tls(tls)
            .build()
            .map_err(ClientError::Transport)?;
        let url = pairing_url(endpoint, "/v1/pairing/confirmations")?;
        let response = pinned
            .post(url)
            .json(&PairingConfirmationDto {
                protocol_version: PROTOCOL_VERSION,
                binding: pairing.host.binding,
                request_id: pairing.receipt.request_id,
                device_id,
                claim_secret: pairing.receipt.claim_secret,
                confirmation: confirmation.confirmation,
            })
            .send()
            .await
            .map_err(ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(host_rejected(response).await);
        }
        let grant: PairingGrantDto = read_bounded_json(response).await?;
        validate_pairing_grant(&grant, &pairing.host, device_id)?;
        Ok(PairedHost {
            host: pairing.host,
            grant,
        })
    }
}
