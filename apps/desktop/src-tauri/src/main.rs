//! Echo desktop application binary.
//!
//! The binary owns only the Tauri application boundary. Domain work remains in
//! `echo-core`; desktop runtime, player and platform implementation grow in
//! `echo-desktop` in their respective tasks.

use std::{env, fs::OpenOptions, io::Write, thread, time::Duration};

use tauri::{
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

fn main() {
    let app = match tauri::Builder::default()
        // The single-instance plugin is registered first, as required by the
        // plugin, so a later platform adapter cannot create a second process.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            focus_main_window(app);
            let paths: Vec<String> = args.into_iter().skip(1).collect();
            record_gate_open(&paths);
            let _ = app.emit("app://file-open-request", paths);
        }))
        .setup(|app| {
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
