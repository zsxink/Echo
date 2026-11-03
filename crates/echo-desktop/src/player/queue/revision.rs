//! 队列投影的修订号。
//!
//! 修订号是「队列投影是否变化」的**唯一**传输判据：传输层只在修订号变化时
//! 才重新下发整份队列投影，位置帧不再重复搬运整队条目。

/// A monotonic revision of the queue projection.
///
/// Every change that can alter the projection advances it, so the transport
/// can ship the projection once per change instead of on every position tick.
/// The revision is *not* a content fingerprint: it may advance for a call
/// that changed nothing observable, which is harmless — a consumer only needs
/// "changed since the last delivery", never "what changed".
///
/// The counter lives inside [`super::Queue`] so that every path that can change
/// the queue — including the ones that reach it through `&mut Queue` — advances
/// it. A missing bump would silently freeze the projection on the frontend,
/// which is far worse than a redundant bump.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct QueueRevision(u64);

impl QueueRevision {
    /// The revision of a queue that has never been changed.
    pub const INITIAL: Self = Self(0);

    /// The revision after this one.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// The raw counter value.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}
