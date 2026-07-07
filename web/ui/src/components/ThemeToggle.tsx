/*
 * ThemeToggle (S-193, FR-UI-23). A single accessible control that flips the
 * dark/light theme and persists the choice (via useTheme → theme.ts). The icon
 * shows the theme you'd switch TO; the accessible label states the action and
 * `aria-pressed` exposes the current (dark) state to assistive tech.
 */

import { useTheme } from "../theme/theme.ts";
import { Button } from "./Button.tsx";
import { IconMoon, IconSun } from "./icons.tsx";

export function ThemeToggle() {
  const { theme, toggleTheme } = useTheme();
  const toLabel = theme === "dark" ? "Switch to light theme" : "Switch to dark theme";
  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={toggleTheme}
      aria-label={toLabel}
      title={toLabel}
      aria-pressed={theme === "dark"}
    >
      {theme === "dark" ? <IconSun /> : <IconMoon />}
    </Button>
  );
}
