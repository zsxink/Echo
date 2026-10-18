//! Authenticated bounded resource transfer endpoints.

#[allow(clippy::wildcard_imports)]
use super::*;

async fn resource_access(
    state: &HostApiState,
    headers: &HeaderMap,
) -> Result<(Arc<dyn ResourceTransferPort>, DeviceId), Box<Response<Body>>> {
    let Some(device) = state.authenticated_device(headers) else {
        return Err(Box::new(api_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
        )));
    };
    if !peer_is_currently_authorized(state, device).await {
        return Err(Box::new(api_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
        )));
    }
    if !allow_resource_request(state, device) {
        return Err(Box::new(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
        )));
    }
    let binding = headers
        .get("x-echo-binding")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| serde_json::from_str::<SyncBinding>(value).ok());
    if binding != Some(state.binding) {
        return Err(Box::new(api_error(
            StatusCode::CONFLICT,
            "library_binding_mismatch",
        )));
    }
    let resources = state.resources.as_ref().map(Arc::clone).ok_or_else(|| {
        Box::new(api_error(
            StatusCode::NOT_IMPLEMENTED,
            "resources_unavailable",
        ))
    })?;
    Ok((resources, device))
}

pub(super) async fn prepare_resource(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Json(request): Json<PrepareResource>,
) -> Response<Body> {
    let (resources, device) = match resource_access(&state, &headers).await {
        Ok(access) => access,
        Err(error) => return *error,
    };
    if request.binding != state.binding
        || validate_resource(&request.resource).is_err()
        || request.resource.length > MAX_RESOURCE_BYTES
    {
        return api_error(StatusCode::BAD_REQUEST, "invalid_resource");
    }
    transfer_response(
        tokio::task::spawn_blocking(move || resources.prepare(device, &request)).await,
    )
}

pub(super) async fn upload_chunk(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Path((upload, index)): Path<(String, u32)>,
    bytes: axum::body::Bytes,
) -> Response<Body> {
    let (resources, device) = match resource_access(&state, &headers).await {
        Ok(access) => access,
        Err(error) => return *error,
    };
    let Some(hash) = headers
        .get("x-echo-chunk-hash")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_chunk_hash");
    };
    let binding = state.binding;
    transfer_response(
        tokio::task::spawn_blocking(move || {
            resources.write_chunk(binding, device, &upload, index, &hash, &bytes)
        })
        .await,
    )
}

pub(super) async fn complete_resource(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Path(upload): Path<String>,
) -> Response<Body> {
    let (resources, device) = match resource_access(&state, &headers).await {
        Ok(access) => access,
        Err(error) => return *error,
    };
    let binding = state.binding;
    transfer_response(
        tokio::task::spawn_blocking(move || resources.complete(binding, device, &upload)).await,
    )
}

pub(super) async fn download_resource(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> Response<Body> {
    let (resources, _) = match resource_access(&state, &headers).await {
        Ok(access) => access,
        Err(error) => return *error,
    };
    let Some(range) = headers
        .get("range")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes="))
    else {
        return api_error(StatusCode::RANGE_NOT_SATISFIABLE, "bounded_range_required");
    };
    let Some((start, end)) = range.split_once('-') else {
        return api_error(StatusCode::RANGE_NOT_SATISFIABLE, "invalid_range");
    };
    let (Ok(offset), Ok(end)) = (start.parse::<u64>(), end.parse::<u64>()) else {
        return api_error(StatusCode::RANGE_NOT_SATISFIABLE, "invalid_range");
    };
    let Some(length) = end
        .checked_sub(offset)
        .and_then(|value| value.checked_add(1))
        .and_then(|value| usize::try_from(value).ok())
    else {
        return api_error(StatusCode::RANGE_NOT_SATISFIABLE, "invalid_range");
    };
    if length > crate::protocol::MAX_RESOURCE_CHUNK_BYTES {
        return api_error(StatusCode::RANGE_NOT_SATISFIABLE, "invalid_range");
    }
    if headers
        .get("if-match")
        .is_some_and(|value| value.to_str().ok() != Some(format!("\"{hash}\"").as_str()))
    {
        return api_error(StatusCode::PRECONDITION_FAILED, "resource_version_mismatch");
    }
    let binding = state.binding;
    match tokio::task::spawn_blocking(move || resources.download(binding, &hash, offset, length))
        .await
    {
        Ok(Ok(chunk)) => {
            let range = if chunk.bytes.is_empty() {
                "bytes */0".to_owned()
            } else {
                format!(
                    "bytes {}-{}/{}",
                    offset,
                    offset + chunk.bytes.len() as u64 - 1,
                    chunk.total_length
                )
            };
            (
                StatusCode::PARTIAL_CONTENT,
                [
                    ("content-range", range),
                    ("etag", format!("\"{}\"", chunk.content_hash)),
                    ("x-echo-chunk-hash", chunk.chunk_hash),
                    ("accept-ranges", "bytes".to_owned()),
                ],
                chunk.bytes,
            )
                .into_response()
        }
        Ok(Err(error)) => transfer_error(&error),
        Err(_) => api_error(StatusCode::SERVICE_UNAVAILABLE, "resource_unavailable"),
    }
}

fn transfer_response(
    result: Result<
        Result<crate::resource_transfer::TransferProgress, TransferError>,
        tokio::task::JoinError,
    >,
) -> Response<Body> {
    match result {
        Ok(Ok(progress)) => (StatusCode::OK, Json(progress)).into_response(),
        Ok(Err(error)) => transfer_error(&error),
        Err(_) => api_error(StatusCode::SERVICE_UNAVAILABLE, "resource_unavailable"),
    }
}

fn transfer_error(error: &TransferError) -> Response<Body> {
    let (status, code) = match error {
        TransferError::Invalid => (StatusCode::BAD_REQUEST, "invalid_resource_transfer"),
        TransferError::Unknown => (StatusCode::NOT_FOUND, "resource_not_found"),
        TransferError::Conflict => (StatusCode::CONFLICT, "resource_content_mismatch"),
        TransferError::Capacity => (
            StatusCode::INSUFFICIENT_STORAGE,
            "resource_capacity_exceeded",
        ),
        TransferError::Storage(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "resource_storage_unavailable",
        ),
    };
    api_error(status, code)
}
