//! Native graph confirmation, independent of the audible transition clock.

use super::{Instant, NativeEffects, CONFIGURATION_TIMEOUT};

impl NativeEffects {
    pub(in crate::player::actor) fn reconfirm(&mut self) {
        self.reconfirm = true;
        self.ramp = None;
        if !self.labels.is_empty() {
            self.install_pending = true;
            self.reconfiguration_observed = true;
            // Repeated notifications cannot extend a failed install forever.
            self.installed_at.get_or_insert_with(Instant::now);
        }
    }

    pub(in crate::player::actor) fn confirmation_labels(&self) -> &[String] {
        &self.labels
    }

    pub(in crate::player::actor) const fn needs_confirmation(&self) -> bool {
        self.install_pending && self.reconfiguration_observed && self.failure.is_none()
    }

    /// Called only after `GET_META` succeeds for every installed lavfi node.
    /// mpv rejects `GET_META` until that node's graph has actually initialized.
    pub(in crate::player::actor) fn configured(&mut self) {
        if self.needs_confirmation() {
            self.install_pending = false;
            self.installed_at = None;
        }
    }

    pub(in crate::player::actor) fn failed(&mut self, reason: &str) -> bool {
        if self.labels.is_empty() {
            return false;
        }
        self.failure = Some(reason.to_owned());
        true
    }

    pub(in crate::player::actor) fn check_confirmation(&self) -> Result<(), String> {
        if let Some(reason) = &self.failure {
            return Err(reason.clone());
        }
        if self.install_pending
            && self
                .installed_at
                .is_some_and(|started| started.elapsed() >= CONFIGURATION_TIMEOUT)
        {
            return Err("native audio filter configuration confirmation timed out".to_owned());
        }
        Ok(())
    }
}
