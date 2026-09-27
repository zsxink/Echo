/**
 * 菜单触发控件的解析（fix-queue-trigger-toggle）。
 *
 * 歌曲列表是窗口化的：滚出可视区再滚回来的行是**重新构造**的 `<tr>`，它的
 * `.song-more` 按钮同样是新节点。所以"哪个入口打开了菜单"不能靠 DOM 节点身份
 * 表达 —— 那样第一次按压记下的元素在第二次按压时已经不在文档里，菜单就再也不会
 * 被 toggle 关闭，正是 issue #33 的症状。
 *
 * 这里改为按**稳定 id**记录归属、按需解析活节点。`.song-more` 按钮和 `<tr>` 都
 * 带 `data-song-id`（`SongRow` 渲染），所以两者都能在按下时从当前文档里找回来：
 *  - `.song-more` 的 click 打开时归属**按钮** —— 于是再次按同一个入口关闭，
 *    而按下行体的其余部分照旧收起菜单；
 *  - 行的 contextmenu 打开时归属**行** —— 右键手势的归属是一整行。
 */

import { useMemo, useRef, type RefObject } from "react";

export interface MenuTriggerBinding {
  /** 喂给 `SongMenu` / `BatchSongMenu` 的 `triggerRef`。 */
  readonly triggerRef: RefObject<HTMLElement | null>;
  /**
   * 当前归属的是否是 `.song-more` 控件（而非整行）。宿主用它把"按同一个入口
   * 再来一次"和"换一个入口"区分开：前者关闭菜单，后者只是重新定位。
   */
  readonly isControl: boolean;
  /**
   * 记录一次触发手势。`control` 指明按下的具体是哪个入口：`.song-more` 的
   * click 传 true，行的 contextmenu 传 false。
   */
  readonly record: (songId: string, control: boolean) => void;
  /** 菜单关闭时清空归属。 */
  readonly clear: () => void;
}

/**
 * 一个按 song.id 记住归属、按需解析活节点的记忆化 ref。
 *
 * 返回的 ref **每次读取**都去当前文档里解析，所以 `useOverlay` 与
 * `isInsideOverlay` 拿到的永远是屏幕上真实存在的那个元素；窗口化回收、重建对
 * 它透明。ref 对象本身被 `useMemo` 固定 —— `useOverlay` 的 effect 依赖它，
 * 每次渲染换新对象会让浮层反复注销/注册并重跑初始聚焦。
 */
export function useMenuTrigger(): MenuTriggerBinding {
  const songIdRef = useRef<string | null>(null);
  const controlRef = useRef(false);
  return useMemo<MenuTriggerBinding>(
    () => ({
      // Getters, not values: this object is memoized once, so anything read at
      // build time would freeze at the first render's state.
      get isControl() {
        return controlRef.current;
      },
      triggerRef: {
        get current() {
          const id = songIdRef.current;
          if (id === null) return null;
          for (const row of document.querySelectorAll<HTMLElement>("[data-song-id]")) {
            if (row.dataset.songId !== id) continue;
            if (!controlRef.current) return row;
            // 记录的是按钮却找不到它（行尚未挂载、或已被窗口化回收）：宁可返回
            // null 让这次按压按"菜单外部"处理，也不要指向别的 song 的控件。
            return row.querySelector<HTMLElement>("button.song-more");
          }
          return null;
        },
      },
      record: (songId, control) => {
        songIdRef.current = songId;
        controlRef.current = control;
      },
      clear: () => {
        songIdRef.current = null;
        controlRef.current = false;
      },
    }),
    [],
  );
}
