fn main() {
    // The packaged libmpv and its FFmpeg dependencies use `@rpath`. This is
    // deliberately owned by the desktop shell, never by echo-core.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

        // `tauri dev` runs `target/{profile}/echo`, rather than an `.app`
        // bundle.  libmpv is loaded at runtime, and its FFmpeg dependencies
        // are addressed as `@rpath/lib*.dylib`; stage the complete vendored
        // set where the development executable's rpath resolves it:
        // `target/Frameworks`.  The bundle configuration below continues to
        // own the release-App staging.
        let manifest_dir = std::path::PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies manifest directory"),
        );
        let source = manifest_dir.join("vendor/libmpv/macos");
        let out_dir = std::path::PathBuf::from(
            std::env::var("OUT_DIR").expect("Cargo supplies output directory"),
        );
        let profile_dir = out_dir
            .ancestors()
            .nth(3)
            .expect("Cargo OUT_DIR has target profile ancestor");
        let frameworks = profile_dir
            .parent()
            .expect("Cargo profile directory has target parent")
            .join("Frameworks");

        println!("cargo:rerun-if-changed={}", source.display());
        std::fs::create_dir_all(&frameworks).expect("create development Frameworks directory");
        for entry in std::fs::read_dir(&source).expect("read vendored libmpv directory") {
            let entry = entry.expect("read vendored libmpv entry");
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "dylib")
            {
                std::fs::copy(&path, frameworks.join(entry.file_name()))
                    .expect("stage vendored libmpv dependency for development");
            }
        }
    }
    tauri_build::build();
}
