#![allow(clippy::float_cmp)] // Exact neutral analysis is the behavior this fixture asserts.

use super::*;
use crate::effects::{ChannelLayout, EqCurve, Spatial};
fn environment() -> ProcessingEnvironment {
    ProcessingEnvironment {
        sample_rate: 48_000,
        channel_layout: ChannelLayout::Stereo,
    }
}
fn complete(native: &mut NativeEffects) {
    native.ramp.as_mut().unwrap().started = Instant::now().checked_sub(TRANSITION).unwrap();
}

#[test]
fn chain_is_typed_float_named_and_uses_three_argument_commands() {
    let mut native = NativeEffects::default();
    let mut commands = Vec::new();
    let mut curve = EqCurve::default();
    curve.gains_db[3] = 4.0;
    let payload = Payload::Eq(curve);
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    assert!(!native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0][..2], ["af", "add"]);
    assert!(!native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    assert_eq!(
        commands.len(),
        1,
        "wait for AUDIO_RECONFIG before node commands"
    );
    native.reconfirm();
    assert!(!native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    complete(&mut native);
    assert!(native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    let graph = commands[0][2].clone();
    assert!(graph.contains("aformat=sample_fmts=dblp"));
    assert!(graph.contains("precision=f64"));
    assert!(!graph.contains("block_size"));
    assert!(graph.contains("equalizer@eq3="));
    assert!(graph.contains(LIMITER));
    assert!(commands
        .iter()
        .filter(|args| args[0] == "af-command")
        .all(|args| args.len() == 4));
    assert!(commands.iter().all(
        |args| !args.iter().any(|arg| arg == "volume" || arg == "mute") || args[0] == "af-command"
    ));
    assert!(graph.find("volume@preamp").unwrap() < graph.find("equalizer@eq3").unwrap());
}

#[test]
fn sample_rate_boundary_omits_inactive_band_without_losing_the_curve() {
    let mut native = NativeEffects::default();
    let mut commands = Vec::new();
    let mut curve = EqCurve::default();
    curve.gains_db[9] = 12.0;
    let payload = Payload::Eq(curve);
    let environment = ProcessingEnvironment {
        sample_rate: 22_050,
        channel_layout: ChannelLayout::Mono,
    };
    let analysis = crate::effects::math::analyze(&payload, environment).unwrap();
    native
        .apply(&payload, environment, &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    assert!(!commands[0][2].contains("equalizer@eq9="));
    assert_eq!(analysis.effective_preamp_db, 0.0);
}

#[test]
fn reconfiguration_reconfirms_once_and_does_not_restart_the_ramp_each_tick() {
    let mut native = NativeEffects::default();
    let payload = Payload::Eq(EqCurve::default());
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    let mut command = |_: &[String]| Ok(());

    assert!(!native
        .apply(&payload, environment(), &analysis, &mut command)
        .unwrap());
    native.reconfirm();
    assert!(!native
        .apply(&payload, environment(), &analysis, &mut command)
        .unwrap());
    complete(&mut native);
    assert!(native
        .apply(&payload, environment(), &analysis, &mut command)
        .unwrap());

    native.reconfirm();
    assert!(!native
        .apply(&payload, environment(), &analysis, &mut command)
        .unwrap());
    complete(&mut native);
    assert!(native
        .apply(&payload, environment(), &analysis, &mut command)
        .unwrap());
    assert!(!native.reconfirm);
}

#[test]
fn missing_audio_reconfigured_event_recovers_after_transition_window() {
    let mut native = NativeEffects::default();
    let payload = Payload::Eq(EqCurve::default());
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    let mut commands = Vec::new();
    native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    assert!(native.install_pending);

    native.installed_at = Some(Instant::now().checked_sub(TRANSITION).unwrap());
    native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();

    assert!(!native.install_pending);
    assert!(commands.iter().any(|args| args[0] == "af-command"));
}

#[test]
fn spatial_width_is_fixed_at_install_and_never_sent_as_runtime_command() {
    let mut native = NativeEffects::default();
    let payload = Payload::Spatial(Spatial::default());
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    let mut commands = Vec::new();
    native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();

    assert!(commands[0][2].contains("extrastereo@spatial=m=1.250000000000000:c=false"));
    native.reconfirm();
    native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    complete(&mut native);
    native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();

    assert!(!commands
        .iter()
        .any(|args| args.iter().any(|arg| arg == "spatial:m")));
}

#[test]
fn disabling_a_partial_edit_ramps_to_neutral_then_removes_owned_filters() {
    let mut native = NativeEffects::default();
    let payload = Payload::Spatial(Spatial::default());
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    let mut commands = Vec::new();
    native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    native.reconfirm();
    native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    complete(&mut native);
    native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    assert!(!native
        .bypass(|args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    complete(&mut native);
    assert!(native
        .bypass(|args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    let last = commands.last().unwrap();
    assert_eq!(&last[..2], &["af", "remove"]);
    assert!(last[2].contains("echo_limiter"));
    assert!(native.current.is_none());
}

#[test]
fn an_injected_command_failure_does_not_publish_a_success_or_reenable() {
    let mut native = NativeEffects::default();
    let payload = Payload::default();
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    let mut commands = Vec::new();
    assert!(!native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    native.reconfirm();
    assert!(!native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    complete(&mut native);
    let result = native.apply(&payload, environment(), &analysis, |args| {
        commands.push(args.to_vec());
        if args[0] == "af-command" {
            Err("injected command error".to_owned())
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    assert!(native.current.is_none());
    assert_eq!(commands.last().unwrap()[1], "remove");
    assert!(native.bypass(|_| Ok(())).unwrap());
}
