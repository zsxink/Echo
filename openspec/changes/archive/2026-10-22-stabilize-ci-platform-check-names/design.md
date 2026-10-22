# Design

## Context

See [proposal.md](proposal.md) for motivation and [the engineering-governance delta](specs/engineering-governance/spec.md) for the observable contract. The CI Rust and Frontend jobs currently select runners through matrix values that are also interpolated into their display names; the release Build matrix already carries platform metadata but displays its runner value. The repository's workflow checks are source-level validation scripts, so their checks need to establish matrix relationships rather than find runner tokens anywhere in a file.

## Goals / Non-Goals

**Goals:**

- Keep platform identity independent of the GitHub Actions runner label for all affected check names.
- Make CI Rust and Frontend macOS jobs use the same runner environment, as their platform labels otherwise hide a material execution difference.
- Ensure structural validation checks each platform-to-runner association, with an injection proof that fails when that association is violated.

**Non-Goals:**

- Change workflow triggers, build commands, artifacts, release behavior, or platform gates.
- Rename unrelated platform gate jobs or freeze every workflow job name.
- Configure branch protection; status-check names change and the proposal notes the existing required-check set is empty.

## Decisions

1. **Represent matrix entries with separate platform and runner fields.** CI Rust and Frontend matrices will use explicit entries carrying a stable platform label and an `os` runner label; job display names interpolate only the platform field while `runs-on` uses only `os`. Release Build will reuse its existing platform metadata for its display name. This keeps runner maintenance independent from branch-protection-facing names. A parallel list or positional mapping was rejected because entries can drift out of alignment.

2. **Align both CI macOS jobs to the existing `xcode-27` runner.** The Rust job already uses this pinned runner and the release macOS target uses it to match the arm64 build environment. Keeping Frontend on `macos-latest` would make the same `(... macOS)` check label conceal different runner images. The alternative, retaining separate runners and documenting a difference, weakens the stable platform check's meaning without a requirement for that split.

3. **Validate parsed matrix structure and prove the guard is falsifiable.** Update the CI and release checks to inspect the intended matrix entries and assert platform-to-runner mappings and display-name interpolation; do not rely on repository-wide token presence. The proof should mutate an actual mapping or remove an entry in a temporary fixture based on the real workflow and demonstrate nonzero failure. Malformed YAML must not be treated as a valid matrix. This is preferred over token-only assertions, which can pass with a runner string in an unrelated job.

4. **Keep the change within engineering governance.** The externally visible behavior is the Actions check name and its runner association, so the delta belongs in `engineering-governance`; release artifact and trigger requirements remain unchanged and do not need a `ci-release-pipeline` delta.

## Risks / Trade-offs

- [Branch protection or external automation may refer to old check names] → Required checks are currently unset in this repository; identify the new names in the change notes and update any external consumers when applying.
- [A text-only matrix parser may accept malformed or reordered YAML incorrectly] → Use a YAML-aware parser already available to the repository, or a narrowly scoped parser that explicitly rejects unsupported structure; include a malformed/mismatched fixture in the falsification proof.
- [Using `xcode-27` for Frontend may change its image from the floating macOS runner] → Verify required frontend toolchain and dependencies on that runner in the workflow; keep this choice within this change's review before merging.

## Migration Plan

Update workflow display names and matrix entries together with their structural checks and injection proof. On merge, new workflow runs will publish the new check names; no data migration or rollback procedure is needed. If a downstream branch-protection consumer fails, restore the previous check naming temporarily or update that consumer to the platform-semantic names before rerunning required checks.
