# Tasks

## 1. Isolate progress updates

- [x] 1.1 Move interpolated position subscriptions into small progress-specific components for the persistent and immersive player surfaces; verify a focused frame-update test leaves sibling player content at one render.
- [x] 1.2 Change the progress fill to a fixed element updated with `scaleX` while preserving native range seeking and accessible labels; verify progress, paused, resumed, and seek behavior in the desktop frontend suite.

## 2. Restore vinyl playback motion

- [x] 2.1 Override the global reduced-motion animation shortening only for the playing vinyl ring; verify the generated reduced-motion CSS restores the 12-second infinite animation only while the playing class is present.

## 3. Integration verification

- [x] 3.1 Run the desktop frontend test suite and applicable lint/type/build checks; record that Windows/WebView2 runtime verification is unavailable on this macOS host.
