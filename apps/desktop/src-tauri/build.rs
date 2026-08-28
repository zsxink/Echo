fn main() {
    // The packaged libmpv and its FFmpeg dependencies use `@rpath`. This is
    // deliberately owned by the desktop shell, never by echo-core.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    }
    tauri_build::build();
}
