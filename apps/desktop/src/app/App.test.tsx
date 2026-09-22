import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";

describe("App shell", () => {
  it("renders the echo shell and shows the first-launch choose-root view when no library is configured", () => {
    render(<App />);
    expect(screen.getByTestId("echo-shell")).toBeInTheDocument();
    // The default library_status mock reports unconfigured, so the shell must
    // show the initialize view — never a working library it doesn't have.
    expect(screen.getByTestId("choose-root")).toBeInTheDocument();
    expect(screen.queryByTestId("sidebar")).not.toBeInTheDocument();
  });

  it("uses a full-width initial layout without reserving a sidebar column", () => {
    render(<App />);
    const container = screen.getByTestId("choose-root");
    expect(screen.getByTestId("echo-shell")).toHaveClass("app-initial");
    expect(container).toHaveClass("workspace-empty");
  });

  it("declares the top inset strip as the window drag region", () => {
    render(<App />);
    // `hiddenTitle: true` leaves no native titlebar to drag the window by, so
    // the shell's first child must be the Tauri drag handle. It is decorative
    // only — hidden from assistive tech, which never loses a control to it.
    const handle = screen.getByTestId("echo-shell").firstElementChild;
    expect(handle).toHaveClass("titlebar-drag");
    expect(handle).toHaveAttribute("data-tauri-drag-region");
    expect(handle).toHaveAttribute("aria-hidden", "true");
  });
});

describe("Library activation without a status event", () => {
  const mocks = (
    globalThis as unknown as {
      __echoTest: { setInvoke: (command: string, value: unknown) => void };
    }
  ).__echoTest;

  it.each(["empty", "songs", "existing"])("opens a %s library after selection", async (kind) => {
    await act(async () => {
      render(<App />);
    });
    mocks.setInvoke("choose_library_root", {
      configured: true,
      readOnly: false,
      activeRoot: "root-1",
    });
    mocks.setInvoke("library_status", {
      configured: true,
      readOnly: false,
      unavailable: false,
      scanning: false,
      activeRoot: "root-1",
    });
    mocks.setInvoke("all_songs", {
      items:
        kind === "empty"
          ? []
          : [
              {
                id: "saved-song",
                title: "已有歌曲",
                artist: "Echo",
                album: "Album",
                durationS: 120,
                favorite: kind === "existing",
                playCount: 0,
                availability: "available",
                relativePath: "song.flac",
              },
            ],
      isLast: true,
      nextCursor: null,
    });
    mocks.setInvoke(
      "playlists",
      kind === "existing" ? [{ id: "saved-list", name: "原有歌单", memberCount: 1 }] : [],
    );

    fireEvent.click(screen.getByRole("button", { name: "选择资料库目录" }));
    expect(await screen.findByTestId("workspace")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("restore_playback_session", {});
    expect(screen.queryByTestId("choose-root")).not.toBeInTheDocument();
    expect(screen.getByTestId("echo-shell")).not.toHaveClass("app-initial");
    if (kind === "empty") expect(await screen.findByText("曲库为空")).toBeInTheDocument();
    else expect(await screen.findByText("已有歌曲")).toBeInTheDocument();
    if (kind === "existing") expect(await screen.findByText("原有歌单")).toBeInTheDocument();
  });

  it("keeps selection available after cancellation", async () => {
    await act(async () => {
      render(<App />);
    });
    mocks.setInvoke("choose_library_root", null);
    fireEvent.click(screen.getByRole("button", { name: "选择资料库目录" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "选择资料库目录" })).toBeEnabled(),
    );
    expect(screen.queryByTestId("workspace")).not.toBeInTheDocument();
  });

  it("allows retry if the post-activation status query fails", async () => {
    await act(async () => {
      render(<App />);
    });
    const call = vi.mocked(invoke);
    mocks.setInvoke("choose_library_root", {
      configured: true,
      readOnly: false,
      activeRoot: "root-1",
    });
    // Reject the refresh, after the picker has successfully returned.
    call.mockResolvedValueOnce({ configured: true, readOnly: false, activeRoot: "root-1" });
    call.mockResolvedValueOnce("empty");
    call.mockRejectedValueOnce(new Error("status unavailable"));
    fireEvent.click(screen.getByRole("button", { name: "选择资料库目录" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "选择资料库目录" })).toBeEnabled();
  });
});

describe("File-open playback", () => {
  it("signals frontend readiness after the listener registers, then still plays drained paths", async () => {
    render(<App />);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("file_open_frontend_ready", {}));
    // The ready signal must not swallow the playback contract: paths drained
    // after cold start are still consumed by the listener that just registered.
    const callbacks = (
      globalThis as unknown as {
        __echoTest: { emit: (event: string, payload: unknown) => void };
      }
    ).__echoTest;
    callbacks.emit("app://file-open-request", ["/tmp/drained.flac"]);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("play_temporary_file", {
        path: "/tmp/drained.flac",
        displayName: "drained.flac",
      }),
    );
  });

  it("replaces playback for every path drained after cold start", async () => {
    render(<App />);
    await waitFor(() =>
      expect((invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls).toContainEqual([
        "library_status",
        {},
      ]),
    );

    const callbacks = (
      globalThis as unknown as {
        __echoTest: { emit: (event: string, payload: unknown) => void };
      }
    ).__echoTest;
    callbacks.emit("app://file-open-request", ["/tmp/first.flac", "/tmp/second.flac"]);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("play_temporary_file", {
        path: "/tmp/first.flac",
        displayName: "first.flac",
      });
      expect(invoke).toHaveBeenCalledWith("play_temporary_file", {
        path: "/tmp/second.flac",
        displayName: "second.flac",
      });
    });
  });

  it("never URL-decodes a payload that already contains a literal %20", async () => {
    // The shell decodes the OS `file://` URL exactly once
    // (normalize-os-file-open-paths). A filename may legitimately contain a
    // literal `%20`, so the frontend must pass the payload through verbatim —
    // a second decode here would corrupt such names.
    render(<App />);
    await waitFor(() =>
      expect((invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls).toContainEqual([
        "library_status",
        {},
      ]),
    );

    const callbacks = (
      globalThis as unknown as {
        __echoTest: { emit: (event: string, payload: unknown) => void };
      }
    ).__echoTest;
    callbacks.emit("app://file-open-request", ["/tmp/My%20Mix.flac"]);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("play_temporary_file", {
        path: "/tmp/My%20Mix.flac",
        displayName: "My%20Mix.flac",
      });
    });
    expect(invoke).not.toHaveBeenCalledWith("play_temporary_file", {
      path: "/tmp/My Mix.flac",
      displayName: "My Mix.flac",
    });
  });
});
describe("Phase-one scope guard", () => {
  const mocks = (
    globalThis as unknown as {
      __echoTest: { setInvoke: (command: string, value: unknown) => void };
    }
  ).__echoTest;

  // Task 10.4 / 13.8 and `docs/ROADMAP.md` phase one exclude sync entirely. A
  // sidebar control reading 同步/资料库已同步 used to be rendered here; it was
  // removed because it was both out of scope and a fabricated state (there is
  // nothing to have "already synced"). This is the regression test.
  it("renders no sync entry anywhere in the activated shell", async () => {
    await act(async () => {
      render(<App />);
    });
    mocks.setInvoke("choose_library_root", {
      configured: true,
      readOnly: false,
      activeRoot: "root-1",
    });
    mocks.setInvoke("library_status", {
      configured: true,
      readOnly: false,
      unavailable: false,
      scanning: false,
      activeRoot: "root-1",
    });
    mocks.setInvoke("all_songs", { items: [], isLast: true, nextCursor: null });
    mocks.setInvoke("playlists", []);

    fireEvent.click(screen.getByRole("button", { name: "选择资料库目录" }));
    expect(await screen.findByTestId("workspace")).toBeInTheDocument();

    expect(screen.queryByTestId("sidebar")).toBeInTheDocument();
    expect(screen.queryByTestId("sync-button")).not.toBeInTheDocument();
    expect(document.querySelector(".sync-button")).toBeNull();
    expect(document.body.textContent ?? "").not.toMatch(/同步|sync/i);
  });
});

describe("Library directory views", () => {
  const mocks = (
    globalThis as unknown as {
      __echoTest: { setInvoke: (command: string, value: unknown) => void };
    }
  ).__echoTest;

  const artistEntry = {
    kind: "artist",
    artistKey: "alice",
    artist: "Alice",
    name: "Alice",
    songCount: 2,
    coverKey: null,
    hasCustomCover: false,
  };
  const albumEntry = {
    kind: "album",
    artistKey: "alice",
    albumKey: "first album",
    artist: "Alice",
    name: "First album",
    songCount: 2,
    coverKey: null,
    hasCustomCover: false,
  };

  // 歌手 and 专辑 render the same component at the same tree position, so its
  // state survives the switch unless the view identity resets it. The reported
  // bug was exactly that: 歌手's cards stayed on screen under 专辑 chrome. This
  // guards the wiring end to end (the sidebar click, the kind the shell passes,
  // and the directory's own reset).
  it("shows 专辑's own entries after switching from 歌手", async () => {
    await act(async () => {
      render(<App />);
    });
    mocks.setInvoke("choose_library_root", {
      configured: true,
      readOnly: false,
      activeRoot: "root-1",
    });
    mocks.setInvoke("library_status", {
      configured: true,
      readOnly: false,
      unavailable: false,
      scanning: false,
      activeRoot: "root-1",
    });
    mocks.setInvoke("all_songs", { items: [], isLast: true, nextCursor: null });
    mocks.setInvoke("playlists", []);
    mocks.setInvoke("library_counts", { all: 0, recent: 0, favorites: 0 });
    let entries: unknown = [artistEntry];
    // The test hook hands the handler no arguments, so the fixture is swapped
    // between the two views instead of being keyed by the requested kind.
    mocks.setInvoke("catalog_collections", () => entries);

    fireEvent.click(screen.getByRole("button", { name: "选择资料库目录" }));
    await screen.findByTestId("workspace");

    fireEvent.click(screen.getByRole("button", { name: "歌手" }));
    await screen.findByRole("button", { name: "打开歌手 Alice" });

    entries = [albumEntry];
    fireEvent.click(screen.getByRole("button", { name: "专辑" }));
    // Match on the entry, not on the chrome prefix: the aria-label is built
    // from the CURRENT kind (`打开${title} ${name}`), so a stale artist card
    // is relabelled 打开专辑 Alice the moment the view switches — asserting
    // on "打开歌手 …" would pass with the bug still present.
    expect(screen.queryByRole("button", { name: /打开.*Alice/ })).not.toBeInTheDocument();
    await screen.findByRole("button", { name: "打开专辑 First album" });
  });
});
