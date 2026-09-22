import { useEffect, useState } from "react";

import { bridge } from "../../bridge";
import { subscribeLibraryInvalidations } from "../library";

/** One timed lyrics line (mirrors the IPC `LyricsLineView`). */
export interface LyricsLine {
  readonly seconds: number;
  readonly text: string;
}

/** The effective lyrics shape (mirrors the IPC `SongLyricsDto`). */
export interface SongLyrics {
  readonly source: "override" | "embedded" | "sidecar" | null;
  readonly timed: boolean;
  readonly lines: readonly LyricsLine[];
  readonly plainText: string;
  readonly parseError?: string | null;
}

/**
 * Load the effective lyrics for a song id, for the immersive surface. This is
 * the data-access hook that the body used to inline; it owns the
 * `get_lyrics` IPC call and the cancelled-response guard so the view stays free
 * of fetch logic (CODE_STANDARDS §6).
 */
export function useLyrics(songId: string | null): SongLyrics | null {
  const [lyrics, setLyrics] = useState<SongLyrics | null>(null);
  const [libraryRevision, setLibraryRevision] = useState(0);

  useEffect(() => {
    return subscribeLibraryInvalidations(() => {
      setLibraryRevision((revision) => revision + 1);
    });
  }, []);

  useEffect(() => {
    let cancelled = false;
    setLyrics(null);
    if (songId) {
      bridge
        .call("get_lyrics", { songId })
        .then((dto) => {
          if (cancelled) return;
          setLyrics(dto);
        })
        .catch(() => {
          // Lyrics failure degrades to the no-lyrics state.
        });
    }
    return () => {
      cancelled = true;
    };
  }, [songId, libraryRevision]);
  return lyrics;
}
