import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

import { CollectionDirectory } from "./CollectionDirectory";
import { invalidateLibrary } from "./libraryInvalidation";
import { playerStore } from "../../player/playerStore";
import { ToastView } from "../../app/ToastView";

/** The last `catalog_collections` request the directory issued. The fixed
 * return-value mock cannot emulate filtering, so “未被旧词过滤” is asserted on
 * the query itself. */
const lastCollectionQuery = () => {
  const calls = vi
    .mocked(invoke)
    .mock.calls.filter(([command]) => command === "catalog_collections");
  return calls.at(-1)?.[1] as { kind: string; search: string };
};

function mockCommand(command: string, value: unknown) {
  // The shared jsdom setup exposes a small Tauri-command test hook.
  // @ts-expect-error -- test-only global declared by src/test/setup.ts
  globalThis.__echoTest.setInvoke(command, value);
}

describe("CollectionDirectory", () => {
  it("sorts each directory independently and restores its persisted preference", async () => {
    localStorage.removeItem("echo-directory-sort:artist");
    localStorage.removeItem("echo-directory-sort:album");
    mockCommand("catalog_collections", [
      {
        kind: "artist",
        artistKey: "beta",
        artist: "Beta",
        name: "Beta",
        songCount: 5,
      },
      {
        kind: "artist",
        artistKey: "alpha",
        artist: "Alpha",
        name: "Alpha",
        songCount: 1,
      },
    ]);

    const { rerender, unmount } = render(<CollectionDirectory kind="artist" root="root-1" />);
    const artistCards = () =>
      screen.getAllByRole("button", { name: /打开歌手/ }).map((button) => button.textContent);
    await screen.findByRole("button", { name: /打开歌手 Alpha/ });
    expect(artistCards()[0]).toContain("Alpha");

    fireEvent.click(screen.getByTestId("sort-button"));
    expect(screen.getByRole("menuitemradio", { name: "歌手名称" })).toBeInTheDocument();
    expect(screen.getByRole("menuitemradio", { name: "歌曲数量" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitemradio", { name: "歌曲数量" }));
    fireEvent.click(screen.getByTestId("sort-button"));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "降序" }));
    expect(artistCards()[0]).toContain("Beta");
    expect(JSON.parse(localStorage.getItem("echo-directory-sort:artist") ?? "null")).toEqual({
      field: "songCount",
      direction: "desc",
    });

    mockCommand("catalog_collections", [
      { kind: "album", artistKey: "a", albumKey: "z", artist: "A", name: "Zulu", songCount: 1 },
      { kind: "album", artistKey: "a", albumKey: "a", artist: "A", name: "Alpha", songCount: 5 },
    ]);
    rerender(<CollectionDirectory kind="album" root="root-1" />);
    const albumCards = () =>
      screen.getAllByRole("button", { name: /打开专辑/ }).map((button) => button.textContent);
    await screen.findByRole("button", { name: /打开专辑 Alpha/ });
    expect(albumCards()[0]).toContain("Alpha");
    fireEvent.click(screen.getByTestId("sort-button"));
    expect(screen.getByRole("menuitemradio", { name: "专辑名称" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitemradio", { name: "专辑名称" }));
    fireEvent.click(screen.getByTestId("sort-button"));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "降序" }));
    expect(albumCards()[0]).toContain("Zulu");

    mockCommand("catalog_collections", [
      {
        kind: "artist",
        artistKey: "beta",
        artist: "Beta",
        name: "Beta",
        songCount: 5,
      },
      {
        kind: "artist",
        artistKey: "alpha",
        artist: "Alpha",
        name: "Alpha",
        songCount: 1,
      },
    ]);
    rerender(<CollectionDirectory kind="artist" root="root-1" />);
    await screen.findByRole("button", { name: /打开歌手 Beta/ });
    expect(artistCards()[0]).toContain("Beta");
    unmount();
  });

  it("shows artist cards and reuses the library song table with selection in the detail", async () => {
    mockCommand("catalog_collections", [
      {
        kind: "artist",
        artistKey: "alice",
        artist: "Alice",
        name: "Alice",
        songCount: 2,
        coverKey: "cv1-alice",
        hasCustomCover: true,
      },
    ]);
    mockCommand("catalog_collection_songs", [
      {
        id: "song-1",
        title: "First song",
        artist: "Alice",
        album: "First album",
        durationS: 211,
        favorite: false,
        playCount: 0,
        availability: "available",
        relativePath: "first.flac",
      },
    ]);

    render(<CollectionDirectory kind="artist" root="root-1" />);

    const card = await screen.findByRole("button", { name: /打开.*Alice/ });
    expect(card.querySelector("img")?.getAttribute("src")).toBe("cover://cv1-alice?s=list");
    expect(screen.queryByRole("button", { name: "编辑Alice的封面" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "恢复专辑封面" })).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.queryByText("全选")).not.toBeInTheDocument();

    fireEvent.click(card);
    await screen.findByText("First song");
    // The opened artist reuses the 全部歌曲 table (SongList), not a bespoke list.
    const songList = screen.getByTestId("song-list");
    expect(songList).toHaveClass("table-wrap");
    // Header cells match the colgroup columns one-to-one (序号/歌曲/专辑/时长/操作)
    // — a missing `<th>` shifts every header one column left under
    // `table-layout: fixed`.
    expect(screen.getAllByRole("columnheader")).toHaveLength(5);
    expect(screen.getByRole("columnheader", { name: "歌曲" })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "专辑" })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "时长" })).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId("selection-mode-button"));
    const selectAll = await screen.findByRole("button", { name: "全选当前已加载歌曲" });
    fireEvent.click(selectAll);
    expect(screen.getByRole("button", { name: "取消全选当前已加载歌曲" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "取消选择First song" })).toBeInTheDocument();

    // The row's ⋯ control opens the same 歌曲操作菜单 as 全部歌曲, including
    // 添加到歌单.
    fireEvent.click(screen.getByRole("button", { name: "歌曲操作" }));
    const menu = await screen.findByTestId("song-menu");
    expect(menu).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /添加到歌单/ })).toBeEnabled();
  });

  it("does not keep the artist directory on screen when switching to 专辑", async () => {
    mockCommand("catalog_collections", [
      {
        kind: "artist",
        artistKey: "alice",
        artist: "Alice",
        name: "Alice",
        songCount: 2,
        coverKey: "cv1-alice",
        hasCustomCover: true,
      },
    ]);
    const { rerender } = render(<CollectionDirectory kind="artist" root="root-1" />);
    await screen.findByRole("button", { name: /打开.*Alice/ });

    mockCommand("catalog_collections", [
      {
        kind: "album",
        artistKey: "alice",
        albumKey: "first album",
        artist: "Alice",
        name: "First album",
        songCount: 2,
        coverKey: "cv1-album",
        hasCustomCover: false,
      },
    ]);
    rerender(<CollectionDirectory kind="album" root="root-1" />);

    // Synchronously after the switch the artist cards must already be gone —
    // the refetch lands later, and a failed refetch must never resurrect the
    // previous view's directory under 专辑 chrome.
    expect(screen.queryByRole("button", { name: /打开.*Alice/ })).not.toBeInTheDocument();
    expect(screen.getByPlaceholderText("搜索专辑或歌手")).toBeInTheDocument();
    await screen.findByRole("button", { name: /打开.*First album/ });
  });

  it("clears the search when switching between artist and album directories", async () => {
    mockCommand("catalog_collections", [
      {
        kind: "artist",
        artistKey: "alice",
        artist: "Alice",
        name: "Alice",
        songCount: 2,
      },
      {
        kind: "artist",
        artistKey: "beta",
        artist: "Beta",
        name: "Beta",
        songCount: 1,
      },
    ]);
    const { rerender } = render(<CollectionDirectory kind="artist" root="root-1" />);
    await screen.findByRole("button", { name: /打开歌手 Alice/ });

    fireEvent.change(screen.getByPlaceholderText("搜索歌手"), { target: { value: "Alice" } });
    await waitFor(() =>
      expect(lastCollectionQuery()).toMatchObject({ kind: "artist", search: "Alice" }),
    );

    mockCommand("catalog_collections", [
      {
        kind: "album",
        artistKey: "alice",
        albumKey: "first album",
        artist: "Alice",
        name: "First album",
        songCount: 2,
      },
      {
        kind: "album",
        artistKey: "beta",
        albumKey: "beta album",
        artist: "Beta",
        name: "Beta album",
        songCount: 1,
      },
    ]);
    rerender(<CollectionDirectory kind="album" root="root-1" />);

    // The term belongs to the directory being left: the album directory opens
    // with an empty box, and its query carries no leftover artist term (the
    // transient “album + Alice” request of the switch is superseded).
    await waitFor(() => expect(screen.getByPlaceholderText("搜索专辑或歌手")).toHaveValue(""));
    await waitFor(() => expect(lastCollectionQuery()).toMatchObject({ kind: "album", search: "" }));
    await screen.findByRole("button", { name: /打开专辑 Beta album/ });
    expect(screen.getByRole("button", { name: /打开专辑 First album/ })).toBeInTheDocument();
  });

  it("loads the album directory when the switch happens from inside an open artist", async () => {
    mockCommand("catalog_collections", [
      {
        kind: "artist",
        artistKey: "alice",
        artist: "Alice",
        name: "Alice",
        songCount: 2,
        coverKey: null,
        hasCustomCover: false,
      },
    ]);
    mockCommand("catalog_collection_songs", [
      {
        id: "song-1",
        title: "First song",
        artist: "Alice",
        album: "First album",
        durationS: 211,
        favorite: false,
        playCount: 0,
        availability: "available",
        relativePath: "first.flac",
      },
    ]);
    const { rerender } = render(<CollectionDirectory kind="artist" root="root-1" />);
    fireEvent.click(await screen.findByRole("button", { name: /打开歌手 Alice/ }));
    await screen.findByText("First song");

    mockCommand("catalog_collections", [
      {
        kind: "album",
        artistKey: "alice",
        albumKey: "first album",
        artist: "Alice",
        name: "First album",
        songCount: 2,
        coverKey: null,
        hasCustomCover: false,
      },
    ]);
    rerender(<CollectionDirectory kind="album" root="root-1" />);

    // The open artist must not swallow the album directory's refetch: the two
    // queries (directory vs. detail songs) are independent, so a late detail
    // response may never invalidate the directory request that replaced it.
    await screen.findByRole("button", { name: /打开专辑 First album/ });
  });

  it("shows the error over an empty grid when the album directory fails", async () => {
    mockCommand("catalog_collections", [
      {
        kind: "artist",
        artistKey: "alice",
        artist: "Alice",
        name: "Alice",
        songCount: 2,
        coverKey: null,
        hasCustomCover: false,
      },
    ]);
    const { rerender } = render(<CollectionDirectory kind="artist" root="root-1" />);
    await screen.findByRole("button", { name: /打开.*Alice/ });

    mockCommand("catalog_collections", new Error("catalog_collections failed"));
    rerender(<CollectionDirectory kind="album" root="root-1" />);

    await screen.findByText("加载目录失败，请重试");
    expect(screen.queryByRole("button", { name: /打开.*Alice/ })).not.toBeInTheDocument();
    expect(screen.getByText("没有找到匹配的内容")).toBeInTheDocument();
  });
});

/**
 * DIR-LOC 聚合详情页定位 (playlist-search-locate-import 4.3).
 *
 * The locate control lives in the opened artist/album detail's `.library-tools`
 * only (the card grid has no song list to scroll). Playing a song of that
 * collection rolls to it; a song outside the collection (here: the list is the
 * whole collection at once) is voiced without moving.
 */
describe("CollectionDirectory — 聚合详情定位 (DIR-LOC)", () => {
  const album = {
    kind: "album" as const,
    artistKey: "alice",
    albumKey: "first",
    artist: "Alice",
    name: "First album",
    songCount: 1,
    coverKey: null,
    hasCustomCover: false,
  };
  const SONG = {
    id: "song-1",
    title: "First song",
    artist: "Alice",
    album: "First album",
    durationS: 211,
    favorite: false,
    playCount: 0,
    availability: "available" as const,
    relativePath: "first.flac",
  };

  const EMPTY_SNAPSHOT = {
    state: "stopped" as const,
    position: null,
    duration: null,
    volume: 1,
    muted: false,
    currentQueueEntryId: null,
    currentSongId: null,
    queueLen: 0,
    mode: "sequential" as const,
    currentTitle: null,
    currentArtist: null,
    currentAlbum: null,
    currentCoverKey: null,
    currentLyrics: null,
    currentCanImport: false,
    queueRevision: 0,
  };

  function setCurrentSong(songId: string | null) {
    playerStore.publish(songId ? { ...EMPTY_SNAPSHOT, currentSongId: songId } : EMPTY_SNAPSHOT);
  }

  afterEach(() => {
    act(() => playerStore.publish(EMPTY_SNAPSHOT));
  });

  async function openDetail(withToast = false) {
    mockCommand("catalog_collections", [album]);
    mockCommand("catalog_collection_songs", [SONG]);
    render(
      withToast ? (
        <>
          <CollectionDirectory kind="album" root="root-1" />
          <ToastView />
        </>
      ) : (
        <CollectionDirectory kind="album" root="root-1" />
      ),
    );
    fireEvent.click(await screen.findByRole("button", { name: /打开专辑/ }));
    await screen.findByText("First song");
  }

  it("offers the locate control only in the opened collection detail", async () => {
    mockCommand("catalog_collections", [album]);
    mockCommand("catalog_collection_songs", [SONG]);
    render(<CollectionDirectory kind="album" root="root-1" />);
    // The card grid has no song list — no locate control yet.
    expect(screen.queryByTestId("locate-song")).not.toBeInTheDocument();

    fireEvent.click(await screen.findByRole("button", { name: /打开专辑/ }));
    await screen.findByText("First song");
    expect(screen.getByTestId("locate-song")).toBeInTheDocument();
  });

  it("rolls to a playing song of the collection without a toast", async () => {
    setCurrentSong(SONG.id);
    await openDetail();

    fireEvent.click(screen.getByTestId("locate-song"));
    // Found in the whole-collection list → settled, no toast; jsdom's tiny
    // viewport clamps the numeric scroll to 0.
    expect(screen.queryByTestId("toast")).toBeNull();
    expect(screen.getByTestId("song-list")).toHaveProperty("scrollTop", 0);
  });

  it("voices 当前没有正在播放的歌曲 when nothing is playing", async () => {
    setCurrentSong(null);
    await openDetail(true);

    await act(async () => {
      fireEvent.click(screen.getByTestId("locate-song"));
    });
    expect(await screen.findByTestId("toast")).toHaveTextContent("当前没有正在播放的歌曲");
    expect(screen.getByTestId("song-list")).toHaveProperty("scrollTop", 0);
  });

  it("voices 当前歌曲不在此列表中 for a playing song outside the collection", async () => {
    setCurrentSong("outside-song");
    await openDetail(true);

    await act(async () => {
      fireEvent.click(screen.getByTestId("locate-song"));
    });
    expect(await screen.findByTestId("toast")).toHaveTextContent("当前歌曲不在此列表中");
    expect(screen.getByTestId("song-list")).toHaveProperty("scrollTop", 0);
  });
});

describe("CollectionDirectory — 歌曲链接跳转目标 (issue #118)", () => {
  const ARTIST = {
    kind: "artist" as const,
    artistKey: "周杰伦",
    artist: "周杰伦",
    name: "周杰伦",
    songCount: 2,
    coverKey: null,
    hasCustomCover: false,
  };
  const OTHER_ARTIST = {
    kind: "artist" as const,
    artistKey: "蔡依林",
    artist: "蔡依林",
    name: "蔡依林",
    songCount: 1,
    coverKey: null,
    hasCustomCover: false,
  };
  const SONG = {
    id: "song-1",
    title: "晴天",
    artist: "周杰伦",
    album: "叶惠美",
    durationS: 239,
    favorite: false,
    playCount: 0,
    availability: "available" as const,
    relativePath: "jay/qingtian.flac",
  };

  it("opens the matching group directly and consumes the target once", async () => {
    mockCommand("catalog_collections", [OTHER_ARTIST, ARTIST]);
    mockCommand("catalog_collection_songs", [SONG]);
    const onTargetConsumed = vi.fn();
    render(
      <CollectionDirectory
        kind="artist"
        root="root-1"
        target={{ kind: "artist", artistKey: "周杰伦" }}
        onTargetConsumed={onTargetConsumed}
      />,
    );

    await screen.findByText("晴天");
    expect(onTargetConsumed).toHaveBeenCalledTimes(1);
    // The directory grid never shows: the jump goes straight into the detail.
    expect(screen.queryByRole("button", { name: /打开歌手/ })).not.toBeInTheDocument();
  });

  it("drops a leftover search term so it cannot filter the target group out", async () => {
    mockCommand("catalog_collections", [OTHER_ARTIST, ARTIST]);
    mockCommand("catalog_collection_songs", [SONG]);
    const onTargetConsumed = vi.fn();
    const { rerender } = render(<CollectionDirectory kind="artist" root="root-1" />);

    // The user had typed a filter that excludes 周杰伦 before clicking a link.
    const search = screen.getByRole("searchbox", { name: "搜索歌手" });
    fireEvent.change(search, { target: { value: "蔡依林" } });
    await screen.findByRole("button", { name: /打开歌手 蔡依林/ });
    expect(lastCollectionQuery().search).toBe("蔡依林");

    rerender(
      <CollectionDirectory
        kind="artist"
        root="root-1"
        target={{ kind: "artist", artistKey: "周杰伦" }}
        onTargetConsumed={onTargetConsumed}
      />,
    );

    await screen.findByText("晴天");
    await waitFor(() => expect(lastCollectionQuery().search).toBe(""));
    expect(onTargetConsumed).toHaveBeenCalledTimes(1);
  });

  it("keeps the directory usable and says so when the target no longer exists", async () => {
    mockCommand("catalog_collections", [OTHER_ARTIST]);
    const onTargetConsumed = vi.fn();
    render(
      <>
        <CollectionDirectory
          kind="artist"
          root="root-1"
          target={{ kind: "artist", artistKey: "周杰伦" }}
          onTargetConsumed={onTargetConsumed}
        />
        <ToastView />
      </>,
    );

    expect(await screen.findByTestId("toast")).toHaveTextContent("未找到该歌手/专辑，可能已被移除");
    expect(onTargetConsumed).toHaveBeenCalledTimes(1);
    // The directory stays usable rather than showing an empty detail.
    expect(await screen.findByRole("button", { name: /打开歌手 蔡依林/ })).toBeInTheDocument();
  });

  it("keeps the target pending when the directory request fails, then applies it on retry", async () => {
    mockCommand("catalog_collections", new Error("boom"));
    const onTargetConsumed = vi.fn();
    render(
      <CollectionDirectory
        kind="artist"
        root="root-1"
        target={{ kind: "artist", artistKey: "周杰伦" }}
        onTargetConsumed={onTargetConsumed}
      />,
    );

    expect(await screen.findByText("加载目录失败，请重试")).toBeInTheDocument();
    expect(onTargetConsumed).not.toHaveBeenCalled();

    mockCommand("catalog_collections", [ARTIST]);
    mockCommand("catalog_collection_songs", [SONG]);
    act(() => invalidateLibrary());

    await screen.findByText("晴天");
    expect(onTargetConsumed).toHaveBeenCalledTimes(1);
  });

  it("returns a link-opened detail to the source view, and a card-opened detail to the directory", async () => {
    mockCommand("catalog_collections", [ARTIST]);
    mockCommand("catalog_collection_songs", [SONG]);
    const onReturnToSource = vi.fn();
    const { rerender } = render(
      <CollectionDirectory
        kind="artist"
        root="root-1"
        target={{ kind: "artist", artistKey: "周杰伦" }}
        onTargetConsumed={() => {}}
        onReturnToSource={onReturnToSource}
        returnLabel="返回全部歌曲"
      />,
    );
    await screen.findByText("晴天");

    // Link-opened detail: the back entry names and restores the source view.
    fireEvent.click(screen.getByRole("button", { name: "返回全部歌曲" }));
    expect(onReturnToSource).toHaveBeenCalledTimes(1);

    // Card-opened detail: the back entry returns to the aggregate directory.
    rerender(
      <CollectionDirectory
        kind="artist"
        root="root-1"
        onReturnToSource={onReturnToSource}
        returnLabel="返回全部歌曲"
      />,
    );
    fireEvent.click(await screen.findByRole("button", { name: /打开歌手 周杰伦/ }));
    await screen.findByText("晴天");
    fireEvent.click(screen.getByRole("button", { name: "返回歌手" }));

    expect(onReturnToSource).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("button", { name: /打开歌手 周杰伦/ })).toBeInTheDocument();
  });

  it("does not reopen the target after the detail returns to the directory", async () => {
    mockCommand("catalog_collections", [ARTIST]);
    mockCommand("catalog_collection_songs", [SONG]);
    const onTargetConsumed = vi.fn();
    const { rerender } = render(
      <CollectionDirectory
        kind="artist"
        root="root-1"
        target={{ kind: "artist", artistKey: "周杰伦" }}
        onTargetConsumed={onTargetConsumed}
      />,
    );
    await screen.findByText("晴天");
    expect(onTargetConsumed).toHaveBeenCalledTimes(1);

    // The shell clears the target once consumed; a later re-render must not
    // reopen the group.
    rerender(
      <CollectionDirectory
        kind="artist"
        root="root-1"
        target={null}
        onTargetConsumed={onTargetConsumed}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "返回歌手" }));

    expect(await screen.findByRole("button", { name: /打开歌手 周杰伦/ })).toBeInTheDocument();
    expect(onTargetConsumed).toHaveBeenCalledTimes(1);
  });

  it("matches an album target by albumKey inside the album directory", async () => {
    mockCommand("catalog_collections", [
      {
        kind: "album",
        artistKey: "周杰伦",
        albumKey: "叶惠美",
        artist: "周杰伦",
        name: "叶惠美",
        songCount: 2,
        coverKey: null,
        hasCustomCover: false,
      },
      {
        kind: "album",
        artistKey: "蔡依林",
        albumKey: "看我72变",
        artist: "蔡依林",
        name: "看我72变",
        songCount: 1,
        coverKey: null,
        hasCustomCover: false,
      },
    ]);
    mockCommand("catalog_collection_songs", [SONG]);
    const onTargetConsumed = vi.fn();
    render(
      <CollectionDirectory
        kind="album"
        root="root-1"
        target={{ kind: "album", albumKey: "叶惠美" }}
        onTargetConsumed={onTargetConsumed}
      />,
    );

    await screen.findByText("晴天");
    expect(onTargetConsumed).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: /打开专辑/ })).not.toBeInTheDocument();
  });
});
