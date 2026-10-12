// The chat turns' bubbles, icon action bar, ↓ control and shortcuts (S-620, CR-204,
// FR-UI-45, FR-UI-46) on the Workspace Chat. See `chatTurns.ts`.
import { test } from "@playwright/test";

import { WORKSPACE_CHAT } from "./chatLayout.ts";
import { expectBubbleAndActionBar, expectScrollToBottom, expectShortcuts } from "./chatTurns.ts";

for (const theme of ["dark", "light"] as const) {
  test(`${theme}: a 24px bubble and 24px icon actions with a hover and focus tint, Copy's check`, async ({ page }) => {
    await expectBubbleAndActionBar(page, WORKSPACE_CHAT, theme);
  });

  test(`${theme}: the ↓ control appears away from the bottom, returns to it, and is absent at the bottom`, async ({ page }) => {
    await expectScrollToBottom(page, WORKSPACE_CHAT, theme);
  });

  test(`${theme}: Enter, Shift+Enter, Esc and Ctrl/⌘K`, async ({ page }) => {
    await expectShortcuts(page, WORKSPACE_CHAT, theme);
  });
}
