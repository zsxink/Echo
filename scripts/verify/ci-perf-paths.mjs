// A PR may skip the heavy Rust scenarios only for audited, content-only edits.
import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";

const SAFE_PATHS = new Set([
  "apps/desktop/src/styles/app-extras.css",
  "apps/desktop/src/styles/artwork.css",
  "apps/desktop/src/styles/effects.css",
  "apps/desktop/src/styles/library.css",
  "apps/desktop/src/styles/notch.css",
  "apps/desktop/src/styles/player.css",
  "apps/desktop/src/styles/responsive.css",
  "apps/desktop/src/styles/shell.css",
  "apps/desktop/src/styles/tokens.css",
  "docs/PRODUCT.md",
  "docs/DESIGN.md",
  "docs/ROADMAP.md",
  "docs/interface-terminology.md",
  "docs/arch-standards-remaining.md",
  "docs/prototype/echo-desktop-player.html",
  "docs/prototype/echo-macos-notch-player.html",
]);

function shouldSkip({ GITHUB_EVENT_NAME, ECHO_PR_BASE_SHA, ECHO_PR_HEAD_SHA }) {
  if (GITHUB_EVENT_NAME !== "pull_request") return false;
  if (![ECHO_PR_BASE_SHA, ECHO_PR_HEAD_SHA].every((sha) => /^[0-9a-f]{40}$/.test(sha ?? ""))) return false;

  const git = (...args) => execFileSync("git", args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  try {
    const base = ECHO_PR_BASE_SHA;
    const head = ECHO_PR_HEAD_SHA;
    // No shared history or multiple merge bases means no safe skip.
    if (!/^[0-9a-f]{40}$/.test(git("merge-base", "--all", base, head).trim())) return false;
    const diff = git("diff", "--raw", "--no-ext-diff", "--no-renames", "--ignore-submodules=none", "-z", `${base}...${head}`, "--");
    const fields = diff.split("\0");
    if (fields.length < 3 || fields.length % 2 === 0 || fields.pop() !== "") return false;
    for (let i = 0; i < fields.length; i += 2) {
      // Restrict to unchanged normal-file mode and M alone (no add/delete/type change).
      if (!/^:100644 100644 [0-9a-f]+ [0-9a-f]+ M$/.test(fields[i]) || !SAFE_PATHS.has(fields[i + 1])) return false;
    }
    return true;
  } catch {
    return false;
  }
}

const skip = shouldSkip(process.env);
console.log(skip ? "Skipping heavy performance scenarios: audited style/docs edits only." : "Running heavy performance scenarios.");
if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `skip=${skip}\n`);
