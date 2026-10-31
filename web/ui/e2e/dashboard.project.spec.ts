// The Dashboard's Graph widget in a real browser (S-636, CR-212, FR-RS-49, FR-UI-39):
// the REAL view the tree-built `logos serve --ui` serves at /, read as computed
// style through the shared layout assertions (S-611's `expectWidgetStackLayout`).
//
// The widget leads with `internal_resolution` — the lower bound over internal
// candidates — and states its population line, the doc-reference figure and its
// absence sentence. Two payloads, one view:
//  - the server's own, read from `GET /api/v1/overview`, to hold the served figure
//    to the server's number (the SPA computes no ratio of its own, NFR-MA-02);
//  - the same payload with `internal_resolution` replaced, served for the same
//    path by `page.route`, so each state of the widget renders in the served
//    bundle under the same CSP without a scan or an index of its own.
import { expect, test, type Locator, type Page } from "@playwright/test";

import { expectWidgetStackLayout } from "./layout.ts";

type Overview = { status: Record<string, unknown> } & Record<string, unknown>;

/** The server's own overview payload. */
async function serverOverview(page: Page): Promise<Overview> {
  const response = await page.request.get("/api/v1/overview");
  expect(response.ok(), "GET /api/v1/overview").toBe(true);
  return (await response.json()) as Overview;
}

/** Serve `/api/v1/overview` with `status` fields replaced. */
async function serveStatus(page: Page, over: Record<string, unknown>): Promise<void> {
  const overview = await serverOverview(page);
  overview.status = { ...overview.status, ...over };
  await page.route("**/api/v1/overview", (route) => route.fulfill({ json: overview }));
}

/** Open the Dashboard and return its stack and its Graph widget. */
async function openDashboard(page: Page): Promise<{ stack: Locator; graph: Locator }> {
  await page.goto("/");
  const main = page.locator("main#view-root");
  const stack = main.locator("[data-widget-stack]:visible");
  await expect(stack, "one visible WidgetStack on the Dashboard").toHaveCount(1);
  const graph = stack.locator("[data-widget]").filter({ has: page.getByRole("heading", { name: "Graph", exact: true }) });
  await expect(graph).toHaveCount(1);
  return { stack, graph };
}

/** The Graph widget lays out inside its frame: its population and doc lines
 *  start at the frame's left edge (the figure row's is held by
 *  `expectWidgetStackLayout`), and nothing overflows it. */
async function expectGraphInsideItsFrame(graph: Locator): Promise<void> {
  const m = await graph.evaluate((frame) => {
    const left = frame.getBoundingClientRect().left + parseFloat(getComputedStyle(frame).paddingLeft);
    const offsetOf = (selector: string) => {
      const node = frame.querySelector(selector);
      return node === null ? null : node.getBoundingClientRect().left - left;
    };
    return {
      overflow: frame.scrollWidth - frame.clientWidth,
      population: offsetOf("[data-resolution-population]"),
      docs: offsetOf("[data-resolution-docs]"),
      pageOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    };
  });
  expect(m.overflow, "the Graph widget's content fits its frame").toBeLessThanOrEqual(0);
  expect(m.pageOverflow, "the page does not scroll sideways").toBeLessThanOrEqual(0);
  for (const [name, offset] of [["population", m.population], ["doc", m.docs]] as const) {
    if (offset !== null) expect(offset, `the ${name} line starts at the frame's left edge`).toBeCloseTo(0, 0);
  }
}

test("the served Graph figure is the server's own lower bound over internal candidates", async ({ page }) => {
  const status = (await serverOverview(page)).status as {
    internal_resolution?: { lower_bound: number | null; internal_candidates: number; code_references: number };
  };
  expect(status.internal_resolution, "the server sends internal_resolution (S-635)").toBeDefined();
  const { lower_bound: lowerBound, internal_candidates: candidates, code_references: codeRefs } = status.internal_resolution!;

  const { stack, graph } = await openDashboard(page);
  const figure = graph.locator('[data-widget-part="figure"]');
  if (lowerBound !== null) {
    await expect(figure).toHaveText(`${(lowerBound * 100).toFixed(1)}% of internal candidates resolved`);
    await expect(graph.locator("[data-resolution-population]")).toContainText(
      `${candidates.toLocaleString("en-US")} internal ${candidates === 1 ? "candidate" : "candidates"} of ${codeRefs.toLocaleString("en-US")} code`,
    );
  } else {
    await expect(figure.locator("[data-widget-absence]")).toHaveCount(1);
  }
  await expect(graph).toContainText("method calls on values of unknown type");
  await expect(graph).not.toContainText("A partial resolution figure is expected");
  await expectGraphInsideItsFrame(graph);
  await expectWidgetStackLayout(stack);
});

test("a populated headline states its population and the doc-reference figure, left-aligned in the frame", async ({
  page,
}) => {
  await serveStatus(page, {
    internal_resolution: {
      resolved: 6_000, internal_candidates: 8_000, provably_external: 4_000, code_references: 12_000,
      lower_bound: 0.75, absence: null,
    },
    doc_resolution: { references: 40, resolved: 34, ratio: 0.85, absence: null },
  });
  const { stack, graph } = await openDashboard(page);
  await expect(graph.locator('[data-widget-part="figure"]')).toHaveText("75.0% of internal candidates resolved");
  await expect(graph.locator("[data-resolution-population]")).toHaveText(
    "6,000 of 8,000 internal candidates resolved — 8,000 internal candidates of 12,000 code references, 4,000 provably external",
  );
  await expect(graph.locator("[data-resolution-docs]")).toHaveText("Documentation references: 85.0% resolved (34 of 40)");
  await expectGraphInsideItsFrame(graph);
  await expectWidgetStackLayout(stack);
  // The population line is the longest string in the widget: at phone width it
  // must wrap inside the frame, not run past it.
  await page.setViewportSize({ width: 420, height: 900 });
  await expectGraphInsideItsFrame(graph);
  await expectWidgetStackLayout(stack);
});

test("an absence sentence is a left-aligned statement in the figure row, and wraps inside a narrow frame", async ({
  page,
}) => {
  await serveStatus(page, {
    internal_resolution: {
      resolved: 0, internal_candidates: 0, provably_external: 12_345, code_references: 12_345,
      lower_bound: null, absence: { cause: "no-internal-candidates", provably_external: 12_345 },
    },
  });
  // A phone-width viewport: the longest absence sentence must wrap, not overflow.
  await page.setViewportSize({ width: 420, height: 900 });
  const { stack, graph } = await openDashboard(page);
  const absence = graph.locator('[data-widget-part="figure"] > [data-widget-absence]');
  await expect(absence).toHaveText(/^All 12,345 code references are provably external/);
  await expect(absence).toHaveCSS("text-align", /^(start|left)$/);
  await expect(graph.locator("[data-resolution-population]")).toHaveCount(0);
  await expectGraphInsideItsFrame(graph);
  await expectWidgetStackLayout(stack);
});
