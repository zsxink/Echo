//! Named native parameter commands and safe graph reassertion.

use super::{AfCommandSyntax, Command, Configuration, Payload};

pub(super) fn preamp(
    syntax: AfCommandSyntax,
    db: f64,
    command: &mut Command<'_>,
) -> Result<(), String> {
    syntax.send(
        "echo_preamp",
        "preamp",
        "volume",
        &format!("{:.15}", 10.0_f64.powf(db / 20.0)),
        command,
    )
}
pub(super) fn reassert(
    syntax: AfCommandSyntax,
    current: &Configuration,
    labels: &[String],
    command: &mut Command<'_>,
) -> Result<(), String> {
    match current.payload {
        Payload::Eq(_) => {
            // Nodes may have reset to installation's zero gains. Restore cuts
            // before boosts: endpoint protection relies on their attenuation.
            for negative in [true, false] {
                if !negative {
                    preamp(syntax, current.preamp, command)?;
                }
                for (index, gain) in current.gains.iter().copied().enumerate() {
                    let label = format!("echo_eq{index}");
                    if (gain < 0.0) == negative && labels.contains(&label) {
                        syntax.send(
                            &label,
                            &format!("eq{index}"),
                            "gain",
                            &format!("{gain:.15}"),
                            command,
                        )?;
                    }
                }
            }
        }
        Payload::Spatial(_) => {
            preamp(syntax, current.preamp, command)?;
            width(syntax, current.width, command)?;
        }
    }
    Ok(())
}
pub(super) fn width(
    syntax: AfCommandSyntax,
    value: f64,
    command: &mut Command<'_>,
) -> Result<(), String> {
    syntax.send(
        "echo_spatial",
        "spatial",
        "m",
        &format!("{value:.15}"),
        command,
    )
}
pub(super) fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
