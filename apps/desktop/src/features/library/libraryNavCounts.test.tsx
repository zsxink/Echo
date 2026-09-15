/**
 * 资料库导航计数 (regression).
 *
 * Bug: the sidebar only printed a count *after* the user opened a view and
 * paged it to the end — `publishLibraryCount` fired from the view that owned
 * the query, once `isLast` became true. So 「喜欢的音乐」 had no number until
 * you clicked into it, which is the one moment the number is worth having.
 *
 * Contract under test: a count comes from the backend's `library_counts`, is
 * visible before its view is ever opened, and never equals "rows loaded so
 * far". This test seeds a one-song page while the counters say 42/7/42, so any
 * regression back to counting loaded rows fails here immediately.
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "../../app/App";

const mocks = (
  globalThis as unknown as {
    __echoTest: { setInvoke: (command: string, value: unknown) => void };
  }
).__echoTest;

const SONG = {
  id: "song-1",
  title: "晴天",
  artist: "周杰伦",
  album: "叶惠美",
  durationS: 239,
  favorite: false,
  playCount: 0,
  availability: "available" as const,
  relativePath: "晴天.flac",
};

/** A configured library whose catalog page is deliberately tiny while the
 *  counters describe a much larger library. */
function configuredLibrary() {
  mocks.setInvoke("library_status", {
    configured: true,
    readOnly: false,
    unavailable: false,
    scanning: false,
    activeRoot: "root-1",
  });
  mocks.setInvoke("all_songs", { items: [SONG], isLast: true, nextCursor: null });
  mocks.setInvoke("song_cover_keys", {});
  mocks.setInvoke("playlists", [{ id: "list-1", name: "通勤", memberCount: 3 }]);
  mocks.setInvoke("library_counts", { all: 42, favorites: 7, recent: 42 });
}

const countOf = (view: string) => screen.getByTestId(`nav-count-${view}`);

beforeEach(configuredLibrary);

describe("资料库导航计数", () => {
  it("shows 喜欢的音乐's count without the user ever opening that view", async () => {
    await act(async () => {
      render(<App />);
    });

    await waitFor(() => expect(countOf("favorites")).toHaveTextContent("7"));
    // And the shell is still showing 全部歌曲 — the favorites view was never opened.
    expect(screen.getByRole("heading", { name: "全部歌曲" })).toBeInTheDocument();
  });

  it("counts every 资料库 view, including 最近添加", async () => {
    await act(async () => {
      render(<App />);
    });

    await waitFor(() => expect(countOf("all")).toHaveTextContent("42"));
    expect(countOf("recent")).toHaveTextContent("42");
    expect(countOf("favorites")).toHaveTextContent("7");
  });

  it("prints the backend total, not the number of rows the page loaded", async () => {
    await act(async () => {
      render(<App />);
    });

    // The loaded page holds exactly one song; printing "1" would be the
    // original bug wearing a different hat.
    await waitFor(() => expect(screen.getByText("晴天")).toBeInTheDocument());
    expect(countOf("all")).toHaveTextContent("42");
  });

  it("shows a playlist's member count without opening the playlist", async () => {
    await act(async () => {
      render(<App />);
    });

    await waitFor(() => expect(screen.getByText("通勤")).toBeInTheDocument());
    expect(screen.getByTestId("playlist-count-list-1")).toHaveTextContent("3");
  });

  it("re-counts after a favorite toggle, so another view's change lands here", async () => {
    mocks.setInvoke("set_favorite", { ...SONG, favorite: true });
    await act(async () => {
      render(<App />);
    });

    await waitFor(() => expect(screen.getByText("晴天")).toBeInTheDocument());
    const before = vi.mocked(invoke).mock.calls.filter(([c]) => c === "library_counts").length;

    const heart = screen.getByRole("button", { name: /喜欢晴天/ });
    await act(async () => {
      fireEvent.click(heart);
    });

    // The committed mutation invalidates the cache and triggers a fresh read —
    // the old code never re-read, so a toggle made outside the favorites view
    // left its count frozen forever.
    await waitFor(() =>
      expect(
        vi.mocked(invoke).mock.calls.filter(([c]) => c === "library_counts").length,
      ).toBeGreaterThan(before),
    );
  });

  it("shows no count when the backend cannot answer, without breaking the shell", async () => {
    // Totals are unknown, not zero: a failed count must not render "0".
    mocks.setInvoke("library_counts", new Error("unavailable"));
    await act(async () => {
      render(<App />);
    });

    await waitFor(() => expect(screen.getByText("晴天")).toBeInTheDocument());
    expect(screen.queryByTestId("nav-count-favorites")).not.toBeInTheDocument();
    expect(screen.getByTestId("sidebar")).toBeInTheDocument();
  });
});
