/**
 * 定位当前播放歌曲 (playlist-search-locate-import 4.3).
 *
 * 资料库三视图（全部歌曲/最近添加/喜欢的音乐、歌单详情、歌手/专辑聚合详情）
 * 共享同一条定位交互：读取播放快照里的当前歌曲，命中则向 `SongList` 下发一次
 * 定位意图（数值滚动由列表内部完成），未播放或当前视图不含该歌曲时给出 Toast
 * 提示。`onLocateSettled("absent")` 由 `SongList` 在“列表已全量加载但仍找不到
 * 目标”时回调，本 hook 负责清空意图并提示——So Toast 文案只在视图层出现。
 */

import { useCallback, useState } from "react";

import { usePlayerSnapshot } from "../../player/playerStore";
import { Icon } from "../../app/Icon";
import { notify } from "../../app/toast";

export interface LocateSongController {
  /** The target SongId for `SongList`'s `locateSongId`, or null when idle. */
  readonly locateSongId: string | null;
  /** The button handler: voices a lack of current playback, else dispatches
   *  the locate intent to the list. */
  readonly onLocate: () => void;
  /** `SongList` settle callback — clear the intent; voice the miss. */
  readonly onLocateSettled: (result: "found" | "absent") => void;
}

export function useLocateSong(): LocateSongController {
  const snapshot = usePlayerSnapshot();
  const [locateSongId, setLocateSongId] = useState<string | null>(null);

  const onLocate = useCallback(() => {
    if (snapshot.currentSongId) {
      setLocateSongId(snapshot.currentSongId);
    } else {
      setLocateSongId(null);
      notify("当前没有正在播放的歌曲");
    }
  }, [snapshot.currentSongId]);

  const onLocateSettled = useCallback((result: "found" | "absent") => {
    // A settled intent is done: clearance stops a later re-run (e.g. a
    // pagination-triggered `songs` change) from re-scrolling or re-toasting.
    setLocateSongId(null);
    if (result === "absent") notify("当前歌曲不在此列表中");
  }, []);

  return { locateSongId, onLocate, onLocateSettled };
}

/** The `.library-tools` locate control (prototype `.tool-button.tool-icon`
 *  anatomy, like the sort control). */
export function LocateButton({ onClick }: { readonly onClick: () => void }) {
  return (
    <button
      type="button"
      className="tool-button tool-icon"
      aria-label="定位当前播放歌曲"
      title="定位当前播放歌曲"
      onClick={onClick}
      data-testid="locate-song"
    >
      <Icon name="locate" />
    </button>
  );
}
