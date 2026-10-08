// The workspace views' widgets in one layout (S-613, CR-203 §3.2 D, FR-UI-40),
// read as COMPUTED style from the real served views — the Workspace Dashboard,
// Workspace Health and the Workspace tab's Cross-service coverage tab — over the
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

test("Workspace Health stacks its cards at one gap, and Workspace rules is laid out as a widget", async ({ page }) => {
  await page.goto("/workspace-health");
  const stack = await stackWith(page, page.locator("main#view-root"), "Workspace rules");
  expect(await widgetTitles(stack)).toEqual(["Workspace rules"]);
  const m = await expectWidgetStackLayout(stack);
  // The callout, then Members answering, Members, Warm state, Workspace rules and
  // Promoted broker topics: every card a child of the one stack.
  expect(m.gaps).toHaveLength(5);
  expect(m.bodySizes).toHaveLength(1);
});

test("the Cross-service coverage tab stacks all five widgets at one gap, the last three included", async ({ page }) => {
  await page.goto("/workspace");
  await page.getByRole("tab", { name: "Cross-service coverage" }).click();
  const panel = page.getByRole("tabpanel");
  const stack = await stackWith(page, panel, "Build dependencies");
  expect(await widgetTitles(stack)).toEqual([
    "Resolved cross-service edges",
    "Spec conformance (declared endpoints vs controllers)",
    "Coverage by intake",
    "Declared contracts and named externals",
    "Build dependencies",
  ]);
  const m = await expectWidgetStackLayout(stack);
  expect(m.gaps).toHaveLength(4);
  // Named, because these are the two gaps CR-203 §3.1 item 10 found at zero:
  // Coverage by intake → Declared contracts → Build dependencies.
  const [, , intakeToDeclared, declaredToBuild] = m.gaps;
  expect(intakeToDeclared, "Coverage by intake → Declared contracts").toBeCloseTo(m.rowGap, 1);
  expect(declaredToBuild, "Declared contracts → Build dependencies").toBeCloseTo(m.rowGap, 1);
});
