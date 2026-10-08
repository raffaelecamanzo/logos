// Files & Risk in a real browser (S-616, FR-UI-44, FR-UI-40): at a 1280-pixel
// viewport both tables fit their widget with no horizontal scrolling on the
// fixture project's first page, an abbreviated path exposes its full path on
// hover and on focus, and the two widgets keep the widget layout standard.
//
// The fixture (e2e/fixtures/single, history from single.history.sh) puts two
// files under a path longer than the PathCell budget and gives them a second
// author, so both tables render and both carry an abbreviated row.
import { expect, test, type Locator } from "@playwright/test";

import { expectWidgetStackLayout } from "./layout.ts";

const BINDER = "src/analysis/structural/resolution/scoped/binder.rs";
const LOOKUP = "src/analysis/structural/resolution/scoped/lookup.rs";

/** The widget frame whose title is `title`. */
function widget(page: import("@playwright/test").Page, title: string): Locator {
  return page.locator("[data-widget]").filter({ has: page.getByRole("heading", { name: title, exact: true }) });
}

test.beforeEach(async ({ page }) => {
  await page.goto("/files");
  await expect(page.getByRole("table", { name: "Files ranked by risk" })).toBeVisible();
});

test("renders at the 1280-pixel viewport the fit criterion names", async ({ page }) => {
  expect(page.viewportSize()?.width).toBe(1280);
});

for (const title of ["Files ranked by risk", "Ownership dispersion"]) {
  test(`${title}: the table fits its widget without horizontal scrolling`, async ({ page }) => {
    const frame = widget(page, title);
    const table = frame.getByRole("table", { name: title });
    await expect(table).toBeVisible();
    // The fixture's long rows are on this page, so the check is not vacuous.
    await expect(table.locator(`[data-path="${BINDER}"]`)).toBeVisible();

    const fit = await table.evaluate((el) => {
      const wrap = el.parentElement!; // DataTable's scroll container
      const card = el.closest("[data-widget]")!;
      const frameRight = card.getBoundingClientRect().right - parseFloat(getComputedStyle(card).paddingRight);
      return {
        wrapScroll: wrap.scrollWidth,
        wrapClient: wrap.clientWidth,
        tableRight: el.getBoundingClientRect().right,
        frameRight,
        pageScroll: document.documentElement.scrollWidth,
        pageClient: document.documentElement.clientWidth,
      };
    });
    expect(fit.wrapScroll, "the table's scroll container does not scroll sideways").toBeLessThanOrEqual(
      fit.wrapClient,
    );
    expect(fit.tableRight, "the table ends inside its widget").toBeLessThanOrEqual(fit.frameRight + 0.5);
    expect(fit.pageScroll, "the page does not scroll sideways").toBeLessThanOrEqual(fit.pageClient);
  });
}

test("an abbreviated path shows its abbreviation and exposes the full path on hover", async ({ page }) => {
  const table = page.getByRole("table", { name: "Files ranked by risk" });
  const cell = table.locator(`[data-path="${BINDER}"]`);
  await expect(cell.locator('[aria-hidden="true"]')).toHaveText("src/…/scoped/binder.rs");
  await expect(table.getByRole("cell", { name: BINDER })).toBeVisible();

  await cell.hover();
  // The element the pointer is over carries the full path as its title: the
  // abbreviation is what the mouse reaches, nothing covers it.
  const hovered = await cell.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return hit?.closest("[title]")?.getAttribute("title") ?? null;
  });
  expect(hovered).toBe(BINDER);
});

test("an abbreviated path shows its full path on keyboard focus", async ({ page }) => {
  const cell = page.getByRole("table", { name: "Ownership dispersion" }).locator(`[data-path="${LOOKUP}"]`);
  const full = cell.locator(":scope > :not([aria-hidden])");
  // Visually hidden until focused: one pixel, clipped.
  expect((await full.boundingBox())?.width ?? 0).toBeLessThanOrEqual(1);
  await cell.focus();
  await expect(cell).toBeFocused();
  await expect(full).toHaveText(LOOKUP);
  expect((await full.boundingBox())?.width ?? 0, "the full path shows as a tip on focus").toBeGreaterThan(50);
});

test("the two widgets keep the widget layout standard", async ({ page }) => {
  await expectWidgetStackLayout(page.locator("[data-widget-stack]"));
});
