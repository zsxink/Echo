/**
 * 播放模式: the single source of the label, the cycle and the glyph.
 *
 * Extracted from `PlayerBar` when the notch player became a second surface that
 * shows the same control: both must agree on the order and the icons, and a
 * duplicated table is exactly how the two would drift apart.
 *
 * 顺序播放的语义是列表循环（播完回到第一条继续，默认模式）——a library is meant
 * to keep playing, not to stop after its last song.
 *
 * The mode control never uses the theme accent on any surface (`desktop-playback`
 * requires it for the player bar; the notch player follows the same rule), so
 * this module carries no colour.
 */

export type PlaybackMode = "sequential" | "shuffle" | "repeatOne";

interface ModeDefinition {
  readonly label: string;
  readonly next: PlaybackMode;
  readonly icon: string;
}

export const PLAYBACK_MODES: Record<PlaybackMode, ModeDefinition> = {
  sequential: { label: "列表循环", next: "shuffle", icon: "modeSequence" },
  shuffle: { label: "随机播放", next: "repeatOne", icon: "modeShuffle" },
  repeatOne: { label: "单曲循环", next: "sequential", icon: "modeRepeatOne" },
};

/** Resolve a snapshot mode string, falling back to the default cycle entry. */
export function modeFor(mode: string): ModeDefinition {
  return PLAYBACK_MODES[mode as PlaybackMode] ?? PLAYBACK_MODES.sequential;
}
