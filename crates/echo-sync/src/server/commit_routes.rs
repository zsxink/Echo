//! Bounded commit pagination and revocable long polling.

#[allow(clippy::wildcard_imports)]
use super::*;

#[derive(Deserialize)]
pub(super) struct CommitQuery {
    after: u64,
    previous_digest: String,
    limit: usize,
    /// Optional bounded long-poll: hold an empty page open until the host
    /// publishes or the budget elapses. Omitted or zero keeps the immediate
    /// response; the bound is `MAX_COMMIT_WAIT_MS`.
    wait_ms: Option<u32>,
}

/// Internal cadence for re-reading the published watermark while a commit page
/// is held open. Each recheck is a rowid seek on `sync_commits`, so a slow
/// cadence keeps idle cost near zero without adding visible latency.
const COMMIT_RECHECK_INTERVAL: Duration = Duration::from_millis(500);

pub(super) async fn pull_commits(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Query(query): Query<CommitQuery>,
) -> Response<Body> {
    let Some(device) = state.authenticated_device(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    if !peer_is_currently_authorized(&state, device).await {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if !allow_request(&state, device) {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    if query.limit == 0 || query.limit > MAX_PAGE_SIZE {
        return api_error(StatusCode::BAD_REQUEST, "invalid_page_size");
    }
    let wait = match query.wait_ms {
        Some(ms) if ms > MAX_COMMIT_WAIT_MS => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_wait");
        }
        Some(ms) => Duration::from_millis(u64::from(ms)),
        None => Duration::ZERO,
    };
    let Ok(anchor_digest) = parse_digest(&query.previous_digest) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_anchor_digest");
    };
    let mut page = match read_commit_page(&state, query.after, anchor_digest, query.limit).await {
        Ok(page) => page,
        Err((status, message)) => return api_error(status, message),
    };
    // Holding an empty page open is the peer's only timely way to learn about
    // commits published by another device: the host cannot push to a
    // listening-less peer. The held request already consumed its rate-limit
    // slot, and stopping the host ends the hold on the next recheck.
    if page.entries.is_empty() && !wait.is_zero() {
        let deadline = Instant::now() + wait;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if !state.active.load(Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(remaining.min(COMMIT_RECHECK_INTERVAL)).await;
            if !state.active.load(Ordering::Acquire) {
                break;
            }
            if !peer_is_currently_authorized(&state, device).await {
                return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
            }
            page = match read_commit_page(&state, query.after, anchor_digest, query.limit).await {
                Ok(page) => page,
                Err((status, message)) => return api_error(status, message),
            };
            if !page.entries.is_empty() {
                break;
            }
        }
    }
    authorized_response(&state, device, (StatusCode::OK, Json(page)).into_response()).await
}

/// Read one bounded commit page after the peer's checkpoint. The page build
/// runs on the blocking pool: the ledger port takes a pooled read connection,
/// which may queue behind a UI query, and a held long-poll rechecks this every
/// [`COMMIT_RECHECK_INTERVAL`] on the async runtime. Errors map to the same
/// host responses as the handler's first read.
async fn read_commit_page(
    state: &HostApiState,
    after: u64,
    anchor_digest: [u8; 32],
    limit: usize,
) -> Result<CommitPageDto, (StatusCode, &'static str)> {
    let ledger = Arc::clone(&state.ledger);
    let binding = state.binding;
    match tokio::task::spawn_blocking(move || {
        let after = echo_core::domain::sync::CommitSeq::from_u64(after);
        let commits = ledger
            .published_commits_after(binding, after, anchor_digest, limit)
            .map_err(|_| ())?;
        let mut page = CommitPageDto {
            binding,
            anchor_sequence: after,
            anchor_digest,
            entries: Vec::with_capacity(commits.len()),
        };
        let mut encoded_len = serde_json::to_vec(&page).map_err(|_| ())?.len();
        for PublishedCommit {
            sequence,
            previous_digest,
            digest,
            canonical_json,
        } in commits
        {
            let commit: CommitDto = serde_json::from_str(&canonical_json).map_err(|_| ())?;
            if commit.binding != binding
                || commit.commit_seq != sequence
                || commit.previous_digest != previous_digest
                || validate_mutation(&commit.mutation).is_err()
                || serde_json::to_vec(&commit).map_err(|_| ())?.len() > MAX_COMMIT_BYTES
                || commit
                    .resource_refs
                    .iter()
                    .any(|resource| validate_resource(resource).is_err())
                || commit_digest(&commit).map_err(|_| ())? != digest
            {
                return Err(());
            }
            let entry = CommittedEntryDto { commit, digest };
            let entry_len = serde_json::to_vec(&entry).map_err(|_| ())?.len();
            let extra = entry_len + usize::from(!page.entries.is_empty());
            if encoded_len.saturating_add(extra) > MAX_SYNC_PAGE_BYTES {
                if page.entries.is_empty() {
                    return Err(());
                }
                break;
            }
            encoded_len += extra;
            page.entries.push(entry);
        }
        Ok::<_, ()>(page)
    })
    .await
    {
        Ok(Ok(page)) => Ok(page),
        Ok(Err(())) => Err((StatusCode::CONFLICT, "commit_anchor_or_log_mismatch")),
        Err(_) => Err((StatusCode::SERVICE_UNAVAILABLE, "host_unavailable")),
    }
}
