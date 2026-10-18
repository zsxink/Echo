//! Initial short-code credential exchange and validation before persistence.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Validate all material before the desktop persists a newly paired identity.
pub(super) fn validate_pairing_bootstrap(
    pairing: &PairingBootstrapResponse,
    code: &str,
    device_id: DeviceId,
) -> Result<(), ClientError> {
    let invitation = &pairing.invitation;
    if invitation.invitation_secret != code
        || invitation.host_certificate_der.is_empty()
        || invitation.host_certificate_der.len() > 16 * 1024
        || invitation.expires_in_secs > 600
        || pairing.grant.binding != invitation.binding
        || pairing.grant.device_id != device_id
        || pairing.grant.bearer_token.len() < 32
        || pairing.grant.bearer_token.len() > 256
    {
        return Err(ClientError::InvalidResponseBody);
    }
    // A nonempty DER blob is not necessarily usable as a pin. Reject it at
    // bootstrap so an incompatible host cannot replace the existing binding.
    tls::pinned_config(&invitation.host_certificate_der)
        .map_err(|_| ClientError::InvalidResponseBody)?;
    Ok(())
}

impl LanClient {
    /// Exchange a local six-digit pairing code before the first certificate
    /// pin exists. The returned certificate is pinned for all later traffic.
    /// This bootstrap is deliberately limited to the one pairing request.
    ///
    /// # Errors
    /// Rejects a non-TLS endpoint or malformed code, an unreachable host, a
    /// host-side rejection, and a malformed or unpinnable response.
    pub async fn pair_with_code(
        endpoint: &Url,
        code: &str,
        device_id: DeviceId,
        device_name: String,
    ) -> Result<PairingBootstrapResponse, ClientError> {
        if endpoint.scheme() != "https"
            || code.len() != 6
            || !code.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ClientError::InvalidEndpoint);
        }
        let http = Client::builder()
            .https_only(true)
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .map_err(ClientError::Transport)?;
        let url = pairing_url(endpoint, "/v1/pairing/requests")?;
        let response = http
            .post(url)
            .json(&PairingCodeRequest {
                code: code.to_owned(),
                device_id,
                device_name,
            })
            .send()
            .await
            .map_err(ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(ClientError::HostRejected(response.status()));
        }
        let pairing: PairingBootstrapResponse = read_bounded_json(response).await?;
        validate_pairing_bootstrap(&pairing, code, device_id)?;
        Ok(pairing)
    }
}
