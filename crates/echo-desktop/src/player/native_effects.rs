//! Typed libmpv filter construction. Strings originate only from validated
//! numeric payloads; user text, preset names and paths never enter this syntax.
//!
//! # Runtime commands are not yet established to take effect
//!
//! `advance` expresses a 30 ms ramp by re-sending `af-command` for each labelled
//! stage. A 2026-10-03 native measurement on the packaged macOS library found
//! that NO runtime parameter change reached the audio, for any filter:
//!
//! | chain | commanded | measured |
//! |---|---|---|
//! | static `g=6` | install-time | **+5.9959 dB** (reproduces the recorded evidence) |
//! | `g=0` installed | `af-command <label> eq5:gain 6`, rc=0 | **+0.0076 dB** (= baseline) |
//! | `m=1.25` installed | `af-command <label> spatial:m 1.4`, rc=0 | **+0.0000 dB** (expected +0.9844) |
//!
//! `volume` behaved the same way, so this is not specific to one filter. The mpv
//! compatibility patch IS present in the shipped dylib: `otool` shows both the
//! colon-splitting lookup and the `"all"` fallback. Upstream, the biquad family
//! re-runs its coefficient setup after a command, so the fault is not explained
//! yet. Because mpv's `f_lavfi.c` only tests `result >= 0`, rc=0 is a FALSE
//! SUCCESS here.
//!
//! Consequences for this module: the ramp below cannot be assumed to reach the
//! output, and `af-command` return values prove nothing. Rebuilding the chain
//! (`install`/`remove`) is the only mechanism measured to change the audio.
//! Treat the in-place path as unproven until a native capture shows the
//! intermediate gain states, and re-verify with
//! `scripts/audio-effects/probe.py --capture-only --runtime-parameter-proof`.

use crate::effects::{
    math::{intermediate_gain_bound_db, EffectsAnalysis, AUTO_HEADROOM_DB},
    Payload, ProcessingEnvironment, BAND_FREQUENCIES,
};
use std::time::{Duration, Instant};

const TRANSITION: Duration = Duration::from_millis(30);
// `latency=true` makes alimiter compensate its own lookahead delay: FFmpeg sets
// `in_trim = out_pad = attack*Fs - 1`, so the output stays sample-aligned with
// the input (measured: 239 samples at 48 kHz/5 ms, matching that formula). The
// alternative `latency=false` leaves the full attack delay in the stream.
// `level=false` is required because auto-level defaults to ON and would
// normalise the limited output back up to 0 dB, destroying the preamp budget.
const LIMITER: &str = "alimiter=limit=0.891250938:attack=5:release=50:level_in=1:level_out=1:asc=false:level=false:latency=true";

type Command<'a> = dyn FnMut(&[String]) -> Result<(), String> + 'a;

/// Upstream mpv added a separate lavfi target argument in 0.37. The pinned
/// macOS 0.36 build instead carries our `<target>:<command>` compatibility patch.
/// Select from the loaded library's command metadata, not its client API version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum AfCommandSyntax {
    #[default]
    LegacyPatched,
    SeparateTarget,
    Unavailable,
}

impl AfCommandSyntax {
    pub(super) const fn from_argument_count(count: Option<usize>) -> Self {
        match count {
            Some(3) => Self::LegacyPatched,
            Some(4) => Self::SeparateTarget,
            _ => Self::Unavailable,
        }
    }

    fn send(
        self,
        label: &str,
        target: &str,
        option: &str,
        value: &str,
        command: &mut Command<'_>,
    ) -> Result<(), String> {
        match self {
            Self::LegacyPatched => command(&strings(&[
                "af-command",
                label,
                &format!("{target}:{option}"),
                value,
            ])),
            Self::SeparateTarget => {
                command(&strings(&["af-command", label, option, value, target]))
            }
            Self::Unavailable => {
                Err("native audio filter command capability unavailable".to_owned())
            }
        }
    }
}

#[derive(Clone)]
struct Configuration {
    payload: Payload,
    environment: ProcessingEnvironment,
    gains: [f64; 10],
    width: f64,
    preamp: f64,
}

struct Ramp {
    target: Configuration,
    from_gains: [f64; 10],
    protected_preamp: f64,
    started: Instant,
    bypass: bool,
}

#[derive(Default)]
pub(super) struct NativeEffects {
    command_syntax: AfCommandSyntax,
    current: Option<Configuration>,
    ramp: Option<Ramp>,
    labels: Vec<String>,
    reconfirm: bool,
    install_pending: bool,
    installed_at: Option<Instant>,
}

impl NativeEffects {
    pub(super) fn new(command_syntax: AfCommandSyntax) -> Self {
        Self {
            command_syntax,
            ..Self::default()
        }
    }

    pub(super) fn reconfirm(&mut self) {
        self.reconfirm = true;
        self.ramp = None;
        self.install_pending = false;
        self.installed_at = None;
    }

    fn recover_stale_install(&mut self) {
        if self.install_pending
            && self
                .installed_at
                .is_some_and(|installed_at| installed_at.elapsed() >= TRANSITION)
        {
            // AUDIO_RECONFIG is normally the confirmation barrier. If mpv
            // coalesces or omits it, let the next actor tick reconcile instead
            // of leaving every later request pending forever.
            self.reconfirm();
        }
    }

    pub(super) fn apply(
        &mut self,
        payload: &Payload,
        environment: ProcessingEnvironment,
        analysis: &EffectsAnalysis,
        mut command: impl FnMut(&[String]) -> Result<(), String>,
    ) -> Result<bool, String> {
        if self.command_syntax == AfCommandSyntax::Unavailable {
            return Err("native audio filter command capability unavailable".to_owned());
        }
        self.recover_stale_install();
        let target = Configuration {
            payload: payload.clone(),
            environment,
            gains: match payload {
                Payload::Eq(curve) => curve.gains_db,
                Payload::Spatial(_) => [0.0; 10],
            },
            width: match payload {
                Payload::Spatial(spatial) => spatial.width,
                Payload::Eq(_) => 1.0,
            },
            preamp: analysis.effective_preamp_db,
        };
        let changed = self.reconfirm
            || self.ramp.as_ref().map_or_else(
                || {
                    self.current.as_ref().map_or(true, |current| {
                        current.payload != *payload || current.environment != environment
                    })
                },
                |ramp| {
                    ramp.bypass
                        || ramp.target.payload != *payload
                        || ramp.target.environment != environment
                },
            );
        let result = (|| {
            if self.install_pending {
                return Ok(false);
            }
            let installed = if changed {
                let installed = self.start(target, analysis.active_bands, false, &mut command)?;
                self.reconfirm = false;
                installed
            } else {
                false
            };
            // `af add` schedules a graph rebuild. Wait for mpv to pump that
            // AUDIO_RECONFIG before issuing commands to its named lavfi nodes.
            if installed {
                Ok(false)
            } else {
                self.advance(&mut command)
            }
        })();
        if result.is_err() {
            // A partially accepted update is never a confirmed configuration.
            // Failure takes the safety path; it cannot restore an enabled target
            // that a subsequent disable request has already superseded.
            let cleanup = self.remove(&mut command);
            self.ramp = None;
            self.current = None;
            if cleanup.is_err() {
                return Err("native effects failed and bypass could not be confirmed".to_owned());
            }
        }
        result
    }

    pub(super) fn bypass(
        &mut self,
        mut command: impl FnMut(&[String]) -> Result<(), String>,
    ) -> Result<bool, String> {
        self.recover_stale_install();
        if self.install_pending {
            return Ok(false);
        }
        let Some(current) = self.current.clone() else {
            return self.remove(&mut command).map(|()| true);
        };
        if !self.ramp.as_ref().is_some_and(|ramp| ramp.bypass) {
            let mut target = current.clone();
            target.gains = [0.0; 10];
            target.width = 1.0;
            target.preamp = 0.0;
            self.ramp = Some(Ramp {
                target,
                from_gains: current.gains,
                protected_preamp: current.preamp,
                started: Instant::now(),
                bypass: true,
            });
        }
        self.advance(&mut command)
    }

    fn start(
        &mut self,
        target: Configuration,
        active: [bool; 10],
        bypass: bool,
        command: &mut Command<'_>,
    ) -> Result<bool, String> {
        let compatible = self.current.as_ref().is_some_and(|current| {
            current.environment == target.environment
                && std::mem::discriminant(&current.payload)
                    == std::mem::discriminant(&target.payload)
                && (matches!(target.payload, Payload::Eq(_))
                    || current.width.to_bits() == target.width.to_bits())
        });
        if !compatible {
            // Reconfiguration is restricted to type/environment changes. The
            // actor holds initial loads behind its pause barrier; audible type
            // changes remain subject to the native no-discontinuity Gate.
            self.remove(command)?;
            self.install(&target, active, command)?;
            let mut neutral = target.clone();
            neutral.gains = [0.0; 10];
            neutral.preamp = target.preamp;
            self.current = Some(neutral);
            self.ramp = Some(Ramp {
                target,
                from_gains: [0.0; 10],
                protected_preamp: self
                    .current
                    .as_ref()
                    .expect("installed configuration")
                    .preamp,
                started: Instant::now(),
                bypass,
            });
            return Ok(true);
        }
        let current = self.current.as_ref().expect("installed configuration");
        // Sum of each peaking stage's positive gain is a conservative bound
        // throughout an interpolation, including interior overlap maxima. It
        // shares AUTO_HEADROOM_DB with the steady-state preamp so the two
        // cannot drift apart.
        let risk = match target.payload {
            Payload::Eq(_) => intermediate_gain_bound_db(
                current
                    .gains
                    .iter()
                    .zip(target.gains)
                    .map(|(old, new)| old.max(new)),
            ),
            Payload::Spatial(_) => 20.0 * current.width.max(target.width).max(1.0).log10(),
        };
        let protected = current.preamp.min(target.preamp).min(if risk == 0.0 {
            0.0
        } else {
            -(risk + AUTO_HEADROOM_DB)
        });
        preamp(self.command_syntax, protected, command)?;
        self.ramp = Some(Ramp {
            target,
            from_gains: current.gains,
            protected_preamp: protected,
            started: Instant::now(),
            bypass,
        });
        Ok(false)
    }

    fn advance(&mut self, command: &mut Command<'_>) -> Result<bool, String> {
        let Some(ramp) = self.ramp.as_ref() else {
            return Ok(true);
        };
        let fraction = (ramp.started.elapsed().as_secs_f64() / TRANSITION.as_secs_f64()).min(1.0);
        let current = self
            .current
            .as_mut()
            .expect("ramp requires installed chain");
        match current.payload {
            Payload::Eq(_) => {
                for index in 0..10 {
                    if !self
                        .labels
                        .iter()
                        .any(|label| label == &format!("echo_eq{index}"))
                    {
                        continue;
                    }
                    let gain = ramp.from_gains[index]
                        + fraction * (ramp.target.gains[index] - ramp.from_gains[index]);
                    self.command_syntax.send(
                        &format!("echo_eq{index}"),
                        &format!("eq{index}"),
                        "gain",
                        &format!("{gain:.15}"),
                        command,
                    )?;
                    current.gains[index] = gain;
                }
            }
            // extrastereo has no process_command implementation in the
            // shipped FFmpeg. Its width is fixed when the filter is installed.
            Payload::Spatial(_) => current.width = ramp.target.width,
        }
        current.preamp = ramp.protected_preamp;
        if fraction < 1.0 {
            return Ok(false);
        }
        let target = ramp.target.clone();
        let bypass = ramp.bypass;
        // Reduce effect risk first, then release the protective preamp. The
        // limiter remains present until the complete bypass ramp has finished.
        preamp(self.command_syntax, target.preamp, command)?;
        self.current = Some(target);
        self.ramp = None;
        if bypass {
            self.remove(command)?;
        }
        Ok(true)
    }

    fn install(
        &mut self,
        target: &Configuration,
        active: [bool; 10],
        command: &mut Command<'_>,
    ) -> Result<(), String> {
        let mut filters = vec![
            "@echo_float:lavfi=[aformat=sample_fmts=dblp]".to_owned(),
            format!(
                "@echo_preamp:lavfi=[volume@preamp=volume={:.15}:precision=double]",
                10.0_f64.powf(target.preamp / 20.0)
            ),
        ];
        let mut labels = vec!["echo_float".to_owned(), "echo_preamp".to_owned()];
        match target.payload {
            Payload::Eq(_) => {
                for (index, frequency) in BAND_FREQUENCIES.iter().enumerate() {
                    if !active[index] {
                        continue;
                    }
                    filters.push(format!("@echo_eq{index}:lavfi=[equalizer@eq{index}=f={frequency}:t=q:w={:.15}:g=0:mix=1:normalize=false:precision=f64]", std::f64::consts::SQRT_2));
                    labels.push(format!("echo_eq{index}"));
                }
            }
            Payload::Spatial(_) => {
                filters.push(format!(
                    "@echo_spatial:lavfi=[extrastereo@spatial=m={:.15}:c=false]",
                    target.width
                ));
                labels.push("echo_spatial".to_owned());
            }
        }
        filters.push(format!("@echo_limiter:lavfi=[{LIMITER}]"));
        labels.push("echo_limiter".to_owned());
        command(&strings(&["af", "add", &filters.join(",")]))?;
        self.labels = labels;
        self.install_pending = true;
        self.installed_at = Some(Instant::now());
        Ok(())
    }

    fn remove(&mut self, command: &mut Command<'_>) -> Result<(), String> {
        if !self.labels.is_empty() {
            let labels = self
                .labels
                .iter()
                .map(|label| format!("@{label}"))
                .collect::<Vec<_>>()
                .join(",");
            command(&strings(&["af", "remove", &labels]))?;
            self.labels.clear();
        }
        self.current = None;
        self.ramp = None;
        self.install_pending = false;
        self.installed_at = None;
        Ok(())
    }
}

fn preamp(syntax: AfCommandSyntax, db: f64, command: &mut Command<'_>) -> Result<(), String> {
    syntax.send(
        "echo_preamp",
        "preamp",
        "volume",
        &format!("{:.15}", 10.0_f64.powf(db / 20.0)),
        command,
    )
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[cfg(test)]
#[path = "native_effects_tests.rs"]
mod tests;
