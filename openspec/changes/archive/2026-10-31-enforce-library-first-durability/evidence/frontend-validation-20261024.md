# Frontend recovery copy and regression (2026-10-24)

Tasks 9.3 and 9.4 were rechecked against the current implementation. This does
not complete 9.1/9.2: full backfill progress, all optimistic favorite/playlist
flows and playback retry visibility still require implementation.

- Settings explicitly says a full backup includes `media/` and `echo/`, audio
  alone cannot restore favorites/playlists, and reinstall requires re-pairing.
- `LibraryRecoveryStatus` distinguishes saved/pending projection, unresolved
  retained evidence and unverified legacy history. An unverified or partial
  result is not described as complete recovery. Its generic read/retry failures
  do not render the backend exception; displayed targets are typed object/state
  identifiers, not SQL or credentials.
- All five generated `RecoveryIssueReasonDto` members have an explicit Chinese
  explanation enforced by `Record<RecoveryIssueReasonDto, string>`. Existing
  bridge/import tests exercise saved results, original root/operation retries
  and mixed per-item results. Recovery status tests exercise failure and reopen,
  retaining the saved fact and avoiding the complete-view message for unknown
  history. These checks cover the currently exposed types, not future additions.

Actual commands, all exit 0:

| Command | Result |
| --- | --- |
| `pnpm --filter @echo/desktop format:check` | Passed |
| `pnpm --filter @echo/desktop lint` | Passed, no error/warning output |
| `pnpm --filter @echo/desktop typecheck` | Passed |
| `pnpm --filter @echo/desktop test` | 42 files, 385 tests passed; existing React act warnings remain |
| `pnpm --filter @echo/desktop build` | Passed |
| `pnpm --filter @echo/desktop test:e2e` | Passed in Chromium with mock bridge; not a native desktop acceptance test |

Logs are `/tmp/echo-library-first-20261024-frontend-{format,lint,typecheck,test,build}.log`
and `/tmp/echo-library-first-20261024-e2e.log`. Further changes to IPC or UI require
the affected checks again; these results are not final whole-change acceptance.
