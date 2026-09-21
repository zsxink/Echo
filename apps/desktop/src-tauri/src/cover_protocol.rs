//! The `cover://` URI-scheme protocol handler and its security helpers
//! (canvas pixel-read origin policy, media-type sniffing).
//!
//! The handler's signature is fixed by tauri's
//! [`register_uri_scheme_protocol`](tauri::Builder::register_uri_scheme_protocol):
//! the request arrives owned and the callback must take it by value, so
//! `needless_pass_by_value` does not apply here.
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use echo_core::application::ports::CoverCache;
use echo_desktop::platform::security::{CoverError, CoverProtocol};
use tauri::http::{Request as HttpRequest, Response as HttpResponse};
use tauri::Manager;

pub fn cover_protocol_handler<R: tauri::Runtime>(
    context: tauri::UriSchemeContext<'_, R>,
    request: HttpRequest<Vec<u8>>,
) -> HttpResponse<Vec<u8>> {
    let key = match CoverProtocol::parse(request.uri().to_string().as_str()) {
        Ok(key) => key,
        Err(CoverError::KeyTooLong) => {
            return HttpResponse::builder()
                .status(400)
                .body(vec![])
                .unwrap_or_default();
        }
        Err(_) => {
            // Wrong scheme, missing or malformed key — the request is not for
            // us, answer 404 (never a redirect or a fallthrough read).
            return HttpResponse::builder()
                .status(404)
                .body(vec![])
                .unwrap_or_default();
        }
    };

    let Some(cache) = context.app_handle().try_state::<Arc<dyn CoverCache>>() else {
        return HttpResponse::builder()
            .status(404)
            .body(vec![])
            .unwrap_or_default();
    };

    match cache.get(&key) {
        Ok(Some(bytes)) => {
            let media_type = cover_media_type(&bytes);
            let mut response = HttpResponse::builder()
                .status(200)
                .header("Content-Type", media_type)
                .header("Vary", "Origin");
            // Canvas extraction needs an origin-clean image. Grant pixel reads
            // only to the bundled renderer (and, in a dev build, the local dev
            // server — whatever port it picked).
            if let Some(origin) = request
                .headers()
                .get("Origin")
                .and_then(|v| v.to_str().ok())
            {
                if cover_canvas_origin_allowed(origin) {
                    response = response.header("Access-Control-Allow-Origin", origin);
                } else if cfg!(debug_assertions) {
                    // A refused pixel read is otherwise completely silent: the
                    // probe's `crossOrigin` request fails, the palette resolves
                    // to `null`, and the immersive surface falls back to the
                    // theme colour for *every* song while the DOM cover still
                    // renders. This line is the only place the refusal is
                    // observable, so make it observable on purpose.
                    eprintln!("cover: refused pixel access to origin {origin}");
                }
            }
            response.body(bytes).unwrap_or_default()
        }
        // Unknown/malformed key for the store, or a transient storage miss —
        // same 404 as any missing asset.
        Ok(None) | Err(_) => HttpResponse::builder()
            .status(404)
            .body(vec![])
            .unwrap_or_default(),
    }
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
    use super::{cover_canvas_origin_allowed, is_loopback_http_origin};

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
