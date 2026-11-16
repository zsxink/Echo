import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { CollectionNavigationProvider, useOptionalCollectionNavigation } from "./index";

function Probe() {
  const navigation = useOptionalCollectionNavigation();
  return (
    <div>
      <span data-testid="state">{navigation ? "inside" : "outside"}</span>
      <button type="button" onClick={() => navigation?.openArtist("artist-a")}>
        artist
      </button>
      <button type="button" onClick={() => navigation?.openAlbum("album-b")}>
        album
      </button>
    </div>
  );
}

describe("collection navigation context", () => {
  it("exposes both callbacks inside the provider", () => {
    const openArtist = vi.fn();
    const openAlbum = vi.fn();
    render(
      <CollectionNavigationProvider openArtist={openArtist} openAlbum={openAlbum}>
        <Probe />
      </CollectionNavigationProvider>,
    );

    expect(screen.getByTestId("state")).toHaveTextContent("inside");
    fireEvent.click(screen.getByRole("button", { name: "artist" }));
    fireEvent.click(screen.getByRole("button", { name: "album" }));

    expect(openArtist).toHaveBeenCalledWith("artist-a");
    expect(openAlbum).toHaveBeenCalledWith("album-b");
  });

  it("returns null outside the provider without throwing", () => {
    render(<Probe />);

    expect(screen.getByTestId("state")).toHaveTextContent("outside");
    expect(() => fireEvent.click(screen.getByRole("button", { name: "artist" }))).not.toThrow();
  });
});
