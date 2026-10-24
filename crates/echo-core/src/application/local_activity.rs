//! Instance-scoped admission and drain barrier for local library work.
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::{self, ThreadId};

use crate::error::Error;

#[derive(Default)]
struct State {
    closed: bool,
    generation: u64,
    exclusive: bool,
    waiting_exclusive: usize,
    #[cfg(test)]
    waiting_shared: usize,
    owners: HashMap<ThreadId, usize>,
}

/// Shared by every local writer belonging to one application runtime.
#[derive(Default)]
pub struct LocalActivity {
    state: Mutex<State>,
    changed: Condvar,
}

impl LocalActivity {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Admit an operation. Already admitted threads may nest during a drain.
    /// # Errors
    /// Returns unavailable after admission closes.
    pub fn enter(&self) -> Result<ActivityLease<'_>, Error> {
        let owner = thread::current().id();
        let mut state = self.lock();
        let generation = state.generation;
        loop {
            if let Some(depth) = state.owners.get_mut(&owner) {
                *depth += 1;
                break;
            }
            if state.closed || state.generation != generation {
                return Err(closed());
            }
            if !state.exclusive && state.waiting_exclusive == 0 {
                state.owners.insert(owner, 1);
                break;
            }
            #[cfg(test)]
            {
                state.waiting_shared += 1;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            #[cfg(test)]
            {
                state.waiting_shared -= 1;
            }
        }
        drop(state);
        Ok(ActivityLease {
            gate: self,
            owner,
            not_send: PhantomData,
        })
    }

    /// Drain readers and execute an enrollment/publication operation alone.
    /// # Errors
    /// Refuses closed admission or an attempted upgrade from a current lease.
    pub fn exclusive(&self) -> Result<ActivityLease<'_>, Error> {
        let owner = thread::current().id();
        let mut state = self.lock();
        if state.closed {
            return Err(closed());
        }
        if state.owners.contains_key(&owner) {
            return Err(Error::conflict("cannot upgrade a local activity lease"));
        }
        let generation = state.generation;
        state.waiting_exclusive += 1;
        loop {
            if state.closed || state.generation != generation {
                state.waiting_exclusive -= 1;
                self.changed.notify_all();
                return Err(closed());
            }
            if state.owners.is_empty() && !state.exclusive {
                state.waiting_exclusive -= 1;
                state.exclusive = true;
                state.owners.insert(owner, 1);
                return Ok(ActivityLease {
                    gate: self,
                    owner,
                    not_send: PhantomData,
                });
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Close admission before cancelling producers, then call `drain`.
    /// The guard reopens admission on failure or after a root activation.
    /// # Errors
    /// Refuses concurrent closure or closure by a thread holding a lease.
    pub fn close(&self) -> Result<ActivityClosure<'_>, Error> {
        let mut state = self.lock();
        if state.closed {
            return Err(closed());
        }
        if state.owners.contains_key(&thread::current().id()) {
            return Err(Error::conflict(
                "cannot drain one's own local activity lease",
            ));
        }
        state.closed = true;
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        self.changed.notify_all();
        Ok(ActivityClosure {
            gate: self,
            permanent: false,
            not_send: PhantomData,
        })
    }

    /// Used after scan registration to close the admission/registration race.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }
}

fn closed() -> Error {
    Error::unavailable("local activity", "library admission is closed")
}

/// A thread-bound lease; dropping it releases one nested admission.
pub struct ActivityLease<'a> {
    gate: &'a LocalActivity,
    owner: ThreadId,
    not_send: PhantomData<Rc<()>>,
}
impl Drop for ActivityLease<'_> {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        if let Some(depth) = state.owners.get_mut(&self.owner) {
            *depth -= 1;
            if *depth == 0 {
                state.owners.remove(&self.owner);
            }
        }
        if state.owners.is_empty() {
            state.exclusive = false;
        }
        drop(state);
        self.gate.changed.notify_all();
    }
}

/// Owns a temporary closure, or seals it permanently after a graceful exit.
pub struct ActivityClosure<'a> {
    gate: &'a LocalActivity,
    permanent: bool,
    not_send: PhantomData<Rc<()>>,
}
impl ActivityClosure<'_> {
    /// Wait until every admitted operation has released all nested leases.
    pub fn drain(&self) {
        let mut state = self.gate.lock();
        while !state.owners.is_empty() {
            state = self
                .gate
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(state);
    }
    /// Keep admission closed after a successful shutdown drain.
    pub fn seal(mut self) {
        self.drain();
        self.permanent = true;
    }
}
impl Drop for ActivityClosure<'_> {
    fn drop(&mut self) {
        if !self.permanent {
            self.gate.lock().closed = false;
            self.gate.changed.notify_all();
        }
    }
}

#[cfg(test)]
mod tests;
