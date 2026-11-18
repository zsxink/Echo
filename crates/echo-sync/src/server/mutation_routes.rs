//! Host mutation submission: validation, arbitration input, confirmation.

#[allow(clippy::wildcard_imports)]
use super::*;

use echo_core::application::sync::ConfirmHostMutation;
use echo_core::domain::sync::MutationCandidate;

use crate::protocol::{validate_mutation, MutationAck};

pub(super) async fn submit_mutation(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Json(mutation): Json<MutationDto>,
) -> Response<Body> {
    let Some(authenticated_device) = state.authenticated_device(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    if !peer_is_currently_authorized(&state, authenticated_device).await {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if !allow_request(&state, authenticated_device) {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    if authenticated_device != mutation.origin_device_id {
        return api_error(StatusCode::FORBIDDEN, "origin_device_mismatch");
    }
    if mutation.binding != state.binding {
        return api_error(StatusCode::CONFLICT, "library_binding_mismatch");
    }
    if validate_mutation(&mutation).is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_mutation");
    }
    if !mutation.resource_refs.is_empty() {
        let Some(resources) = state.resources.as_ref().map(Arc::clone) else {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_storage_unavailable",
            );
        };
        let binding = state.binding;
        let references = mutation.resource_refs.clone();
        match tokio::task::spawn_blocking(move || {
            references
                .iter()
                .try_for_each(|reference| resources.verify_complete(binding, reference))
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return api_error(StatusCode::CONFLICT, "resource_not_complete"),
            Err(_) => {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "resource_storage_unavailable",
                )
            }
        }
        // Revocation or stop may happen while a large file is being verified.
        if !peer_is_currently_authorized(&state, authenticated_device).await {
            return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
        }
    }
    let Ok(resource_refs) = mutation
        .resource_refs
        .iter()
        .map(crate::protocol::ResourceRefDto::to_manifest)
        .collect::<Result<Vec<_>, _>>()
    else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_resource_reference");
    };
    let candidate = MutationCandidate {
        resource_refs,
        mutation_id: mutation.mutation_id,
        origin_device_id: mutation.origin_device_id,
        origin_seq: mutation.origin_seq,
        previous_mutation_id: mutation.previous_mutation_id,
        object: mutation.object,
        base_object_version: mutation.base_object_version,
        payload: mutation.payload,
    };
    let ledger = Arc::clone(&state.ledger);
    let checkpoint = Arc::clone(&state.checkpoint);
    let binding = state.binding;
    // A peer-supplied edit time is untrusted: clamp it before arbitration so a
    // device whose clock is far ahead cannot outrank every future local edit.
    let changed_at_ms = state.arbitrated_changed_at_ms(mutation.changed_at_ms);
    match tokio::task::spawn_blocking(move || {
        ConfirmHostMutation::new(ledger.as_ref(), checkpoint.as_ref()).execute_at(
            binding,
            &candidate,
            changed_at_ms,
        )
    })
    .await
    {
        Ok(Ok(confirmation)) => {
            authorized_response(
                &state,
                authenticated_device,
                (StatusCode::OK, Json(MutationAck { confirmation })).into_response(),
            )
            .await
        }
        Ok(Err(_)) => api_error(StatusCode::CONFLICT, "mutation_rejected"),
        Err(_) => api_error(StatusCode::SERVICE_UNAVAILABLE, "host_unavailable"),
    }
}
