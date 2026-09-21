# Native system-trash smoke evidence

- Date: 2026-09-21
- Host: macOS (current development host)
- Result: passed. A temporary Echo-shaped `echo/tmp/trash/<operation-id>/` directory containing audio and same-basename lyrics fixtures was handed to the production `TrashCrateBackend`; the call returned success and the staging directory disappeared from the library root. The Core regression `application::trash::tests::explicit_system_trash_success_persists_applied_then_finalizes` separately verifies the expired-delete state transition and relationship cleanup without a restart.
- Scope: this verifies the real macOS `trash` backend call and the production path guard without touching a user music library. The temporary fixture was created solely by the test and was moved to the OS Trash.
- Windows/Linux: not runnable on this host; the backend uses the same cross-platform `trash` crate entry point, while Windows lock retry and Linux/headless failure policies remain covered by injected adapter tests and the existing cross-platform CI/native matrix.
