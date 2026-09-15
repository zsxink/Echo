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
  it("sends only the chosen playlist targets in one mutation, then closes", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([
      { id: "pl-1", name: "Chill", memberCount: 3 },
      { id: "pl-2", name: "Focus", memberCount: 5 },
    ] as never);
    call.mockResolvedValueOnce(undefined as never); // add_to_playlists

    const onClose = vi.fn();
    const onDone = vi.fn();
    render(
      <AddToPlaylistDialog songId="song-9" songTitle="心房" onClose={onClose} onDone={onDone} />,
    );

    await screen.findByLabelText("Chill");
    // The prototype's sub line names the song being added.
    expect(screen.getByText("将「心房」添加到：")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Focus"));
    fireEvent.click(screen.getByText("确认"));

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
    call.mockResolvedValueOnce([{ id: "pl-1", name: "Chill", memberCount: 0 }] as never);

    const onClose = vi.fn();
    const onDone = vi.fn();
    render(<AddToPlaylistDialog songId="song-9" onClose={onClose} onDone={onDone} />);
    await screen.findByLabelText("Chill");
    fireEvent.click(screen.getByText("确认"));

    expect(await screen.findByText("请至少选择一个歌单")).toBeInTheDocument();
    expect(call).not.toHaveBeenCalledWith("add_to_playlists", expect.anything());
    expect(onDone).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("shows the prototype's empty-list copy when there is no playlist yet", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([] as never);

    render(<AddToPlaylistDialog songId="song-9" onClose={vi.fn()} onDone={vi.fn()} />);
    expect(await screen.findByText("还没有歌单，先创建一个吧。")).toBeInTheDocument();
    expect(screen.getByText("新建歌单")).toBeInTheDocument();
  });

  it("cancelling closes without sending any mutation", async () => {
    call.mockReset();
    call.mockResolvedValueOnce([{ id: "pl-1", name: "Chill", memberCount: 0 }] as never);

    const onClose = vi.fn();
    render(<AddToPlaylistDialog songId="song-9" onClose={onClose} onDone={() => {}} />);
    await screen.findByLabelText("Chill");
    fireEvent.click(screen.getByText("取消"));

    expect(call).not.toHaveBeenCalledWith("add_to_playlists", expect.anything());
    expect(onClose).toHaveBeenCalled();
  });
});
