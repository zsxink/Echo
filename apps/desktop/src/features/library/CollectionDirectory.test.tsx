import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { CollectionDirectory } from "./CollectionDirectory";

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
