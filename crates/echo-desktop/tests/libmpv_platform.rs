//! Platform libmpv load Gate (task 9.8): drive a **real** vendored libmpv on
//! the platform this test is compiled for — Windows (libmpv-2.dll) and Linux
//! (libmpv.so) — through the exact same `MpvSys::load` / `Handle` code path the
//! packaged app uses, assert the client API major matches the vendored
//! manifest, and run one real mpv command on the live handle.
//!
//! This is the no-silent-skip counterpart to task 8.12 for the Windows/Linux
//! platforms the macOS smoke does not cover. A `#[ignore]` or an early-return
//! skip here is what `task-9.8.mjs` scans for: deleting the vendor library or an
//! ABI mismatch must **fail** this gate, never let it pass without touching
//! libmpv.
//!
//! macOS is deliberately excluded — task 8.12 already drives the vendored
//! macOS dylib through a full fixture matrix. Here the non-Windows/Linux
//! platforms compile an always-green module so the gate stays uniform.
//!
//! `unsafe` is denied workspace-wide; the sole documented exception lives in
//! `player::ffi`. This Gate is the "real library" probing counterpart, so the
//! same scoped `#[allow(unsafe_code)]` carve-out is applied to the small
//! FFI-driving module below and nothing else.

#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::path::Path;

/// Drive the real vendored libmpv: load it through `MpvSys::load`, create a
/// live `Handle`, assert the client API major matches the manifest, run one
/// real mpv command, and terminate — the load/ABI/command core of the 9.8 gate.
///
/// Mirrors how `PlayerActor::spawn_mpv` uses the same FFI; this is the API /
/// command probe, not a playback smoke (that is task 8.12's job on macOS and
/// `cargo test` fixture drives on the other platforms).
#[cfg(any(target_os = "windows", target_os = "linux"))]
#[allow(unsafe_code)]
mod gate {
    use std::ffi::CString;
    use std::path::Path;

    use echo_desktop::player::ffi;

    /// The vendored manifest records the client API version major of the
    /// bundled libmpv (e.g. macOS `2.0.0`, Windows `2.5` → major 2).
    /// `mpv_client_api_version` returns `(2 << 16) | minor`; asserting the
    /// loaded library's major against the manifest is the ABI check the
    /// supply-chain spec demands.
    pub fn manifest_abi_major() -> u32 {
        let mut dir = std::env::current_dir().ok();
        while let Some(d) = dir {
            let Some(manifest_path) = platform_manifest_path(&d) else {
                return 0; // Platform outside this change (macOS: task 8.12).
            };
            let Ok(src) = std::fs::read_to_string(&manifest_path) else {
                dir = d.parent().map(|p| p.to_path_buf());
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&src) else {
                break;
            };
            let abi = value
                .get("libmpvAbi")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            return abi
                .split('.')
                .next()
                .and_then(|m| m.parse().ok())
                .unwrap_or(0);
        }
        0
    }

    /// Resolve the platform's vendored manifest rooted at `dir` (searching
    /// `apps/desktop/src-tauri/vendor/libmpv/<os>/manifest.json`), or `None`
    /// on platforms the Windows/Linux Gate does not cover (macOS → task 8.12).
    #[cfg(target_os = "windows")]
    fn platform_manifest_path(dir: &std::path::Path) -> Option<std::path::PathBuf> {
        Some(
            dir.join("apps/desktop/src-tauri/vendor/libmpv/windows")
                .join("manifest.json"),
        )
    }
    #[cfg(target_os = "linux")]
    fn platform_manifest_path(dir: &std::path::Path) -> Option<std::path::PathBuf> {
        Some(
            dir.join("apps/desktop/src-tauri/vendor/libmpv/linux")
                .join("manifest.json"),
        )
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    fn platform_manifest_path(_dir: &std::path::Path) -> Option<std::path::PathBuf> {
        None
    }

    /// Load + create + api_version + one command + terminate.
    pub fn probe(path: &Path) {
        // SAFETY: trusted, pinned vendored artifact; single-threaded handle use.
        let sys = unsafe { ffi::MpvSys::load(path) }
            .unwrap_or_else(|e| panic!("MpvSys::load failed for {}: {e}", path.display()));
        // SAFETY: valid `sys`, bare mpv handle created on this thread.
        let mut handle = unsafe { ffi::Handle::create(&sys, &[], &[]) }
            .unwrap_or_else(|e| panic!("mpv handle create/initialize failed: {e}"));

        // ABI sanity: manifest major must equal the live library's client API
        // major.
        let expected = manifest_abi_major();
        let version = handle.api_version(&sys);
        let major = (version >> 16) as u32;
        assert_eq!(
            major, expected,
            "vendored libmpv ABI major {major} != manifest major {expected} \
             (client_api_version={version:#x}) — mismatched or drifted library"
        );

        // One real command on the live handle: `set volume 0` is a stable mpv
        // command returning MPV_ERROR_SUCCESS on an initialized handle. Nothing
        // here exercises audio hardware, only the command pipeline.
        let ok = unsafe {
            handle.command(
                &sys,
                &[
                    CString::new("set").expect("static"),
                    CString::new("volume").expect("static"),
                    CString::new("0").expect("static"),
                ],
            )
        };
        assert!(
            ok.is_ok(),
            "mpv `set volume` command failed on live handle: {ok:?}"
        );

        // SAFETY: orderly teardown (idempotent) — last call on this handle.
        unsafe { handle.terminate(&sys) };

        // The no-silent-skip sentinel: task-9.8.mjs requires this exact line in
        // the test output on Windows/Linux, so a deleted-gateway or an early
        // `return` that never touched libmpv FAILS the Gate instead of passing
        // on a green test name.
        println!("__LPV_PLATFORM_GATE_OK__");
    }
}

/// Resolve the vendored libmpv the platform bundles, by the same relative
/// search the packaged app uses (install root / dev `target/<profile>/`).
#[cfg(target_os = "windows")]
fn vendored_libmpv() -> Option<std::path::PathBuf> {
    for candidate in [
        // Packaged / install-root layout: next to the executable (`cargo test`
        // runs with CWD = crate dir; target/ lives two levels up at repo root).
        "../../target/debug/libmpv-2.dll",
        "../../target/release/libmpv-2.dll",
        "target/debug/libmpv-2.dll",
        "target/release/libmpv-2.dll",
        // Dev layout: the vendored tree in the source checkout.
        "apps/desktop/src-tauri/vendor/libmpv/windows/libmpv-2.dll",
        "../../apps/desktop/src-tauri/vendor/libmpv/windows/libmpv-2.dll",
        "src-tauri/vendor/libmpv/windows/libmpv-2.dll",
    ] {
        let p = Path::new(candidate);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..6 {
        let probe = dir.join("apps/desktop/src-tauri/vendor/libmpv/windows/libmpv-2.dll");
        if probe.exists() {
            return Some(probe);
        }
        dir.pop();
    }
    None
}

/// Linux: libmpv.so resolves through the same relative layout.
#[cfg(target_os = "linux")]
fn vendored_libmpv() -> Option<std::path::PathBuf> {
    for candidate in [
        // Packaged / install-root layout: next to the executable (`cargo test`
        // runs with CWD = crate dir; target/ lives two levels up at repo root).
        "../../target/debug/libmpv.so",
        "../../target/release/libmpv.so",
        "target/debug/libmpv.so",
        "target/release/libmpv.so",
        // Dev layout: the vendored tree in the source checkout.
        "apps/desktop/src-tauri/vendor/libmpv/linux/libmpv.so",
        "../../apps/desktop/src-tauri/vendor/libmpv/linux/libmpv.so",
        "src-tauri/vendor/libmpv/linux/libmpv.so",
    ] {
        let p = Path::new(candidate);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..6 {
        let probe = dir.join("apps/desktop/src-tauri/vendor/libmpv/linux/libmpv.so");
        if probe.exists() {
            return Some(probe);
        }
        dir.pop();
    }
    None
}

/// Windows: WebView2 runtime presence is a *report*, never a gate failure. The
/// design's risk section explicitly makes a missing WebView2 a non-failing,
/// reportable condition so the Gate does not misclassify an environment issue
/// as a supply-chain regression.
#[cfg(target_os = "windows")]
fn probe_webview2_report() {
    const REG_KEY: &str = r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let out = std::process::Command::new("reg")
        .args(["query", REG_KEY, "/v", "pv"])
        .output();
    let present = out.is_ok_and(|o| o.status.success());
    if present {
        println!("__WEBVIEW2_PRESENT__");
    } else {
        println!("__WEBVIEW2_MISSING__ (report only — not a gate failure)");
    }
}

/// Windows gate: real libmpv-2.dll load + ABI + command, plus the WebView2
/// report-only probe.
#[cfg(target_os = "windows")]
#[test]
fn windows_vendored_libmpv_loads_and_reports_webview2() {
    let Some(libmpv) = vendored_libmpv() else {
        panic!(
            "Windows vendored libmpv (libmpv-2.dll) is missing — the platform \
             cannot build a playable install. This gate must FAIL, not skip."
        );
    };
    probe_webview2_report();
    gate::probe(&libmpv);
}

/// Linux gate: real libmpv.so load + ABI + command.
#[cfg(target_os = "linux")]
#[test]
fn linux_vendored_libmpv_loads_and_commands() {
    let Some(libmpv) = vendored_libmpv() else {
        panic!(
            "Linux vendored libmpv (libmpv.so) is missing — the platform \
             cannot build a playable install. This gate must FAIL, not skip."
        );
    };
    gate::probe(&libmpv);
}

/// Non-Windows/Linux (macOS): 8.12 covers the real dylib; compile an
/// always-green module so the gate stays uniform.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
#[test]
fn macos_covered_by_task_8_12() {
    // Coverage stub: task 8.12 drives the real macOS libmpv. A runtime check
    // keeps this from reading as a no-op to the -D warnings lint while
    // asserting the Gate runs on the expected platform.
    assert_ne!(
        std::env::consts::OS,
        "windows",
        "this stub must only cover the non-Windows/Linux Gate platform"
    );
    assert_ne!(
        std::env::consts::OS,
        "linux",
        "this stub must only cover the non-Windows/Linux Gate platform"
    );
}
