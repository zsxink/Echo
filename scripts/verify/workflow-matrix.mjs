import yaml from "js-yaml";

export function parseWorkflow(source, label) {
  try {
    const workflow = yaml.load(source);
    if (!workflow || typeof workflow !== "object" || Array.isArray(workflow)) {
      throw new Error("top-level document is not a mapping");
    }
    return workflow;
  } catch (error) {
    throw new Error(`${label} is not valid YAML: ${error.message}`);
  }
}

export function matrixEntries(workflow, jobId, label) {
  const job = workflow?.jobs?.[jobId];
  const matrix = job?.strategy?.matrix;
  const entries = matrix?.include;
  if (job?.["runs-on"] !== "${{ matrix.os }}") {
    throw new Error(`${label} job '${jobId}' runs-on must use matrix.os`);
  }
  if (!matrix || Object.keys(matrix).some((key) => key !== "include")) {
    throw new Error(`${label} job '${jobId}' must use an include-only matrix`);
  }
  if (!Array.isArray(entries)) {
    throw new Error(`${label} job '${jobId}' must define strategy.matrix.include`);
  }
  return entries;
}

export function assertPlatformMatrix(entries, expected, label) {
  if (new Set(entries.map((entry) => entry.platform)).size !== entries.length) {
    throw new Error(`${label} matrix contains duplicate platform entries`);
  }
  const actual = new Map(entries.map((entry) => [entry.platform, entry]));
  for (const [platform, mapping] of Object.entries(expected)) {
    const entry = actual.get(platform);
    if (!entry) throw new Error(`${label} matrix is missing platform '${platform}'`);
    for (const [field, value] of Object.entries(mapping)) {
      if (entry[field] !== value) {
        throw new Error(`${label} platform '${platform}' maps ${field} to '${entry[field]}', expected '${value}'`);
      }
    }
  }
  if (actual.size !== Object.keys(expected).length) {
    throw new Error(`${label} matrix contains unexpected platform entries`);
  }
}

export function assertShardMatrix(entries, expected, label) {
  const idOf = (entry) => `${entry.platform}/${entry.shard}`;
  const actual = new Map(entries.map((entry) => [idOf(entry), entry]));
  if (actual.size !== entries.length) {
    throw new Error(`${label} matrix contains duplicate platform/shard entries`);
  }
  for (const [id, mapping] of Object.entries(expected)) {
    const entry = actual.get(id);
    if (!entry) throw new Error(`${label} matrix is missing entry '${id}'`);
    for (const [field, value] of Object.entries(mapping)) {
      if (entry[field] !== value) {
        throw new Error(`${label} matrix entry '${id}' maps ${field} to '${entry[field]}', expected '${value}'`);
      }
    }
    const extra = Object.keys(entry).filter((field) => field !== "platform" && field !== "shard" && !(field in mapping));
    if (extra.length) throw new Error(`${label} matrix entry '${id}' has unexpected field(s): ${extra.join(", ")}`);
  }
  if (actual.size !== Object.keys(expected).length) {
    const unexpected = [...actual.keys()].filter((id) => !(id in expected));
    throw new Error(`${label} matrix contains unexpected entries: ${unexpected.join(", ")}`);
  }
}

export function assertPlatformName(name, expected, label) {
  if (name !== expected) {
    throw new Error(`${label} must use '${expected}', got '${name}'`);
  }
}
