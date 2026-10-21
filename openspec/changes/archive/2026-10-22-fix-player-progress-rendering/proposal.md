# Proposal

## Why

Issue #54 reports a visible lag between the seek thumb and progress fill during playback on Windows, plus a stationary vinyl record in immersive mode. The progress UI currently receives frame-rate position updates through the same React components that render the full player, while a global reduced-motion rule turns the infinite vinyl animation into a one-shot 1 ms animation.

## What Changes

- Keep frame-rate playback position updates from rerendering the full player contents.
- Render the progress fill with a compositor-friendly transform so the fill and seek thumb remain synchronized while playing, pausing, resuming, or seeking.
- Keep vinyl rotation active during playback regardless of the operating system reduced-motion setting, as requested for Issue #54; other reduced-motion behavior remains in effect.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `desktop-playback`: Specify that the visible progress fill and seek thumb stay synchronized through playback and seeking.
- `immersive-lyrics`: Specify continuous vinyl rotation during playback regardless of reduced-motion preference, while lyrics and page transitions continue to respect that preference.

## Impact

- Affected modules: desktop React player state and presentation components, and desktop player styles.
- No command, IPC, persisted data, Core, or dependency changes are expected.
- Product and interface baseline: immersive vinyl playback and the persistent playback bar described in `docs/PRODUCT.md`, `docs/ROADMAP.md`, and `openspec/specs/immersive-lyrics/spec.md`.
- Implementation must follow the React/TypeScript and testing standards in `openspec/CODE_STANDARDS.md`.
