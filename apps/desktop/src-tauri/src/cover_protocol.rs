//! The `cover://` URI-scheme protocol handler and its security helpers
//! (canvas pixel-read origin policy, media-type sniffing).
//!
//! The handler's signature is fixed by tauri's
//! [`register_asynchronous_uri_scheme_protocol`](tauri::Builder::register_asynchronous_uri_scheme_protocol):
//! the request arrives owned and the callback must take it by value, so
//! `needless_pass_by_value` does not apply here. The first thumbnail request
//! for a key decodes and scales the original; that work runs on a blocking
//! thread so it never stalls the `WebView` scheme callback (on macOS the
//! `WKURLSchemeHandler` callback is on the main thread).
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use echo_core::application::ports::CoverCache;
use echo_core::error::Error;
use echo_desktop::platform::security::{CoverError, CoverProtocol, CoverRequest, CoverSize};
use tauri::http::{Request as HttpRequest, Response as HttpResponse};
use tauri::Manager;

/// Where a cover URI goes: served from the cache, or rejected before the
/// cache is touched. Keeping this pure makes "a malformed key never reaches
/// the blocking worker" a tested fact rather than an ordering assumption.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoverRoute {
    Serve(CoverRequest),
    Respond(u16),
}

#[must_use]
pub fn route_cover_uri(uri: &str) -> CoverRoute {
    match CoverProtocol::parse(uri) {
        Ok(request) => CoverRoute::Serve(request),
        Err(CoverError::KeyTooLong) => CoverRoute::Respond(400),
        // Wrong scheme, missing or malformed key — the request is not for us,
        // answer 404 (never a redirect or a fallthrough read).
        Err(_) => CoverRoute::Respond(404),
    }
}

/// Resolve a parsed request against the cache. `Original` reads the original
/// bytes; the two thumbnail tiers share the Core cache's two-tier API.
pub fn cover_bytes(
    cache: &dyn CoverCache,
    request: &CoverRequest,
) -> Result<Option<Vec<u8>>, Error> {
    match request.size {
        CoverSize::Original => cache.get(&request.asset_key),
        CoverSize::List => cache.thumbnail(&request.asset_key, false),
        CoverSize::Detail => cache.thumbnail(&request.asset_key, true),
    }
}

/// Build the response for a resolved cover.
///
/// The asset key is content-addressed, so the bytes for a given `(key, tier)`
/// never change; mark the response immutable so the `WebView` can keep its
/// decoded copy instead of refetching it for every list row that reuses it.
#[must_use]
pub fn cover_response(bytes: Vec<u8>, origin: Option<&str>) -> HttpResponse<Vec<u8>> {
    let mut response = HttpResponse::builder()
        .status(200)
        .header("Content-Type", cover_media_type(&bytes))
        .header("Vary", "Origin")
        .header("Cache-Control", "private, max-age=31536000, immutable");
    // Canvas extraction needs an origin-clean image. Grant pixel reads only to
    // the bundled renderer (and, in a dev build, the local dev server —
    // whatever port it picked).
    if let Some(origin) = origin {
        if cover_canvas_origin_allowed(origin) {
            response = response.header("Access-Control-Allow-Origin", origin);
        } else if cfg!(debug_assertions) {
            // A refused pixel read is otherwise completely silent: the probe's
            // `crossOrigin` request fails, the palette resolves to `null`, and
            // the immersive surface falls back to the theme colour for *every*
            // song while the DOM cover still renders. This line is the only
            // place the refusal is observable, so make it observable on
            // purpose.
            eprintln!("cover: refused pixel access to origin {origin}");
        }
    }
    response.body(bytes).unwrap_or_default()
}

/// Resolve + map cache outcomes to a response. Unknown/malformed key for the
/// store, a transient storage miss, and a failed thumbnail all become the same
/// 404: the UI never sees a partial or guessed image.
#[must_use]
pub fn cover_result_response(
    cache: &dyn CoverCache,
    request: &CoverRequest,
    origin: Option<&str>,
) -> HttpResponse<Vec<u8>> {
    match cover_bytes(cache, request) {
        Ok(Some(bytes)) => cover_response(bytes, origin),
        Ok(None) | Err(_) => empty_response(404),
    }
}

fn empty_response(status: u16) -> HttpResponse<Vec<u8>> {
    HttpResponse::builder()
        .status(status)
        .body(vec![])
        .unwrap_or_default()
}

pub fn cover_protocol_handler<R: tauri::Runtime>(
    context: tauri::UriSchemeContext<'_, R>,
    request: HttpRequest<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let uri = request.uri().to_string();
    let asset = match route_cover_uri(&uri) {
        CoverRoute::Serve(request) => request,
        CoverRoute::Respond(status) => {
            responder.respond(empty_response(status));
            return;
        }
    };

    let Some(cache) = context
        .app_handle()
        .try_state::<Arc<dyn CoverCache>>()
        .map(|state| Arc::clone(&state))
    else {
        responder.respond(empty_response(404));
        return;
    };
    let origin = request
        .headers()
        .get("Origin")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);

    // First-visit thumbnail generation decodes and scales an original; keep it
    // off the scheme callback thread. The cache's per-(key, tier) single-flight
    // gate collapses concurrent requests for the same image.
    tauri::async_runtime::spawn_blocking(move || {
        responder.respond(cover_result_response(
            cache.as_ref(),
            &asset,
            origin.as_deref(),
        ));
    });
}

/// The origins allowed to read cover **pixels** (an origin-clean `<img>` plus a
/// canvas read-back).
///
/// The bundled renderer has one fixed origin per platform — see the three
/// literals below. A dev build does not: with no `devUrl` configured, `tauri
/// dev` serves `frontendDist` from its own static server, which picks its port
/// at runtime (1430, or the next free one). Naming a single dev port here made
/// the whitelist stop matching the moment the CLI chose another one — and that
/// failure is invisible end to end: the probe's `crossOrigin` request gets no
/// `Access-Control-Allow-Origin`, the palette resolves to `null`, and the
/// immersive surface falls back to the theme colour for *every* song, while the
/// DOM cover (which needs no CORS) keeps rendering normally.
///
/// A dev build therefore accepts any loopback origin rather than a guessed
/// port. Release builds keep the fixed, enumerated set.
pub fn cover_canvas_origin_allowed(origin: &str) -> bool {
    if matches!(
        origin,
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    ) {
        return true;
    }
    cfg!(debug_assertions) && is_loopback_http_origin(origin)
}

/// `http://<loopback>:<port>` — the shape of a local dev server's origin.
///
/// The port is required, and the host must be loopback: `Origin: null` (a
/// sandboxed or `data:` document), `http://localhost` with no port, and
/// `http://localhost.example.com` all parse as URLs but none of them is the
/// renderer.
pub fn is_loopback_http_origin(origin: &str) -> bool {
    tauri::Url::parse(origin).is_ok_and(|url| {
        url.scheme() == "http"
            && url.port().is_some()
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    })
}

/// The media type of an embedded cover, derived from the bytes themselves.
///
/// The cache stores the original `mime.txt` next to the bytes, but the read port
/// resolves bytes only — and a wildcard `image/*` is not a concrete media type,
/// so `WebKit` will not decode the response into an `<img>`. The protocol
/// therefore sniffs the container magic of the bytes it is about to serve. An
/// unrecognised container is still served (as `application/octet-stream`) rather
/// than hidden: a candidate that fails to decode is a local-library fact, not
/// something the shell should pretend is missing.
pub const fn cover_media_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use echo_core::error::Error;

    /// Records which cache entry point each tier selects; the bytes are
    /// irrelevant to dispatch.
    #[derive(Default)]
    struct RecordingCache {
        calls: Mutex<Vec<String>>,
        original: Option<Vec<u8>>,
        thumbnail: Option<Vec<u8>>,
        thumbnail_error: bool,
    }

    impl CoverCache for RecordingCache {
        fn put(&self, _bytes: &[u8], _mime: &str) -> Result<String, Error> {
            unreachable!("protocol reads never write covers")
        }

        fn get(&self, asset_key: &str) -> Result<Option<Vec<u8>>, Error> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("get:{asset_key}"));
            Ok(self.original.clone())
        }

        fn thumbnail(&self, asset_key: &str, detail: bool) -> Result<Option<Vec<u8>>, Error> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("thumbnail:{asset_key}:{detail}"));
            if self.thumbnail_error {
                return Err(Error::io(
                    "test thumbnail",
                    std::io::Error::other("injected failure"),
                    std::path::PathBuf::from("/tmp/thumb-128.bin"),
                ));
            }
            Ok(self.thumbnail.clone())
        }

        fn gc(&self, _referenced_keys: &[String]) -> Result<(), Error> {
            unreachable!("protocol reads never garbage-collect covers")
        }
    }

    fn request(size: CoverSize) -> CoverRequest {
        CoverRequest {
            asset_key: "cv1-x".to_owned(),
            size,
        }
    }

    #[test]
    fn cover_bytes_dispatches_original_and_thumbnail_tiers() {
        let cache = RecordingCache {
            original: Some(vec![1]),
            thumbnail: Some(vec![2]),
            ..RecordingCache::default()
        };
        for size in [CoverSize::Original, CoverSize::List, CoverSize::Detail] {
            assert_eq!(
                cover_bytes(&cache, &request(size)).unwrap(),
                Some(match size {
                    CoverSize::Original => vec![1],
                    CoverSize::List | CoverSize::Detail => vec![2],
                })
            );
        }
        assert_eq!(
            *cache
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![
                "get:cv1-x".to_owned(),
                "thumbnail:cv1-x:false".to_owned(),
                "thumbnail:cv1-x:true".to_owned(),
            ]
        );
    }

    #[test]
    fn cover_responses_are_immutable_and_keep_cors_vary() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];

        let allowed = cover_response(jpeg.clone(), Some("tauri://localhost"));
        assert_eq!(allowed.status(), 200);
        assert_eq!(allowed.headers().get("Content-Type").unwrap(), "image/jpeg");
        assert_eq!(allowed.headers().get("Vary").unwrap(), "Origin");
        assert_eq!(
            allowed.headers().get("Cache-Control").unwrap(),
            "private, max-age=31536000, immutable"
        );
        assert_eq!(
            allowed
                .headers()
                .get("Access-Control-Allow-Origin")
                .unwrap(),
            "tauri://localhost"
        );

        let denied = cover_response(jpeg, Some("https://example.com"));
        assert_eq!(denied.status(), 200);
        assert_eq!(
            denied.headers().get("Cache-Control").unwrap(),
            "private, max-age=31536000, immutable"
        );
        assert!(denied
            .headers()
            .get("Access-Control-Allow-Origin")
            .is_none());
    }

    #[test]
    fn missing_and_failing_assets_map_to_404() {
        let missing = RecordingCache::default();
        assert_eq!(
            cover_result_response(&missing, &request(CoverSize::List), None).status(),
            404
        );
        assert_eq!(
            cover_result_response(&missing, &request(CoverSize::Original), None).status(),
            404
        );

        let failing = RecordingCache {
            thumbnail_error: true,
            ..RecordingCache::default()
        };
        assert_eq!(
            cover_result_response(&failing, &request(CoverSize::Detail), None).status(),
            404
        );
    }

    #[test]
    fn malformed_cover_uris_are_rejected_before_the_cache_is_touched() {
        for uri in [
            "cover://asset/../../etc/passwd?s=detail",
            "cover://asset/a/b?s=list",
            "file:///etc/passwd",
            "cover://",
        ] {
            assert_eq!(route_cover_uri(uri), CoverRoute::Respond(404), "{uri}");
        }
        let too_long = format!("cover://asset/{}", "a".repeat(129));
        // The current key whitelist reports an overlong key as `InvalidKey`
        // (the `KeyTooLong` 400 arm is retained but unreachable, as before).
        assert_eq!(route_cover_uri(&too_long), CoverRoute::Respond(404));

        assert_eq!(
            route_cover_uri("cover://asset/cv1-x?s=detail"),
            CoverRoute::Serve(CoverRequest {
                asset_key: "cv1-x".to_owned(),
                size: CoverSize::Detail,
            })
        );
    }

    #[test]
    fn only_echo_renderer_origins_can_read_cover_pixels() {
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ] {
            assert!(cover_canvas_origin_allowed(origin));
        }
        for origin in [
            "null",
            "https://example.com",
            "http://tauri.localhost.evil.com",
            "http://localhost.evil.com:1430",
            "http://192.168.1.10:1430",
            "file:///tmp/cover.html",
        ] {
            assert!(!cover_canvas_origin_allowed(origin), "{origin}");
        }
    }

    /// The port of the dev server is chosen by the CLI at runtime, so the
    /// renderer's dev origin is *not* a constant. Pinning one port — this suite
    /// used to pin 1420, while `tauri dev` served 1430 — is precisely how the
    /// immersive palette came to fall back to the theme for every song: the
    /// whitelist stopped matching and nothing anywhere said so.
    #[test]
    fn a_dev_build_accepts_whatever_loopback_port_the_renderer_is_served_from() {
        for origin in [
            "http://localhost:1420",
            "http://localhost:1430",
            "http://localhost:1431",
            "http://127.0.0.1:1430",
        ] {
            assert!(is_loopback_http_origin(origin), "{origin}");
            assert_eq!(
                cover_canvas_origin_allowed(origin),
                cfg!(debug_assertions),
                "{origin}"
            );
        }
        // Loopback-shaped but not a renderer: no port, not http, not loopback.
        for origin in [
            "http://localhost",
            "http://127.0.0.1",
            "https://localhost:1430",
            "ftp://localhost:1430",
            "localhost:1430",
        ] {
            assert!(!is_loopback_http_origin(origin), "{origin}");
        }
    }
}
