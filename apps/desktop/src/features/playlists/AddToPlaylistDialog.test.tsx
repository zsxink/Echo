/**
 * Task 10.9 — add-to-playlist selector: multi-playlist membership via a single
 * `add_to_playlists(song, targets)` mutation; cancel never mutates; the targets
 * are exactly the selected playlist ids.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AddToPlaylistDialog } from "./AddToPlaylistDialog";

vi.mock("../../bridge", () => ({
  bridge: {
    call: vi.fn(),
  },
}));

import { bridge } from "../../bridge";

const call = vi.mocked(bridge.call);

describe("AddToPlaylistDialog (task 10.9)", () => {
  it("sends only the checked playlist targets in one mutation, then closes", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([
      { id: "pl-1", name: "Chill" },
      { id: "pl-2", name: "Focus" },
    ] as never);
    call.mockResolvedValueOnce(undefined as never); // add_to_playlists

    const onClose = vi.fn();
    const onDone = vi.fn();
    render(<AddToPlaylistDialog songId="song-9" onClose={onClose} onDone={onDone} />);

    await screen.findByLabelText("Chill");
    fireEvent.click(screen.getByLabelText("Focus"));
    fireEvent.click(screen.getByText("添加"));

    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("add_to_playlists", {
        song: "song-9",
        targets: ["pl-2"],
      }),
    );
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(onClose).toHaveBeenCalled();
  });

  it("does not mutate and reports when no playlist is selected", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([{ id: "pl-1", name: "Chill" }] as never);

    const onClose = vi.fn();
    const onDone = vi.fn();
    render(<AddToPlaylistDialog songId="song-9" onClose={onClose} onDone={onDone} />);
    await screen.findByLabelText("Chill");
    fireEvent.click(screen.getByText("添加"));

    expect(await screen.findByText("请至少选择一个歌单")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("add_to_playlists", expect.anything());
    expect(onDone).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("cancelling closes without sending any mutation", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([{ id: "pl-1", name: "Chill" }] as never);

    const onClose = vi.fn();
    render(<AddToPlaylistDialog songId="song-9" onClose={onClose} onDone={() => {}} />);
    await screen.findByLabelText("Chill");
    fireEvent.click(screen.getByText("取消"));

    expect(call).not.toHaveBeenCalledWith("add_to_playlists", expect.anything());
    expect(onClose).toHaveBeenCalled();
  });
});
