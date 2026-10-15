#[test]
fn fullscreen_close_waits_for_appkit_completion() {
    let pending = super::PendingFullscreenHide::default();

    pending.begin();
    pending.will_exit();
    assert!(pending.is_exiting(), "native exit is still in progress");
    assert!(pending.did_exit(), "native exit completes pending close");
    assert!(!pending.is_exiting(), "native exit is complete");
    pending.cancel();
    assert!(!pending.did_exit(), "hide is completed only once");

    pending.begin();
    pending.cancel();
    assert!(!pending.did_exit(), "failed fullscreen exit is cancelled");
}

#[test]
fn main_window_activation_restores_one_existing_window_without_creating_another() {
    use tauri::Manager;

    let app = tauri::test::mock_app();
    let main =
        tauri::WebviewWindowBuilder::new(&app, super::MAIN_WINDOW, tauri::WebviewUrl::default())
            .build()
            .expect("mock main window");

    main.hide().expect("hide mock window");
    super::focus_main_window(app.handle());
    assert!(main.is_visible().expect("read visible state"));

    main.minimize().expect("minimize mock window");
    super::focus_main_window(app.handle());
    assert!(!main.is_minimized().expect("read minimized state"));

    for _ in 0..4 {
        super::focus_main_window(app.handle());
    }
    assert_eq!(app.webview_windows().len(), 1);
    assert!(app.get_webview_window(super::MAIN_WINDOW).is_some());
}
