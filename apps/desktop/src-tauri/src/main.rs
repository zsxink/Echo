//! Echo desktop application binary.
//!
//! The binary owns only the Tauri application boundary. Domain work remains in
//! `echo-core`; desktop runtime, player and platform implementation grow in
//! `echo-desktop` in their respective tasks.

use std::{env, fs::OpenOptions, io::Write, sync::Arc, thread, time::Duration};

use echo_core::application::ports::CoverCache;
use echo_desktop::platform::security::{CoverError, CoverProtocol};
use tauri::{
    http::{Request as HttpRequest, Response as HttpResponse},
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Emitter, Manager, RunEvent,
};

const MAIN_WINDOW: &str = "main";

fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The Gate sets this only for its short-lived subprocesses. It lets the
/// native check prove an explicit application exit without adding a public
/// command or privileged frontend capability.
fn schedule_gate_exit(app: tauri::AppHandle) {
    let Ok(milliseconds) = env::var("ECHO_GATE_QUIT_AFTER_MS") else {
        return;
    };
    let Ok(milliseconds) = milliseconds.parse::<u64>() else {
        return;
    };

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(milliseconds));
        app.exit(0);
    });
}

/// The Gate uses this opt-in log to assert that a hot second launch delivered
/// its file path to the already-running instance. Production never sets it.
fn record_gate_open(paths: &[String]) {
    let Ok(log_path) = env::var("ECHO_GATE_OPEN_LOG") else {
        return;
    };
    let Ok(mut log) = OpenOptions::new().create(true).append(true).open(log_path) else {
        return;
    };
    for path in paths {
        let _ = writeln!(log, "{path}");
    }
}

/// The `cover://` asset protocol (design §16): serves cover art bytes from the
/// [`CoverCache`] by opaque asset key only.
///
/// Every security decision lives in [`CoverProtocol`] (echo-desktop):
///   - the URI must be `cover://` and its key must pass the shape whitelist,
///     so a request can never be turned into a filesystem path;
///   - the bytes come exclusively from `cache.get(key)` — never from a
///     client-supplied path.
///
/// When no [`CoverCache`] is registered yet (the runtime has not wired the
/// cover store), the protocol answers [`http::StatusCode::NOT_FOUND`] rather
/// than erroring — the shell stays thin and the protocol is inert until the
/// runtime injects the store.
/// `context`/`request` are consumed by the framework signature (the trait bound
/// fixes them by value); the handler only borrows them, hence the allow.
#[allow(clippy::needless_pass_by_value)]
fn cover_protocol_handler<R: tauri::Runtime>(
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
        Ok(Some(bytes)) => HttpResponse::builder()
            .status(200)
            .header("Content-Type", "image/*")
            .body(bytes)
            .unwrap_or_default(),
        // Unknown/malformed key for the store, or a transient storage miss —
        // same 404 as any missing asset.
        Ok(None) | Err(_) => HttpResponse::builder()
            .status(404)
            .body(vec![])
            .unwrap_or_default(),
    }
}

fn main() {
    let app = match tauri::Builder::default()
        // The single-instance plugin is registered first, as required by the
        // plugin, so a later platform adapter cannot create a second process.
        .register_uri_scheme_protocol("cover", cover_protocol_handler)
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            focus_main_window(app);
            let paths: Vec<String> = args.into_iter().skip(1).collect();
            record_gate_open(&paths);
            let _ = app.emit("app://file-open-request", paths);
        }))
        .setup(|app| {
            // Structured tracing + panic hook (task 7.8, design §17): install
            // the rolling-file logger and the local crash diagnostic before
            // anything else so all startup work is observable. The diagnostics
            // directory is `app_data_dir/logs`; logs stay local, never uploaded.
            let app_data = app.path().app_data_dir()?;
            let diagnostics = echo_desktop::platform::diagnostics::Diagnostics::new(app_data);
            let _installed = diagnostics.install_logger();
            diagnostics.install_panic_hook();

            let show = MenuItemBuilder::with_id("show", "显示 Echo").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "退出").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show)
                .separator()
                .item(&quit)
                .build()?;

            TrayIconBuilder::with_id("echo-gate")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => focus_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            schedule_gate_exit(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => {
            eprintln!("failed to initialize Echo: {error}");
            std::process::exit(1);
        }
    };

    app.run(|app, event| {
        if let RunEvent::Opened { urls } = event {
            focus_main_window(app);
            let paths = urls
                .into_iter()
                .map(|url| url.to_string())
                .collect::<Vec<_>>();
            record_gate_open(&paths);
            let _ = app.emit("app://file-open-request", paths);
        }
    });
}
