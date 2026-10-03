//! Bridge from effects use cases to the existing single player actor.
use super::EffectsPlaybackPort;
use crate::effects::{EffectsRuntime, EffectsState};
use crate::player::port::PlayerPort;
use std::sync::Arc;

pub struct PlayerEffectsAdapter {
    player: Arc<dyn PlayerPort>,
}
impl PlayerEffectsAdapter {
    #[must_use]
    pub fn new(player: Arc<dyn PlayerPort>) -> Self {
        Self { player }
    }
}
impl EffectsPlaybackPort for PlayerEffectsAdapter {
    fn submit(&self, state: &EffectsState) -> Result<EffectsRuntime, String> {
        self.player
            .effects_submit(
                state.runtime.revision,
                state.document.requested_enabled,
                state.document.retained_payload.clone(),
            )
            .map_err(|error| error.to_string())
    }
    fn bypass(&self, state: &EffectsState) -> Result<EffectsRuntime, String> {
        self.player
            .effects_bypass(state.runtime.revision)
            .map_err(|error| error.to_string())
    }
    fn runtime(&self) -> EffectsRuntime {
        self.player.effects_runtime()
    }
}
