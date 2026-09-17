import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { SettingsView } from "./SettingsView";

beforeEach(() => {
  localStorage.clear();
  // @ts-expect-error - the test hook installed by setup.ts
  globalThis.__echoTest.setInvoke("get_close_behavior", "exit");
});

describe("SettingsView close behavior", () => {
  it("shows the persisted exit preference instead of the background default", async () => {
    render(<SettingsView onClose={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByLabelText("退出应用")).toBeChecked();
    });
    expect(screen.getByLabelText("保持在后台运行")).not.toBeChecked();
  });
});
