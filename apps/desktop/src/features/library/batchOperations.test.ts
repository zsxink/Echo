import { describe, expect, it, vi } from "vitest";

import type { SongView } from "../../ipc/ipc-types.generated";

vi.mock("../../bridge", () => ({
  bridge: { call: vi.fn() },
}));

import { bridge } from "../../bridge";
import {
  runAddToPlaylistsBatch,
  runFavoriteBatch,
  runQueueBatch,
  runSequentialBatch,
  undoDeleteBatch,
} from "./batchOperations";

const call = vi.mocked(bridge.call);

function song(id: string, availability: SongView["availability"] = "available"): SongView {
  return {
    id,
    title: id,
    favorite: false,
    playCount: 0,
    availability,
    relativePath: `${id}.flac`,
  };
}

describe("batch operations", () => {
  it("keeps per-item failures observable and continues in stable order", async () => {
    const result = await runSequentialBatch(["a", "b", "c"], async (id) => {
      if (id === "b") throw new Error("offline");
      if (id === "c") return { status: "skipped", message: "not applicable" };
      return { status: "success", value: id.toUpperCase() };
    });

    expect(result.items.map((item) => [item.item, item.status])).toEqual([
      ["a", "success"],
      ["b", "failed"],
      ["c", "skipped"],
    ]);
    expect(result.succeeded).toBe(1);
    expect(result.skipped).toBe(1);
    expect(result.failed).toBe(1);
  });

  it("submits play-next in reverse while reporting results in display order", async () => {
    call.mockReset();
    const submitted: string[] = [];
    call.mockImplementation(((command: string, args: { songId?: string }) => {
      if (command === "queue_command" && args.songId) submitted.push(args.songId);
      return Promise.resolve(undefined);
    }) as never);

    const result = await runQueueBatch([song("a"), song("b"), song("c")], "playNext");

    expect(submitted).toEqual(["c", "b", "a"]);
    expect(result.items.map((item) => item.item.id)).toEqual(["a", "b", "c"]);
    expect(result.succeeded).toBe(3);
  });

  it("skips unavailable favorite targets and commits available targets", async () => {
    call.mockReset();
    call.mockResolvedValue({ ...song("a"), favorite: true } as never);

    const result = await runFavoriteBatch(
      [song("a"), song("missing", "missing"), song("pending", "pending-delete")],
      true,
    );

    expect(call).toHaveBeenCalledTimes(1);
    expect(result.succeeded).toBe(1);
    expect(result.skipped).toBe(2);
    expect(result.items[1]?.message).toBe("歌曲不可用");
  });

  it("aggregates playlist duplicate, failure and unavailable members", async () => {
    call.mockReset();
    call.mockImplementation(((command: string, args: { song?: string }) => {
      if (command === "add_to_playlists" && args.song === "duplicate") {
        return Promise.reject(Object.assign(new Error("duplicate"), { code: "conflict" }));
      }
      if (command === "add_to_playlists" && args.song === "failed") {
        return Promise.reject(new Error("offline"));
      }
      return Promise.resolve(undefined);
    }) as never);

    const result = await runAddToPlaylistsBatch(
      [song("ok"), song("duplicate"), song("failed"), song("missing", "missing")],
      ["playlist-1"],
    );

    expect(result.succeeded).toBe(1);
    expect(result.skipped).toBe(2);
    expect(result.failed).toBe(1);
    expect(call).toHaveBeenCalledTimes(3);
  });

  it("reports partial undo failure without claiming the whole batch recovered", async () => {
    call.mockReset();
    call.mockImplementation(((command: string, args: { operation?: string }) => {
      if (command === "undo_delete" && args.operation === "op-2") {
        return Promise.reject(new Error("expired"));
      }
      return Promise.resolve(undefined);
    }) as never);

    await expect(undoDeleteBatch("root-1", ["op-1", "op-2"])).resolves.toEqual({
      succeeded: 1,
      failed: 1,
    });
  });
});
