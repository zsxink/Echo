//! Observed continuous properties, the vendored libmpv probes and the actor's ordered destruction contract.

#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn file_loaded_subscribes_to_the_continuous_properties() {
    // Regression: `mpv_observe_property` was never called, so the actor
    // received no `PROPERTY_CHANGE` at all — `position`/`duration` stayed
    // `None`, freezing 播放进度条 and the synced-lyrics highlight while the
    // song played. The load path is where the subscription must happen.
    let (mut actor, snapshot, observations) = spawn_test_observed(vec![BackendEvent::FileLoaded]);
    assert_eq!(
        observations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing is subscribed before a file is loaded"
    );

    actor.send(load_temporary("/music/a.flac")).unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

    assert!(
        observations.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "a loaded file must subscribe to time-pos/duration or the UI has no live values"
    );
    actor.shutdown();
}

#[test]
fn observed_properties_cover_position_duration_and_the_audio_controls() {
    // The subscription set has to include everything the snapshot claims to
    // report: the two the UI reads (progress bar / lyrics) *and* the audio
    // controls, which are what make `volume` / `muted` mpv's values rather
    // than the actor's private guess.
    let names: Vec<&str> = OBSERVED_PROPERTIES.iter().map(|(name, _)| *name).collect();
    for required in ["time-pos", "duration", "volume", "mute", "pause"] {
        assert!(
            names.contains(&required),
            "{required} must be observed or the snapshot reports a guess \
             instead of the value mpv actually holds [set={names:?}]"
        );
    }
    // …and each with the format that actually delivers a payload. Measured
    // against the vendored libmpv: observing `mute`/`pause` as DOUBLE is
    // accepted by mpv but every notification then carries MPV_FORMAT_NONE
    // (no value) — a subscription that looks fine and delivers nothing.
    let format_of = |want: &str| {
        OBSERVED_PROPERTIES
            .iter()
            .find(|(name, _)| *name == want)
            .map(|(_, format)| *format)
    };
    assert_eq!(format_of("time-pos"), Some(ffi::format_::DOUBLE));
    assert_eq!(format_of("duration"), Some(ffi::format_::DOUBLE));
    assert_eq!(format_of("volume"), Some(ffi::format_::DOUBLE));
    assert_eq!(format_of("mute"), Some(ffi::format_::FLAG));
    assert_eq!(format_of("pause"), Some(ffi::format_::FLAG));
}

#[test]
fn a_flag_property_event_is_decoded_from_its_native_format() {
    // The real backend decodes a FLAG payload (a C `int`) into the same
    // `f64` event vocabulary. If that path regressed, `mute`/`pause`
    // notifications would be dropped as "unavailable" forever and the
    // snapshot would silently fall back to the actor's guesses.
    assert_eq!(ffi::format_::FLAG, 3);
    let on = [1i32];
    // SAFETY: `on` is a live `int` for the duration of the call.
    let decoded = unsafe { ffi::read_flag(on.as_ptr().cast()) };
    assert_eq!(decoded, Some(1.0), "FLAG 1 → 1.0");
    let off = [0i32];
    // SAFETY: as above.
    assert_eq!(unsafe { ffi::read_flag(off.as_ptr().cast()) }, Some(0.0));
    // SAFETY: a null data pointer is explicitly allowed (unavailable).
    assert_eq!(unsafe { ffi::read_flag(std::ptr::null()) }, None);
}

/// The vendored macOS libmpv, if this host has one — the same skip rule the
/// `player_smoke` suite uses, so a host without the bundle still passes.
fn vendored_libmpv() -> Option<std::path::PathBuf> {
    // The vendored library is the macOS dylib bundled into the .app; its
    // Mach-O image cannot be dlopened on Windows/Linux (the workspace
    // builds these tests everywhere, and `--all-features` gates run them
    // on every platform). Resolve it only on macOS and let the caller's
    // SKIP branch handle the rest.
    if !cfg!(target_os = "macos") {
        return None;
    }
    for candidate in [
        "../../apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib",
        "apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib",
    ] {
        let path = std::path::Path::new(candidate);
        if path.exists() {
            return Some(path.to_path_buf());
        }
    }
    None
}

#[test]
fn libmpv_treats_volume_and_mute_as_independent_properties() {
    // The causal chain behind 调大音量反而没声: in mpv, `set volume` does
    // **not** clear `mute`. The actor used to write only the volume and then
    // record `muted: false` in its own snapshot, so the UI claimed sound
    // while the output was silenced — and the next 调大音量 landed on it.
    // Checked against mpv, because "surely setting the volume unmutes" is
    // precisely the assumption that produced the bug.
    let Some(lib) = vendored_libmpv() else {
        eprintln!("SKIP: no vendored libmpv on this host");
        return;
    };
    // SAFETY: this test thread owns the handle for the whole test.
    unsafe {
        let sys = ffi::MpvSys::load(&lib).expect("dlopen the vendored libmpv");
        let mut handle = ffi::Handle::create(&sys, &[], &[]).expect("create mpv handle");
        assert!(
            matches!(handle.af_command_argument_count(&sys), Some(3 | 4)),
            "af-command signature must be discoverable from bundled mpv metadata"
        );
        let muted = std::ffi::CString::new("mute").expect("no NUL");
        handle
            .observe_property(&sys, 0, muted.as_c_str(), ffi::format_::FLAG)
            .expect("observe mute");

        // Wait (bounded) for mpv to report `mute == want`; `false` on timeout.
        let wait_mute = |want: f64| -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while std::time::Instant::now() < deadline {
                let ev = handle.wait_event(&sys, 0.05);
                if ev.is_null() || (*ev).event_id != ffi::event_id::PROPERTY_CHANGE {
                    continue;
                }
                let data = (*ev).data;
                if data.is_null() {
                    continue;
                }
                let property = data as *const ffi::mpv_event_property;
                if ffi::read_c_str((*property).name).as_deref() != Some("mute")
                    || (*property).format != ffi::format_::FLAG
                {
                    continue;
                }
                if ffi::read_flag((*property).data) == Some(want) {
                    return true;
                }
            }
            false
        };
        let set = |name: &str, value: &str| {
            let args: Vec<std::ffi::CString> = ["set", name, value]
                .iter()
                .map(|s| std::ffi::CString::new(*s).expect("no NUL"))
                .collect();
            handle
                .command(&sys, &args)
                .unwrap_or_else(|e| panic!("set {name} {value}: {e}"));
        };

        set("mute", "yes");
        assert!(wait_mute(1.0), "mute=yes must take effect");
        // The load path's volume write must not be assumed to un-mute.
        set("volume", "40");
        assert!(
            !wait_mute(0.0),
            "`set volume` must NOT clear mpv's `mute` — that is why the actor has to \
             write both, and why only writing the volume made the UI claim 未静音"
        );
        set("mute", "no");
        assert!(wait_mute(0.0), "the explicit un-mute must take effect");
        handle.terminate(&sys);
    }
}

#[test]
fn observed_properties_really_deliver_values_from_libmpv() {
    // The subscription table is a claim about *mpv's* behaviour, so it is
    // checked against mpv rather than trusted: observing `mute`/`pause` with
    // DOUBLE is **accepted** by `mpv_observe_property` and then delivers
    // MPV_FORMAT_NONE forever — a subscription that looks healthy and
    // reports nothing. That is how the audio controls ended up "observed"
    // nowhere at all, leaving the snapshot to report the actor's own guess
    // (UI 说未静音、实际静音；UI 说在播、时钟不走).
    //
    // Skipped only when the host has no vendored libmpv.
    let Some(lib) = vendored_libmpv() else {
        eprintln!("SKIP: no vendored libmpv on this host");
        return;
    };
    // SAFETY: this test thread owns the handle for the whole test and
    // terminates it before returning.
    unsafe {
        let sys = ffi::MpvSys::load(&lib).expect("dlopen the vendored libmpv");
        let mut handle = ffi::Handle::create(&sys, &[], &[]).expect("create mpv handle");

        for (name, format) in OBSERVED_PROPERTIES {
            let cname = std::ffi::CString::new(*name).expect("no NUL");
            handle
                .observe_property(&sys, 0, cname.as_c_str(), *format)
                .unwrap_or_else(|e| panic!("observe {name} as {format}: {e}"));
        }
        // Drive one change per property, so each observation must deliver.
        for (name, value) in [("pause", "yes"), ("mute", "yes"), ("volume", "40")] {
            let args: Vec<std::ffi::CString> = ["set", name, value]
                .iter()
                .map(|s| std::ffi::CString::new(*s).expect("no NUL"))
                .collect();
            handle
                .command(&sys, &args)
                .unwrap_or_else(|e| panic!("set {name} {value}: {e}"));
        }

        let mut seen: Vec<(String, f64)> = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let ev = handle.wait_event(&sys, 0.05);
            if ev.is_null() || (*ev).event_id != ffi::event_id::PROPERTY_CHANGE {
                continue;
            }
            let data = (*ev).data;
            if data.is_null() {
                continue;
            }
            let property = data as *const ffi::mpv_event_property;
            let Some(name) = ffi::read_c_str((*property).name) else {
                continue;
            };
            let payload = match (*property).format {
                ffi::format_::DOUBLE => ffi::read_double((*property).data),
                ffi::format_::FLAG => ffi::read_flag((*property).data),
                _ => None,
            };
            if let Some(value) = payload {
                seen.push((name, value));
            }
        }
        handle.terminate(&sys);

        for wanted in ["pause", "mute", "volume"] {
            assert!(
                seen.iter().any(|(name, _)| name == wanted),
                "mpv delivered no value for `{wanted}` with the format the actor observes \
                 it with — the snapshot would silently fall back to a guess [seen={seen:?}]"
            );
        }
        // `pause`/`mute` are observed as MPV_FORMAT_FLAG and `volume` as a
        // DOUBLE, but all three arrive through one payload, so every value
        // is compared with the same tolerance `volume` already uses.
        assert!(
            seen.iter()
                .any(|(name, v)| name == "pause" && (*v - 1.0).abs() < 1e-9),
            "pause=yes must arrive as 1.0 [seen={seen:?}]"
        );
        assert!(
            seen.iter()
                .any(|(name, v)| name == "mute" && (*v - 1.0).abs() < 1e-9),
            "mute=yes must arrive as 1.0 [seen={seen:?}]"
        );
        assert!(
            seen.iter()
                .any(|(name, v)| name == "volume" && (*v - 40.0).abs() < 1e-9),
            "set volume 40 must arrive as 40.0 [seen={seen:?}]"
        );
    }
}

#[test]
fn ordered_destruction_terminates_backend_then_joins() {
    // Spawn over a TestBackend; after shutdown the actor must be stopped.
    // Because terminate() runs on the actor thread before join returns and
    // the handle never leaves that thread, destruction is single-owner.
    let (mut actor, _snapshot, _props) = spawn_test(vec![]);
    actor.shutdown();
    assert!(actor.is_stopped());
}

#[test]
fn degraded_start_without_backend_stays_alive_until_shutdown() {
    // If the backend factory fails, the actor must not panic: it drains
    // commands, reports Stopped, then shuts down cleanly on Shutdown.
    let snapshot = snapshot_stub();
    let mut actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> { Err(ffi::HandleError::Create) },
        snapshot,
        None,
    )
    .expect("spawn");
    let _ = actor.send(PlayerCommand::Play);
    actor.shutdown();
    assert!(actor.is_stopped());
}
