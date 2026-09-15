import { act, renderHook } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import { useLibraryStatus } from "./useLibraryStatus";

it("does not let a delayed startup response replace the activated library", async () => {
  let finishStartup!: (status: unknown) => void;
  const startup = new Promise<unknown>((resolve) => {
    finishStartup = resolve;
  });
  vi.mocked(invoke).mockReturnValueOnce(startup);
  const { result } = renderHook(() => useLibraryStatus());
  const active = {
    configured: true,
    readOnly: false,
    unavailable: false,
    scanning: false,
    activeRoot: "chosen-root",
  };
  vi.mocked(invoke).mockResolvedValueOnce(active);
  await act(async () => {
    await result.current.refresh();
  });
  expect(result.current.activeRoot).toBe("chosen-root");

  await act(async () => {
    finishStartup({ configured: false, readOnly: false, unavailable: false, scanning: false });
    await startup;
  });
  expect(result.current.configured).toBe(true);
  expect(result.current.activeRoot).toBe("chosen-root");
});
