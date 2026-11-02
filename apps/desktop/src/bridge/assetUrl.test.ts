import { beforeEach, describe, expect, it, vi } from "vitest";
import { convertFileSrc } from "@tauri-apps/api/core";

import { assetUrl } from "./index";

// The shared setup mock is enough for component tests, but these cases need to
// control what `convertFileSrc` returns (Windows base, percent-encoded path,
// data/blob passthrough).
vi.mock("@tauri-apps/api/core", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@tauri-apps/api/core")>();
  return {
    ...actual,
    convertFileSrc: vi.fn(),
  };
});

const mockedConvertFileSrc = vi.mocked(convertFileSrc);

beforeEach(() => {
  mockedConvertFileSrc.mockImplementation(
    (key: string, protocol = "asset") => `${protocol}://${key}`,
  );
});

describe("assetUrl", () => {
  it("defaults to the list tier", () => {
    expect(assetUrl("cv1-abc")).toBe("cover://cv1-abc?s=list");
  });

  it("appends detail and original tiers after convertFileSrc", () => {
    expect(assetUrl("cv1-abc", "detail")).toBe("cover://cv1-abc?s=detail");
    expect(assetUrl("cv1-abc", "original")).toBe("cover://cv1-abc?s=original");
    expect(mockedConvertFileSrc).toHaveBeenCalledWith("cv1-abc", "cover");
  });

  it("keeps the Windows cover base", () => {
    mockedConvertFileSrc.mockReturnValue("http://cover.localhost/cv1-abc");
    expect(assetUrl("cv1-abc", "detail")).toBe("http://cover.localhost/cv1-abc?s=detail");
  });

  it("adds the query after conversion so the key stays intact", () => {
    // The real convertFileSrc percent-encodes the path argument; a query built
    // before it would have its `?`/`=` encoded into the key.
    mockedConvertFileSrc.mockReturnValue("cover://localhost/cv1%2Dabc");
    expect(assetUrl("cv1-abc")).toBe("cover://localhost/cv1%2Dabc?s=list");
  });

  it("does not append a tier to data or blob URLs", () => {
    mockedConvertFileSrc.mockReturnValue("data:image/png;base64,AAAA");
    expect(assetUrl("cv1-abc")).toBe("data:image/png;base64,AAAA");

    mockedConvertFileSrc.mockReturnValue("blob:http://localhost/abc");
    expect(assetUrl("cv1-abc", "original")).toBe("blob:http://localhost/abc");
  });
});
