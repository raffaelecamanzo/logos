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
  // Security is a read of its own (`workspace/security/summary`), so "Members"
  // rendering says nothing about it: wait for it too, or a slower fan-out — another
  // view's, read by the other worker — drops it from the titles below.
  await expect(stack.getByRole("heading", { name: "Security", exact: true })).toBeVisible();
  expect(await widgetTitles(stack)).toEqual([
    "Security",
    "Resolved cross-service edges",
    "Spec conformance (declared endpoints vs controllers)",
    "Coverage by intake",
    "Cross-service reachability",
    "Reference resolution across the workspace",
    "Members",
  ]);
  const m = await expectWidgetStackLayout(stack);
  // The workspace callout and the seven widgets (Security leads, S-790; the
  // workspace reference resolution sits before Members, S-637): seven gaps, all
  // the stack's one.
  expect(m.gaps).toHaveLength(7);
  expect(m.bodySizes).toHaveLength(7);
});

// S-637 (FR-RS-49, FR-UI-36): the workspace reference resolution states its
// pooled figure with its population line and the members it summed over, and the
// Members column reads each member's own figure over its internal candidates —
// over the real two-member fixture, as the served view renders it.
test("the Workspace Dashboard states the pooled reference resolution beside each member's own", async ({ page }) => {
  await page.goto("/workspace-dashboard");
  const stack = await stackWith(page.locator("main#view-root"), "Members");
  const widget = stack
    .locator("section")
    .filter({ has: page.getByRole("heading", { name: "Reference resolution across the workspace", exact: true }) });
  const figure = widget.locator('[data-widget-part="figure"]');
  await expect(figure).toContainText(/\d+\.\d% at least, of the internal candidates resolved|No figure:/);
  await expect(figure).toContainText(/internal candidates of [\d,]+ code references \([\d,]+ provably external\)\./);
  await expect(figure).toContainText("Pooled over 2 of 2 members");
  const members = stack.locator("section").filter({ has: page.getByRole("heading", { name: "Members", exact: true }) });
  for (const member of ["api", "web"]) {
    const row = members.getByRole("row", { name: new RegExp(`^${member}\\b`) });
    await expect(row).toContainText(/internal candidates; [\d,]+ code references, [\d,]+ provably external\)|no code reference recorded|provably external/);
  }
});

// S-790 (FR-UI-36): the Security widget leads the Workspace Dashboard with one row
// per member, each linking to that member's own Security page. The fixture stores
// one report in `api` (one finding, in production code) and none in `web`, so `api`
// reads fresh with that one finding and `web` reads "never run" — not a row of
// zeros — and the figure row counts members, one of two.
test("the Workspace Dashboard's Security widget leads with one row per member", async ({ page }) => {
  await page.goto("/workspace-dashboard");
  const stack = await stackWith(page.locator("main#view-root"), "Security");
  const widget = stack.locator("section").filter({ has: page.getByRole("heading", { name: "Security", exact: true }) });
  await expect(widget.locator('[data-widget-part="figure"]')).toHaveText("1 of 2 members have a fresh scan");
  // The header row and one row per member.
  await expect(widget.getByRole("row")).toHaveCount(3);
  for (const member of ["api", "web"]) {
    const row = widget.getByRole("row", { name: new RegExp(`^${member}\\b`) });
    await expect(row.getByRole("link", { name: member })).toHaveAttribute("href", `/security?repo=${member}`);
  }
  const api = widget.getByRole("row", { name: /^api\b/ });
  await expect(api.getByText("fresh", { exact: true })).toBeVisible();
  const counts = await api.locator("[data-severity] .num").allTextContents();
  expect(counts, "the severity row states every severity").toHaveLength(6);
  expect(counts.map(Number).reduce((a, b) => a + b, 0), "the fixture's one finding").toBe(1);
  const web = widget.getByRole("row", { name: /^web\b/ });
  await expect(web.getByText("never run")).toHaveCount(2);
  await expect(web.locator("[data-severity-row]")).toHaveCount(0);
  // No composite across members: the figure row is the one count of members.
  await expect(widget).not.toContainText(/\b(total|mean|overall)\b/i);
});

// S-796 (FR-SC-16, FR-SC-28): the Workspace Security figure row states the
// findings in test code, which no total counts; the fixture has none.
test("Workspace Security's figure row states the test-code remainder", async ({ page }) => {
  await page.goto("/workspace-security");
  const members = page.locator("section").filter({ has: page.getByRole("heading", { name: "Members", exact: true }) });
  await expect(members.locator('[data-widget-part="figure"]')).toContainText("· 0 in test code, left out of every total above.");
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
