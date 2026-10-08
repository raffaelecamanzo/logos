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

test("the attribution notes sit in the explanation at its one body size, muted", async ({ page }) => {
  await page.goto("/statistics");
  const frame = page
    .locator("[data-widget]")
    .filter({ has: page.getByRole("heading", { name: "Tool attribution by class", exact: true }) });
  const note = frame.locator('[data-widget-part="explanation"] [data-widget-note]');
  await expect(note).toBeVisible();
  const sizes = await frame.evaluate((el) => {
    const css = (sel: string) => getComputedStyle(el.querySelector(sel)!);
    return {
      what: css('[data-widget-copy="what"]').fontSize,
      note: css("[data-widget-note] p").fontSize,
      whatColor: css('[data-widget-copy="what"]').color,
      noteColor: css("[data-widget-note] p").color,
    };
  });
  expect(sizes.note, "the note and the explanation share one size").toBe(sizes.what);
  expect(sizes.noteColor, "the note is muted against the explanation").not.toBe(sizes.whatColor);
});
