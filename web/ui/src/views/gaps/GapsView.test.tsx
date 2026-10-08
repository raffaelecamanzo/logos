import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { GapsModel, RulesReport } from "../../api/types.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import { ruleFindings } from "../../copy/ruleFindings.copy.ts";
import { actionKind, expectOneWidgetStack } from "../../test/widgetStack.ts";
import { GapsView } from "./GapsView.tsx";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function stub(model: GapsModel) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve({ ok: true, json: () => Promise.resolve(model) } as Response)),
  );
}

const INDEXED = {
  indexed: true,
  file_count: 1,
  node_count: 1,
  edge_count: 1,
  db_path: ".logos/logos.db",
  db_size_bytes: 12288,
  last_full_index_at: "1719600000",
  last_sync_at: null,
  graph_revision: 7,
  refs_total: 10,
  refs_resolved: 10,
  refs_unresolved: 0,
  resolution_coverage: 1,
  total_line_count: null,
  source_line_count: null,
  test_line_count: null,
  freshness: "fresh",
  warnings: [],
};

function model(over: Partial<GapsModel>): GapsModel {
  return {
    status: INDEXED,
    rules: { passed: true, checked_rules: 3, rules_present: true, violations: [], freshness: "fresh", warnings: [] },
    ...over,
  };
}

describe("GapsView → Rule findings over mocked /api/v1 (S-189, FR-UI-06; CR-079)", () => {
  it("renders the honest empty state when the graph is not indexed", async () => {
    stub(model({ status: { ...INDEXED, indexed: false } }));
    render(<GapsView />);
    expect(await screen.findByText(/No index yet/i)).toBeInTheDocument();
  });

  it("leads with the rule-findings count and no longer shows a test-gaps table", async () => {
    stub(model({}));
    render(<GapsView />);
    expect(await screen.findByText("0 rule findings")).toBeInTheDocument();
    // The retired test-gaps table is gone.
    expect(screen.queryByRole("table", { name: "Test gaps" })).not.toBeInTheDocument();
    // Clean state (rules present, no violations).
    expect(screen.getByText("No findings — 3 rules checked")).toBeInTheDocument();
  });

  it("shows the no-rules onboarding empty state, not an always-empty table", async () => {
    // `passed: null` (S-352, FR-GV-22): no contract loaded and nothing else fired —
    // a verdict over an empty evaluated set is not a verdict.
    stub(model({ rules: { passed: null, checked_rules: 0, rules_present: false, violations: [], freshness: "fresh", warnings: [] } }));
    render(<GapsView />);
    expect(await screen.findByText(/No architecture rules yet/i)).toBeInTheDocument();
    expect(screen.getByText(/\[\[forbidden_imports\]\]/)).toBeInTheDocument();
    expect(screen.queryByText(/^No findings/)).not.toBeInTheDocument();
  });

  it("renders the findings table over the onboarding prompt when a violation fires despite no rules.toml (S-354)", async () => {
    // The always-on structural/admission fold-ins (FR-GV-18/FR-GV-20) fire
    // independent of a loaded contract, so `rules_present: false` can still carry
    // a real violation (`passed: Some(false)`, never `None`). Findings must win.
    stub(
      model({
        rules: {
          passed: false,
          checked_rules: 0,
          rules_present: false,
          violations: [
            { rule: "graph-admission-drift", rule_type: "constraint", severity: "error", file: "", node_id: null, message: "unadmitted file" },
          ],
          freshness: "fresh",
          warnings: [],
        },
      }),
    );
    render(<GapsView />);
    const table = await screen.findByRole("table", { name: "Rule findings" });
    expect(within(table).getByText("graph-admission-drift")).toBeInTheDocument();
    expect(screen.queryByText(/No architecture rules yet/i)).not.toBeInTheDocument();
  });

  it("renders rule violations with severity badges (error → red, warning → orange)", async () => {
    stub(
      model({
        rules: {
          passed: false,
          checked_rules: 4,
          rules_present: true,
          violations: [
            { rule: "max_cc", rule_type: "constraint", severity: "error", file: "src/big.rs", node_id: null, message: "too complex" },
            { rule: "layer", rule_type: "layer", severity: "warning", file: "src/dep.rs", node_id: null, message: "wrong layer" },
          ],
          freshness: "fresh",
          warnings: [],
        },
      }),
    );
    render(<GapsView />);
    const table = await screen.findByRole("table", { name: "Rule findings" });
    expect(within(table).getByText("error")).toBeInTheDocument();
    expect(within(table).getByText("warning")).toBeInTheDocument();
    expect(within(table).getByText("max_cc")).toBeInTheDocument();
    // The verdict line reflects the finding count.
    expect(screen.getByText("2 rule findings")).toBeInTheDocument();
    expect(screen.getByText("2 findings across 4 checked rules")).toBeInTheDocument();
  });
});

// ── S-617 (CR-203, FR-UI-39/40): the widget explains itself in each state ────

const RULES: { name: string; rules: RulesReport; state: { findings: number; checked: number }; kind: "act" | "none" }[] = [
  {
    name: "clean over three rules",
    rules: { passed: true, checked_rules: 3, rules_present: true, violations: [], freshness: "fresh", warnings: [] },
    state: { findings: 0, checked: 3 },
    kind: "none",
  },
  {
    name: "findings",
    rules: {
      passed: false,
      checked_rules: 2,
      rules_present: true,
      violations: [{ rule: "layer", rule_type: "layer", severity: "error", file: "src/a.rs", node_id: null, message: "bad" }],
      freshness: "fresh",
      warnings: [],
    },
    state: { findings: 1, checked: 2 },
    kind: "act",
  },
  {
    name: "no rules.toml",
    rules: { passed: null, checked_rules: 0, rules_present: false, violations: [], freshness: "fresh", warnings: [] },
    state: { findings: 0, checked: 0 },
    kind: "act",
  },
  {
    // `logos init`'s default contract declares nothing: not a pass (CR-141).
    name: "a contract declaring no rule",
    rules: { passed: true, checked_rules: 0, rules_present: true, violations: [], freshness: "fresh", warnings: [] },
    state: { findings: 0, checked: 0 },
    kind: "act",
  },
];

describe("the Rule findings widget explains itself (S-617, FR-UI-39/40)", () => {
  it.each(RULES)("$name: one stack, the shared catalogue entry at its state", async ({ rules, state, kind }) => {
    stub(model({ rules }));
    const { container } = render(<GapsView />);
    await screen.findByRole("heading", { name: "Rule findings" });
    const [widget] = expectOneWidgetStack(container);
    expect(actionKind(widget)).toBe(kind);
    expectWidgetCopy(widget, ruleFindings, state);
  });

  it("a contract declaring no rule renders the onboarding, never a PASS", async () => {
    stub(model({ rules: RULES[3].rules }));
    render(<GapsView />);
    expect(await screen.findByText(/No architecture rules yet/i)).toBeInTheDocument();
    expect(screen.queryByText("PASS")).not.toBeInTheDocument();
    expect(screen.getByText(".logos/rules.toml").closest('[data-widget-copy="where"]')).not.toBeNull();
  });
});
