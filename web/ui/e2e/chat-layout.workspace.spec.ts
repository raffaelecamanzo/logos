// The Workspace Chat's full-height layout (S-619, FR-WS-34, FR-UI-45): the same
// surface as the member chat, held to the same checks. See `chatLayout.ts`.
import { expect, test } from "@playwright/test";

import { expectFullHeightLayout, expectPillRailAndChip, openLongThread, readLayout, VIEWPORTS, WORKSPACE_CHAT } from "./chatLayout.ts";

for (const theme of ["dark", "light"] as const) {
  for (const viewport of VIEWPORTS) {
    test(`${theme} ${viewport.width}px: the page holds still and the transcript is the only scroller`, async ({ page }) => {
      await openLongThread(page, WORKSPACE_CHAT, theme, viewport);
      await expect.poll(async () => (await readLayout(page)).transcriptFromBottom).toBeLessThanOrEqual(2);
      expectFullHeightLayout(await readLayout(page), `${theme} ${viewport.width}px`);

      await page.getByRole("textbox", { name: "Your message" }).fill("one more?");
      await page.getByRole("button", { name: "Send" }).click();
      await expect(page.locator("main#view-root").getByText("one more?")).toBeVisible();
      await expect(page.getByRole("button", { name: "Send" })).toBeVisible();
      await expect.poll(async () => (await readLayout(page)).transcriptFromBottom).toBeLessThanOrEqual(2);
      expectFullHeightLayout(await readLayout(page), `${theme} ${viewport.width}px, after a streamed answer`);
    });
  }
}

for (const theme of ["dark", "light"] as const) {
  test(`${theme}: one pill, a cardless rail, a one-line chip and a remembered collapse`, async ({ page }) => {
    await openLongThread(page, WORKSPACE_CHAT, theme, { width: 1600, height: 900 });
    await expectPillRailAndChip(page, WORKSPACE_CHAT);
  });
}
