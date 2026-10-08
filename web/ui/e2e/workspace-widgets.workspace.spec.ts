// The workspace views' widgets in one layout (S-613, CR-203 §3.2 D, FR-UI-40),
// read as COMPUTED style from the real served views — the Workspace Dashboard,
// Workspace Health and the Workspace tab's Cross-service coverage tab and (S-614)
// Service map — over the
// two-member workspace fixture. Each view stacks its widgets in one
// `WidgetStack`: consecutive widgets sit one equal gap apart (the coverage tab's
// last three included, the gap CR-203 §3.1 item 10 found missing), every widget
// part is left-aligned, and explanation and action share one body size.
import { expect, test, type Locator, type Page } from "@playwright/test";

import { expectWidgetStackLayout } from "./layout.ts";

/** The one visible stack on the page, once `title` has rendered in it. */
async function stackWith(page: Page, scope: Locator, title: string): Promise<Locator> {
  await expect(scope.getByRole("heading", { name: title, exact: true })).toBeVisible();
  const stack = scope.locator("[data-widget-stack]");
  await expect(stack, "one WidgetStack per view").toHaveCount(1);
  return stack;
}

/** Every widget's title in the stack, in order (a gloss's tooltip left out). */
async function widgetTitles(stack: Locator): Promise<string[]> {
  return stack.locator(":scope > section [data-widget-part='title'] h3").evaluateAll((nodes) =>
    nodes.map((h) => {
      const clone = h.cloneNode(true) as Element;
      clone.querySelectorAll('[role="tooltip"]').forEach((tip) => tip.remove());
      return (clone.textContent ?? "").replace(/\s+/g, " ").trim();
    }),
  );
}

test("the Workspace Dashboard stacks its widgets at one gap, left-aligned, in one body size", async ({ page }) => {
  await page.goto("/workspace-dashboard");
  const stack = await stackWith(page, page.locator("main#view-root"), "Members");
  expect(await widgetTitles(stack)).toEqual([
    "Resolved cross-service edges",
    "Spec conformance (declared endpoints vs controllers)",
    "Coverage by intake",
    "Cross-service reachability",
    "Members",
  ]);
  const m = await expectWidgetStackLayout(stack);
  // The workspace callout and the five widgets: five gaps, all the stack's one.
  expect(m.gaps).toHaveLength(5);
  expect(m.bodySizes).toHaveLength(5);
});

test("Workspace Health stacks its widgets at one gap, every one laid out as a widget", async ({ page }) => {
  await page.goto("/workspace-health");
  const stack = await stackWith(page, page.locator("main#view-root"), "Workspace rules");
  // S-617 converted the four cards S-613 left beside Workspace rules.
  expect(await widgetTitles(stack)).toEqual([
    "Members answering",
    "Members",
    "Warm state across the workspace",
    "Workspace rules",
    "Broker topics",
  ]);
  const m = await expectWidgetStackLayout(stack);
  // The callout, then the five widgets: every one a child of the one stack.
  expect(m.gaps).toHaveLength(5);
  expect(m.bodySizes).toHaveLength(5);
});

test("the Cross-service coverage tab stacks its widgets at one gap, the last three included", async ({ page }) => {
  await page.goto("/workspace");
  await page.getByRole("tab", { name: "Cross-service coverage" }).click();
  const panel = page.getByRole("tabpanel");
  const stack = await stackWith(page, panel, "Build dependencies");
  // The fixture vendors no spec, so Declared contracts is not rendered (FR-UI-29
  // AC8); Build dependencies states its absence.
  expect(await widgetTitles(stack)).toEqual([
    "Resolved cross-service edges",
    "Spec conformance (declared endpoints vs controllers)",
    "Coverage by intake",
    "Build dependencies",
  ]);
  const m = await expectWidgetStackLayout(stack);
  expect(m.gaps).toHaveLength(3);
  // The last three widgets, named: Build dependencies used to sit outside the
  // coverage boards' wrapper with no gap at all (CR-203 §3.1 item 10).
  const [, specToIntake, intakeToBuild] = m.gaps;
  expect(specToIntake, "Spec conformance → Coverage by intake").toBeCloseTo(m.rowGap, 1);
  expect(intakeToBuild, "Coverage by intake → Build dependencies").toBeCloseTo(m.rowGap, 1);
});

test("the Service map stacks the map and its widgets at one gap, left-aligned, in one body size", async ({ page }) => {
  await page.goto("/workspace");
  // The Service map is the tab the Workspace view opens on.
  await expect(page.getByRole("tab", { name: "Service map" })).toHaveAttribute("aria-selected", "true");
  const panel = page.getByRole("tabpanel");
  const stack = await stackWith(page, panel, "Cross-service bindings");
  // The fixture resolves no cross-service binding, vendors no spec and holds no
  // build manifest, so the bindings widget states that absence in its figure
  // row and is the stack's one widget (S-419/S-461/S-464 gates hold).
  expect(await widgetTitles(stack)).toEqual(["Cross-service bindings"]);
  await expect(stack.locator("[data-widget-absence]")).toBeVisible();
  const m = await expectWidgetStackLayout(stack);
  // The map (canvas, legend and notes) is the stack's first child, the widget
  // its second: one gap, the stack's own — not the map's inner spacing.
  expect(m.gaps).toHaveLength(1);
  expect(m.bodySizes).toHaveLength(1);
});

