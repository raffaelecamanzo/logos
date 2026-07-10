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
  rules: { passed: true, checked_rules: 3, rules_present: true, violations: [], freshness: "fresh", warnings: [] },
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

describe("DashboardView migration (S-187, FR-UI-09 / FR-UI-21; CR-079)", () => {
  it("renders the verdict-first roll-up: every figure from the read-model", async () => {
    stub(OVERVIEW);
    render(<DashboardView />);

    // Freshness verdict leads (the honest caveat is always present).
    expect(await screen.findByText(/reflects the last index/i)).toBeInTheDocument();
    // Quality index: BR-34 band + raw signal + PASS badge.
    expect(screen.getByText("Excellent")).toBeInTheDocument();
    expect(screen.getAllByText("8600 / 10000").length).toBeGreaterThanOrEqual(1);
    // Code coverage roll-up reprojects basis points to a percent.
    expect(screen.getAllByText("72.0%").length).toBeGreaterThanOrEqual(1);
    // Languages sized by node count.
    expect(screen.getByText("rust")).toBeInTheDocument();
    expect(screen.getByText(/1 grammar\(s\) skipped/i)).toBeInTheDocument();
    // Graph compact counts.
    expect(screen.getByText("1200")).toBeInTheDocument();
    // Project Overview snippet (markdown reduced to prose).
    expect(screen.getByText(/Logos is a structural code-intelligence engine/i)).toBeInTheDocument();
    // Rule findings widget: the passing (green) state names the checked-rule count.
    expect(screen.getByText(/No findings/i)).toBeInTheDocument();
    // Both the quality and rule-findings cards carry a PASS badge.
    expect(screen.getAllByText("PASS").length).toBe(2);
    // The retired Coverage-trust card and reachability roll-up cards are gone.
    expect(screen.queryByText("Coverage trust")).not.toBeInTheDocument();
    expect(screen.queryByText("Test coverage")).not.toBeInTheDocument();
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
    m.overview_page = null; // not yet generated
    m.stats.calls_total = 0; // no telemetry
    m.composition.languages = []; // nothing indexed
    stub(m);
    render(<DashboardView />);

    expect(await screen.findByText(/No quality signal yet/i)).toBeInTheDocument();
    expect(screen.getAllByText(/No coverage ingested/i).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText(/No project overview generated yet/i)).toBeInTheDocument();
    expect(screen.getByText(/No telemetry yet/i)).toBeInTheDocument();
    expect(screen.getByText(/No languages indexed/i)).toBeInTheDocument();
    // No fabricated percentage when there is no coverage / no signal.
    expect(screen.queryByText(/%$/)).not.toBeInTheDocument();
  });

  it("Rule findings widget — green passing state when rules pass with zero violations", async () => {
    const m = clone();
    m.rules = { passed: true, checked_rules: 5, rules_present: true, violations: [], freshness: "fresh", warnings: [] };
    stub(m);
    render(<DashboardView />);
    expect(await screen.findByText(/No findings — 5 rule\(s\) checked/i)).toBeInTheDocument();
    // A green PASS badge appears in the rule-findings card (as well as quality).
    expect(screen.getAllByText("PASS").length).toBeGreaterThanOrEqual(1);
  });

  it("Rule findings widget — red failing state naming the violation count", async () => {
    const m = clone();
    m.rules = {
      passed: false,
      checked_rules: 4,
      rules_present: true,
      violations: [
        { rule: "layer", rule_type: "layer", severity: "error", file: "src/a.rs", node_id: null, message: "bad" },
        { rule: "cycles", rule_type: "constraint", severity: "error", file: "src/b.rs", node_id: null, message: "cycle" },
      ],
      freshness: "fresh",
      warnings: [],
    };
    stub(m);
    render(<DashboardView />);
    expect(await screen.findByText("FAIL")).toBeInTheDocument();
    expect(screen.getByText(/2 rule finding\(s\) across 4 checked rule\(s\)/i)).toBeInTheDocument();
  });

  it("Rule findings widget — muted onboarding state when no rules.toml is authored", async () => {
    const m = clone();
    m.rules = { passed: true, checked_rules: 0, rules_present: false, violations: [], freshness: "fresh", warnings: [] };
    stub(m);
    render(<DashboardView />);
    expect(await screen.findByText(/No architecture rules yet/i)).toBeInTheDocument();
    expect(screen.getByText("logos check")).toBeInTheDocument();
    // Never a fabricated PASS/FAIL verdict when no rules exist yet.
    expect(screen.queryByText("FAIL")).not.toBeInTheDocument();
  });
});
