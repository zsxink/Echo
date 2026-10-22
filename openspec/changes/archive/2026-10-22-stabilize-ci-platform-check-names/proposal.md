# Proposal

## Why

PR status checks currently expose mutable runner labels such as `xcode-27` and `ubuntu-latest`, so they do not communicate which platform was validated and can change when runner images change. Use stable platform names for CI and release build checks while keeping runner labels solely for runner selection.

## What Changes

- Give Rust, Frontend, and release Build matrix checks explicit platform-semantic display names: macOS, Linux, and Windows.
- Keep runner labels as execution configuration, and align the CI Rust and Frontend macOS runner choice so a shared platform label represents the same runner environment.
- Strengthen the relevant workflow validation to check platform-to-runner mapping and preserve a reproducible failure when a required platform entry is removed.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This changes CI status-check presentation and workflow validation, not release behavior defined by an existing capability.

## Impact

- GitHub Actions status-check names in `.github/workflows/ci.yml` and `.github/workflows/release.yml` will change; consumers relying on old check names must update branch-protection rules. The repository currently has no required status checks configured.
- Workflow structure checks in `scripts/verify/checks/task-1.5.mjs` and `scripts/verify/checks/task-crp.mjs` need corresponding updates and falsifiable validation.
- No product runtime behavior, APIs, or dependencies change. No product or architecture documentation update is expected.
