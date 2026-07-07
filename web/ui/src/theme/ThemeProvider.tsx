/*
 * ThemeProvider (S-193, FR-UI-23). Wraps the app so any component can read the
 * effective theme and toggle it via `useTheme()`. The heavy lifting (persistence,
 * attribute sync, OS-follow) lives in theme.ts; this is the context shell.
 */

import type { ReactNode } from "react";

import { ThemeContext, useThemeState } from "./theme.ts";

export function ThemeProvider({ children }: { children: ReactNode }) {
  const value = useThemeState();
  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}
