// The member chat's full-height layout (S-619, CR-204 L2, FR-UI-45) at 1024, 1600
// and 2560px in both themes, on a 200-message thread. See `chatLayout.ts`.
import { expect, test } from "@playwright/test";

import { expectFullHeightLayout, expectPillRailAndChip, MEMBER_CHAT, openLongThread, readLayout, VIEWPORTS } from "./chatLayout.ts";

for (const theme of ["dark", "light"] as const) {
  for (const viewport of VIEWPORTS) {
    test(`${theme} ${viewport.width}px: the page holds still and the transcript is the only scroller`, async ({ page }) => {
      await openLongThread(page, MEMBER_CHAT, theme, viewport);
      await expect.poll(async () => (await readLayout(page)).transcriptFromBottom).toBeLessThanOrEqual(2);
      expectFullHeightLayout(await readLayout(page), `${theme} ${viewport.width}px`);

      // A new answer streamed into the bottom of the thread is followed there.
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
    await openLongThread(page, MEMBER_CHAT, theme, { width: 1600, height: 900 });
    await expectPillRailAndChip(page, MEMBER_CHAT);
  });
}
