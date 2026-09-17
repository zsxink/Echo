/** Stable placeholder tint for coverless songs and playlists. */
const PALETTE = ["", "cover-b", "cover-c", "cover-d", "cover-e"] as const;

/** A stable A–E cover tint class for a song or playlist id. */
export function coverClass(seed: string): string {
  let hash = 0;
  for (let i = 0; i < seed.length; i += 1) {
    hash = (hash * 31 + seed.charCodeAt(i)) % 1_000_003;
  }
  return PALETTE[hash % PALETTE.length];
}
