/**
 * Live song detail for the playing surfaces (当前播放区 / 沉浸式播放器).
 *
 * The `PlayerSnapshot` carries identity and playback state only — title for a
 * temporary item, nothing for a library song. The 当前播放区 also has to show the
 * artist and a working 收藏 control (`docs/interface-terminology.md`), and the
 * immersive player needs album/cover, so both read `song_detail` for the current
 * SongId.
 *
 * Deliberately **no process-wide cache**: a favourite toggle, a re-scan or an
 * edit made anywhere else must be visible the next time a surface mounts, and a
 * cached detail is exactly how a stale heart would survive. One request per
 * mounted surface per track is cheap and always authoritative.
 */

import { useEffect, useState } from "react";

import { bridge } from "../../bridge";
import { subscribeSongUpdates } from "../library";

/** Mirrors the IPC `SongDetailView` (`crates/echo-desktop/src/ipc/dto.rs`). */
export interface SongDetail {
  readonly songId: string;
  readonly relativePath: string;
  readonly title?: string | null;
  readonly artist?: string | null;
  readonly album?: string | null;
  readonly durationS?: number | null;
  readonly format?: string | null;
  readonly playCount: number;
  readonly favorite: boolean;
  readonly hasCover: boolean;
  readonly availability: string;
}

export function useSongDetail(songId: string | null): SongDetail | null {
  const [detail, setDetail] = useState<SongDetail | null>(null);

  useEffect(() => {
    let cancelled = false;
    setDetail(null);
    if (!songId) return;
    bridge
      .call("song_detail", { songId })
      .then((dto) => {
        if (!cancelled) setDetail(dto);
      })
      .catch(() => {
        // A failed detail read degrades to "unknown", never to a fabricated one.
      });
    return () => {
      cancelled = true;
    };
  }, [songId]);

  // Adopt committed mutations broadcast by any surface (the row's heart, the
  // player bar's own 收藏): without this a toggle elsewhere would leave this
  // detail stale until the track changes — exactly the stale-heart bug. The
  // update carries the committed `favorite`, so this is adoption, not guessing.
  useEffect(() => {
    return subscribeSongUpdates((song) => {
      if (song.id !== songId) return;
      setDetail((prev) =>
        prev && prev.favorite !== song.favorite ? { ...prev, favorite: song.favorite } : prev,
      );
    });
  }, [songId]);

  return detail;
}
