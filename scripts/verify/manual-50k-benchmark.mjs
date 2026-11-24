// The benchmark is explicit verification, not a delegating default CI job.
export const MANUAL_BENCHMARK_WORKFLOW = "benchmark-50k.yml";
export const MANUAL_BENCHMARK_SCENARIO = "LE-R07-S01";
export const MANUAL_BENCHMARK_FLAG = "--defer-manual-benchmark";

const RUN = "node scripts/verify/checks/task-12.5.mjs";

function strings(value) {
  if (typeof value === "string") return [value];
  if (typeof value === "object" && value !== null) return Object.values(value).flatMap(strings);
  return [];
}

export function assertNoDefaultBenchmark(workflow) {
  for (const [name, job] of Object.entries(workflow.jobs || {})) {
    if (strings(job).some((entry) =>
      /(?:^|\s)12\.5(?:$|\s)|task-12\.5\.mjs|bench_50k_search_and_first_screen_p95_meet_prd_budgets|benchmark-50k/i.test(entry),
    )) throw new Error(`default CI job '${name}' must not run or upload the manual 50k benchmark`);
  }
}

export function assertManualBenchmark(workflow) {
  const label = MANUAL_BENCHMARK_WORKFLOW;
  if (JSON.stringify(Object.keys(workflow.on || {})) !== '["workflow_dispatch"]') {
    throw new Error(`${label} must trigger on workflow_dispatch only`);
  }
  const job = workflow.jobs?.benchmark;
  if (Object.keys(workflow.jobs || {}).length !== 1 || job?.["runs-on"] !== "ubuntu-latest" ||
      job.if !== undefined || job["continue-on-error"] !== undefined ||
      job.needs !== undefined || !Array.isArray(job.steps)) {
    throw new Error(`${label} must have one unconditional Ubuntu benchmark job`);
  }
  if (job.steps.some((step) => /(?:actions\/cache|rust-cache)@/.test(step.uses || ""))) {
    throw new Error(`${label} must not use or write a Rust/project cache`);
  }
  const runs = job.steps.filter((step) => step.name === "Run 50k benchmark");
  const uploads = job.steps.filter((step) => step.name === "Upload 50k benchmark result");
  const [run] = runs;
  const [upload] = uploads;
  if (runs.length !== 1 || run.run !== RUN || run.if !== undefined ||
      run["continue-on-error"] !== undefined) {
    throw new Error(`${label} must run '${RUN}' unconditionally`);
  }
  if (uploads.length !== 1 || upload.if !== undefined || upload["continue-on-error"] !== undefined ||
      upload.uses !== "actions/upload-artifact@v4" || upload.with?.name !== "benchmark-50k" ||
      upload.with?.path !== "apps/desktop/benchmark-50k.json" ||
      upload.with?.["if-no-files-found"] !== "error" ||
      job.steps.indexOf(upload) <= job.steps.indexOf(run)) {
    throw new Error(`${label} must upload the 50k result after the benchmark and fail on a missing artifact`);
  }
}
