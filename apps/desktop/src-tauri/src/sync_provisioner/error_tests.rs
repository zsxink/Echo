//! Preserve resource failures separately from device authorization failures.

use super::{map_sync_client_error, ClientError, SyncRuntimeError};
use reqwest::StatusCode;

#[test]
fn missing_resource_is_not_a_pairing_or_authorization_failure() {
    let error = map_sync_client_error(ClientError::HostRejectedCode(
        StatusCode::NOT_FOUND,
        "resource_not_found".to_owned(),
    ));
    assert_eq!(error, SyncRuntimeError::SyncResourceUnavailable);
    assert_eq!(error.message_key(), "sync.resourceUnavailable");
}

#[test]
fn authentication_and_other_resource_errors_keep_their_classification() {
    for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
        assert_eq!(
            map_sync_client_error(ClientError::HostRejectedCode(
                status,
                "resource_not_found".to_owned(),
            )),
            SyncRuntimeError::SyncAuthorizationFailed,
        );
    }
    assert_eq!(
        map_sync_client_error(ClientError::HostRejectedCode(
            StatusCode::CONFLICT,
            "resource_content_mismatch".to_owned(),
        )),
        SyncRuntimeError::SyncResourceMismatch,
    );
}

#[tokio::test]
async fn transport_timeout_keeps_the_public_connection_error() {
    // Hold an actual socket open without serving HTTP: no external network or
    // endpoint text needs to enter the diagnostic classification.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_millis(100))
        .build()
        .unwrap();
    let error = client
        .get(format!(
            "http://{}/private?token=test",
            listener.local_addr().unwrap()
        ))
        .send()
        .await
        .unwrap_err();
    assert!(error.is_timeout());
    assert_eq!(
        map_sync_client_error(ClientError::Transport(error)),
        SyncRuntimeError::SyncConnectionFailed
    );
}
