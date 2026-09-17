//! `DesktopStateStore` cases (parse, resolve, mutate, atomic write).

use super::*;

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("echo-ls-{name}-{}", std::process::id()))
}

fn cleanup(p: &Path) {
    let _ = fs::remove_file(p);
}

#[test]
fn missing_store_yields_defaults_without_corruption() {
    let p = temp_path("missing");
    cleanup(&p);
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
    let s = store
        .load()
        .expect("missing file is defaults, not an error");
    assert_eq!(s.theme, DesktopTheme::Coral);
    assert_eq!(s.close_behavior, CloseBehavior::Exit);
    assert_eq!(s.window, None);
    assert!(!s.had_corruption, "first launch is not corruption");
    cleanup(&p);
}

#[test]
fn perfectly_corrupt_file_falls_back_to_defaults_and_marks_corruption() {
    let p = temp_path("corrupt");
    fs::write(&p, b"{ this is not json ").expect("write");
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
    let s = store.load().expect("corrupt file falls back, not an error");
    assert_eq!(s.theme, DesktopTheme::Coral);
    assert_eq!(s.close_behavior, CloseBehavior::Exit);
    assert!(s.had_corruption);
    cleanup(&p);
}

#[test]
fn corrupt_file_on_macos_falls_back_to_coral_and_background_default() {
    let p = temp_path("corrupt-macos");
    fs::write(&p, b"\x00\xff\xfe{not valid").expect("write");
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Macos);
    let s = store.load().expect("corrupt file falls back, not an error");
    assert_eq!(s.theme, DesktopTheme::Coral, "损坏偏好回退到珊瑚主题");
    assert_eq!(
        s.close_behavior,
        CloseBehavior::Background,
        "macOS 平台默认关窗值 = 后台运行"
    );
    assert!(s.had_corruption);
    cleanup(&p);
}

#[test]
fn unknown_theme_and_close_values_fall_back_per_field_preserving_siblings() {
    let p = temp_path("unknown");
    fs::write(
        &p,
        br#"{"theme":"neon","closeBehavior":"sleepy","window":{"x":0,"y":0,"width":1280,"height":800,"maximized":false}}"#,
    )
    .expect("write valid");
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Macos);
    let s = store.load().expect("unknown values fall back");
    assert_eq!(s.theme, DesktopTheme::Coral, "unknown theme -> coral");
    assert_eq!(
        s.close_behavior,
        CloseBehavior::Background,
        "unknown close -> platform default (macos = background)"
    );
    // A *valid* sibling field (window) is preserved even though others fell back.
    assert_eq!(
        s.window,
        Some(WindowState {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
            maximized: false,
        })
    );
    assert!(s.had_corruption);
    cleanup(&p);
}

#[test]
fn wrong_typed_field_degrades_only_itself_and_keeps_valid_siblings() {
    let p = temp_path("wrong-type");
    // `window` is a string, not an object: only that field falls back; the
    // valid cobalt theme and the playback session must survive.
    fs::write(
        &p,
        br#"{"theme":"cobalt","closeBehavior":"background","window":"not-an-object","playbackSession":{"queue":[7,8]}}"#,
    )
    .expect("write");
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
    let s = store
        .load()
        .expect("wrong-typed field degrades, not an error");
    assert_eq!(s.theme, DesktopTheme::Cobalt, "valid sibling preserved");
    assert_eq!(
        s.close_behavior,
        CloseBehavior::Background,
        "valid sibling preserved"
    );
    assert_eq!(s.window, None, "wrong-typed window falls back");
    assert!(s.had_corruption);
    assert_eq!(
        store.playback_session().expect("session"),
        Some(serde_json::json!({"queue": [7, 8]})),
        "session isolated from the bad window field"
    );
    cleanup(&p);
}

#[test]
fn known_values_round_trip_with_no_corruption() {
    let p = temp_path("roundtrip");
    cleanup(&p);
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
    store.set_theme(DesktopTheme::Cobalt).expect("theme");
    store
        .set_close_behavior(CloseBehavior::Background)
        .expect("close");
    store
        .set_window_state(Some(WindowState {
            x: 10,
            y: 20,
            width: 900,
            height: 600,
            maximized: true,
        }))
        .expect("window");
    store
        .set_playback_session(Some(serde_json::json!({"queue": [1, 2, 3]})))
        .expect("session");
    let s = store.load().expect("reload");
    assert_eq!(s.theme, DesktopTheme::Cobalt);
    assert_eq!(s.close_behavior, CloseBehavior::Background);
    assert_eq!(
        s.window,
        Some(WindowState {
            x: 10,
            y: 20,
            width: 900,
            height: 600,
            maximized: true,
        })
    );
    assert!(!s.had_corruption);
    assert_eq!(
        store.playback_session().expect("session"),
        Some(serde_json::json!({"queue": [1, 2, 3]}))
    );
    cleanup(&p);
}

#[test]
fn degenerate_window_state_is_rejected_and_falls_back() {
    let p = temp_path("degenerate-window");
    fs::write(
        &p,
        br#"{"theme":"coral","closeBehavior":"exit","window":{"x":-5,"y":-5,"width":0,"height":0,"maximized":true}}"#,
    )
    .expect("write");
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
    let s = store.load().expect("degenerate window falls back");
    assert_eq!(s.window, None, "zero-size window is not usable");
    assert_eq!(s.theme, DesktopTheme::Coral);
    assert!(s.had_corruption);
    cleanup(&p);
}

#[test]
fn windows_platform_default_is_exit() {
    let p = temp_path("win-default");
    cleanup(&p);
    let store = DesktopStateStore::new(p, PlatformCloseDefault::Other);
    let s = store.load().expect("defaults");
    assert_eq!(s.close_behavior, CloseBehavior::Exit);
}

#[test]
fn window_left_on_a_disconnected_display_is_recentered_on_screen() {
    let window = WindowState {
        x: -1000,
        y: 2000,
        width: 800,
        height: 600,
        maximized: false,
    };
    let work = WorkArea {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    // Off every visible area => centred into the primary work area.
    let clamped = window.clamp_to_visible(work);
    assert_eq!(clamped.x, 560); // (1920 - 800) / 2
    assert_eq!(clamped.y, 240); // (1080 - 600) / 2
    assert_eq!(clamped.width, 800);
    assert_eq!(clamped.height, 600);
    assert!(!clamped.maximized, "size/maximize untouched by re-anchor");
}

#[test]
fn a_still_visible_window_is_left_untouched() {
    let window = WindowState {
        x: 100,
        y: 100,
        width: 800,
        height: 600,
        maximized: false,
    };
    let work = WorkArea {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    assert_eq!(window.clamp_to_visible(work), window);
}

#[test]
fn a_window_off_screen_and_bigger_than_the_screen_snaps_to_the_work_origin() {
    let window = WindowState {
        x: 5000,
        y: 5000,
        width: 3000,
        height: 2500,
        maximized: false,
    };
    let work = WorkArea {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let clamped = window.clamp_to_visible(work);
    // Fully off-screen and larger than the work area: the top-left lands at
    // the work origin (never spilled past the right/bottom edge).
    assert_eq!((clamped.x, clamped.y), (0, 0));
    assert_eq!((clamped.width, clamped.height), (3000, 2500));
}

#[test]
fn a_partially_visible_window_is_left_untouched() {
    // Even a window that spills past an edge is reachable while any part
    // overlaps — only a fully off-screen window is re-anchored.
    let window = WindowState {
        x: 1500,
        y: 800,
        width: 1200,
        height: 800,
        maximized: false,
    };
    let work = WorkArea {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    assert_eq!(window.clamp_to_visible(work), window);
}

#[test]
fn a_degenerate_work_area_leaves_the_position_unchanged() {
    let window = WindowState {
        x: -1000,
        y: 2000,
        width: 800,
        height: 600,
        maximized: false,
    };
    // No usable work area (shell couldn't resolve a display): don't guess.
    assert_eq!(
        window.clamp_to_visible(WorkArea {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }),
        window
    );
    // An unusable window (zero size) is likewise left alone.
    assert_eq!(
        WindowState {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            maximized: true,
        }
        .clamp_to_visible(WorkArea {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }),
        WindowState {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            maximized: true,
        }
    );
}

#[test]
fn mutation_preserves_valid_session_across_a_partially_corrupt_file() {
    let p = temp_path("preserve-session");
    // The close field is corrupt, but the session is valid: fixing the
    // theme must NOT erase the session.
    fs::write(
        &p,
        br#"{"closeBehavior":{"bad":true},"playbackSession":{"queue":[42]}}"#,
    )
    .expect("write");
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
    store.set_theme(DesktopTheme::Cobalt).expect("mutate theme");
    let s = store.load().expect("reload");
    assert_eq!(s.theme, DesktopTheme::Cobalt, "edited field written");
    assert_eq!(
        s.close_behavior,
        CloseBehavior::Exit,
        "bad close drops to default"
    );
    assert_eq!(
        store.playback_session().expect("session"),
        Some(serde_json::json!({"queue": [42]})),
        "valid session survived the theme mutation"
    );
    cleanup(&p);
}

#[test]
fn failed_write_preserves_previous_file() {
    // A store whose target sits under a path component that is a *file* —
    // `create_dir_all` must fail — so the mutation errors and leaves
    // nothing partial behind.
    let base = std::env::temp_dir().join(format!("echo-ls-block-{}", std::process::id()));
    let blocker = base.join("not_a_dir");
    fs::create_dir_all(&base).expect("base");
    fs::write(&blocker, b"i am a file not a dir").expect("seed blocker");
    let target = blocker.join("state.json");
    let store = DesktopStateStore::new(target.clone(), PlatformCloseDefault::Other);
    assert!(
        store.set_theme(DesktopTheme::Cobalt).is_err(),
        "unwritable parent errors"
    );
    assert!(!target.exists(), "no partial file left behind");
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn typed_mutators_are_read_modify_write_and_clear_with_none() {
    let p = temp_path("mutate");
    cleanup(&p);
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
    store
        .set_window_state(Some(WindowState {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            maximized: false,
        }))
        .expect("write window");
    assert_eq!(
        store.window_state().expect("read").map(|w| w.width),
        Some(640)
    );

    // Updating theme must preserve the window written earlier.
    store
        .set_theme(DesktopTheme::Turquoise)
        .expect("write theme");
    let s = store.load().expect("load");
    assert_eq!(s.theme, DesktopTheme::Turquoise);
    assert_eq!(s.window.map(|w| w.width), Some(640), "sibling preserved");

    // Clearing the window removes it, theme stays.
    store.set_window_state(None).expect("clear window");
    let s = store.load().expect("load");
    assert_eq!(s.window, None);
    assert_eq!(s.theme, DesktopTheme::Turquoise);

    // Clearing the playback session removes it.
    store
        .set_playback_session(Some(serde_json::json!({"x": 1})))
        .expect("write session");
    assert_eq!(
        store.playback_session().expect("read"),
        Some(serde_json::json!({"x": 1}))
    );
    store.set_playback_session(None).expect("clear session");
    assert_eq!(store.playback_session().expect("read"), None);
    cleanup(&p);
}

#[test]
fn parent_directory_is_created_on_first_write() {
    let base = std::env::temp_dir().join(format!("echo-ls-dir-{}", std::process::id()));
    let p = base.join("sub/deeper/state.json");
    cleanup(&p);
    let store = DesktopStateStore::new(p, PlatformCloseDefault::Other);
    store
        .set_theme(DesktopTheme::Turquoise)
        .expect("creates parents and saves");
    assert_eq!(store.theme().expect("read"), DesktopTheme::Turquoise);
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn serialized_file_uses_camel_case_and_skips_absent_fields() {
    let p = temp_path("serialized-shape");
    cleanup(&p);
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
    store.set_theme(DesktopTheme::Coral).expect("theme");
    let text = fs::read_to_string(&p).expect("read");
    assert!(
        text.contains("\"theme\": \"coral\""),
        "camel-case key: {text}"
    );
    assert!(
        !text.contains("closeBehavior"),
        "absent field skipped: {text}"
    );
    cleanup(&p);
}

#[test]
fn stale_temp_file_is_cleaned_before_the_next_write() {
    let p = temp_path("stale-temp");
    cleanup(&p);
    let file_name = p
        .file_name()
        .and_then(|n| n.to_str())
        .expect("temp basename");
    // Simulate a crashed write: a leftover temp with our own pid.
    let stale = p
        .parent()
        .expect("parent")
        .join(format!(".{file_name}.{}.0.tmp", std::process::id()));
    let _ = fs::remove_file(&stale);
    fs::write(&stale, b"partial").expect("seed stale temp");
    let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
    store
        .set_theme(DesktopTheme::Coral)
        .expect("write after stale temp");
    assert!(
        !stale.exists(),
        "stale temp removed so it cannot collide on a reused pid"
    );
    cleanup(&p);
    let _ = fs::remove_file(&stale);
}
