// Statistics in a real browser (S-616, FR-UI-40): the five widgets keep the
// widget layout standard. The fixture's own CLI runs (index, hotspots) are
// recorded telemetry, so the view renders its populated state, not awaiting-data.
import { expect, test } from "@playwright/test";

import { expectWidgetStackLayout } from "./layout.ts";

test("the Statistics widgets keep the widget layout standard", async ({ page }) => {
  await page.goto("/statistics");
  await expect(page.getByRole("heading", { name: "Tool attribution by class", exact: true })).toBeVisible();
  const stack = page.locator("[data-widget-stack]");
  await expect(stack.locator(":scope > *")).toHaveCount(5);
  await expectWidgetStackLayout(stack);
});
