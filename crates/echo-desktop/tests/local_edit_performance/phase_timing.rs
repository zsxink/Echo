//! Direct timing of the real foreground directory commit and asynchronous
//! projection. Each sample begins after the previous wake has drained; after
//! the command completes, its wake (if any) must complete before the next
//! sample. Favorite deliberately projects inline; it must not be reported as
//! a zero-duration background wake that never happened.

use echo_core::domain::ids::LibraryRootId;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

use super::{distribution, Fixture};

const TARGET: &str = "echo::local_edit_performance";
const PROJECTION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct PhaseEvent {
    root: Option<String>,
    stage: Option<String>,
    milliseconds: Option<f64>,
}

impl Visit for PhaseEvent {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "root" {
            self.root = Some(format!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "stage" {
            self.stage = Some(value.to_owned());
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_ms" {
            self.milliseconds = Some(value);
        }
    }
}

#[derive(Default)]
struct ObservedPhases {
    directory_commit: Vec<Duration>,
    foreground_projection: Vec<Duration>,
    background_projection: Vec<Duration>,
}

#[derive(Clone)]
struct Capture {
    root: String,
    events: Arc<Mutex<ObservedPhases>>,
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let mut fields = PhaseEvent::default();
        event.record(&mut fields);
        if fields.root.as_deref() != Some(self.root.as_str()) {
            return;
        }
        let Some(milliseconds) = fields.milliseconds else {
            return;
        };
        let duration = Duration::from_secs_f64(milliseconds / 1_000.0);
        let mut observations = self.events.lock().expect("phase observations");
        match fields.stage.as_deref() {
            Some("directory_commit") => observations.directory_commit.push(duration),
            Some("sqlite_project_commit") => observations.foreground_projection.push(duration),
            Some("background_projection") => observations.background_projection.push(duration),
            _ => {}
        }
    }
}

pub struct PhaseTiming(Arc<Mutex<ObservedPhases>>);

impl PhaseTiming {
    pub(super) fn install(root: LibraryRootId) -> Self {
        let events = Arc::new(Mutex::new(ObservedPhases::default()));
        let capture = Capture {
            root: root.to_string(),
            events: Arc::clone(&events),
        };
        // A thread-local subscriber cannot observe the SQLite writer or projection
        // worker. This dedicated benchmark executable has only one measured test.
        let phase_only =
            tracing_subscriber::filter::filter_fn(|metadata| metadata.target() == TARGET);
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(capture.with_filter(phase_only)),
        )
        .expect("install benchmark phase subscriber");
        Self(events)
    }

    pub(super) fn measure<T>(
        &self,
        f: &Fixture,
        samples: &mut CommandSamples,
        command: impl FnOnce() -> T,
    ) -> T {
        let start = Instant::now();
        let value = command();
        samples.command.push(start.elapsed());
        if !samples.foreground {
            let acknowledged = Instant::now();
            f.database
                .flush_library_projection(f.root, PROJECTION_TIMEOUT)
                .expect("sample background projection caught up");
            samples.projection_after_ack.push(acknowledged.elapsed());
        }
        let observed = std::mem::take(&mut *self.0.lock().expect("phase observations"));
        samples
            .directory_commit
            .push(only_phase(&observed.directory_commit, "directory commit"));
        let (expected, unexpected, phase) = if samples.foreground {
            (
                observed.foreground_projection,
                observed.background_projection,
                "foreground projection",
            )
        } else {
            (
                observed.background_projection,
                observed.foreground_projection,
                "background projection",
            )
        };
        assert!(
            unexpected.is_empty(),
            "unexpected projection execution mode"
        );
        samples.projected.push(only_phase(&expected, phase));
        value
    }
}

fn only_phase(observed: &[Duration], phase: &str) -> Duration {
    let [duration] = observed else {
        panic!(
            "one measured command must complete exactly one {phase}, got {}",
            observed.len()
        );
    };
    *duration
}

#[derive(Default)]
pub struct CommandSamples {
    foreground: bool,
    command: Vec<Duration>,
    directory_commit: Vec<Duration>,
    projected: Vec<Duration>,
    projection_after_ack: Vec<Duration>,
}

impl CommandSamples {
    pub(super) fn foreground() -> Self {
        Self {
            foreground: true,
            ..Self::default()
        }
    }

    pub(super) fn report(&self) -> serde_json::Value {
        let mut report = distribution(&self.command);
        report["directory_commit"] = distribution(&self.directory_commit);
        report["projection_mode"] = if self.foreground {
            "foreground".into()
        } else {
            "background".into()
        };
        report["foreground_projection"] = serde_json::Value::Null;
        report["background_projection"] = serde_json::Value::Null;
        report["projection_after_ack"] = serde_json::Value::Null;
        if self.foreground {
            report["foreground_projection"] = distribution(&self.projected);
        } else {
            report["background_projection"] = distribution(&self.projected);
            // Includes worker debounce/wait, outside the interaction threshold.
            report["projection_after_ack"] = distribution(&self.projection_after_ack);
        }
        report
    }
}
