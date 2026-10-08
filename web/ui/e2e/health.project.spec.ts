// Health on the widget frame, in a real browser (S-615, FR-UI-40, FR-UI-43): the
// REAL view the tree-built `logos serve --ui` serves at /health, read as computed
// style through the shared layout assertions (S-611's `expectWidgetStackLayout`).
//
// Two payloads, one view:
//  - the server's own: the single-repository fixture is indexed but never scanned,
//    so Health renders its absent state (Gate, Quality signal, Signal trend);
//  - a populated one, served for `/api/v1/health` by `page.route`, so every widget
//    — the ten dimension widgets with an offender table, a named absence and a
//    not-recorded statement among them — renders in the same served bundle under
//    the same CSP. The fixture is not scanned here because its server is shared
//    with every other project spec, and a scan would change what they see.
import { expect, test, type Page } from "@playwright/test";

import { expectWidgetStackLayout } from "./layout.ts";
import { widgetTitles } from "./views.ts";

const DIMENSIONS = [
  "Modularity", "Acyclicity", "Depth", "Equality", "Redundancy",
  "Nesting", "Conciseness", "Cohesion", "Focus", "Uniqueness",
];

/** A populated `/api/v1/health` payload: a current FAIL, one listed offender, a
 *  Cohesion drop-out, and the not-applicable Modularity of CR-156. */
function populated(recorded: boolean) {
  return {
    status: {
      indexed: true, file_count: 3, node_count: 40, edge_count: 60, db_path: "", db_size_bytes: 0,
      last_full_index_at: "50", last_sync_at: null, graph_revision: 1, refs_total: 0, refs_resolved: 0,
      refs_unresolved: 0, resolution_coverage: 0, total_line_count: null, source_line_count: null,
      test_line_count: null, freshness: "", warnings: [],
    },
    gate: {
      passed: false, saved: false, signal: 7700, baseline_signal: 7800, test_function_count: 4,
      threshold: null, epsilon: 1, freshness: "", message: "", warnings: [],
    },
    scan: {
      signal: 7700,
      freshness: "",
      metrics: {
        modularity: { raw: -0.5, normalized: 0 },
        modularity_not_applicable: { edges: 3, min_edges: 5, reason: "3 of 5 dependency edges — too few for community structure" },
        // Raw values in their own units, as a real scan records them.
        acyclicity: { raw: 1, normalized: 0.5 }, depth: { raw: 3, normalized: 0.73 },
        equality: { raw: 0.4, normalized: 0.6 }, redundancy: { raw: 0.1, normalized: 0.9 },
        nesting: { raw: 0.2, normalized: 0.8 }, conciseness: { raw: 0, normalized: 1 }, cohesion: null,
        focus: { raw: 0, normalized: 1 }, uniqueness: { raw: 0, normalized: 1 },
        thresholds_hash: "abc123", node_count: 40, edge_count: 3, function_count: 30,
        test_function_count: 4, empty: false, aggregate_signal: 7700,
      },
      worst_offenders: {
        recorded,
        nesting: recorded ? [{ name: "deep_fn", file: "src/lib.rs", line: 12, detail: "nesting depth 6" }] : [],
        conciseness: [], cohesion: [], focus: [], uniqueness: [],
      },
      warnings: [],
    },
    evolution: {
      snapshots: [
        { snapshot_id: 1, created_at: 100, commit_sha: "0123456789ab", signal: 7800, signal_delta: null },
        { snapshot_id: 2, created_at: 200, commit_sha: null, signal: 7700, signal_delta: -100 },
      ],
      warnings: [],
    },
  };
}

/** Opens /health, failing on any CSP violation the page reports. */
async function openHealth(page: Page) {
  const violations: string[] = [];
  page.on("console", (msg) => {
    if (/Content Security Policy/i.test(msg.text())) violations.push(msg.text());
  });
  await page.goto("/health");
  const stack = page.locator("main#view-root [data-widget-stack]");
  await expect(stack).toHaveCount(1);
  return { stack, violations };
}

test("the served Health view's absent state lays out as one widget stack", async ({ page }) => {
  const { stack, violations } = await openHealth(page);
  await expect(stack.locator("[data-widget]")).toHaveCount(3);
  expect(await widgetTitles(stack)).toEqual(["Gate", "Quality signal", "Signal trend"]);
  // The fixture is indexed and never scanned: each absence is a statement in its
  // widget's figure row, never a centred EmptyState.
  await expect(stack.locator('[data-widget-part="figure"] [data-widget-absence]')).toHaveCount(3);
  await expectWidgetStackLayout(stack);
  expect(violations).toEqual([]);
});

for (const recorded of [true, false]) {
  test(`every Health widget lays out as one widget stack (offenders ${recorded ? "recorded" : "not recorded"})`, async ({
    page,
  }) => {
    await page.route("**/api/v1/health", (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(populated(recorded)) }),
    );
    const { stack, violations } = await openHealth(page);
    await expect(stack.locator("[data-widget]")).toHaveCount(13);
    expect(await widgetTitles(stack)).toEqual(["Gate", "Quality signal", ...DIMENSIONS, "Signal trend"]);
    if (recorded) {
      await expect(page.getByRole("table", { name: "Worst offenders" })).toHaveCount(1);
    } else {
      await expect(stack.locator('[data-offender-state="not-recorded"]')).toHaveCount(4);
    }
    // The five dimensions with no list state that, and none renders a table.
    await expect(stack.locator('[data-offender-state="unlisted"]')).toHaveCount(5);
    const m = await expectWidgetStackLayout(stack);
    expect(m.gaps).toHaveLength(12);
    expect(violations).toEqual([]);
  });
}
