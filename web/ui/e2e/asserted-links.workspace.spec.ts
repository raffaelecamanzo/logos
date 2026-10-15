// The service map's asserted layer in a real browser (S-709, CR-224, FR-WS-41,
// BR-75): a valid `[[links]]` entry is drawn dashed, with its own legend entry,
// and listed in its own widget marked `(asserted)` — never as a binding.
//
// The shared two-member fixture declares no link, and every other workspace
// spec reads it as it is. So this spec takes the REAL `workspace/status` answer
// the tree-built server gives and adds one valid link row and its tally to it —
// the shape `logos-core` emits, pinned there by `tests/workspace_asserted_links.rs`
// — leaving every other route and every other spec untouched. The canvas is an
// ECharts bitmap with no DOM per line; its stroke is pinned at the last point it
// is data (`graphOptions.test.ts`), and the legend swatch read here is the
// rendered statement of the same style.
import { expect, test } from "@playwright/test";

const ASSERTED_EDGE = {
  relation: "route",
  from: { member: "web", symbol: "logos . example.com/web . src/`main.rs`/render()." },
  to: { member: "api", symbol: "logos . example.com/api . src/`main.rs`/route()." },
  intake: "asserted",
  from_value: { provenance: "literal" },
  to_value: { provenance: "literal" },
};

const TALLY =
  "1 asserted edge from 1 declared link (0 invalid; 0 redundant); declared, never counted as evidence";

test("a valid link is drawn dashed on the service map, with a legend entry and its own widget", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (err) => errors.push(err.message));
  await page.route("**/api/v1/workspace/status*", async (route) => {
    const response = await route.fetch();
    const status = await response.json();
    status.links = [
      { relation: "http_call", from: "web::render", to: "api::route", status: "valid", edge: ASSERTED_EDGE },
    ];
    status.coverage.asserted_edges = { valid: 1, invalid: 0, invalid_by_reason: {}, redundant: 0, summary: TALLY };
    await route.fulfill({ response, json: status });
  });

  await page.goto("/workspace");
  await expect(page.getByRole("tab", { name: "Service map" })).toHaveAttribute("aria-selected", "true");
  const panel = page.getByRole("tabpanel");

  // The legend states the layer: its heading, a DASHED swatch in the asserted
  // hue, and the server's tally verbatim.
  const legend = panel.locator("details", { has: page.getByText("Legend", { exact: true }) });
  await expect(legend.getByText("Asserted links", { exact: true })).toBeVisible();
  const swatch = legend.locator("li", { hasText: "Asserted — declared in [[links]], never observed" }).locator("line");
  await expect(swatch).toHaveAttribute("stroke-dasharray", "6 4");
  const computed = await swatch.evaluate((el) => getComputedStyle(el).strokeDasharray);
  expect(computed, "the browser draws the swatch dashed").not.toBe("none");
  await expect(swatch).toHaveAttribute("stroke", "#a16207");
  await expect(legend.getByText(TALLY)).toBeVisible();

  // The canvas the layer is drawn on rendered.
  await expect(panel.locator("canvas").first()).toBeVisible();

  // The accessible twin: one row, the declared link marked (asserted); the
  // bindings widget still states it resolved nothing.
  const card = panel.locator("section", { has: page.getByRole("heading", { name: "Asserted links", exact: true }) });
  await expect(card.getByRole("table")).toContainText("web::render → api::route (asserted)");
  await expect(panel.getByRole("heading", { name: "Cross-service bindings", exact: true })).toBeVisible();

  expect(errors, "no page error").toEqual([]);
});
