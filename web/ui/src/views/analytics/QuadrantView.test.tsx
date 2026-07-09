import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { QuadrantModel } from "../../api/types.ts";

// Mock the uPlot seam: jsdom has no real canvas, so the imperative chart is
// exercised through a fake instance. The pure point placement is tested directly
// in analyticsModel.test.ts; the chart mount only needs to not throw here.
vi.mock("./uplot.ts", () => ({
  createQuadrantChart: () => ({ setSize: vi.fn(), destroy: vi.fn() }),
}));

import { QuadrantView } from "./QuadrantView.tsx";

const STATUS = { indexed: true, file_count: 9, node_count: 99, edge_count: 80, db_path: ".logos/graph.db", db_size_bytes: 12288, last_full_index_at: "1719600000", last_sync_at: null, graph_revision: 7, refs_total: 120, refs_resolved: 118, refs_unresolved: 2, resolution_coverage: 0.983, freshness: "fresh", warnings: [] };

const xsym = (over: Partial<QuadrantModel["cross"]["symbols"][number]>) => ({
  symbol: over.symbol ?? `scip::${over.name ?? "x"}`,
  name: over.name ?? "x",
  file: over.file ?? "src/x.rs",
  start_line: over.start_line ?? 10,
  end_line: over.end_line ?? 20,
  reachable_from_test: over.reachable_from_test ?? false,
  runtime_exec_bp: over.runtime_exec_bp ?? null,
  quadrant: over.quadrant ?? null,
});

function populated(): QuadrantModel {
  return {
    status: STATUS,
    cross: {
      head_sha: "head",
      config_hash: "cfg",
      has_fresh_coverage: true,
      symbols: [
        xsym({ name: "falsegreen", file: "src/a.rs", reachable_from_test: false, runtime_exec_bp: 5000, quadrant: "q1" }),
        xsym({ name: "trust", file: "src/b.rs", reachable_from_test: true, runtime_exec_bp: 9000, quadrant: "q4" }),
        xsym({ name: "unplaced", file: "src/c.rs", runtime_exec_bp: null, quadrant: null }),
      ],
      totals: { q1: 1, q2: 0, q3: 0, q4: 1, na_runtime: 1, total: 3 },
      notice: null,
    },
    hotspots: {
      tier: "temporal (non-gated, advisory)",
      defect_label: "heuristic",
      head_sha: "head",
      config_hash: "cfg",
      limit: null,
      ranked_files: 0,
      files: [],
      degraded: null,
      notice: null,
      untested: false,
      production_scope: false,
      coverage_basis: "coverage",
      coverage_label: null,
    },
  };
}

function noCoverage(): QuadrantModel {
  const m = populated();
  m.cross.has_fresh_coverage = false;
  m.cross.notice = "no coverage";
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

describe("QuadrantView (S-188, FR-UI-17)", () => {
  it("renders the trust verdict + disagreement count and the 2×2 chart region", async () => {
    stubFetch(populated);
    render(<QuadrantView />);
    await screen.findByRole("table", { name: "Symbols by urgency" });
    const verdict = screen.getByRole("status");
    // weighted Q4 share over the 2 placed symbols (equal weight) = 50.0%
    expect(verdict).toHaveTextContent("Trust 50.0%");
    expect(verdict).toHaveTextContent("1 disagreement symbol(s)");
    // the canvas is role=img with a descriptive label; the table carries the data
    expect(screen.getByRole("img", { name: /2×2 grid/ })).toBeInTheDocument();
  });

  it("renders the accessible urgency table ordered most-dangerous-first (n/a runtime preserved)", async () => {
    stubFetch(populated);
    render(<QuadrantView />);
    const table = await screen.findByRole("table", { name: "Symbols by urgency" });
    const rows = within(table).getAllByRole("row").slice(1); // drop header
    // falsegreen (Q1) leads; the unplaced n/a symbol still appears with muted n/a cells
    expect(within(rows[0]).getByText("falsegreen")).toBeInTheDocument();
    expect(within(table).getAllByText("n/a").length).toBeGreaterThanOrEqual(1);
  });

  it("re-sorts the full dataset on a header click (accessible sortable affordance)", async () => {
    const user = userEvent.setup();
    stubFetch(populated);
    render(<QuadrantView />);
    const table = await screen.findByRole("table", { name: "Symbols by urgency" });
    await user.click(within(table).getByRole("button", { name: /Symbol/ }));
    const rows = within(table).getAllByRole("row").slice(1);
    // ascending by symbol name → "falsegreen" leads
    expect(within(rows[0]).getByText("falsegreen")).toBeInTheDocument();
  });

  it("shows the honest no-fresh-coverage empty state (no scatter, no fabricated score)", async () => {
    stubFetch(noCoverage);
    render(<QuadrantView />);
    expect(await screen.findByText(/No coverage ingested/)).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: /2×2 grid/ })).toBeNull();
  });

  it("shows the stale-coverage empty state when fresh=false and there is no ingest notice", async () => {
    stubFetch(() => {
      const m = populated();
      m.cross.has_fresh_coverage = false;
      m.cross.notice = null; // ingested once, now stale at a different HEAD
      return m;
    });
    render(<QuadrantView />);
    expect(await screen.findByText(/Coverage is stale/)).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: /2×2 grid/ })).toBeNull();
  });
});
