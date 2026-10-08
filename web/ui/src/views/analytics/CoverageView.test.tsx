import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { CoverageModel } from "../../api/types.ts";
import { perFileCoverage, untestedHotspots } from "../../copy/coverageView.copy.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import type { CopyEntry } from "../../copy/types.ts";
import { actionKind, expectOneWidgetStack, widgetTitle } from "../../test/widgetStack.ts";
import { CoverageView } from "./CoverageView.tsx";

const STATUS = { indexed: true, file_count: 9, node_count: 99, edge_count: 80, db_path: ".logos/graph.db", db_size_bytes: 12288, last_full_index_at: "1719600000", last_sync_at: null, graph_revision: 7, refs_total: 120, refs_resolved: 118, refs_unresolved: 2, resolution_coverage: 0.983, total_line_count: null, source_line_count: null, test_line_count: null, freshness: "fresh", warnings: [] };

const UNTESTED_REPORT = {
  tier: "temporal (non-gated, advisory)",
  defect_label: "heuristic",
  head_sha: "head",
  config_hash: "cfg",
  limit: 20,
  ranked_files: 1,
  files: [
    { path: "src/untested.rs", score: 15, churn_rank: 1, churn_commits: 4, complexity_rank: 1, complexity: 9, co_change_count: 0, defect_commits: 0, coverage: { state: "n/a", coverage_bp: null } },
  ],
  degraded: null,
  notice: null,
  untested: true,
  production_scope: false,
  coverage_basis: "coverage",
  coverage_label: null,
};

function populated(): CoverageModel {
  return {
    status: STATUS,
    coverage: {
      head_sha: "abc123",
      config_hash: "cfg",
      formats: ["lcov"],
      report_count: 1,
      total_files: 3,
      fresh_files: 2,
      stale_files: 1,
      freshness_bp: 6667,
      overall_coverage_bp: 7300,
      files: [
        { path: "src/fresh.rs", freshness: "fresh", coverage_bp: 8200, instrumented_lines: 100, covered_lines: 82 },
        { path: "src/stale.rs", freshness: "stale", coverage_bp: null, instrumented_lines: 0, covered_lines: 0 },
      ],
      notice: null,
      current_head: "abc123",
      head_stale: false,
      staleness_prompt: null,
    },
    untested: UNTESTED_REPORT as CoverageModel["untested"],
  };
}

function empty(): CoverageModel {
  const m = populated();
  m.coverage.notice = "no coverage ingested";
  return m;
}

function stubFetch(byUrl: () => unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve({ ok: true, json: () => Promise.resolve(byUrl()) } as Response)),
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("CoverageView (S-188, FR-UI-11)", () => {
  it("renders the coverage status verdict and the untested-hotspots table", async () => {
    stubFetch(populated);
    render(<CoverageView />);
    await screen.findByText("src/untested.rs");
    expect(screen.getByRole("status")).toHaveTextContent("2/3 files fresh");
  });

  it("renders a fresh file as a <meter> + percent and a stale file as a STALE label (never a shifted number)", async () => {
    stubFetch(populated);
    render(<CoverageView />);
    await screen.findByText("src/fresh.rs");
    // the fresh row carries a native meter (accessible bar) anchored to the bp value
    const meter = screen.getByRole("meter");
    expect(meter).toHaveAttribute("value", "8200");
    // the percent shows (in the meter fallback + the figure span)
    expect(screen.getAllByText("82.0%").length).toBeGreaterThanOrEqual(1);
    // the stale row is a label, not a number
    expect(screen.getByText("STALE")).toBeInTheDocument();
  });

  it("shows the honest ingest empty state when no coverage exists", async () => {
    stubFetch(empty);
    render(<CoverageView />);
    expect((await screen.findAllByText(/No coverage ingested/)).length).toBe(2);
    // Both widgets name the ingest command in their where chip.
    expect(screen.getAllByText("logos coverage ingest <report>")).toHaveLength(2);
  });
});

// ── S-617 (CR-203, FR-UI-39/40): each widget explains itself in each state ───

function allFresh(): CoverageModel {
  const m = populated();
  m.coverage.stale_files = 0;
  m.coverage.files = m.coverage.files.filter((f) => f.freshness === "fresh");
  m.untested = { ...m.untested, files: [] };
  return m;
}

const STATES = [
  {
    name: "populated (an untested file, a stale file)",
    model: populated,
    expected: [
      ["Untested hotspots", { ingested: true, files: 1 }, "act"],
      ["Per-file coverage", { ingested: true, stale: 1 }, "act"],
    ],
  },
  {
    name: "all fresh, nothing untested",
    model: allFresh,
    expected: [
      ["Untested hotspots", { ingested: true, files: 0 }, "none"],
      ["Per-file coverage", { ingested: true, stale: 0 }, "none"],
    ],
  },
  {
    name: "no coverage ingested",
    model: empty,
    expected: [
      ["Untested hotspots", { ingested: false, files: 0 }, "act"],
      ["Per-file coverage", { ingested: false, stale: 0 }, "act"],
    ],
  },
] as const;

describe("Coverage widgets explain themselves (S-617, FR-UI-39/40)", () => {
  it.each(STATES)("$name: one stack, each widget its catalogue entry at its state", async ({ model, expected }) => {
    stubFetch(model);
    const { container } = render(<CoverageView />);
    await screen.findByRole("heading", { name: "Per-file coverage" });
    const widgets = expectOneWidgetStack(container);
    expect(widgets.map(widgetTitle)).toEqual(expected.map(([title]) => title));
    for (const [i, [, state, kind]] of expected.entries()) {
      expect(actionKind(widgets[i])).toBe(kind);
      expectWidgetCopy(widgets[i], (i === 0 ? untestedHotspots : perFileCoverage) as CopyEntry<never>, state as never);
    }
  });

  it("states an un-ingested report in each widget's figure row, never as a centred empty state", async () => {
    stubFetch(empty);
    const { container } = render(<CoverageView />);
    await screen.findByRole("heading", { name: "Per-file coverage" });
    expect(container.querySelectorAll("[data-widget-absence]")).toHaveLength(2);
    expect(container.querySelector('[class*="empty"]')).toBeNull();
  });

  it("states the stale files with their denominator in the figure row", async () => {
    stubFetch(populated);
    render(<CoverageView />);
    const heading = await screen.findByRole("heading", { name: "Per-file coverage" });
    const figure = heading.closest("[data-widget]")!.querySelector('[data-widget-part="figure"]');
    expect(figure).toHaveTextContent("73.0% of lines covered · 1 of 3 files stale");
  });
});
