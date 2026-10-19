//! The audio-only / least-privilege mpv option set applied at handle creation
//! (task 8.3) and the `ECHO_MPV_EXTRA_OPTIONS` diagnostics escape hatch.

/// The audio-only / least-privilege mpv option set applied at handle creation
/// (task 8.3). These options are set with `mpv_set_option_string` before
/// `mpv_initialize`, so none can be overridden by user config.
///
/// Hardening intent (design §11 "audio-only mpv 配置并禁用用户脚本、ytdl、非必要
/// 网络协议和用户配置"):
/// - `config=no` … ignore any user `mpv.conf` (also covered by `--no-config`).
/// - `load-scripts=no` … do not run user lua/js scripts.
/// - `ytdl=no` … do not spawn youtube-dl/yt-dlp (network + arbitrary exec).
/// - `video=no`, `vo=null`, `audio-display=no`, `osc=no` … pure audio; no video
///   window, no on-screen controls, no cover rendering (React draws cover art).
/// - `protocol-whitelist=file` … only the local `file://` scheme; everything else
///   (http, smb, mms, …) is refused by mpv itself.
///
/// This is a *defense-in-depth* layer: the actor additionally refuses any path
/// that carries a URL scheme (see [`Backend::queue_load`] guards), so only
/// Rust-validated local paths reach mpv.
///
/// **Build-compat split.** The vendor `audio-default` libmpv **(media-kit
/// v0.7.2)** compiles out Lua scripts, yt-dl and the OSC entirely — its
/// `set_option_string` answers `MPV_ERROR_OPTION_NOT_FOUND` for
/// `load-scripts`/`ytdl`/`osc`/`protocol-whitelist`. Those options are *always*
/// desirable hardening, but on such a build the capability is simply absent,
/// which already satisfies the "disable it" intent. They therefore live in
/// [`HARDENED_OPTIONAL`] and are applied best-effort ([`ffi::Handle::create`]
/// skips them with a warning when unsupported). The *required* set below is
/// what the handle must genuinely enforce to be a safe audio-only player.
pub(super) const HARDENED_REQUIRED: &[(&str, &str)] = &[
    ("config", "no"),
    ("video", "no"),
    ("vo", "null"),
    ("audio-display", "no"),
];

/// Hardening options that target capabilities a reduced build may compile out;
/// applied best-effort (see [`HARDENED_REQUIRED`] comment).
pub(super) const HARDENED_OPTIONAL: &[(&str, &str)] = &[
    ("load-scripts", "no"),
    ("ytdl", "no"),
    ("osc", "no"),
    ("protocol-whitelist", "file"),
];

/// Optional options extended from `ECHO_MPV_EXTRA_OPTIONS` (`name=value`,
/// semicolon-separated), applied best-effort like [`HARDENED_OPTIONAL`].
///
/// A diagnostics escape hatch: the real-libmpv smoke sets `ao=null` so the
/// event-plumbing assertions stay hermetic (a sandboxed CI runner can stall
/// mpv's core in `CoreAudio` init — `FILE_LOADED` arrives but the playloop never
/// starts and no property/EOF event is ever delivered). It must never carry
/// security-relevant intent: everything here is best-effort.
pub(super) fn extra_options() -> Vec<(&'static str, &'static str)> {
    let mut opts: Vec<(&'static str, &'static str)> = HARDENED_OPTIONAL.to_vec();
    if let Ok(extra) = std::env::var("ECHO_MPV_EXTRA_OPTIONS") {
        for pair in extra.split(';').filter_map(|s| s.split_once('=')) {
            // Only a small allowlist of names can be injected, so the escape
            // hatch can never re-enable a capability the hardening disabled.
            const ALLOWED: &[&str] = &["ao"];
            if ALLOWED.contains(&pair.0.trim()) {
                // The option table needs 'static strs; the actor spawns once
                // per process, so leaking the value is bounded and deliberate.
                let value: &'static str = Box::leak(pair.1.to_owned().into_boxed_str());
                opts.push(("ao", value));
            }
        }
    }
    opts
}
