import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { InternalResolution, OverviewModel, OverviewSecurity, SecuritySummary } from "../../api/types.ts";
import {
  DASHBOARD_ABSENCE,
  RESOLUTION_LOWER_BOUND,
  activity,
  codeCoverage,
  graph,
  languages,
  qualityIndex,
  securityWidget,
} from "../../copy/dashboard.copy.ts";
import { expectWidgetCopy, readerText } from "../../copy/expectWidgetCopy.ts";
import { ruleFindings } from "../../copy/ruleFindings.copy.ts";
import type { CopyEntry } from "../../copy/types.ts";
import { clearExtensions, registerExtensionUi, setActiveExtensions, type DashboardCardProps } from "../../extensions/registry.ts";
import { extensionsInfo, STUB_ID, stubInfo } from "../../test/extensions.tsx";
import { expectOneWidgetStack, widgetsIn, widgetTitle, widgetTitled } from "../../test/widgetStack.ts";
import { DashboardView } from "./DashboardView.tsx";

// ── The Security widget's stored summary (S-789) ──────────────────────────────
const NOW_UNIX = Math.floor(Date.now() / 1000);

/** A scanned project: 6 counted findings, 2 of 3 scanners fine, newest scan 3 days old. */
const SCANNED: SecuritySummary = {
  scan: "fresh",
  tally: {
    counted: { critical: 0, high: 2, medium: 1, low: 3, info: 0, unrated: 0, total: 6 },
    excluded: { not_fresh: 0, suppressed: 4, test: 2, dev: 0, needs_review: 0 },
  },
  newest_scan_at: NOW_UNIX - 3 * 86_400,
  scanners_failed: 1,
  scanners_total: 3,
  consent: { consented: ["semgrep"], pending: [] },
  gate: null,
};

const GATE_PASS = { passed: true, checked_rules: 2, required_scanners: ["semgrep"], missing_scans: [], violations: [], notes: [] };
const GATE_FAIL = {
  passed: false,
  checked_rules: 2,
  required_scanners: ["semgrep", "bandit"],
  missing_scans: [{ scanner: "bandit", state: "never_run" as const, message: "never run" }],
  violations: [{ rule: "security:ratchet", rule_type: "constraint", severity: "error", file: "app.py", node_id: null, message: "high" }],
  notes: [],
};

const NEVER_SCANNED: SecuritySummary = {
  scan: "never_run",
  tally: null,
  newest_scan_at: null,
  scanners_failed: 0,
  scanners_total: 2,
  consent: { consented: [], pending: ["semgrep"] },
  gate: null,
};

const read = (summary: SecuritySummary): OverviewSecurity => ({ summary, error: null });

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
    // 6,000 of 8,000 internal candidates resolved (75.0%) among 12,000 code
    // references, 4,000 provably external — deliberately not the deprecated 80%.
    internal_resolution: {
      resolved: 6_000,
      internal_candidates: 8_000,
      provably_external: 4_000,
      code_references: 12_000,
      lower_bound: 0.75,
      absence: null,
    },
    doc_resolution: { references: 40, resolved: 34, ratio: 0.85, absence: null },
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
  security: read(SCANNED),
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
beforeEach(() => {
  vi.clearAllMocks();
});
afterEach(clearExtensions);

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
    // Graph compact counts — grouped for readability; Files/Nodes/Edges
    // otherwise unchanged by CR-085.
    expect(screen.getByText("42")).toBeInTheDocument();
    expect(screen.getByText("1,200")).toBeInTheDocument();
    expect(screen.getByText("3,400")).toBeInTheDocument();
    // The resolution headline is the lower bound over internal candidates
    // (S-636), not the deprecated whole-ledger ratio.
    expect(screen.getByText("75.0%")).toBeInTheDocument();
    expect(screen.queryByText(/80\.0%/)).not.toBeInTheDocument();
    // Graph card LOC figures (CR-085): total/source/test from the read-model,
    // digit-grouped, plus a last-full-index caption.
    expect(screen.getByText("54,321")).toBeInTheDocument();
    expect(screen.getByText("43,210")).toBeInTheDocument();
    expect(screen.getByText("11,111")).toBeInTheDocument();
    // Full LOC labels (the compact "Total LOC" wording was expanded on reorder).
    expect(screen.getByText(/Total Lines of Code/i)).toBeInTheDocument();
    expect(screen.getByText(/Source Lines of Code/i)).toBeInTheDocument();
    expect(screen.getByText(/Test Lines of Code/i)).toBeInTheDocument();
    // Both footnotes render; the resolution caveat is the lower-bound explanation.
    expect(screen.getByText(/reflect the last full index/i)).toBeInTheDocument();
    expect(screen.getByText(RESOLUTION_LOWER_BOUND)).toBeInTheDocument();
    expect(screen.queryByText(/partial resolution figure is expected/i)).not.toBeInTheDocument();
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

  it("renders honest per-widget empty states, never fabricated figures", async () => {
    const m = clone();
    m.gate.signal = null; // no scan run → no quality signal
    m.coverage.overall_coverage_bp = null; // no coverage ingested
    m.stats.calls_total = 0; // no telemetry
    m.composition.languages = []; // nothing indexed
    m.status.internal_resolution = NO_CODE_REFERENCES().status.internal_resolution; // no code reference
    m.status.doc_resolution = { references: 0, resolved: 0, ratio: null, absence: ABSENT };
    stub(m);
    render(<DashboardView />);

    expect(await screen.findByText(/No quality signal recorded yet/i)).toBeInTheDocument();
    expect(screen.getByText(DASHBOARD_ABSENCE.noCodeReferences)).toBeInTheDocument();
    expect(screen.getAllByText(/No coverage ingested/i).length).toBeGreaterThanOrEqual(1);
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
    const { container } = render(<DashboardView />);
    // The rest of the Graph card (Files/Nodes/Edges/Resolution) is unchanged.
    expect(await screen.findByText("42")).toBeInTheDocument();
    expect(screen.getByText("1,200")).toBeInTheDocument();
    expect(screen.getByText("3,400")).toBeInTheDocument();
    expect(screen.getByText("75.0%")).toBeInTheDocument();
    expect(screen.getByText(/Lines of code not yet counted/i)).toBeInTheDocument();
    // Never a fabricated `0` LOC figure (scoped to Graph: the Security widget's
    // severity row states its own honest zeros).
    expect(within(widgetTitled(container, "Graph")).queryByText("0")).not.toBeInTheDocument();
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

// ── S-636: the Graph widget's reference-resolution states ────────────────────

/** An absence's `cause` tag: the view reads the counts beside it, not the tag, and
 *  the tags' spellings belong to the sites the absence census pins. */
const ABSENT = { cause: "unread" };

/** `OVERVIEW` with the Graph widget's headline replaced. */
function withResolution(over: Partial<InternalResolution>): OverviewModel {
  const m = clone();
  m.status.internal_resolution = { ...m.status.internal_resolution!, ...over };
  return m;
}

/** The two absences of the headline: no code reference at all, and every one provably external. */
const NO_CODE_REFERENCES = (): OverviewModel =>
  withResolution({
    resolved: 0, internal_candidates: 0, provably_external: 0, code_references: 0,
    lower_bound: null, absence: ABSENT,
  });
const ALL_EXTERNAL = (): OverviewModel =>
  withResolution({
    resolved: 0, internal_candidates: 0, provably_external: 9, code_references: 9,
    lower_bound: null, absence: ABSENT,
  });
const UNREPORTED = (): OverviewModel => {
  const m = clone();
  delete m.status.internal_resolution;
  delete m.status.doc_resolution;
  return m;
};

// ── S-617 (CR-203, FR-UI-39/40): every widget explains itself, in one stack ──

/** Each widget's catalogue entry, by title. */
const ENTRIES: Record<string, CopyEntry> = {
  "Quality index": qualityIndex,
  Security: securityWidget,
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
      "Quality index",
      "Security",
      "Code coverage",
      "Languages",
      "Graph",
      "Activity",
      "Rule findings",
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
      "Quality index",
      "Security",
      "Code coverage",
      "Languages",
      "Graph",
      "Activity",
      "Rule findings",
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
      "Quality index",
      "Security",
      "Code coverage",
      "Languages",
      "Graph",
      "Activity",
      "Rule findings",
    ],
  },
  {
    name: "nothing recorded yet (indexed, but no scan, coverage, telemetry or rules)",
    model: () => {
      const m = clone();
      m.gate.signal = null;
      m.coverage.overall_coverage_bp = null;
      m.stats.calls_total = 0;
      m.composition.languages = [];
      m.status.total_line_count = null;
      m.status.source_line_count = null;
      m.status.test_line_count = null;
      m.rules = { passed: null, checked_rules: 0, rules_present: false, violations: [], freshness: "fresh", warnings: [] };
      m.security = read(NEVER_SCANNED);
      return m;
    },
    expected: [
      "Quality index",
      "Security",
      "Code coverage",
      "Languages",
      "Graph",
      "Activity",
      "Rule findings",
    ],
  },
  {
    name: "the reference resolution headline is absent (no code reference)",
    model: NO_CODE_REFERENCES,
    expected: ["Quality index", "Security", "Code coverage", "Languages", "Graph", "Activity", "Rule findings"],
  },
  {
    name: "the reference resolution headline is absent (every code reference provably external)",
    model: ALL_EXTERNAL,
    expected: ["Quality index", "Security", "Code coverage", "Languages", "Graph", "Activity", "Rule findings"],
  },
  {
    name: "the server reported no reference resolution",
    model: UNREPORTED,
    expected: ["Quality index", "Security", "Code coverage", "Languages", "Graph", "Activity", "Rule findings"],
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

  it.each(STATES)("$name: every widget renders its explanation part — none is a no-explanation entry (CR-208)", async ({ model }) => {
    stub(model());
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Code coverage" });
    const explained = widgetsIn(container).map((w) => [
      widgetTitle(w),
      w.querySelector('[data-widget-part="explanation"]') !== null,
    ]);
    expect(explained).toEqual(STATES[0].expected.map((title) => [title, true]));
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
      ["Quality index", "logos scan"],
      ["Security", "logos security scan --consent"],
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

// ── S-636 (CR-212, FR-RS-49): the Graph widget's reference resolution ─────────

describe("the Graph widget states reference resolution over internal candidates (S-636, FR-RS-49)", () => {
  async function graphWidget(model: OverviewModel): Promise<HTMLElement> {
    stub(model);
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Graph" });
    return widgetTitled(container, "Graph");
  }

  it("leads with internal_resolution's lower bound in the figure row, never 36% of references resolved", async () => {
    const m = clone();
    // The deprecated whole-ledger ratio: 36% of everything, as the owner saw it.
    m.status.resolution_coverage = 0.36;
    m.status.refs_resolved = 36;
    m.status.refs_total = 100;
    const graph = await graphWidget(m);
    const figure = graph.querySelector('[data-widget-part="figure"]')!;
    expect(readerText(figure)).toBe("75.0% of internal candidates resolved");
    expect(readerText(graph)).not.toMatch(/36\.0%|of references resolved|\brefs\b/);
  });

  it("states the population line in its evidence: x internal candidates of y code references, z provably external", async () => {
    const graph = await graphWidget(clone());
    const evidence = graph.querySelector('[data-widget-part="evidence"]')!;
    expect(readerText(evidence)).toContain(
      "6,000 of 8,000 internal candidates resolved — 8,000 internal candidates of 12,000 code references, 4,000 provably external",
    );
  });

  it("states the doc-reference figure apart from the headline", async () => {
    const graph = await graphWidget(clone());
    const evidence = graph.querySelector('[data-widget-part="evidence"]')!;
    expect(readerText(evidence)).toContain("Documentation references: 85.0% resolved (34 of 40)");
    // Apart: the figure row carries the code headline alone.
    expect(readerText(graph.querySelector('[data-widget-part="figure"]')!)).not.toMatch(/Documentation/);
  });

  it("explains the lower bound in one sentence, replacing the partial-figure footnote", async () => {
    const graph = await graphWidget(clone());
    expect(readerText(graph)).toContain(RESOLUTION_LOWER_BOUND);
    expect(RESOLUTION_LOWER_BOUND).toMatch(/method calls on values of unknown type/);
    expect(RESOLUTION_LOWER_BOUND.match(/[.!?](\s|$)/g)).toHaveLength(1);
    expect(readerText(graph)).not.toMatch(/partial resolution figure is expected/i);
  });

  it.each([
    ["no code reference at all", NO_CODE_REFERENCES, DASHBOARD_ABSENCE.noCodeReferences],
    [
      "every code reference provably external",
      ALL_EXTERNAL,
      "All 9 code references are provably external, so none could resolve here and there is no share to state.",
    ],
    ["a server that reported no figure", UNREPORTED, DASHBOARD_ABSENCE.resolutionUnreported],
  ] as const)("renders its absence sentence when there is %s — never 100% or 0%", async (_name, model, sentence) => {
    const graph = await graphWidget(model());
    const absence = graph.querySelector('[data-widget-part="figure"] > [data-widget-absence]')!;
    expect(absence.textContent).toBe(sentence);
    // No share anywhere in the figure row, and no population line to hang one on.
    expect(readerText(graph.querySelector('[data-widget-part="figure"]')!)).not.toMatch(/\d+(\.\d)?%/);
    expect(readerText(graph)).not.toMatch(/(?<![\d.])(100|0)(\.0)?%/);
    expect(readerText(graph)).not.toMatch(/internal candidates of/);
    // No empty population line, and no wrapper left standing for it.
    expect(graph.querySelector("[data-resolution-population]")).toBeNull();
    // The doc line stays only where the server reported a doc figure.
    expect(graph.querySelector("[data-resolution-docs]") === null).toBe(model === UNREPORTED);
    // The structural counts still render: an absent share does not hide the graph's size.
    expect(within(graph).getByText("1,200")).toBeInTheDocument();
  });

  it("states the all-external sentence in the singular and with grouped digits", () => {
    expect(DASHBOARD_ABSENCE.allReferencesExternal(1)).toBe(
      "All 1 code reference is provably external, so none could resolve here and there is no share to state.",
    );
    expect(DASHBOARD_ABSENCE.allReferencesExternal(12_345)).toBe(
      "All 12,345 code references are provably external, so none could resolve here and there is no share to state.",
    );
  });

  it("states a zero lower bound as a measured 0.0%, not an absence", async () => {
    const graph = await graphWidget(withResolution({ resolved: 0, lower_bound: 0 }));
    expect(graph.querySelector("[data-widget-absence]")).toBeNull();
    expect(readerText(graph.querySelector('[data-widget-part="figure"]')!)).toBe("0.0% of internal candidates resolved");
  });

  it("names a graph with no documentation reference instead of a doc ratio", async () => {
    const m = clone();
    m.status.doc_resolution = { references: 0, resolved: 0, ratio: null, absence: ABSENT };
    const graph = await graphWidget(m);
    expect(readerText(graph)).toContain("No documentation references recorded.");
    expect(readerText(graph)).not.toMatch(/Documentation references:/);
  });
});

/** A stub extension's Dashboard card: it renders the overview it is handed. */
function StubCard({ card, overview }: DashboardCardProps) {
  return (
    <section data-testid="stub-card">
      {card.label}: {overview.status.node_count} nodes
    </section>
  );
}

/** Mount the stub with one Dashboard card, as its server would report it. */
function mountStubCard(): void {
  registerExtensionUi({ id: STUB_ID, dashboardCards: { "stub-card": StubCard } });
  setActiveExtensions(
    extensionsInfo(stubInfo({ ui: { nav: [], views: [], config_panels: [], dashboard_cards: [{ id: "stub-card", label: "Stub card" }] } })),
  );
}

describe("the extension card slot (S-644)", () => {
  it("renders a mounted extension's card directly after the three leading widgets, over the overview", async () => {
    mountStubCard();
    stub(OVERVIEW);
    const { container } = render(<DashboardView />);
    const card = await screen.findByTestId("stub-card");
    expect(card).toHaveTextContent("Stub card: 1200 nodes");
    // The slot sits after the three leading widgets — Quality index, Security and
    // Code coverage (S-789) — and ahead of Languages.
    const stack = card.parentElement!;
    const children = [...stack.children];
    expect(children.indexOf(card)).toBe(4);
    const titles = expectOneWidgetStack(container).map(widgetTitle);
    expect(titles.slice(0, 3)).toEqual(["Quality index", "Security", "Code coverage"]);
    expect(titles[3]).toBe("Languages");
  });

  it("is empty in this build: no extension card, and every built-in widget unchanged", async () => {
    stub(OVERVIEW);
    const { container } = render(<DashboardView />);
    await screen.findByText("Quality index");
    expect(screen.queryByTestId("stub-card")).toBeNull();
    expect(expectOneWidgetStack(container).map(widgetTitle)).toEqual(STATES[0].expected);
    expect(screen.queryByText(/project overview|wiki/i)).toBeNull();
  });
});

// ── S-789 (CR-244, FR-UI-09): the Dashboard leads with the Security widget ────

describe("the Security widget (S-789)", () => {
  it("states the counted findings by severity, the age of the newest scan and N of M failed scanners", async () => {
    stub(OVERVIEW);
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    const widget = widgetTitled(container, "Security");
    // The total with its noun, and each severity with its own count — zero states, not omits.
    expect(within(widget).getByText("6")).toBeInTheDocument();
    expect(widget.querySelector('[data-widget-part="figure"]')!.textContent).toContain("6 findings");
    // The findings the counts leave out because they sit in test code.
    expect(within(widget).getByText("2 more in test code")).toBeInTheDocument();
    const counts = Object.fromEntries(
      [...widget.querySelectorAll("[data-severity]")].map((li) => [li.getAttribute("data-severity"), li.querySelector(".num")?.textContent]),
    );
    expect(counts).toEqual({ critical: "0", high: "2", medium: "1", unrated: "0", low: "3", info: "0" });
    // Relative age and the failure ratio, each beside its own label.
    expect(within(widget).getByText("3d ago")).toBeInTheDocument();
    expect(within(widget).getByText("1 of 3 scanners")).toBeInTheDocument();
    const valueOf = (label: string) => within(widget).getByText(label).nextElementSibling?.textContent;
    expect(valueOf("Newest scan")).toBe("3d ago");
    expect(valueOf("Last scan failed")).toBe("1 of 3 scanners");
    expect(valueOf("Gate")).toBe(DASHBOARD_ABSENCE.noSecurityTable);
    // A current scan carries no "not current" note.
    expect(within(widget).queryByText(/not current/)).not.toBeInTheDocument();
    // The detail link.
    expect(within(widget).getByRole("link", { name: /Security/ })).toHaveAttribute("href", "/security");
  });

  it("agrees the noun with the count: 1 finding", async () => {
    const m = clone();
    m.security = read({ ...SCANNED, tally: { counted: { critical: 0, high: 1, medium: 0, low: 0, info: 0, unrated: 0, total: 1 }, excluded: { not_fresh: 0, suppressed: 0, test: 0, dev: 0, needs_review: 0 } } });
    stub(m);
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    const figure = widgetTitled(container, "Security").querySelector('[data-widget-part="figure"]')!.textContent;
    expect(figure).toContain("1 finding");
    expect(figure).not.toContain("1 findings");
    // Nothing in test code: no statement about it.
    expect(within(widgetTitled(container, "Security")).queryByText(/in test code/)).not.toBeInTheDocument();
  });

  it("states how many more findings need review, in the Dashboard's own words, and nothing when none do (S-817)", async () => {
    const excluded = (needs_review: number) => ({ not_fresh: 0, suppressed: 0, test: 0, dev: 0, needs_review });
    const counted = { critical: 0, high: 1, medium: 0, low: 0, info: 0, unrated: 0, total: 1 };
    for (const [n, said] of [[3, "3 more need review"], [1, "1 more needs review"]] as const) {
      const m = clone();
      m.security = read({ ...SCANNED, tally: { counted, excluded: excluded(n) } });
      stub(m);
      const { container, unmount } = render(<DashboardView />);
      await screen.findByRole("heading", { name: "Security" });
      expect(within(widgetTitled(container, "Security")).getByText(said)).toBeInTheDocument();
      unmount();
    }
    const m = clone();
    m.security = read({ ...SCANNED, tally: { counted, excluded: excluded(0) } });
    stub(m);
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    expect(within(widgetTitled(container, "Security")).queryByText(/review/)).not.toBeInTheDocument();
  });

  it("states how many more findings are on a dev-only dependency, in the Dashboard's own words, and nothing when none are (S-810)", async () => {
    const excluded = (dev: number) => ({ not_fresh: 0, suppressed: 0, test: 0, dev, needs_review: 0 });
    const counted = { critical: 0, high: 1, medium: 0, low: 0, info: 0, unrated: 0, total: 1 };
    for (const [n, said] of [[60, "60 more in dev-only dependencies"], [1, "1 more in dev-only dependencies"]] as const) {
      const m = clone();
      m.security = read({ ...SCANNED, tally: { counted, excluded: excluded(n) } });
      stub(m);
      const { container, unmount } = render(<DashboardView />);
      await screen.findByRole("heading", { name: "Security" });
      expect(within(widgetTitled(container, "Security")).getByText(said)).toBeInTheDocument();
      unmount();
    }
    const m = clone();
    m.security = read({ ...SCANNED, tally: { counted, excluded: excluded(0) } });
    stub(m);
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    expect(within(widgetTitled(container, "Security")).queryByText(/dev-only/)).not.toBeInTheDocument();
  });

  it("states that no [security] table is configured, with no verdict badge", async () => {
    stub(OVERVIEW);
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    const widget = widgetTitled(container, "Security");
    expect(within(widget).getByText(DASHBOARD_ABSENCE.noSecurityTable)).toBeInTheDocument();
    expect(within(widget).queryByText("PASS")).not.toBeInTheDocument();
    expect(within(widget).queryByText("FAIL")).not.toBeInTheDocument();
  });

  it("shows the [security] gate verdict as PASS or FAIL with its reasons", async () => {
    const pass = clone();
    pass.security = read({ ...SCANNED, gate: GATE_PASS });
    stub(pass);
    const first = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    let widget = widgetTitled(first.container, "Security");
    expect(within(widget).getByText("PASS")).toBeInTheDocument();
    expect(within(widget).getByText("passes 2 rules")).toBeInTheDocument();
    // The tone follows the verdict: a passing gate is the green badge.
    expect(within(widget).getByText("PASS").className).toMatch(/green/);
    first.unmount();

    const fail = clone();
    fail.security = read({ ...SCANNED, gate: GATE_FAIL });
    stub(fail);
    const second = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    widget = widgetTitled(second.container, "Security");
    expect(within(widget).getByText("FAIL")).toBeInTheDocument();
    expect(within(widget).getByText("fails: 1 finding fail a rule, 1 required scan missing")).toBeInTheDocument();
    // ... and a failing one the red badge.
    expect(within(widget).getByText("FAIL").className).toMatch(/red/);
  });

  it("names logos security scan when never scanned, and --consent when no scanner is consented to", async () => {
    const consented = clone();
    consented.security = read({ ...NEVER_SCANNED, consent: { consented: ["semgrep"], pending: [] } });
    stub(consented);
    const first = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    let widget = widgetTitled(first.container, "Security");
    expect(within(widget).getByText(DASHBOARD_ABSENCE.noSecurityScan).textContent).toMatch(/run logos security scan\.$/);
    expect(within(widget).queryByText(/--consent/)).not.toBeInTheDocument();
    // Never a count of zero where nothing was looked at.
    expect(widget.querySelector("[data-severity-row]")).toBeNull();
    first.unmount();

    const none = clone();
    none.security = read(NEVER_SCANNED);
    stub(none);
    const second = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    widget = widgetTitled(second.container, "Security");
    expect(within(widget).getByText(DASHBOARD_ABSENCE.noSecurityConsent).textContent).toContain("logos security scan --consent");
    expect(widget.querySelector("[data-severity-row]")).toBeNull();
  });

  it("states a read fault with its cause, never as zero findings", async () => {
    const m = clone();
    m.security = { summary: null, error: "invalid .logos/security-ignore.toml: expected `]`" };
    stub(m);
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    const widget = widgetTitled(container, "Security");
    expect(within(widget).getByText(/could not be read/).textContent).toContain("invalid .logos/security-ignore.toml");
    expect(widget.querySelector("[data-severity-row]")).toBeNull();
    expect(within(widget).queryByText("0")).not.toBeInTheDocument();
    // The rest of the Dashboard still renders.
    expect(screen.getByText("Excellent")).toBeInTheDocument();
  });

  it("says when the last scan is not current", async () => {
    const m = clone();
    m.security = read({ ...SCANNED, scan: "stale" });
    stub(m);
    const { container } = render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    expect(within(widgetTitled(container, "Security")).getByText(/the last scan is not current/)).toBeInTheDocument();
  });

  it("loading writes nothing: one GET of the overview and no other request", async () => {
    const fetchMock = vi.fn(() => Promise.resolve({ ok: true, json: () => Promise.resolve(OVERVIEW) } as Response));
    vi.stubGlobal("fetch", fetchMock);
    render(<DashboardView />);
    await screen.findByRole("heading", { name: "Security" });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit | undefined];
    expect(url).toBe("/api/v1/overview");
    expect(init?.method ?? "GET").toBe("GET");
  });
});
