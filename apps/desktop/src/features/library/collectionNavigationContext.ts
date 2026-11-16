/**
 * Collection link navigation context (issue #118).
 *
 * Song rows, the song-detail dialog and the two playing surfaces all need to
 * request “switch to the artist/album directory and open this group”. The
 * workspace and the players sit on different branches of the shell, so the
 * capability is published on a context instead of being threaded through every
 * intermediate component.
 *
 * The context is deliberately optional: a surface rendered on its own (as the
 * component tests do, and as any future standalone embedding would) gets
 * `null` and falls back to plain text — no provider means no navigation, not a
 * crash.
 *
 * The context and its hook live apart from the provider component so the
 * provider module exports only a component (react-refresh).
 */

import { createContext, useContext } from "react";

export interface CollectionNavigation {
  readonly openArtist: (artistKey: string) => void;
  readonly openAlbum: (albumKey: string) => void;
}

export const CollectionNavigationContext = createContext<CollectionNavigation | null>(null);

/** The collection navigation callbacks, or `null` outside the app shell. */
export function useOptionalCollectionNavigation(): CollectionNavigation | null {
  return useContext(CollectionNavigationContext);
}
