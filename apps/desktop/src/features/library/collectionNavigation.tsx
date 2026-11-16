/**
 * Collection link navigation provider (issue #118).
 *
 * The app shell installs this once, above both the workspace and the players,
 * so every surface that can show an artist/album can ask for the same
 * navigation without prop threading. The context itself lives in
 * `collectionNavigationContext.ts` — this module exports only a component, so
 * fast refresh keeps working.
 */

import { useMemo, type ReactNode } from "react";

import {
  CollectionNavigationContext,
  type CollectionNavigation,
} from "./collectionNavigationContext";

export function CollectionNavigationProvider(
  props: CollectionNavigation & { readonly children: ReactNode },
) {
  const { openArtist, openAlbum, children } = props;
  const value = useMemo(() => ({ openArtist, openAlbum }), [openArtist, openAlbum]);
  return (
    <CollectionNavigationContext.Provider value={value}>
      {children}
    </CollectionNavigationContext.Provider>
  );
}
