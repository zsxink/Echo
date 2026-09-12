/**
 * Global playback keyboard shortcuts (task 11.8).
 *
 * Binds an application-wide `keydown` handler so the media/transport controls
 * (play/pause, previous, next, volume/mute, seek) work from anywhere in the
 * shell — equivalent to the on-screen buttons. Two guards keep it safe:
 *
 *  - **Input guard**: when focus is inside an `input`, `textarea`, `select`,
 *    `contenteditable`, or a Tauri WebView search field, the handler does not
 *    consume the event — so typing `Space`/arrows into the search box or a
 *    dialog does NOT toggle playback (spec: 不得因当前焦点位于歌曲列表或输入框
 *    而产生歧义).
 *  - **Modifier guard**: nothing fires when Ctrl/Cmd/Alt/Shift are held, so
 *    native/browser shortcuts are never shadowed.
 *
 * The handler only sends coarse bridge commands; the Rust coordinator owns the
 * queue + snapshot authority, so a rejected command simply leaves the snapshot
 * unchanged. Media keys (PlayPause/Next/Previous/etc.) are handled by the
 * platform media-control adapter; this hook covers the in-app keys.
 */

import { useEffect } from "react";

import { bridge } from "../bridge";
import { playerStore } from "./playerStore";

/** Keys that effectively behave like typed text; hotkeys must ignore them. */
function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select";
}

/** Whether a dialog-like overlay (queue panel) is open that owns Space. */
function hasQueueDialogOpen(): boolean {
  return playerStore.getUi().queueOpen;
}

/** Whether any overlay owns the arrow/letter navigation keys. */
function hasOverlayOpen(): boolean {
  const ui = playerStore.getUi();
  return ui.queueOpen || ui.immersiveOpen || ui.focusOpen;
}

export function useGlobalPlayerHotkeys(): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // Never short-circuit native shortcuts.
      if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
      // Typing in an editor or inside an overlay's focused input must not
      // trigger playback (task 11.8: Space in inputs/dialogs must not play).
      if (isEditableTarget(event.target)) return;

      // Space toggles play/pause — but only when a song is current, never in an
      // editable surface, and never while a dialog-like overlay (the queue
      // panel) is open where Space would activate a focused button. It *does*
      // keep working inside the immersive player (the primary playback surface).
      if (event.code === "Space") {
        if (hasQueueDialogOpen()) return;
        const snap = playerStore.getSnapshot();
        if (snap.currentQueueEntryId === null) return;
        event.preventDefault();
        void bridge.call("player_control", { action: "toggle" });
        return;
      }

      // When any overlay (queue/immersive/focus) is open, it owns the arrow and
      // letter keys for its own navigation — the global hotkeys stand down so
      // they never fire behind an open surface.
      if (hasOverlayOpen()) return;

      switch (event.key) {
        case "ArrowLeft":
          seekRelative(-5);
          break;
        case "ArrowRight":
          seekRelative(5);
          break;
        case "ArrowUp":
          volumeRelative(0.05);
          break;
        case "ArrowDown":
          volumeRelative(-0.05);
          break;
        case "m":
        case "M":
          void bridge.call("toggle_mute");
          break;
        case ".":
          void bridge.call("player_control", { action: "next" });
          break;
        case ",":
          void bridge.call("player_control", { action: "previous" });
          break;
        default:
          return;
      }
      event.preventDefault();
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  /** Seek by a relative delta from the authoritative snapshot position. */
  function seekRelative(delta: number): void {
    const snap = playerStore.getSnapshot();
    if (snap.currentQueueEntryId === null || snap.position === null) return;
    const target = Math.max(0, snap.position + delta);
    void bridge.call("seek", { position: target });
  }

  /** Step volume by a relative delta, clamped to [0, 1]. */
  function volumeRelative(delta: number): void {
    const snap = playerStore.getSnapshot();
    const next = Math.min(1, Math.max(0, snap.volume + delta));
    void bridge.call("set_volume", { volume: next });
  }
}
