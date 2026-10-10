//! Typed libmpv filter construction. Strings originate only from validated
//! numeric payloads; user text, preset names and paths never enter this syntax.
//!
//! Continuous-input PCM capture confirms runtime EQ/preamp/width commands on
//! the packaged macOS library. A command return code alone remains insufficient
//! evidence; output smoothness and the other platform packages still require
//! their native gates. See the change's `native-gate.md` for reproduction.

use crate::effects::{
    math::{intermediate_eq_peak_db, EffectsAnalysis, AUTO_HEADROOM_DB},
    Payload, ProcessingEnvironment, BAND_FREQUENCIES,
};
use std::time::{Duration, Instant};

const TRANSITION: Duration = Duration::from_millis(30);
const CONFIGURATION_TIMEOUT: Duration = Duration::from_secs(2);
// `latency=true` compensates the look-ahead delay: trimmed initial output
// belongs to the delay buffer, and zero input at EOF drains buffered program
// audio. At 48 kHz / 5 ms the compensation is 239 samples. Native head/tail
// content and timestamp alignment still require measurement.
// `level=false` is required because auto-level defaults to ON and would
// normalise the limited output back up to 0 dB, destroying the preamp budget.
const LIMITER: &str = "alimiter=limit=0.891250938:attack=5:release=50:level_in=1:level_out=1:asc=false:level=false:latency=true";

type Command<'a> = dyn FnMut(&[String]) -> Result<(), String> + 'a;

#[path = "native_effects_commands.rs"]
mod commands;
use commands::{preamp, reassert, strings, width};

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
    peak_gain: f64,
    headroom: f64,
}

struct Ramp {
    target: Configuration,
    from: Configuration,
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
    reconfiguration_observed: bool,
    failure: Option<String>,
}

impl NativeEffects {
    pub(super) fn new(command_syntax: AfCommandSyntax) -> Self {
        Self {
            command_syntax,
            ..Self::default()
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
            peak_gain: analysis.peak_gain_db,
            // Manual settings keep their strict floor during interpolation;
            // automatic compensation is released continuously with the curve.
            headroom: (analysis.effective_preamp_db + analysis.peak_gain_db.max(0.0))
                .clamp(0.0, AUTO_HEADROOM_DB),
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
            self.check_confirmation()?;
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
        let result = self.start_bypass(&mut command);
        if result.is_err() {
            let cleanup = self.remove(&mut command);
            self.ramp = None;
            self.current = None;
            if cleanup.is_err() {
                return Err("native effects failed and bypass could not be confirmed".to_owned());
            }
        }
        result
    }

    fn start_bypass(&mut self, command: &mut Command<'_>) -> Result<bool, String> {
        if self.install_pending || self.failure.is_some() {
            // A cancelled or failed installation has no audible target to ramp.
            // Remove it immediately, including a request whose reconfig is absent.
            return self.remove(command).map(|()| true);
        }
        let Some(current) = self.current.clone() else {
            return self.remove(command).map(|()| true);
        };
        if self.reconfirm {
            reassert(self.command_syntax, &current, &self.labels, command)?;
            self.reconfirm = false;
        }
        if !self.ramp.as_ref().is_some_and(|ramp| ramp.bypass) {
            let mut target = current.clone();
            target.gains = [0.0; 10];
            target.width = 1.0;
            target.preamp = 0.0;
            target.peak_gain = 0.0;
            target.headroom = 0.0;
            self.ramp = Some(Ramp {
                target,
                from: current,
                started: Instant::now(),
                bypass: true,
            });
        }
        self.advance(command)
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
        });
        if !compatible {
            // Reconfiguration is restricted to type/environment changes. The
            // actor holds initial loads behind its pause barrier; audible type
            // changes remain subject to the native no-discontinuity Gate.
            self.remove(command)?;
            self.install(&target, active, command)?;
            let mut neutral = target.clone();
            neutral.gains = [0.0; 10];
            neutral.width = 1.0;
            neutral.preamp = 0.0;
            neutral.peak_gain = 0.0;
            neutral.headroom = 0.0;
            self.current = Some(neutral.clone());
            self.ramp = Some(Ramp {
                target,
                from: neutral,
                started: Instant::now(),
                bypass,
            });
            return Ok(true);
        }
        let current = self.current.as_ref().expect("installed configuration");
        if self.reconfirm {
            // A rebuilt lavfi graph starts at the installation arguments, not
            // the last runtime values. Restore the protective preamp before
            // reasserting EQ gains even when the logical state is unchanged.
            reassert(self.command_syntax, current, &self.labels, command)?;
        }
        self.ramp = Some(Ramp {
            target,
            from: current.clone(),
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
        let gains = std::array::from_fn(|index| {
            ramp.from.gains[index] + fraction * (ramp.target.gains[index] - ramp.from.gains[index])
        });
        let next_width = ramp.from.width + fraction * (ramp.target.width - ramp.from.width);
        let peak = match current.payload {
            Payload::Eq(_) => {
                if gains
                    .iter()
                    .zip(current.gains)
                    .all(|(left, right)| left.to_bits() == right.to_bits())
                {
                    current.peak_gain
                } else if fraction >= 1.0 {
                    ramp.target.peak_gain
                } else {
                    intermediate_eq_peak_db(gains, current.environment.sample_rate)
                }
            }
            Payload::Spatial(_) => 20.0 * next_width.max(1.0).log10(),
        };
        let headroom = ramp.from.headroom + fraction * (ramp.target.headroom - ramp.from.headroom);
        let requested = ramp.from.preamp + fraction * (ramp.target.preamp - ramp.from.preamp);
        let next_preamp = requested.min(-peak.max(0.0) + headroom);
        match current.payload {
            Payload::Eq(_) => {
                // A peaking stage's response is monotonic in gain at every
                // frequency. Reduce all stages first, then tighten preamp,
                // then increase stages: intermediate command states cannot
                // exceed the larger old/new response budget. Release preamp
                // only after the reduced-risk curve has reached the backend.
                for increasing in [false, true] {
                    if increasing && next_preamp < current.preamp {
                        preamp(self.command_syntax, next_preamp, command)?;
                        current.preamp = next_preamp;
                    }
                    for (index, gain) in gains.iter().copied().enumerate() {
                        if !self
                            .labels
                            .iter()
                            .any(|label| label == &format!("echo_eq{index}"))
                        {
                            continue;
                        }
                        if (gain > current.gains[index]) != increasing {
                            continue;
                        }
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
            }
            Payload::Spatial(_) => {
                if next_preamp < current.preamp {
                    preamp(self.command_syntax, next_preamp, command)?;
                    current.preamp = next_preamp;
                }
                if current.width.to_bits() != next_width.to_bits() {
                    width(self.command_syntax, next_width, command)?;
                }
                current.width = next_width;
            }
        }
        if current.preamp.to_bits() != next_preamp.to_bits() {
            preamp(self.command_syntax, next_preamp, command)?;
        }
        current.preamp = next_preamp;
        current.peak_gain = peak;
        current.headroom = headroom;
        if fraction < 1.0 {
            return Ok(false);
        }
        let target = ramp.target.clone();
        let bypass = ramp.bypass;
        // The final tick already reaches target preamp; no end-of-ramp jump.
        // The limiter remains present until the complete bypass has finished.
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
            "@echo_preamp:lavfi=[volume@preamp=volume=1:precision=double]".to_owned(),
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
                // Width is fixed in the user payload. The internal neutral
                // value only supports protected enable/bypass transitions.
                filters.push("@echo_spatial:lavfi=[extrastereo@spatial=m=1:c=false]".to_owned());
                labels.push("echo_spatial".to_owned());
            }
        }
        filters.push(format!("@echo_limiter:lavfi=[{LIMITER}]"));
        labels.push("echo_limiter".to_owned());
        command(&strings(&["af", "add", &filters.join(",")]))?;
        self.labels = labels;
        self.install_pending = true;
        self.reconfiguration_observed = false;
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
        self.reconfiguration_observed = false;
        self.failure = None;
        Ok(())
    }
}

#[path = "native_effects_confirmation.rs"]
mod confirmation;

#[cfg(test)]
#[path = "native_effects_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "native_effects_transition_tests.rs"]
mod transition_tests;
