// Shared contract for the actual timed release carrier, not default CI delegation.
export const RELEASE_LATENCY_WORKFLOW = "release-edit-latency.yml";
export const RELEASE_LATENCY_SCENARIOS = ["LE-R21-S02", "PM-R13-S05"];
export const RELEASE_LATENCY_FLAG = "--defer-release-edit-latency";
export const RELEASE_LATENCY_ENTRY = "scripts/verify/run-release-edit-latency.mjs";
export const RELEASE_LATENCY_RUN = `node ${RELEASE_LATENCY_ENTRY}`;
export const RELEASE_LATENCY_REPORT = "artifacts/release-edit-latency.json";

function strings(value) {
  if (typeof value === "string") return [value];
  if (value && typeof value === "object") return Object.values(value).flatMap(strings);
  return [];
}

export function assertNoDefaultReleaseLatency(workflow) {
  for (const [name, job] of Object.entries(workflow.jobs ?? {})) {
    if (strings(job).some((s) => /verify:perf\b|run-release-edit-latency\.mjs|--test\s+local_edit_performance|completed_local_edit_latency/.test(s))) {
      throw new Error(`default CI job '${name}' must not run Release edit latency`);
    }
  }
}

export function assertReleaseLatencyCarrier(workflow) {
  const label = RELEASE_LATENCY_WORKFLOW;
  if (Object.keys(workflow.on ?? {}).sort().join(",") !== "schedule,workflow_dispatch" ||
      !workflow.on.schedule?.length) throw new Error(`${label} must trigger on schedule + workflow_dispatch only`);
  const job = workflow.jobs?.latency;
  if (Object.keys(workflow.jobs ?? {}).length !== 1 || job?.["runs-on"] !== "ubuntu-latest" ||
      job.if !== undefined || job.needs !== undefined || job["continue-on-error"] !== undefined ||
      !Array.isArray(job.steps)) throw new Error(`${label} must have one unconditional Ubuntu latency job`);
  if (job.steps.some((step) => /(?:actions\/cache|rust-cache)@/.test(step.uses || ""))) {
    throw new Error(`${label} must not use or write a shared Rust/project cache`);
  }
  const runs = job.steps.filter((step) => step.name === "Measure release edit latency");
  const uploads = job.steps.filter((step) => step.name === "Upload latency evidence");
  const [run] = runs;
  const [upload] = uploads;
  if (runs.length !== 1 || run.run !== RELEASE_LATENCY_RUN || run.if !== undefined ||
      run.env !== undefined || run["continue-on-error"] !== undefined) {
    throw new Error(`${label} must run '${RELEASE_LATENCY_RUN}' unconditionally`);
  }
  if (uploads.length !== 1 || upload.if !== "always()" || upload.uses !== "actions/upload-artifact@v4" ||
      upload.with?.name !== "release-edit-latency" || upload.with?.path !== RELEASE_LATENCY_REPORT ||
      upload.with?.["if-no-files-found"] !== "error" || upload["continue-on-error"] !== undefined ||
      job.steps.indexOf(upload) <= job.steps.indexOf(run)) {
    throw new Error(`${label} must upload phase evidence even when latency fails`);
  }
}
