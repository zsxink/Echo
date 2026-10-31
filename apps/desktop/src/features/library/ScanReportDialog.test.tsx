import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ScanReportDialog } from "./ScanReportDialog";

describe("ScanReportDialog", () => {
  it("shows each relative path and diagnostic, then closes", () => {
    const onClose = vi.fn();
    render(
      <ScanReportDialog
        issues={[
          {
            relativePath: "media/坏文件.flac",
            code: "corrupt_media",
            message: "无法读取音频内容",
            songCreated: false,
          },
          {
            relativePath: "media/重复.mp3",
            code: "duplicate_content",
            message: "内容已由另一首歌曲收录",
            songCreated: true,
          },
        ]}
        onClose={onClose}
      />,
    );

    expect(screen.getAllByTestId("scan-issue")).toHaveLength(2);
    expect(screen.getByText("media/坏文件.flac")).toBeInTheDocument();
    expect(screen.getByText("无法读取音频内容")).toBeInTheDocument();
    expect(screen.getByText("歌曲记录已保留")).toBeInTheDocument();
    expect(screen.getByText("未创建歌曲记录")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("scan-report-close"));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
