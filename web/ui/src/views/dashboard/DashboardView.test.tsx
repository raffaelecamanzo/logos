import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OverviewModel } from "../../api/types.ts";
import { DashboardView } from "./DashboardView.tsx";

// ── A fully-populated, indexed overview read-model ────────────────────────────
const OVERVIEW: OverviewModel = {
  status: {
    indexed: true,
    file_count: 42,
    node_count: 1200,
    edge_count: 3400,
    db_path: "/x/.logos",
    db_size_bytes: 1024,
    last_full_index_at: null,
    last_sync_at: null,
    graph_revision: 7,
    refs_total: 100,
    refs_resolved: 80,
    refs_unresolved: 20,
    resolution_coverage: 0.8,
    freshness: "internal-citation",
    warnings: [],
  },
  composition: { languages: [{ language: "rust", nodes: 1000, files: 30 }, { language: "python", nodes: 200, files: 12 }] },
  languages: { skipped: [{ name: "ocaml", reason: "abi mismatch" }] },
  gate: { passed: true, saved: false, signal: 8600, baseline_signal: 8500, test_function_count: 12, threshold: null, epsilon: 0, freshness: "", message: "", warnings: [] },
  coverage: { head_sha: "abc1234", config_hash: "cfg", formats: ["lcov"], report_count: 1, total_files: 42, fresh_files: 42, stale_files: 0, freshness_bp: 10_000, overall_coverage_bp: 7200, files: [], notice: null, current_head: "abc1234", head_stale: false, staleness_prompt: null },
  gaps: { untested: [], total_functions: 90, covered_functions: 60, coverage_ratio: 6400, limit: 100, truncated: false, caveat: "", freshness: "", warnings: [], smells: { label: "", findings: [], not_analyzed: [] } },
  stats: { window_days: 7, calls_total: 5, calls_by_tool: [], latency_p50_ms: 3, latency_p95_ms: 9, latency_p99_ms: 12, reads_saved_estimate: 40, tokens_saved_estimate: 100, artifact_bindings: {}, activity_by_day: [], calls_by_origin: [], warnings: [] },
  overview_page: {
    slug: "overview/project-overview",
    title: "Project Overview",
    body: "# Project Overview\n\nLogos is a structural code-intelligence engine.",
    generator: "agent",
    built_at_revision: 7,
    stale: false,
    has_missing: false,
  },
  cross: {
    head_sha: "abc1234",
    config_hash: "cfg",
    has_fresh_coverage: true,
    symbols: [
      { symbol: "a", name: "a", file: "a.rs", start_line: null, end_line: null, reachable_from_test: true, runtime_exec_bp: 10_000, quadrant: "q4" },
      { symbol: "b", name: "b", file: "b.rs", start_line: null, end_line: null, reachable_from_test: false, runtime_exec_bp: 0, quadrant: "q3" },
    ],
    totals: { q1: 1, q2: 0, q3: 1, q4: 3, na_runtime: 0, total: 5 },
    notice: null,
  },
  hotspots: { tier: "advisory", defect_label: "heuristic", head_sha: "abc1234", config_hash: "cfg", limit: null, degraded: null, untested: false, production_scope: false, coverage_basis: "coverage", coverage_label: null, ranked_files: 2, files: [{ path: "a.rs", score: 3, churn_rank: 1, churn_commits: 5, complexity_rank: 1, complexity: 12, co_change_count: 0, defect_commits: 0, coverage: { state: "n/a", coverage_bp: null } }, { path: "b.rs", score: 1, churn_rank: 2, churn_commits: 2, complexity_rank: 2, complexity: 6, co_change_count: 0, defect_commits: 0, coverage: { state: "n/a", coverage_bp: null } }], notice: null },
};

/** Deep-clone the canned model so a test can null out a field without bleeding. */
function clone(): OverviewModel {
  return JSON.parse(JSON.stringify(OVERVIEW)) as OverviewModel;
}

function stub(model: OverviewModel) {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      expect(url).toBe("/api/v1/overview");
      return Promise.resolve({ ok: true, json: () => Promise.resolve(model) } as Response);
    }),
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
beforeEach(() => vi.clearAllMocks());

describe("DashboardView migration (S-187, FR-UI-09 / FR-UI-21)", () => {
  it("renders the verdict-first roll-up: every figure from the read-model", async () => {
    stub(OVERVIEW);
    render(<DashboardView />);

    // Freshness verdict leads (the honest caveat is always present).
    expect(await screen.findByText(/reflects the last index/i)).toBeInTheDocument();
    // Quality index: BR-34 band + raw signal + PASS badge.
    expect(screen.getByText("Excellent")).toBeInTheDocument();
    expect(screen.getAllByText("8600 / 10000").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("PASS")).toBeInTheDocument();
    // Coverage / test roll-ups reproject basis points to a percent.
    expect(screen.getAllByText("72.0%").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("64.0%").length).toBeGreaterThanOrEqual(1);
    // Languages sized by node count.
    expect(screen.getByText("rust")).toBeInTheDocument();
    expect(screen.getByText(/1 grammar\(s\) skipped/i)).toBeInTheDocument();
    // Graph compact counts.
    expect(screen.getByText("1200")).toBeInTheDocument();
    // Project Overview snippet (markdown reduced to prose).
    expect(screen.getByText(/Logos is a structural code-intelligence engine/i)).toBeInTheDocument();
    // Trust card: the weighted Q4 share (3 / (3+1) = 75.0%) + the mini-quadrant.
    expect(screen.getAllByText("75.0%").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByRole("link", { name: /Coverage quadrant/i })).toBeInTheDocument();
    // The always-visible mini-quadrant legend names each cell's meaning (not hover-only).
    expect(screen.getByText(/false-green/i)).toBeInTheDocument();
    expect(screen.getByText(/true gap/i)).toBeInTheDocument();
  });

  it("shows the single honest empty state (not zeroed roll-ups) when unindexed", async () => {
    const m = clone();
    m.status.indexed = false;
    stub(m);
    render(<DashboardView />);
    expect(await screen.findByText(/No index yet/i)).toBeInTheDocument();
    expect(screen.getByText("logos index")).toBeInTheDocument();
    // None of the roll-up cards render.
    expect(screen.queryByText("Quality index")).not.toBeInTheDocument();
  });

  it("renders honest per-widget empty states, never fabricated figures", async () => {
    const m = clone();
    m.gate.signal = null; // empty graph → no quality signal
    m.coverage.overall_coverage_bp = null; // no coverage ingested
    m.gaps.coverage_ratio = null; // nothing to cover
    m.overview_page = null; // not yet generated
    m.cross.has_fresh_coverage = false;
    m.cross.notice = "no coverage";
    m.stats.calls_total = 0; // no telemetry
    m.composition.languages = []; // nothing indexed
    stub(m);
    render(<DashboardView />);

    expect(await screen.findByText(/No quality signal yet/i)).toBeInTheDocument();
    expect(screen.getAllByText(/No coverage ingested/i).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText(/n\/a — no functions to cover/i)).toBeInTheDocument();
    expect(screen.getByText(/No project overview generated yet/i)).toBeInTheDocument();
    expect(screen.getByText(/No telemetry yet/i)).toBeInTheDocument();
    expect(screen.getByText(/No languages indexed/i)).toBeInTheDocument();
    // No fabricated trust percentage when there is no fresh coverage.
    expect(screen.queryByText(/%$/)).not.toBeInTheDocument();
  });

  it("degrades the trust card honestly when coverage is stale (not absent)", async () => {
    const m = clone();
    m.cross.has_fresh_coverage = false;
    m.cross.notice = null; // stale, not un-ingested
    stub(m);
    render(<DashboardView />);
    expect(await screen.findByText(/Coverage is stale/i)).toBeInTheDocument();
    expect(screen.getByText("logos coverage refresh")).toBeInTheDocument();
  });

  it("names the third trust state honestly when coverage is fresh but nothing is placeable", async () => {
    const m = clone();
    m.cross.has_fresh_coverage = true; // ingested + fresh …
    m.cross.symbols = m.cross.symbols.map((s) => ({ ...s, quadrant: null })); // … but no symbol placed
    stub(m);
    render(<DashboardView />);
    expect(await screen.findByText(/no symbol spans could be placed/i)).toBeInTheDocument();
    // Never a fabricated 0% when nothing can be placed.
    expect(screen.queryByText("0.0%")).not.toBeInTheDocument();
  });
});
