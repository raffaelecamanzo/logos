import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OverviewModel } from "../../api/types.ts";
import {
  DASHBOARD_ABSENCE,
  activity,
  codeCoverage,
  graph,
  languages,
  projectOverview,
  qualityIndex,
} from "../../copy/dashboard.copy.ts";
import { expectWidgetCopy, readerText } from "../../copy/expectWidgetCopy.ts";
import { ruleFindings } from "../../copy/ruleFindings.copy.ts";
import type { CopyEntry } from "../../copy/types.ts";
import { expectOneWidgetStack, widgetsIn, widgetTitle } from "../../test/widgetStack.ts";
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
    total_line_count: 54_321,
    source_line_count: 43_210,
    test_line_count: 11_111,
    freshness: "internal-citation",
    warnings: [],
  },
  composition: { languages: [{ language: "rust", nodes: 1000, files: 30 }, { language: "python", nodes: 200, files: 12 }] },
  languages: { skipped: [{ name: "ocaml", reason: "abi mismatch" }] },
  gate: { passed: true, saved: false, signal: 8600, baseline_signal: 8500, test_function_count: 12, threshold: null, epsilon: 0, freshness: "", message: "", warnings: [] },
  coverage: { head_sha: "abc1234", config_hash: "cfg", formats: ["lcov"], report_count: 1, total_files: 42, fresh_files: 42, stale_files: 0, freshness_bp: 10_000, overall_coverage_bp: 7200, files: [], notice: null, current_head: "abc1234", head_stale: false, staleness_prompt: null },
  rules: { passed: true, checked_rules: 3, rules_present: true, violations: [], freshness: "fresh", warnings: [] },
  stats: { window_days: 7, calls_total: 5, calls_by_tool: [], latency_p50_ms: 3, latency_p95_ms: 9, latency_p99_ms: 12, reads_saved_estimate: 40, tokens_saved_estimate: 100, artifact_bindings: {}, activity_by_day: [], calls_by_origin: [], calls_by_tool_origin: [], calls_by_class: [], attribution_coverage: { raw_events_only: true, requested_window_days: 7, covered_window_days: 7, truncated_by_retention: false, legacy_null_origin_folds_into_main: true, notes: [] }, warnings: [] },
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
    expect(screen.getAllByText("8,600 / 10,000").length).toBeGreaterThanOrEqual(1);
    // Code coverage roll-up reprojects basis points to a percent.
    expect(screen.getAllByText("72.0%").length).toBeGreaterThanOrEqual(1);
    // Languages sized by node count.
    expect(screen.getByText("rust")).toBeInTheDocument();
    expect(screen.getByText("1 grammar skipped at load")).toBeInTheDocument();
    // Graph compact counts — grouped for readability; Files/Nodes/Edges/Resolution
    // otherwise unchanged by CR-085.
    expect(screen.getByText("42")).toBeInTheDocument();
    expect(screen.getByText("1,200")).toBeInTheDocument();
    expect(screen.getByText("3,400")).toBeInTheDocument();
    expect(screen.getByText("80.0% (80 of 100 refs)")).toBeInTheDocument();
    // Graph card LOC figures (CR-085): total/source/test from the read-model,
    // digit-grouped, plus a last-full-index caption.
    expect(screen.getByText("54,321")).toBeInTheDocument();
    expect(screen.getByText("43,210")).toBeInTheDocument();
    expect(screen.getByText("11,111")).toBeInTheDocument();
    // Full LOC labels (the compact "Total LOC" wording was expanded on reorder).
    expect(screen.getByText(/Total Lines of Code/i)).toBeInTheDocument();
    expect(screen.getByText(/Source Lines of Code/i)).toBeInTheDocument();
    expect(screen.getByText(/Test Lines of Code/i)).toBeInTheDocument();
    // Both footnotes render; the resolution caveat moved into the `**` footnote.
    expect(screen.getByText(/reflect the last full index/i)).toBeInTheDocument();
    expect(screen.getByText(/partial resolution figure is expected/i)).toBeInTheDocument();
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

  // FR-EH-04 (CR-130): the Quality index card is gated on `gate.signal`, which the
  // *scan* produces, so it never names `logos index`. It also never names a CAUSE:
  // `OverviewModel` cannot separate "never scanned" from "scanned but nothing in
  // production scope", so the card reports the absence and leaves the why to Health.
  it("names `logos scan` on the Quality index card, and no cause it cannot establish (FR-EH-04)", async () => {
    const m = clone();
    m.gate.signal = null;
    stub(m);
    render(<DashboardView />);
    const absence = await screen.findByText(/No quality signal recorded yet/i);
    // The absence names the command in its sentence (CR-206), and only that one.
    expect(absence.textContent).toBe(DASHBOARD_ABSENCE.noSignal);
    expect(absence.textContent).toMatch(/; run logos scan to record one\.$/);
    expect(readerText(absence.closest("[data-widget]")!)).not.toMatch(/logos index/);
    // Neither of the two causes is asserted, and the graph is never blamed.
    expect(screen.queryByText(/no scan has been run/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/empty graph/i)).not.toBeInTheDocument();
  });

  it("names the command that writes the Project Overview, not the one that lists it (FR-EH-04)", async () => {
    const m = clone();
    m.overview_page = null;
    stub(m);
    render(<DashboardView />);
    const absence = await screen.findByText(/No project overview generated yet/i);
    expect(absence.textContent).toMatch(/logos wiki write overview\/project-overview\.$/);
    expect(readerText(absence.closest("[data-widget]")!)).not.toMatch(/logos wiki status/);
  });

  it("renders honest per-widget empty states, never fabricated figures", async () => {
    const m = clone();
    m.gate.signal = null; // no scan run → no quality signal
    m.coverage.overall_coverage_bp = null; // no coverage ingested
    m.overview_page = null; // not yet generated
    m.stats.calls_total = 0; // no telemetry
    m.composition.languages = []; // nothing indexed
    stub(m);
    render(<DashboardView />);

    expect(await screen.findByText(/No quality signal recorded yet/i)).toBeInTheDocument();
    expect(screen.getAllByText(/No coverage ingested/i).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText(/No project overview generated yet/i)).toBeInTheDocument();
    expect(screen.getByText(/No telemetry yet/i)).toBeInTheDocument();
    expect(screen.getByText(/No languages indexed/i)).toBeInTheDocument();
    // No fabricated percentage when there is no coverage / no signal.
    expect(screen.queryByText(/%$/)).not.toBeInTheDocument();
  });

  it("Graph card LOC rows show an honest empty state when the roll-up is absent, never a fabricated 0 (CR-085, NFR-CC-04)", async () => {
    const m = clone();
    m.status.total_line_count = null;
    m.status.source_line_count = null;
    m.status.test_line_count = null;
    stub(m);
    render(<DashboardView />);
    // The rest of the Graph card (Files/Nodes/Edges/Resolution) is unchanged.
    expect(await screen.findByText("42")).toBeInTheDocument();
    expect(screen.getByText("1,200")).toBeInTheDocument();
    expect(screen.getByText("3,400")).toBeInTheDocument();
    expect(screen.getByText("80.0% (80 of 100 refs)")).toBeInTheDocument();
    expect(screen.getByText(/Lines of code not yet counted/i)).toBeInTheDocument();
    // Never a fabricated `0` LOC figure.
    expect(screen.queryByText("0")).not.toBeInTheDocument();
  });

  it("Rule findings widget — green passing state when rules pass with zero violations", async () => {
    const m = clone();
    m.rules = { passed: true, checked_rules: 5, rules_present: true, violations: [], freshness: "fresh", warnings: [] };
    stub(m);
    render(<DashboardView />);
    expect(await screen.findByText("No findings — 5 rules checked")).toBeInTheDocument();
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
    expect(screen.getByText("2 findings across 4 checked rules")).toBeInTheDocument();
  });

  it("Rule findings widget — muted onboarding state when no rules.toml is authored", async () => {
    const m = clone();
    // `passed: null` (S-352, FR-GV-22): no contract loaded and nothing else fired —
    // a verdict over an empty evaluated set is not a verdict.
    m.rules = { passed: null, checked_rules: 0, rules_present: false, violations: [], freshness: "fresh", warnings: [] };
    stub(m);
    render(<DashboardView />);
    expect(await screen.findByText(/No architecture rules yet/i)).toBeInTheDocument();
    // The absence names the file to author and the evaluating command (CR-206).
    expect(screen.getByText(/No architecture rules yet/i).textContent).toMatch(
      /; declare rules in \.logos\/rules\.toml, then run logos check\.$/,
    );
    // Never a fabricated PASS/FAIL verdict when no rules exist yet.
    expect(screen.queryByText("FAIL")).not.toBeInTheDocument();
  });

  it("Rule findings widget — a violation wins over the onboarding prompt even with no rules.toml (S-354)", async () => {
    // The always-on structural/admission fold-ins (FR-GV-18/FR-GV-20) fire
    // independent of a loaded contract, so `rules_present: false` can still carry
    // a real violation (`passed: Some(false)`, never `None`). The widget must
    // show the finding, not swallow it behind the no-rules onboarding prompt.
    const m = clone();
    m.rules = {
      passed: false,
      checked_rules: 0,
      rules_present: false,
      violations: [
        { rule: "graph-structural-integrity", rule_type: "constraint", severity: "error", file: "", node_id: null, message: "orphan shingle" },
      ],
      freshness: "fresh",
      warnings: [],
    };
    stub(m);
    render(<DashboardView />);
    expect(await screen.findByText("FAIL")).toBeInTheDocument();
    expect(screen.getByText("1 finding across 0 checked rules")).toBeInTheDocument();
    expect(screen.queryByText(/No architecture rules yet/i)).not.toBeInTheDocument();
  });

  it("Rule findings widget — a contract authoring zero rules renders no PASS badge, reusing the onboarding empty state (S-438, CR-141)", async () => {
    // `logos init`'s default: a contract exists (`rules_present: true`) but
    // authors nothing to check. Before S-438 this fell through the final `else`
    // and rendered a green PASS over "No findings — 0 rule(s) checked" — a
    // check over an empty evaluated set is not a pass.
    const m = clone();
    m.rules = { passed: true, checked_rules: 0, rules_present: true, violations: [], freshness: "fresh", warnings: [] };
    stub(m);
    render(<DashboardView />);
    const heading = await screen.findByText("Rule findings");
    const card = heading.closest("section") as HTMLElement;
    // Reuses the one onboarding absence — no fourth state.
    expect(within(card).getByText(/No architecture rules yet/i).textContent).toMatch(/then run logos check\.$/);
    expect(within(card).queryByText("PASS")).not.toBeInTheDocument();
    expect(within(card).queryByText(/No findings/i)).not.toBeInTheDocument();
  });

  it("Rule findings widget — violations on a zero-rule contract still render FAIL, proving findings are checked before the widened onboarding condition (S-354)", async () => {
    // Same vacuous contract as above (`checked_rules: 0`, `rules_present: true`),
    // but with a violation present. If the widened onboarding condition
    // (`!rules_present || checked_rules === 0`) were evaluated before the
    // findings check, this would wrongly render the onboarding prompt instead
    // of the failure.
    const m = clone();
    m.rules = {
      passed: false,
      checked_rules: 0,
      rules_present: true,
      violations: [
        { rule: "graph-structural-integrity", rule_type: "constraint", severity: "error", file: "", node_id: null, message: "orphan shingle" },
      ],
      freshness: "fresh",
      warnings: [],
    };
    stub(m);
    render(<DashboardView />);
    const heading = await screen.findByText("Rule findings");
    const card = heading.closest("section") as HTMLElement;
    expect(within(card).getByText("FAIL")).toBeInTheDocument();
    expect(within(card).getByText("1 finding across 0 checked rules")).toBeInTheDocument();
    expect(within(card).queryByText(/No architecture rules yet/i)).not.toBeInTheDocument();
  });
});

// ── S-617 (CR-203, FR-UI-39/40): every widget explains itself, in one stack ──

/** Each widget's catalogue entry, by title. */
const ENTRIES: Record<string, CopyEntry> = {
  "Project Overview": projectOverview,
  "Quality index": qualityIndex,
  Languages: languages,
  Graph: graph,
  Activity: activity,
  "Rule findings": ruleFindings,
  "Code coverage": codeCoverage,
};

/** Every state the tests below cover, with the widgets each renders, in order. */
const STATES: {
  name: string;
  model: () => OverviewModel;
  expected: string[];
}[] = [
  {
    name: "healthy (everything recorded, rules pass)",
    model: clone,
    expected: [
      "Project Overview",
      "Quality index",
      "Languages",
      "Graph",
      "Activity",
      "Rule findings",
      "Code coverage",
    ],
  },
  {
    name: "problems (gate FAIL, rule findings)",
    model: () => {
      const m = clone();
      m.gate.passed = false;
      m.rules = {
        passed: false,
        checked_rules: 4,
        rules_present: true,
        violations: [{ rule: "layer", rule_type: "layer", severity: "error", file: "src/a.rs", node_id: null, message: "bad" }],
        freshness: "fresh",
        warnings: [],
      };
      return m;
    },
    expected: [
      "Project Overview",
      "Quality index",
      "Languages",
      "Graph",
      "Activity",
      "Rule findings",
      "Code coverage",
    ],
  },
  {
    // The always-on structural fold-ins fire with no contract (S-354): a finding
    // over zero checked rules shows the finding, never the onboarding.
    name: "a fold-in finding with no rules contract",
    model: () => {
      const m = clone();
      m.rules = {
        passed: false,
        checked_rules: 0,
        rules_present: false,
        violations: [{ rule: "graph-structural-integrity", rule_type: "constraint", severity: "error", file: "", node_id: null, message: "orphan shingle" }],
        freshness: "fresh",
        warnings: [],
      };
      return m;
    },
    expected: [
      "Project Overview",
      "Quality index",
      "Languages",
      "Graph",
      "Activity",
      "Rule findings",
      "Code coverage",
    ],
  },
  {
    name: "nothing recorded yet (indexed, but no scan, coverage, telemetry, overview or rules)",
    model: () => {
      const m = clone();
      m.gate.signal = null;
      m.coverage.overall_coverage_bp = null;
      m.overview_page = null;
      m.stats.calls_total = 0;
      m.composition.languages = [];
      m.status.total_line_count = null;
      m.status.source_line_count = null;
      m.status.test_line_count = null;
      m.rules = { passed: null, checked_rules: 0, rules_present: false, violations: [], freshness: "fresh", warnings: [] };
      return m;
    },
    expected: [
      "Project Overview",
      "Quality index",
      "Languages",
      "Graph",
      "Activity",
      "Rule findings",
      "Code coverage",
    ],
  },
];

describe("Dashboard widgets explain themselves (S-617, FR-UI-39/40)", () => {
  it.each(STATES)("$name: one stack, each widget its catalogue entry", async ({ model, expected }) => {
    stub(model());
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Code coverage" });
    const widgets = expectOneWidgetStack(container);
    expect(widgets.map(widgetTitle)).toEqual(expected);
    for (const widget of widgets) expectWidgetCopy(widget, ENTRIES[widgetTitle(widget)]);
  });

  it("states each absence in its widget's figure row, never as a centred empty state", async () => {
    stub(STATES[3].model());
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Code coverage" });
    expect(container.querySelectorAll("[data-widget-absence]")).toHaveLength(6);
    // The one view-level empty state is for an un-indexed root (no widget at all).
    expect(container.querySelector('[class*="empty"]')).toBeNull();
  });

  it("names the command that ends each absence in the absence sentence itself (CR-206)", async () => {
    stub(STATES[3].model());
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Code coverage" });
    const absenceOf = (title: string) =>
      readerText(widgetsIn(container).find((w) => widgetTitle(w) === title)!.querySelector('[data-widget-part="figure"], [data-widget-part="evidence"]')!);
    for (const [title, cmd] of [
      ["Project Overview", "logos wiki write overview/project-overview"],
      ["Quality index", "logos scan"],
      ["Languages", "logos index"],
      ["Activity", "logos stats"],
      ["Rule findings", ".logos/rules.toml, then run logos check"],
      ["Code coverage", "logos coverage ingest <report>"],
    ] as const) {
      expect(absenceOf(title), title).toContain(cmd);
    }
    // Graph's not-yet-counted lines name the full index, in its evidence.
    expect(screen.getByText(DASHBOARD_ABSENCE.noLines)).toBeInTheDocument();
    expect(DASHBOARD_ABSENCE.noLines).toContain("a full logos index counts them");
  });
});
