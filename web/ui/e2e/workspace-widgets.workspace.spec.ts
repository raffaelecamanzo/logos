// The workspace views' widgets in one layout (S-613, CR-203 §3.2 D, FR-UI-40),
// read as COMPUTED style from the real served views — the Workspace Dashboard,
// Workspace Health and the Workspace tab's Cross-service coverage tab and (S-614)
// Service map — over the
// two-member workspace fixture. Each view stacks its widgets in one
// `WidgetStack`: consecutive widgets sit one equal gap apart (the coverage tab's
// last three included, the gap CR-203 §3.1 item 10 found missing), every widget
// part is left-aligned, the explanation is set at the body size, and no widget
// renders an action line (CR-206). The bindings filter's inputs share one row
// (CR-208).
import { expect, test, type Locator } from "@playwright/test";

import { expectWidgetStackLayout } from "./layout.ts";
import { widgetTitles } from "./views.ts";

/** The one visible stack on the page, once `title` has rendered in it. */
async function stackWith(scope: Locator, title: string): Promise<Locator> {
  await expect(scope.getByRole("heading", { name: title, exact: true })).toBeVisible();
  const stack = scope.locator("[data-widget-stack]");
  await expect(stack, "one WidgetStack per view").toHaveCount(1);
  return stack;
}

test("the Workspace Dashboard stacks its widgets at one gap, left-aligned, in one body size", async ({ page }) => {
  await page.goto("/workspace-dashboard");
  const stack = await stackWith(page.locator("main#view-root"), "Members");
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
  const stack = await stackWith(page.locator("main#view-root"), "Workspace rules");
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
  const stack = await stackWith(panel, "Build dependencies");
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
  const stack = await stackWith(panel, "Cross-service bindings");
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


// CR-208 item 6 (FR-UI-42): the bindings filter's three inputs sit on one row —
// one top edge and one height — though only the first field has a hint under it.
// The fixture resolves no binding, so the filter is not rendered over it; the
// route-providers answer is served with two bindings instead, one admitted from
// committed configuration so the provenance filter (the third input) exists.
test("the bindings filter's three inputs share one top edge and one height", async ({ page }) => {
  await page.route("**/api/v1/workspace/route-providers*", (route) =>
    route.fulfill({
      json: {
        providers: [
          {
            relation: "route",
            from: { member: "api", symbol: "op" },
            to: { member: "web", symbol: "route" },
            from_value: { provenance: "literal" },
            to_value: { provenance: "literal" },
          },
          {
            relation: "route",
            from: { member: "api", symbol: "op2" },
            to: { member: "web", symbol: "route2" },
            from_value: { provenance: "literal" },
            to_value: {
              provenance: "config-bound",
              bound: [
                {
                  key: "billing.base-url",
                  source: "properties",
                  values: [{ value: "http://web:8080", profiles: [], unprofiled: true, sources: ["application.yml"] }],
                },
              ],
            },
          },
        ],
      },
    }),
  );
  await page.goto("/workspace");
  const filter = page.getByRole("search", { name: "Filter the cross-service bindings" });
  const inputs = filter.locator("input, select");
  await expect(inputs).toHaveCount(3);
  // The first field carries the hint the other two do not: the case that drifted.
  await expect(filter.locator("p")).toHaveCount(1);
  const boxes = await inputs.evaluateAll((els) =>
    els.map((el) => {
      const r = el.getBoundingClientRect();
      return { top: r.top, height: r.height };
    }),
  );
  const [first, ...rest] = boxes;
  for (const box of rest) {
    expect(box.top, "every input's top edge is the first's").toBeCloseTo(first.top, 0);
    expect(box.height, "every input's height is the first's").toBeCloseTo(first.height, 0);
  }
});
