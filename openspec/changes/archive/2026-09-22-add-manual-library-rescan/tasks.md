## 1. Settings action

- [x] 1.1 Add a “重新扫描” action to the settings dialog’s local-library section, wiring it to the active root through the existing `start_scan` IPC and verifying the settings component test covers the request and success feedback
- [x] 1.2 Add busy, unavailable-root, concurrent-scan, picker-conflict, and scan-failure states to the action, verifying disabled/loading/error behavior in the settings tests

## 2. Post-scan refresh

- [x] 2.1 Invalidate mounted library queries, navigation counts, and cover keys after a successful scan commit, verifying the refresh signals run only after the scan resolves
- [x] 2.2 Re-fetch lyrics for an already-mounted current song when the library invalidation fires, verifying the existing immersive-player tests continue to pass

## 3. Verification

- [x] 3.1 Run the settings and player tests, TypeScript typecheck, changed-file lint, and `git diff --check`; record the results in the implementation handoff
