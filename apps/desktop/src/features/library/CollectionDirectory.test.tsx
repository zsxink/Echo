import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { CollectionDirectory } from "./CollectionDirectory";

function mockCommand(command: string, value: unknown) {
  // The shared jsdom setup exposes a small Tauri-command test hook.
  // @ts-expect-error -- test-only global declared by src/test/setup.ts
  globalThis.__echoTest.setInvoke(command, value);
}

describe("CollectionDirectory", () => {
  it("shows artist cards and song details without any selection controls", async () => {
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
    const songList = screen.getByRole("region", { name: "Alice的歌曲" });
    expect(songList).toHaveClass("collection-song-list");
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.queryByText("全选")).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId("selection-mode-button"));
    const selectAll = await screen.findByRole("button", { name: "全选当前歌曲" });
    fireEvent.click(selectAll);
    expect(screen.getByRole("button", { name: "取消全选当前歌曲" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "取消选择First song" })).toBeInTheDocument();
  });
});
