#![allow(clippy::float_cmp)] // Exact neutral analysis is the behavior this fixture asserts.

use super::*;
use crate::effects::{ChannelLayout, EqCurve, Spatial};

#[test]
fn af_command_syntax_tracks_the_loaded_library_signature() {
    assert_eq!(
        AfCommandSyntax::from_argument_count(Some(3)),
        AfCommandSyntax::LegacyPatched
    );
    assert_eq!(
        AfCommandSyntax::from_argument_count(Some(4)),
        AfCommandSyntax::SeparateTarget
    );
    assert_eq!(
        AfCommandSyntax::from_argument_count(None),
        AfCommandSyntax::Unavailable
    );
    assert_eq!(
        AfCommandSyntax::from_argument_count(Some(5)),
        AfCommandSyntax::Unavailable
    );
}

#[test]
fn af_command_sends_filter_target_in_the_library_specific_position() {
    let mut legacy = Vec::new();
    AfCommandSyntax::LegacyPatched
        .send("echo_eq4", "eq4", "gain", "3.5", &mut |args| {
            legacy.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    assert_eq!(legacy, [vec!["af-command", "echo_eq4", "eq4:gain", "3.5"]]);

    let mut modern = Vec::new();
    AfCommandSyntax::SeparateTarget
        .send("echo_eq4", "eq4", "gain", "3.5", &mut |args| {
            modern.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        modern,
        [vec!["af-command", "echo_eq4", "gain", "3.5", "eq4"]]
    );
}

#[test]
fn unknown_af_command_signature_fails_before_installing_audio_filters() {
    let mut native = NativeEffects::new(AfCommandSyntax::Unavailable);
    let payload = Payload::Eq(EqCurve::default());
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    let mut commands = Vec::new();
    assert!(native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .is_err());
    assert!(commands.is_empty());
}

#[test]
fn separate_target_runtime_updates_keep_equalizer_and_preamp_targets_isolated() {
    let mut native = NativeEffects::new(AfCommandSyntax::SeparateTarget);
    let mut commands = Vec::new();
    let payload = Payload::Eq(EqCurve::default());
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    native.reconfirm();
    native.configured();
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

    let mut edited = EqCurve::default();
    edited.gains_db[4] = 2.0;
    let edited = Payload::Eq(edited);
    let analysis = crate::effects::math::analyze(&edited, environment()).unwrap();
    native
        .apply(&edited, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    complete(&mut native);
    native
        .apply(&edited, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();

    let updates = commands
        .iter()
        .filter(|args| args[0] == "af-command")
        .collect::<Vec<_>>();
    assert!(!updates.is_empty());
    assert!(updates.iter().all(|args| args.len() == 5));
    assert!(updates
        .iter()
        .any(|args| args[1] == "echo_eq4" && args[2] == "gain" && args[4] == "eq4"));
    assert!(updates
        .iter()
        .any(|args| args[1] == "echo_preamp" && args[2] == "volume" && args[4] == "preamp"));
}

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
    native.configured();
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
    native.configured();
    assert!(!native
        .apply(&payload, environment(), &analysis, &mut command)
        .unwrap());
    complete(&mut native);
    assert!(native
        .apply(&payload, environment(), &analysis, &mut command)
        .unwrap());

    native.reconfirm();
    native.configured();
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
fn absent_reconfiguration_cannot_be_replaced_by_the_transition_clock() {
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
    native.installed_at = Some(Instant::now().checked_sub(TRANSITION).unwrap());
    native.configured(); // A configured graph alone cannot replace reconfig.
    assert!(!native
        .apply(&payload, environment(), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    assert!(native.install_pending);
    assert_eq!(commands.len(), 1);

    native.installed_at = Some(Instant::now().checked_sub(CONFIGURATION_TIMEOUT).unwrap());
    let result = native.apply(&payload, environment(), &analysis, |args| {
        commands.push(args.to_vec());
        Ok(())
    });
    assert!(result.unwrap_err().contains("confirmation timed out"));
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[1][..2], ["af", "remove"]);
    assert!(native.current.is_none());
    assert!(!native.install_pending);
}

#[test]
fn reconfiguration_without_initialized_nodes_remains_pending() {
    let mut native = NativeEffects::default();
    let payload = Payload::Eq(EqCurve::default());
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    native
        .apply(&payload, environment(), &analysis, |_| Ok(()))
        .unwrap();
    native.reconfirm();
    assert!(!native
        .apply(&payload, environment(), &analysis, |_| {
            panic!("unconfigured nodes cannot receive runtime commands")
        })
        .unwrap());
    native.failed("native audio filter failed or was disabled");
    let mut commands = Vec::new();
    let result = native.apply(&payload, environment(), &analysis, |args| {
        commands.push(args.to_vec());
        Ok(())
    });
    assert!(result.unwrap_err().contains("failed or was disabled"));
    assert_eq!(commands[0][..2], ["af", "remove"]);
}

#[test]
fn disabling_an_unconfirmed_install_cleans_it_without_waiting_or_applying() {
    let mut native = NativeEffects::default();
    let payload = Payload::Eq(EqCurve::default());
    let analysis = crate::effects::math::analyze(&payload, environment()).unwrap();
    native
        .apply(&payload, environment(), &analysis, |_| Ok(()))
        .unwrap();
    let mut commands = Vec::new();
    assert!(native
        .bypass(|args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap());
    assert_eq!(commands[0][..2], ["af", "remove"]);
    assert_eq!(commands.len(), 1);
    assert!(native.current.is_none());
}

#[test]
fn fixed_spatial_payload_uses_internal_width_ramp_from_neutral() {
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

    assert!(commands[0][2].contains("extrastereo@spatial=m=1:c=false"));
    native.reconfirm();
    native.configured();
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

    assert!(commands.iter().any(|args| args[0] == "af-command"
        && args[2] == "spatial:m"
        && args[3] == "1.250000000000000"));
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
    native.configured();
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
    native.configured();
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
