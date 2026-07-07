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

const EMPTY_SMELLS = { label: "", findings: [], not_analyzed: [] };

function model(over: Partial<GapsModel>): GapsModel {
  return {
    status: INDEXED,
    test_gaps: {
      untested: [],
      total_functions: 10,
      covered_functions: 4,
      coverage_ratio: 4000,
      limit: 25,
      truncated: false,
      caveat: "static reachability — not a runtime coverage guarantee",
      freshness: "fresh",
      warnings: [],
      smells: EMPTY_SMELLS,
    },
    rules: { passed: true, checked_rules: 3, rules_present: true, violations: [], freshness: "fresh", warnings: [] },
    ...over,
  };
}

describe("GapsView over mocked /api/v1 (S-189, FR-UI-06)", () => {
  it("renders the honest empty state when the graph is not indexed", async () => {
    stub(model({ status: { ...INDEXED, indexed: false } }));
    render(<GapsView />);
    expect(await screen.findByText(/No index yet/i)).toBeInTheDocument();
  });

  it("renders gaps in the read-model order verbatim (FR-GV-17, never re-sorted)", async () => {
    stub(
      model({
        test_gaps: {
          ...model({}).test_gaps,
          untested: [
            { name: "zzz_hot", file: "src/z.rs", line: 1 },
            { name: "aaa_cold", file: "src/a.rs", line: 2 },
          ],
        },
      }),
    );
    render(<GapsView />);
    const table = await screen.findByRole("table", { name: "Test gaps" });
    const text = within(table).getAllByRole("cell").map((c) => c.textContent);
    // The hot (high blast-radius) gap precedes the cold one, as the read-model ranked them.
    expect(text.indexOf("zzz_hot")).toBeLessThan(text.indexOf("aaa_cold"));
    // The mandatory static-coverage caveat rides above the table (BR-16).
    expect(screen.getByText(/static reachability/i)).toBeInTheDocument();
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
  });

  it("shows the `n/a` coverage badge when the ratio is null", async () => {
    stub(model({ test_gaps: { ...model({}).test_gaps, coverage_ratio: null } }));
    render(<GapsView />);
    expect(await screen.findByText("n/a")).toBeInTheDocument();
  });

  it("paginates the test-gap table at 20 rows/page, keeping the read-model order (S-195, FR-UI-11)", async () => {
    // 25 gaps → the previously-unbounded table now pages at the shared 20, never
    // rendering more than one page, and page 1 holds the read-model order verbatim.
    stub(
      model({
        test_gaps: {
          ...model({}).test_gaps,
          untested: Array.from({ length: 25 }, (_, i) => ({
            name: `fn_${String(i).padStart(2, "0")}`,
            file: "src/m.rs",
            line: i + 1,
          })),
        },
      }),
    );
    render(<GapsView />);
    const table = await screen.findByRole("table", { name: "Test gaps" });
    // 20 body rows + the header row — no page renders more than 20 rows.
    expect(within(table).getAllByRole("row").length).toBe(20 + 1);
    expect(screen.getByText(/Showing 1–20 of 25/)).toBeInTheDocument();
    // Page 1 is the first 20 in read-model order (the worklist ranking, not re-sorted).
    expect(within(table).getByText("fn_00")).toBeInTheDocument();
    expect(within(table).queryByText("fn_20")).not.toBeInTheDocument();
  });
});
