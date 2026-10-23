# Design

## Context

See proposal.md for the reported behavior and scope. `useSmoothPosition` currently publishes a React state update on every animation frame. Both `PlayerBar` and `ImmersiveBody` consume that hook, so state interpolation can rerender broad component trees. The fill is painted by changing a pseudo-element's width while the native range thumb is painted separately. A global reduced-motion rule also changes every animation to one 1 ms iteration, which makes the infinite vinyl rotation finish at its original angle.

## Goals / Non-Goals

**Goals:**
- Confine frame-rate position rendering to small progress-specific components.
- Update the fill using a transform while preserving the native seek range and existing interaction/accessibility behavior.
- Exempt only vinyl rotation from the global reduced-motion animation override, while keeping its playback-state class as the start/stop authority.

**Non-Goals:**
- Change the desktop-to-UI snapshot cadence, player commands, seek semantics, lyrics timing, or the reduced-motion behavior of other interface transitions.
- Add diagnostic telemetry or change GPU/layer promotion policy without measurements.

## Decisions

- Keep the authoritative snapshot and its existing interpolation in `playerStore`, but move `useSmoothPosition` consumption into isolated progress subcomponents. Parent player trees continue to update from coarse snapshots and metadata, not each animation frame. This keeps the existing data flow and avoids a new store abstraction.
- Use a fixed-size fill element with `transform: scaleX(...)` and a left transform origin, setting the normalized progress on that element. Keep the native range input above it for seeking and thumb rendering. This avoids per-frame layout-affecting width changes; do not force `will-change` or a separate layer until Windows/WebView2 measurement demonstrates a need.
- In the reduced-motion stylesheet, preserve the general rule but explicitly restore the vinyl ring's 12-second infinite linear animation when its cover is playing. Playback pause removes the playing class, stopping the animation through existing state styling.
- Add or update focused component/style verification for both the persistent and immersive progress views, playback transitions, and the reduced-motion exception. Run the applicable desktop frontend checks from the task list.

Alternatives considered: moving the entire progress UI to direct DOM writes would bypass React's normal rendering contract; keeping a React update in the full player trees preserves the current lag source; animating width still incurs layout/paint work and does not reduce that fan-out.

## Risks / Trade-offs

- [Transform and range-thumb paint may still differ on a specific WebView2 build] → verify both surfaces in the Windows/WebView2 target and retain one shared normalized progress value for both visuals.
- [Always rotating can affect users who rely on reduced motion] → this is the explicit product decision for the vinyl; other motion remains reduced, and pause/stop halts the rotation.
- [Splitting position subscriptions can alter test assumptions about render timing] → cover playback, pause, resume, seek, and track replacement in focused component tests.

## Migration Plan

No persisted state, protocol, or data migration is required. Rollback consists of reverting the isolated progress rendering and vinyl-specific CSS override.
