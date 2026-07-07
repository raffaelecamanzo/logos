/*
 * Theme state (S-193, CR-050, FR-UI-23, ADR-44): the runtime owner of the
 * dark/light choice. The pre-paint application of a PERSISTED choice happens in
 * the CSP-clean external head script (public/theme-init.js); this module owns
 * runtime toggling, persistence, and exposing the effective theme to React.
 *
 * Contract (must stay in sync with theme-init.js):
 *   - Persisted choice lives in localStorage under `logos-theme` ∈ {"light","dark"}.
 *   - The choice is applied as a `data-theme` attribute on <html>; the token layer
 *     (styles/tokens.css) remaps the semantic tokens off it. No component reads the
 *     theme to style itself — switching is a pure token remap (no layout shift).
 *   - With NO persisted choice, no attribute is set and CSS `prefers-color-scheme`
 *     resolves the first-visit default (dark, honouring a light OS).
 */

import { createContext, useCallback, useContext, useEffect, useState } from "react";

export type Theme = "light" | "dark";

/** localStorage key for the persisted choice (mirrors theme-init.js). */
const STORAGE_KEY = "logos-theme";

/** The OS preference, with dark as the canonical fallback (ADR-44). */
function osPreference(): Theme {
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

/** The persisted choice, or null when the user has not chosen (first visit). */
function storedChoice(): Theme | null {
  try {
    const v = window.localStorage.getItem(STORAGE_KEY);
    return v === "light" || v === "dark" ? v : null;
  } catch {
    return null;
  }
}

/**
 * The theme in effect right now: the persisted choice if any, else the OS
 * preference. Matches what theme-init.js + the CSS default already painted, so the
 * toggle shows the correct initial label with no flicker.
 */
export function effectiveTheme(): Theme {
  return storedChoice() ?? osPreference();
}

/** Apply a theme as the `data-theme` attribute and persist the choice. */
function applyAndPersist(theme: Theme): void {
  document.documentElement.setAttribute("data-theme", theme);
  try {
    window.localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    /* Persistence is best-effort; the attribute still themes this session. */
  }
}

export interface ThemeContextValue {
  /** The theme in effect. */
  theme: Theme;
  /** Switch to an explicit theme (persisted). */
  setTheme: (theme: Theme) => void;
  /** Flip between light and dark (persisted). */
  toggleTheme: () => void;
}

export const ThemeContext = createContext<ThemeContextValue | null>(null);

/**
 * The state hook backing the provider. Initialises from the already-applied
 * effective theme, then keeps `data-theme` and localStorage in sync on change.
 * Also follows the OS preference live UNTIL the user makes an explicit choice.
 */
export function useThemeState(): ThemeContextValue {
  const [theme, setThemeRaw] = useState<Theme>(() => effectiveTheme());

  // Reflect the effective theme onto <html> on mount. For a first-visit user this
  // promotes the implicit CSS default into an explicit attribute only once they
  // have a value to reflect — but we keep it implicit until an explicit choice so
  // the OS-follow below still works; so on mount we set the attribute only when a
  // choice is already stored.
  useEffect(() => {
    if (storedChoice() !== null) {
      document.documentElement.setAttribute("data-theme", theme);
    }
  }, [theme]);

  // Follow the OS preference live while the user has made no explicit choice.
  useEffect(() => {
    if (storedChoice() !== null) return;
    const mql = window.matchMedia?.("(prefers-color-scheme: light)");
    if (!mql) return;
    const onChange = () => {
      if (storedChoice() === null) setThemeRaw(osPreference());
    };
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);

  const setTheme = useCallback((next: Theme) => {
    applyAndPersist(next);
    setThemeRaw(next);
  }, []);

  const toggleTheme = useCallback(() => {
    setThemeRaw((prev) => {
      const next: Theme = prev === "dark" ? "light" : "dark";
      applyAndPersist(next);
      return next;
    });
  }, []);

  return { theme, setTheme, toggleTheme };
}

/** Access the theme context. Throws if used outside a ThemeProvider. */
export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used within a ThemeProvider");
  return ctx;
}
