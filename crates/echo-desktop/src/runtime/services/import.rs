//! Import flows (task 7.3 / 11.7). The dialog runs entirely on the Rust/Tauri
//! boundary; the `WebView` only ever receives relative paths and user-safe
//! errors.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use echo_core::application::import::{ImportBatchReport, ImportOutcome, PlanImport};
use echo_core::application::ports::ImportSource;
use echo_core::error::Error;

use crate::ipc::dto::{ImportBatchDto, ImportResultDto};
use crate::platform::import::SingleFileImport;

impl super::AppServices {
    /// Run the desktop import-file dialog and import the chosen files into the
    /// active root. The dialog runs entirely on the Rust/Tauri boundary; the
    /// `WebView` only ever receives the per-input result DTO (relative paths and
    /// user-safe errors).
    ///
    /// Returns `Ok(None)` when the user **cancelled** the dialog — a cancel is
    /// a no-op, never a successful (or failed) batch.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled or no root is active; after a
    /// picker selection every requested source receives a typed batch result,
    /// including an unavailable root, so the UI can render one consistent
    /// failure surface.
    pub fn choose_and_import_files(&self) -> Result<Option<ImportBatchDto>, Error> {
        let Some(picked) = self.dialogs.pick_audio_files()? else {
            // Cancelled: not an error, not a success — a genuine no-op.
            return Ok(None);
        };
        if picked.sources.is_empty() {
            return Ok(Some(ImportBatchDto { results: vec![] }));
        }
        if let Err(error) = self.guard_writes() {
            return Ok(Some(unavailable_batch(picked.sources.len(), &error)));
        }
        let Some(root) = self.deps.roots.active_root()?.map(|root| root.id()) else {
            return Ok(Some(unavailable_batch(
                picked.sources.len(),
                &Error::unavailable("library", "no active library root"),
            )));
        };
        let report =
            self.import_batch_concurrently(root, picked.reader.as_ref(), &picked.sources)?;
        Ok(Some(ImportBatchDto::from(report)))
    }

    /// Run independent files on a tracked, bounded worker pool.  Each worker
    /// deliberately invokes the existing one-input Core flow: that keeps the
    /// journal, destination claim and `SQLite` transaction ownership per file.
    /// The coordinator only schedules work and restores the picker order.
    fn import_batch_concurrently(
        &self,
        root: echo_core::domain::ids::LibraryRootId,
        reader: &dyn echo_core::application::ports::ImportSourceReader,
        sources: &[ImportSource],
    ) -> Result<ImportBatchReport, Error> {
        let worker_count =
            bounded_worker_count(sources.len(), std::thread::available_parallelism());
        if worker_count <= 1 {
            return PlanImport::new(self.deps.as_ref(), reader).run(root, sources);
        }

        let results =
            run_bounded_in_order(sources, worker_count, |source| {
                PlanImport::new(self.deps.as_ref(), reader)
                    .run(root, std::slice::from_ref(source))
                    .map_or_else(
                        |error| ImportOutcome::Failed {
                            code: "import_worker",
                            message: error.to_string(),
                        },
                        |report| {
                            report.results.into_iter().next().unwrap_or_else(|| {
                                ImportOutcome::Failed {
                                    code: "import_worker",
                                    message: "import worker returned no result".to_owned(),
                                }
                            })
                        },
                    )
            })
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|| ImportOutcome::Failed {
                    code: "import_worker",
                    message: "import worker did not complete".to_owned(),
                })
            })
            .collect();
        Ok(ImportBatchReport { results })
    }

    /// Import a single file (by absolute, desktop-owned path) into the active
    /// library root (task 11.7: "import current temporary playback item").
    ///
    /// The path stays entirely desktop-side — it is never forwarded to the
    /// `WebView`. The caller (the Tauri command layer) extracts it from the
    /// coordinator's current queue entry and passes it here.
    ///
    /// Returns `Ok(result)` with the single-file import result (imported /
    /// duplicate / unsupported / failed) or `Err` when the root is unavailable
    /// or the path is unreadable.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled, no root is active, or the path
    /// is unreadable; infrastructure errors propagate; per-input problems
    /// become `ImportResultDto::Failed`.
    ///
    /// # Panics
    ///
    /// Never: `PlanImport::run` yields exactly one result per submitted source
    /// and this call submits exactly one, so the single-source invariant the
    /// `expect` names always holds.
    pub fn import_single_path(
        &self,
        absolute_path: &std::path::Path,
        display_name: &str,
    ) -> Result<ImportResultDto, Error> {
        self.guard_writes()?;
        let root_id = self
            .deps
            .roots
            .active_root()?
            .map(|r| r.id())
            .ok_or_else(|| Error::unavailable("library", "no active library root"))?;
        let reader = SingleFileImport::new(absolute_path, display_name)?;
        let source = reader.source().clone();
        let report = PlanImport::new(self.deps.as_ref(), &reader).run(root_id, &[source])?;
        // A single-source batch always has exactly one result.
        let outcome = report
            .results
            .into_iter()
            .next()
            .expect("import batch has at least one result");
        Ok(ImportResultDto::from(outcome))
    }
}

fn unavailable_batch(count: usize, _error: &Error) -> ImportBatchDto {
    ImportBatchDto {
        results: std::iter::repeat_with(|| ImportResultDto::LibraryUnavailable)
            .take(count)
            .collect(),
    }
}

fn bounded_worker_count(
    batch_size: usize,
    available: std::io::Result<std::num::NonZeroUsize>,
) -> usize {
    batch_size
        .min(available.map_or(2, usize::from).max(2))
        .min(4)
}

/// Execute independent work on at most `worker_count` scoped threads and
/// return results in input order. A missing slot means a worker panicked; the
/// caller can turn that into its domain-specific per-input failure.
fn run_bounded_in_order<T, R, F>(items: &[T], worker_count: usize, run: F) -> Vec<Option<R>>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> =
        Mutex::new(std::iter::repeat_with(|| None).take(items.len()).collect());
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= items.len() {
                    return;
                }
                let result = run(&items[index]);
                results
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)[index] = Some(result);
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod concurrency_tests {
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::{Duration, Instant};

    use super::{bounded_worker_count, run_bounded_in_order};

    #[test]
    fn bounded_pool_enters_two_heavy_tasks_concurrently_and_keeps_picker_order() {
        let both_workers_at_probe = Arc::new(Barrier::new(2));
        let observed_threads = Arc::new(Mutex::new(Vec::new()));
        let inputs = ["first", "second", "third"];
        let results = run_bounded_in_order(&inputs, 2, {
            let barrier = Arc::clone(&both_workers_at_probe);
            let threads = Arc::clone(&observed_threads);
            move |input| {
                if *input != "third" {
                    threads
                        .lock()
                        .expect("thread record")
                        .push(std::thread::current().id());
                    barrier.wait();
                }
                *input
            }
        });

        let threads = observed_threads.lock().expect("thread record");
        assert_eq!(threads.len(), 2, "both heavy tasks reached the barrier");
        assert_ne!(threads[0], threads[1], "they ran on distinct workers");
        drop(threads);
        assert_eq!(results, vec![Some("first"), Some("second"), Some("third")]);
    }

    #[test]
    fn controlled_fake_io_is_faster_than_serial_execution() {
        let inputs = [(), ()];
        let work = || std::thread::sleep(Duration::from_millis(45));
        let serial_started = Instant::now();
        for () in inputs {
            work();
        }
        let serial = serial_started.elapsed();

        let concurrent_started = Instant::now();
        let results = run_bounded_in_order(&inputs, 2, |()| work());
        let concurrent = concurrent_started.elapsed();

        assert_eq!(results.len(), inputs.len());
        assert!(
            concurrent + Duration::from_millis(20) < serial,
            "bounded concurrent fake-I/O ({concurrent:?}) should beat serial ({serial:?})"
        );
    }

    #[test]
    fn worker_count_is_bounded_and_never_exceeds_batch_size() {
        assert_eq!(bounded_worker_count(0, Ok(std::num::NonZeroUsize::MIN)), 0);
        assert_eq!(bounded_worker_count(1, Ok(std::num::NonZeroUsize::MIN)), 1);
        assert_eq!(bounded_worker_count(20, Ok(std::num::NonZeroUsize::MIN)), 2);
        assert_eq!(
            bounded_worker_count(20, Ok(std::num::NonZeroUsize::new(8).unwrap())),
            4
        );
    }
}
