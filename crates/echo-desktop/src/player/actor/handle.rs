//! Actor handle lifecycle and port implementations, split from `mod.rs` to
//! keep the command-loop file under the repository's 1000-line ceiling.

use super::{
    mpsc, NormalizationPort, Ordering, PlayerActor, PlayerCommand, PlayerError, PlayerPort,
    PlayerSnapshot,
};

impl PlayerActor {
    /// Ordered shutdown: stop accepting commands, terminate the backend on the
    /// actor thread, join the thread. Blocks until teardown completes.
    /// Takes `&mut self` (not `self`) so a caller can observe
    /// [`Self::is_stopped`] after teardown; idempotent.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(PlayerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        self.stopped.store(true, Ordering::Release);
    }

    /// Whether the actor has fully shut down.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

impl Drop for PlayerActor {
    /// Ordered teardown if the caller did not run [`Self::shutdown`] explicitly
    /// (e.g. the actor is moved around by the runtime). Prevents a leaked
    /// actor thread at process exit.
    fn drop(&mut self) {
        if self.join.is_some() && !self.is_stopped() {
            let _ = self.tx.send(PlayerCommand::Shutdown);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            self.stopped.store(true, Ordering::Release);
        }
    }
}

impl NormalizationPort for PlayerActor {
    fn normalization_submit(
        &self,
        preference: crate::effects::loudness::NormalizationPreference,
    ) -> Result<(), PlayerError> {
        self.effects.set_normalization(preference)
    }
    fn normalization_runtime(&self) -> crate::effects::loudness::NormalizationRuntime {
        self.effects.normalization_runtime()
    }
}

impl PlayerPort for PlayerActor {
    fn effects_submit(
        &self,
        revision: u64,
        enabled: bool,
        payload: crate::effects::Payload,
    ) -> Result<crate::effects::EffectsRuntime, PlayerError> {
        self.effects.submit(revision, enabled, payload)
    }
    fn effects_runtime(&self) -> crate::effects::EffectsRuntime {
        self.effects.runtime()
    }
    fn effects_bypass(&self, revision: u64) -> Result<crate::effects::EffectsRuntime, PlayerError> {
        self.effects.bypass(revision)
    }

    fn send(&self, cmd: PlayerCommand) -> Result<(), PlayerError> {
        self.tx.try_send(cmd).map_err(|_| {
            // A full or closed channel means the actor cannot accept work now.
            PlayerError::ActorClosed
        })
    }

    fn snapshot(&self) -> PlayerSnapshot {
        self.snapshot
            .read()
            .expect("actor snapshot poisoned")
            .clone()
    }

    fn subscribe_snapshots(&self) -> mpsc::Receiver<PlayerSnapshot> {
        // A real, bounded mailbox: the actor loop fans every published snapshot
        // into it. The previous "never-delivering placeholder" here made the
        // whole `player://snapshot` stream a no-op in production — the player
        // played fine while every snapshot-driven surface stayed blank.
        let (tx, rx) = mpsc::sync_channel(1);
        self.subscribers
            .lock()
            .expect("player subscribers poisoned")
            .push(tx);
        rx
    }
}
