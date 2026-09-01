/**
 * Theme preference hook (task 10.1).
 *
 * Loads the persisted theme (coral default) from the desktop local state and
 * exposes a `setTheme` that round-trips to the backend (which persists it
 * atomically). The active theme stamps `data-echo-theme` on the document root
 * so the CSS theme blocks take effect. Corrupt/missing values fall back to
 * coral server-side; the UI never invents a theme.
 */

import { useCallback, useEffect, useState } from "react";

import { bridge } from "../../bridge";
import type { Theme } from "../../ipc/ipc-types.generated";

const THEMES: readonly Theme[] = ["coral", "cobalt", "turquoise"];

export type { Theme };

const THEME_STORAGE_KEY = "echo-theme";

export interface UseThemeResult {
  readonly theme: Theme;
  readonly setTheme: (next: Theme) => void;
}

export function useTheme(): UseThemeResult {
  const [theme, setThemeState] = useState<Theme>(() => {
    // Best-effort local read; the desktop Store is authoritative. We never
    // trust an unknown value — fall back to coral.
    try {
      const stored = localStorage.getItem(THEME_STORAGE_KEY);
      if (stored && (THEMES as readonly string[]).includes(stored)) return stored as Theme;
    } catch {
      /* storage may be unavailable */
    }
    return "coral";
  });

  useEffect(() => {
    // Stamp the active theme on the document root so the CSS theme blocks take
    // effect (coral/cobalt/turquoise). This is the single writer of the
    // `data-echo-theme` attribute.
    document.documentElement.dataset.echoTheme = theme;
  }, [theme]);

  const setTheme = useCallback((next: Theme) => {
    setThemeState(next);
    try {
      localStorage.setItem(THEME_STORAGE_KEY, next);
    } catch {
      /* ignore storage failure */
    }
    // Persist atomically desktop-side; a failed persist keeps the local value
    // and the store falls back on next load — never fatal.
    void bridge.call("set_theme", { theme: next }).catch(() => {
      /* non-fatal */
    });
  }, []);

  return { theme, setTheme };
}

export { THEMES };
