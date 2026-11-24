// The package allowlist is intentionally smaller than the performance allowlist:
// Vite styles and generated prototypes can be bytes embedded in Echo.app.
import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";

// These JS tests run only in governance: none is read by Tauri, the
// macOS build/inspect tasks, Vite, or release verification. The historical
// docs directories are prose-only and are not packaging inputs.
const SAFE_FILES = new Set([
  "docs/verification/interactive-edit-latency-2026-10-28.md",
  "scripts/verify/checks/task-1.7.test.mjs",
  "scripts/verify/isolate-linux-rustup-home.test.mjs",
]);
const HISTORICAL_DOC = /^docs\/superpowers\/(?:plans|specs)\/\d{4}-\d{2}-\d{2}-[a-z0-9-]+\.md$/;
const auditedPath = (path) => SAFE_FILES.has(path) || HISTORICAL_DOC.test(path);

function shouldSkip({ GITHUB_EVENT_NAME, ECHO_PR_BASE_SHA: base, ECHO_PR_HEAD_SHA: head }) {
  if (GITHUB_EVENT_NAME !== "pull_request" ||
      ![base, head].every((sha) => /^[0-9a-f]{40}$/.test(sha ?? ""))) return false;
  const git = (...args) => execFileSync("git", args, {
    encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
  });
  try {
    if (!/^[0-9a-f]{40}$/.test(git("merge-base", "--all", base, head).trim())) return false;
    const fields = git("diff", "--raw", "--no-ext-diff", "--no-renames",
      "--ignore-submodules=none", "-z", `${base}...${head}`, "--").split("\0");
    if (fields.length < 3 || fields.length % 2 === 0 || fields.pop() !== "") return false;
    for (let i = 0; i < fields.length; i += 2) {
      if (!/^:100644 100644 [0-9a-f]+ [0-9a-f]+ M$/.test(fields[i]) ||
          !auditedPath(fields[i + 1])) return false;
    }
    return true;
  } catch {
    return false;
  }
}

const skip = shouldSkip(process.env);
console.log(skip
  ? "macOS packaging classification: all changed paths are audited, non-package inputs."
  : "macOS packaging classification: full build required.");
if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `skip=${skip}\n`);
