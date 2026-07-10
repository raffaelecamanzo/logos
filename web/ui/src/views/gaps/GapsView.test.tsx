import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { GapsModel } from "../../api/types.ts";
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
    expect(await screen.findByText(/0 rule finding\(s\)/i)).toBeInTheDocument();
    // The retired test-gaps table is gone.
    expect(screen.queryByRole("table", { name: "Test gaps" })).not.toBeInTheDocument();
    // Clean state (rules present, no violations).
    expect(screen.getByText("No rule findings.")).toBeInTheDocument();
  });

  it("shows the no-rules onboarding empty state, not an always-empty table", async () => {
    stub(model({ rules: { passed: true, checked_rules: 0, rules_present: false, violations: [], freshness: "fresh", warnings: [] } }));
    render(<GapsView />);
    expect(await screen.findByText(/architecture rules are not configured/i)).toBeInTheDocument();
    expect(screen.getByText(/\[\[forbidden_imports\]\]/)).toBeInTheDocument();
    expect(screen.queryByText("No rule findings.")).not.toBeInTheDocument();
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
    expect(screen.getByText(/2 rule finding\(s\)/i)).toBeInTheDocument();
  });
});
