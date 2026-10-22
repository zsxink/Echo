# Tasks

## 1. Stable Workflow Check Names

- [x] 1.1 Update Rust and Frontend CI matrices to pair each stable platform name with its runner label, use only the platform field in job display names, and align both macOS jobs to `xcode-27`; verify the workflow YAML parses and its matrices contain all three platforms with the intended runner mappings.
- [x] 1.2 Update the release Build display name to use the existing platform matrix field; verify the macOS, Windows, and Linux entries retain their current runner, architecture, target, and expected artifact configuration.

## 2. Workflow Guard Validation

- [x] 2.1 Update `task-1.5.mjs` and `task-crp.mjs` to validate the relevant matrix entries, platform-to-runner associations, and platform-based display names; run `pnpm verify:task -- 1.5` and `node scripts/verify/checks/task-crp.mjs`.
- [x] 2.2 Add or update injection proofs that remove or corrupt a real platform mapping in each guarded workflow and assert the corresponding check exits nonzero; run the focused proof suite and `pnpm verify:governance`.

## 3. Integration Verification

- [x] 3.1 Verify the complete affected workflow and governance checks together with `pnpm verify:task -- 1.5`, `node scripts/verify/checks/task-crp.mjs`, and `pnpm verify:governance`; inspect generated job names or a representative Actions run to confirm no affected display name contains runner labels.
