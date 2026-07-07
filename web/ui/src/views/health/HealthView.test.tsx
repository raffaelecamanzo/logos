import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { HealthModel, MetricSnapshot, MetricValue } from "../../api/types.ts";
import { HealthView } from "./HealthView.tsx";

function mv(n: number): MetricValue {
  return { raw: n, normalized: n };
}

function metrics(over: Partial<MetricSnapshot> = {}): MetricSnapshot {
  return {
    modularity: mv(0.9),
    acyclicity: mv(0.8),
    depth: mv(0.7),
    equality: mv(0.6),
    redundancy: mv(0.5),
    nesting: mv(0.9),
    conciseness: mv(0.8),
    cohesion: mv(0.6),
    focus: mv(0.6),
    uniqueness: mv(0.7),
    thresholds_hash: "abc123",
    node_count: 120,
    edge_count: 240,
    function_count: 90,
    test_function_count: 12,
    empty: false,
    aggregate_signal: 8000,
    ...over,
  };
}

const HEALTH: HealthModel = {
  status: { indexed: true, file_count: 1, node_count: 1, edge_count: 1, db_path: "", db_size_bytes: 0, last_full_index_at: null, last_sync_at: null, graph_revision: 1, refs_total: 0, refs_resolved: 0, refs_unresolved: 0, resolution_coverage: 0, freshness: "", warnings: [] },
  gate: { passed: true, saved: false, signal: 8000, baseline_signal: 7800, test_function_count: 12, threshold: null, epsilon: 0, freshness: "", message: "", warnings: [] },
  scan: {
    signal: 8000,
    freshness: "",
    metrics: metrics(),
    worst_offenders: {
      nesting: [{ name: "deep_fn", file: "src/a.rs", line: 42, detail: "nesting depth 6" }],
      conciseness: [],
      cohesion: [],
      focus: [],
      uniqueness: [],
    },
    warnings: [],
  },
  evolution: {
    snapshots: [
      { snapshot_id: 1, created_at: 100, commit_sha: "0123456789ab", signal: 7800, signal_delta: null },
      { snapshot_id: 2, created_at: 200, commit_sha: null, signal: 8000, signal_delta: 200 },
    ],
    warnings: [],
  },
};

function clone(): HealthModel {
  return JSON.parse(JSON.stringify(HEALTH)) as HealthModel;
}

function stub(model: HealthModel) {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      expect(url).toBe("/api/v1/health");
      return Promise.resolve({ ok: true, json: () => Promise.resolve(model) } as Response);
    }),
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
beforeEach(() => vi.clearAllMocks());

describe("HealthView migration (S-187, FR-UI-04 / FR-UI-21)", () => {
  it("leads with the gate verdict band (PASS + current vs baseline)", async () => {
    stub(HEALTH);
    render(<HealthView />);
    expect(await screen.findByText("PASS")).toBeInTheDocument();
    expect(screen.getByText(/current 8000 vs baseline 7800/i)).toBeInTheDocument();
  });

  it("names a missing baseline honestly in the gate band", async () => {
    const m = clone();
    m.gate.baseline_signal = null;
    stub(m);
    render(<HealthView />);
    expect(await screen.findByText(/current 8000 vs baseline no baseline/i)).toBeInTheDocument();
  });

  it("renders the quality grid, aggregate, and the folded structural drill-downs", async () => {
    stub(HEALTH);
    render(<HealthView />);
    // The aggregate signal.
    expect(await screen.findByText("/ 10000")).toBeInTheDocument();
    // The accessible metric grid.
    const grid = screen.getByRole("table", { name: "Quality metrics" });
    expect(within(grid).getByText("Modularity")).toBeInTheDocument();
    expect(within(grid).getByText("Uniqueness")).toBeInTheDocument();
    // The Nesting drill-down is open with its worst offender.
    expect(screen.getByText(/1 flagged/i)).toBeInTheDocument();
    const offenders = screen.getByRole("table", { name: "Worst offenders" });
    expect(within(offenders).getByText("deep_fn")).toBeInTheDocument();
    expect(within(offenders).getByText("src/a.rs")).toBeInTheDocument();
    // The applicable-but-unflagged dimensions show the honest "none flagged" middle state.
    expect(screen.getAllByText(/none flagged/i).length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText(/No offenders flagged within thresholds/i).length).toBeGreaterThanOrEqual(1);
    // The aggregate-scope provenance (FR-QM-14) traces to the read-model.
    expect(screen.getByText(/90 production functions/i)).toBeInTheDocument();
    expect(screen.getByText(/thresholds abc123/i)).toBeInTheDocument();
    // Every structural drill-down is rendered open (no-JS readable).
    const details = document.querySelectorAll("details");
    expect(details.length).toBe(5);
    expect([...details].every((d) => d.hasAttribute("open"))).toBe(true);
    // The non-gated tier points to Files & Risk (no second copy of that table).
    expect(screen.getByRole("link", { name: /Files & Risk/i })).toHaveAttribute("href", "/files");
  });

  it("renders an ADR-21 metric drop-out as a muted n/a, never a fabricated zero", async () => {
    const m = clone();
    m.scan.metrics.cohesion = null;
    m.scan.metrics.focus = null;
    stub(m);
    render(<HealthView />);
    const grid = await screen.findByRole("table", { name: "Quality metrics" });
    // The Cohesion + Focus rows render n/a badges (no 0.00 figure fabricated).
    expect(within(grid).getAllByText("n/a").length).toBeGreaterThanOrEqual(2);
    // The Cohesion drill-down explains the drop-out, with no offenders table.
    expect(screen.getAllByText(/no applicable construct in this codebase/i).length).toBeGreaterThanOrEqual(2);
  });

  it("renders the evolution trend table oldest-first with signed deltas", async () => {
    stub(HEALTH);
    render(<HealthView />);
    const trend = await screen.findByRole("table", { name: "Signal evolution" });
    expect(within(trend).getByText("012345678")).toBeInTheDocument(); // abbreviated sha
    expect(within(trend).getByText("+200")).toBeInTheDocument(); // signed delta
    expect(within(trend).getAllByText("—").length).toBeGreaterThanOrEqual(1); // delta-less point + commit-less snapshot
  });

  it("paginates the signal trend at 20 rows/page (shared page size, FR-UI-11)", async () => {
    const m = clone();
    m.evolution.snapshots = Array.from({ length: 30 }, (_, i) => ({
      snapshot_id: i + 1,
      created_at: (i + 1) * 100,
      commit_sha: null,
      signal: 7000 + i,
      signal_delta: i === 0 ? null : 1,
    }));
    stub(m);
    render(<HealthView />);
    const trend = await screen.findByRole("table", { name: "Signal evolution" });
    expect(within(trend).getAllByRole("row").length).toBe(20 + 1); // 20 body rows + header
    expect(screen.getByText(/Showing 1–20 of 30/)).toBeInTheDocument();
  });

  it("renders honest empty states for an empty graph and no snapshots", async () => {
    const m = clone();
    m.gate.signal = null;
    m.scan.metrics.empty = true;
    m.evolution.snapshots = [];
    stub(m);
    render(<HealthView />);
    expect(await screen.findByText(/n\/a — empty graph/i)).toBeInTheDocument();
    expect(screen.getByText(/No metrics yet/i)).toBeInTheDocument();
    expect(screen.getByText(/No snapshots yet/i)).toBeInTheDocument();
    expect(screen.getByText("logos scan")).toBeInTheDocument();
  });
});
