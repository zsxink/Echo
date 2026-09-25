// Stage the vendored libmpv set next to the dev executable, so `tauri dev`
// resolves the same layout the packaged app uses.
//
//   • macOS: `vendor/libmpv/macos/*.dylib` → `target/<profile>/Frameworks/`
//     (the packaged app carries the dylib set in `Contents/Frameworks`; the
//     rpath below locates it two levels up from the executable).
//   • Windows: `vendor/libmpv/windows/*.dll` → `target/<profile>/` (the
//     install root — the executable's own directory).
//   • Linux: `vendor/libmpv/linux/lib{mpv,av,sw}*.so*` → `target/<profile>/`,
//     preserving symlink semantics (an unversioned `libmpv.so` → its versioned
//     real file), because the loader resolves DT_NEEDED by SONAME.
//
// This is deliberately owned by the desktop shell, never by echo-core.

fn profile_dir() -> std::path::PathBuf {
    let out_dir = std::path::PathBuf::from(
        std::env::var("OUT_DIR").expect("Cargo supplies output directory"),
    );
    out_dir
        .ancestors()
        .nth(3)
        .expect("Cargo OUT_DIR has target profile ancestor")
        .to_path_buf()
}

/// Stage the library files of a vendored libmpv set into `target`.
///
/// `is_library` selects which entries to stage; anything else (manifests,
/// NOTICE, licenses) is intentionally left out of the dev layout. Symlink
/// entries keep their link semantics when `preserve_links` is set — `fs::copy`
/// would dereference the link and produce a plain copy, breaking the
/// unversioned `libmpv.so` → versioned real-file indirection the loader needs.
fn stage_library_source(
    source: &std::path::Path,
    target: &std::path::Path,
    is_library: impl Fn(&str) -> bool,
    preserve_links: bool,
) {
    // A vendored tree may be absent in development (the Linux first build is
    // not committed yet, or a checkout is partial). Don't fail the whole build
    // over it — the loader returns an explicit error at startup if the library
    // is then missing.
    let Ok(entries) = std::fs::read_dir(source) else {
        eprintln!(
            "build.rs: note: vendored libmpv dir {} absent; skipping dev staging",
            source.display()
        );
        return;
    };
    std::fs::create_dir_all(target).expect("create dev staging directory");
    for entry in entries {
        let entry = entry.expect("read vendored libmpv entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_library(&name) {
            continue;
        }
        let path = entry.path();
        #[cfg(unix)]
        let is_symlink = std::fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .is_ok_and(|v| v);
        #[cfg(not(unix))]
        let is_symlink = false;

        let dest = target.join(entry.file_name());
        if is_symlink && preserve_links {
            let cached = std::fs::read_link(&path).expect("read vendored libmpv symlink");
            // Recreate the same relative/absolute link target next to the dest.
            stage_symlink(&cached, &dest);
        } else {
            std::fs::copy(&path, dest).expect("stage vendored libmpv dependency for development");
        }
    }
}

// Whether a symlinked vendored entry keeps its link next to the dev
// executable. Only unix can express symlinks at all (Windows/msvc needs
// Developer Mode and cannot rely on it), so the non-unix stub is unreachable:
// `is_symlink` is always false there. Keeping the call site unconditional lets
// the `if is_symlink && preserve_links` branch type-check on every target.
#[cfg(unix)]
fn stage_symlink(cached: &std::path::Path, dest: &std::path::Path) {
    std::os::unix::fs::symlink(cached, dest).expect("recreate dev staging symlink");
}

#[cfg(not(unix))]
const fn stage_symlink(_cached: &std::path::Path, _dest: &std::path::Path) {}

fn is_vendored_library(name: &str, target_os: &str) -> bool {
    // A vendored library is picked by the OS-appropriate extension. `.dll` /
    // `.dylib` are plain file extensions; the Linux set carries version
    // suffixes (`libmpv.so.2.5`) the unversioned symlink points at, so it is
    // matched by base name + presence of the `.so` component.
    match target_os {
        "windows" => std::path::Path::new(name)
            .extension()
            .is_some_and(|ext| ext == "dll"),
        "macos" => std::path::Path::new(name)
            .extension()
            .is_some_and(|ext| ext == "dylib"),
        _ => {
            (name.starts_with("libmpv") || name.starts_with("libav") || name.starts_with("libsw"))
                && name.contains(".so")
        }
    }
}

fn main() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies manifest directory"),
    );
    let profile_dir = profile_dir();

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo supplies target OS");

    match target_os.as_str() {
        "macos" => {
            // rpath into `Contents/Frameworks` relative to the executable, and
            // the MediaPlayer framework for Now Playing.
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
            println!("cargo:rustc-link-lib=framework=MediaPlayer");

            let frameworks = profile_dir
                .parent()
                .expect("Cargo profile directory has target parent")
                .join("Frameworks");
            stage_library_source(
                &manifest_dir.join("vendor/libmpv/macos"),
                &frameworks,
                |name| is_vendored_library(name, "macos"),
                false,
            );
        }
        "windows" | "linux" => {
            // The dev stage is the executable's own directory: `target/<profile>/`,
            // matching the packaged install root.
            let vendor_dir = manifest_dir.join(format!("vendor/libmpv/{target_os}"));
            stage_library_source(
                &vendor_dir,
                &profile_dir,
                |name| is_vendored_library(name, &target_os),
                true,
            );
        }
        other => {
            // Emscripten/wasm/other targets have no vendored libmpv; nothing to stage.
            let _ = other;
        }
    }
    tauri_build::build();
}
