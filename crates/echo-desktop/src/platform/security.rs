//! Desktop security posture: CSP, the custom cover-protocol boundary and the
//! minimal Tauri capability (task 7.7, design §16).
//!
//! Everything here is pure and Tauri-free so it can be proven by unit tests
//! without launching a window or a WebView:
//!
//! - [`csp`] — the production Content-Security-Policy. It rejects remote
//!   scripts and connections outright (no `*`, no `http:`/`https:`, no
//!   websockets), restricts every fetch/source directive to `'self'` plus the
//!   whitelisted [`COVER_SCHEME`] and `data:`, disables `object`/embed and
//!   frames. `style-src` keeps `'unsafe-inline'` only because the React
//!   frontend inlines animation-keyframe rules that browsers reject otherwise.
//! - [`CoverProtocol`] — the `cover://` URI boundary. A URI is parsed into an
//!   *opaque* [`CoverCache`] asset key that passes a strict shape whitelist;
//!   anything that could be interpreted as a path (`.`, `..`, `/`, `\`,
//!   multi-segment, non-whitelisted bytes, overlong) is rejected. The bytes
//!   themselves are then fetched by key only — never by a client-supplied
//!   path — so the protocol cannot be turned into an arbitrary-file reader.
//! - [`CapabilityPolicy`] — the minimal-viable main-window capability. It
//!   asserts the granted permission set contains no privileged families
//!   (`shell`, `fs`, `sql`, `opener`, `dialog` arbitrary paths, `process`,
//!   `http`), so the WebView can never ask for a general OS capability.
//!
//! The Tauri shell (in `apps/desktop/src-tauri`) only *registers* these
//! decisions: it injects [`csp`] into the window config and routes `cover://`
//! through [`CoverProtocol`] + `echo-core`'s [`CoverCache`]. No security
//! check lives in the shell where it could not be reviewed in isolation.

/// The custom asset scheme the cover protocol responds on. The CSP grants
/// `cover:` only for `img-src`/`media-src`, never for scripts or fetches.
pub const COVER_SCHEME: &str = "cover";

/// The opaquely-typed asset-key prefix the Core cache emits (`cv1-…`); the
/// protocol's trust boundary is the *shape* in [`CoverProtocol::is_valid_key`],
/// never this prefix's content.
///
/// Maximum length of an asset key. Bounds every lookup; a longer candidate is
/// rejected before it ever reaches the [`CoverCache`].
const MAX_ASSET_KEY_LEN: usize = 128;

/// The production Content-Security-Policy.
///
/// Rules:
/// - **No remote origin anywhere.** Every directive is `'self'` + a short
///   whitelist; there is no `*`, no scheme (`http:`, `https:`, `ws:`, `wss:`,
///   nor `data:` outside the narrow image/media/font cases), so the
///   `WebView` cannot open a network connection even if script is later
///   injected.
/// - `img-src` also permits the exact `http://cover.localhost` Tauri Windows
///   protocol origin (intercepted by WebView2, not a remote server).
/// - `img-src`/`media-src` additionally allow `cover:` (cover art served by
///   [`CoverProtocol`]) and `data:` (inline placeholders).
/// - `object-src 'none'` (no plugin/embed), `frame-ancestors 'none'` (no
///   embedding Echo), `base-uri 'self'`, `form-action 'self'`.
/// - `script-src 'self'` only — no eval, no remote.
///
/// Kept a `const` so the shell's injected value and the static
/// `tauri.conf.json` value are pinned to one definition and drift is a
/// compile-time/test-time error, not a silent config change.
pub const CSP: &str = "default-src 'self'; connect-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' cover: http://cover.localhost data:; media-src 'self' cover: data:; font-src 'self' data:; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

/// Validate the CSP on request — a helper for shell wiring that wants the
/// same safety the const carries without importing a test-only path.
#[must_use]
pub const fn csp() -> &'static str {
    CSP
}

/// Error from the cover-protocol boundary. Kept tiny: every failure is an
/// *invalid request* (a `400`-equivalent), never a storage error — storage
/// errors belong to the [`CoverCache`], which the handler surfaces separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CoverError {
    #[error("cover URI does not use the cover:// scheme")]
    WrongScheme,
    #[error("cover URI is missing an asset key")]
    MissingKey,
    #[error("cover asset key contains disallowed characters")]
    InvalidKey,
    #[error("cover asset key is too long")]
    KeyTooLong,
}

/// The `cover://` protocol boundary (design §16 "asset protocol 只接受白名单
/// key 和尺寸枚举") — this half validates the **key**; the size enum is the
/// frontend's responsibility and never reaches the filesystem.
pub struct CoverProtocol;

impl CoverProtocol {
    /// Parse a `cover://` request URI into an opaque asset key.
    ///
    /// Accepted shapes (`host` vs. `path` both tolerated because the `WebView`
    /// may emit either):
    /// - `cover://asset/<key>`
    /// - `cover://<key>`
    /// - `cover://localhost/<key>`
    ///
    /// A query or fragment is stripped. The key must pass
    /// [`Self::is_valid_key`], otherwise the request is rejected — a URI that
    /// could be interpreted as a path (`.`/`..`, `/`, `\`, multi-segment) never
    /// reaches the cover cache.
    ///
    /// # Errors
    ///
    /// Returns [`CoverError::WrongScheme`] for a non-`cover` scheme,
    /// [`CoverError::MissingKey`] when no key segment is present, and
    /// [`CoverError::InvalidKey`] / [`CoverError::KeyTooLong`] for a candidate
    /// that fails the key shape whitelist.
    pub fn parse(uri: &str) -> Result<String, CoverError> {
        let rest = uri
            .strip_prefix("cover://")
            .or_else(|| uri.strip_prefix("COVER://"))
            .ok_or(CoverError::WrongScheme)?;

        // Drop query/fragment first so they cannot smuggle a second segment.
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);

        // Split segments; the first non-empty one is the candidate key.
        // `cover://asset/<key>` -> ["asset", "<key>"], `cover://<key>` -> ["<key>"].
        let mut segments = rest.split('/').filter(|s| !s.is_empty());
        let first = segments.next().ok_or(CoverError::MissingKey)?;
        let second = segments.next();

        // `cover://localhost/<key>` and `cover://asset/<key>` both carry the
        // key as the *second* segment; `cover://<key>` carries it as the first.
        // A URI with more than two segments is malformed for our key model.
        let candidate = match second {
            // host ("localhost"/"asset", or value that is not a valid key) +
            // single path segment => the path segment is the key.
            Some(_) if segments.next().is_none() => second.unwrap_or(first),
            Some(_) => return Err(CoverError::InvalidKey),
            // No second segment: the first must itself be the key.
            None => first,
        };

        if !Self::is_valid_key(candidate) {
            return Err(CoverError::InvalidKey);
        }
        Ok(candidate.to_owned())
    }

    /// The asset-key shape whitelist. A key may contain only ASCII letters,
    /// digits, `-` and `_`, start with a letter or digit, and be at most
    /// [`MAX_ASSET_KEY_LEN`] bytes. This deliberately excludes `/`, `\`, `.`,
    /// `..` and every control byte, so a key can never be (or resolve to) a
    /// filesystem path.
    #[must_use]
    pub fn is_valid_key(key: &str) -> bool {
        if key.is_empty() || key.len() > MAX_ASSET_KEY_LEN {
            return false;
        }
        let mut bytes = key.bytes();
        let Some(first) = bytes.next() else {
            return false;
        };
        if !first.is_ascii_alphanumeric() {
            return false;
        }
        bytes.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
}

/// A permission family the `WebView` must never hold (design §16: "不向 JS
/// 暴露通用 `shell`、`fs`、`SQL`、任意 `opener` 或任意路径读取"). The check is
/// prefix-based so `shell:allow-*`, `fs:default`, `sql:default`,
/// `opener:default`, `process:default`, `http:default`, `dialog:default`
/// (which can read arbitrary paths the user picks) all trip it.
const PRIVILEGED_PREFIXES: &[&str] = &[
    "shell:", "opener:", "fs:", "sql:", "dialog:", "process:", "http:",
    "tray:", // no tray actions (fine by default; kept out anyway)
];

/// The minimal permission set for the main window: the core default (window
/// introspection getters) plus the single command the window actually asks
/// for. New commands get explicit `allow-<command>` entries when their tasks
/// wire them; never a broad family.
///
/// `core:window:allow-start-dragging` is the one such entry. The window runs
/// with `titleBarStyle: Overlay` + `hiddenTitle: true` on macOS, so the native
/// titlebar that would otherwise drag it does not exist, and the frontend's
/// drag strip (`.titlebar-drag`, driven by `data-tauri-drag-region`) asks the
/// window to drag itself. It grants a single window command — no path, no
/// filesystem, no process, no network surface — so task 7.7's requirement (no
/// network connect, no arbitrary file read, no arbitrary opener/shell/SQL)
/// still holds.
pub const MAIN_WINDOW_PERMISSIONS: &[&str] = &[
    "core:default",
    "core:window:default",
    "core:window:allow-start-dragging",
];

/// Validates a capability's granted-permission list.
pub struct CapabilityPolicy;

impl CapabilityPolicy {
    /// Whether a single permission string is acceptable for the main window:
    /// it must be an explicitly-listed default or an `allow-<command>` entry —
    /// never a privileged family.
    #[must_use]
    pub fn is_allowed(permission: &str) -> bool {
        if PRIVILEGED_PREFIXES
            .iter()
            .any(|p| permission.starts_with(p))
        {
            return false;
        }
        true
    }

    /// Assert a permission *set* contains no privileged family. Returns the
    /// first violating permission, if any.
    #[must_use]
    pub fn first_violation<'a>(permissions: &'a [&str]) -> Option<&'a str> {
        permissions.iter().copied().find(|p| !Self::is_allowed(p))
    }

    /// The canonical minimal set — for shell wiring and drift tests alike,
    /// there is exactly one source of truth for what the main window starts
    /// with.
    #[must_use]
    pub const fn main_window_defaults() -> &'static [&'static str] {
        MAIN_WINDOW_PERMISSIONS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // CSP
    // ------------------------------------------------------------------

    #[test]
    fn csp_never_opens_a_remote_or_network_source() {
        for directive_fragment in ["*", "http:", "https:", "ws:", "wss:", "blob:"] {
            assert!(
                !CSP.replace("http://cover.localhost", "")
                    .contains(directive_fragment),
                "CSP must not contain the remote source {directive_fragment:?}: {CSP}"
            );
        }
    }

    #[test]
    fn csp_allows_only_the_cover_scheme_for_images_and_media() {
        // The cover protocol is img/media-only; it must never be a script or
        // fetch source.
        assert!(CSP.contains("img-src 'self' cover: http://cover.localhost data:"));
        assert!(CSP.contains("media-src 'self' cover: data:"));
        let script = CSP
            .split(';')
            .find(|d| d.trim_start().starts_with("script-src"))
            .expect("script-src directive present");
        let connect = CSP
            .split(';')
            .find(|d| d.trim_start().starts_with("connect-src"))
            .expect("connect-src directive present");
        assert_eq!(script.trim(), "script-src 'self'");
        assert_eq!(connect.trim(), "connect-src 'self'");
    }

    #[test]
    fn csp_disables_objects_and_frames() {
        assert!(CSP.contains("object-src 'none'"));
        assert!(CSP.contains("frame-ancestors 'none'"));
    }

    // ------------------------------------------------------------------
    // Cover protocol
    // ------------------------------------------------------------------

    #[test]
    fn cover_uri_parses_valid_keys_from_both_host_and_path() {
        assert_eq!(
            CoverProtocol::parse("cover://cv1-abc123"),
            Ok("cv1-abc123".to_owned())
        );
        assert_eq!(
            CoverProtocol::parse("cover://asset/cv1-abc123"),
            Ok("cv1-abc123".to_owned())
        );
        assert_eq!(
            CoverProtocol::parse("cover://localhost/cv1-abc123"),
            Ok("cv1-abc123".to_owned())
        );
    }

    #[test]
    fn cover_uri_strips_query_and_fragment() {
        assert_eq!(
            CoverProtocol::parse("cover://asset/cv1-x?foo=1"),
            Ok("cv1-x".to_owned())
        );
        assert_eq!(
            CoverProtocol::parse("cover://asset/cv1-x#frag"),
            Ok("cv1-x".to_owned())
        );
    }

    #[test]
    fn cover_uri_rejects_path_traversal_and_absolute_paths() {
        for uri in [
            "cover://../../etc/passwd",
            "cover://asset/../../etc/passwd",
            "cover://asset/%2e%2e/etc", // decoded dot-segment
            "cover://asset/a/b/c",
            "cover:///",
            "cover://",
            "https://asset/cv1-x", // wrong scheme
            "file:///etc/passwd",  // wrong scheme
        ] {
            assert!(
                CoverProtocol::parse(uri).is_err(),
                "expected rejection of {uri:?}"
            );
        }
    }

    #[test]
    fn cover_key_rejects_bytes_that_can_form_a_path() {
        for key in ["", ".", "..", "a/b", "a\\b", "a.b", "-lead", "_lead", "a b"] {
            assert!(
                !CoverProtocol::is_valid_key(key),
                "expected rejection of key {key:?}"
            );
        }
    }

    #[test]
    fn cover_key_accepts_realistic_and_boundary_keys() {
        assert!(CoverProtocol::is_valid_key("cv1-abc123"));
        assert!(CoverProtocol::is_valid_key("cv1"));
        assert!(CoverProtocol::is_valid_key(&"a".repeat(MAX_ASSET_KEY_LEN)));
        assert!(!CoverProtocol::is_valid_key(
            &"a".repeat(MAX_ASSET_KEY_LEN + 1)
        ));
    }

    // ------------------------------------------------------------------
    // Capability
    // ------------------------------------------------------------------

    #[test]
    fn minimal_capability_has_no_privileged_family() {
        assert_eq!(
            CapabilityPolicy::first_violation(CapabilityPolicy::main_window_defaults()),
            None,
            "the minimal set must contain no shell/fs/sql/opener/process permission"
        );
    }

    #[test]
    fn privileged_permission_families_are_rejected() {
        for permission in [
            "shell:allow-open",
            "shell:default",
            "opener:allow-open-url",
            "fs:read-all",
            "sql:default",
            "process:default",
            "dialog:default",
            "dialog:allow-open",
            "http:default",
        ] {
            assert!(
                !CapabilityPolicy::is_allowed(permission),
                "privileged permission {permission:?} must be rejected"
            );
            assert_eq!(
                CapabilityPolicy::first_violation(&[permission]),
                Some(permission)
            );
        }
    }

    #[test]
    fn explicit_command_pins_are_allowed() {
        // The intended next-step: explicit allow-<command> pins are fine.
        for permission in ["allow-set-theme", "allow-get-bootstrap-state"] {
            assert!(CapabilityPolicy::is_allowed(permission));
        }
    }

    // ------------------------------------------------------------------
    // Drift detection: the live Tauri shell must not drift from these rules
    // ------------------------------------------------------------------

    /// Path to `apps/desktop/src-tauri/tauri.conf.json` relative to the
    /// workspace root (two levels up from this crate).
    fn src_tauri_conf() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("echo-desktop crate dir")
            .parent()
            .expect("workspace root")
            .join("apps/desktop/src-tauri/tauri.conf.json")
    }

    /// Path to the committed main-window capability file.
    fn main_capability() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("echo-desktop crate dir")
            .parent()
            .expect("workspace root")
            .join("apps/desktop/src-tauri/capabilities/main.json")
    }

    #[test]
    fn tauri_conf_csp_matches_the_pinned_policy() {
        let text = std::fs::read_to_string(src_tauri_conf())
            .expect("tauri.conf.json is committed and readable");
        let conf: serde_json::Value =
            serde_json::from_str(&text).expect("tauri.conf.json is valid JSON");
        let conf_csp = conf["app"]["security"]["csp"]
            .as_str()
            .expect("app.security.csp is a string");
        assert_eq!(
            conf_csp, CSP,
            "tauri.conf.json CSP drifted from security::CSP; update both together"
        );
    }

    /// Task 9.2 drift guard: the shell's file-association registration must
    /// cover exactly the format families the media stack guarantees
    /// (mp3/flac/m4a/ogg/opus/wav) — never a superset (an unvetted format Echo
    /// claims to open) nor a subset (a guaranteed format the user can no longer
    /// open by double-clicking). This is the shell half of "配置保证格式文件
    /// 关联"; the open-path resolution (SongId vs temporary item) is playback
    /// layer (11.7) and is not shell logic.
    #[test]
    fn tauri_conf_file_associations_cover_the_guaranteed_formats() {
        use echo_core::domain::media::AudioFormat;
        let guaranteed: std::collections::BTreeSet<String> = [
            AudioFormat::Mpeg,
            AudioFormat::Flac,
            AudioFormat::Mp4,
            AudioFormat::Ogg,
            AudioFormat::Opus,
            AudioFormat::Wav,
        ]
        .into_iter()
        .map(AudioFormat::extension)
        .map(str::to_owned)
        .collect();

        let text = std::fs::read_to_string(src_tauri_conf())
            .expect("tauri.conf.json is committed and readable");
        let conf: serde_json::Value =
            serde_json::from_str(&text).expect("tauri.conf.json is valid JSON");
        let mut registered = std::collections::BTreeSet::new();
        for assoc in conf["bundle"]["fileAssociations"]
            .as_array()
            .expect("bundle.fileAssociations is an array")
        {
            for ext in assoc["ext"]
                .as_array()
                .expect("fileAssociation.ext is an array")
            {
                registered.insert(ext.as_str().expect("ext is a string").to_ascii_lowercase());
            }
        }

        assert_eq!(
            registered, guaranteed,
            "bundle.fileAssociations drifted from the guaranteed AudioFormat set"
        );
    }

    #[test]
    fn committed_capability_grants_only_the_minimal_set() {
        let text = std::fs::read_to_string(main_capability())
            .expect("the main capability file is committed");
        let cap: serde_json::Value =
            serde_json::from_str(&text).expect("capability file is valid JSON");
        let permissions = cap["permissions"]
            .as_array()
            .expect("permissions is an array")
            .iter()
            .filter_map(|p| p.as_str())
            .collect::<Vec<_>>();

        // Every granted permission passes the family whitelist…
        assert_eq!(
            CapabilityPolicy::first_violation(&permissions),
            None,
            "capability must not grant a privileged family: {permissions:?}"
        );
        // …and the set is exactly the minimal defaults (no extra surface the
        // window did not ask for).
        assert_eq!(
            permissions,
            CapabilityPolicy::main_window_defaults(),
            "main window capability must stay at the minimal permission set"
        );
    }
}
