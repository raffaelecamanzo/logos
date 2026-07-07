import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ImpactResult, TraceLink } from "../../api/types.ts";
import { DecisionsPanel } from "./DecisionsPanel.tsx";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const RESOLVED_NO_DOCS: ImpactResult = {
  query: "scip::x",
  resolved: { symbol: "scip::x", name: "lonely", kind: "function", file: "src/x.rs", line: 4 },
  depth: 3,
  upstream_label: "breaks if changed",
  upstream: [],
  downstream_label: "depends on",
  downstream: [],
  docs_label: "documented by",
  docs: [],
  suggestions: [],
  warnings: [],
};

function stubImpact(body: ImpactResult) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response)),
  );
}

describe("DecisionsPanel honest states (NFR-CC-04, FR-NV-09)", () => {
  it("shows the opening prompt when nothing is selected (no fetch)", () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);
    render(<DecisionsPanel seed={null} onFocus={() => {}} />);
    expect(screen.getByText(/Lock a symbol to see the requirements/i)).toBeInTheDocument();
    expect(fetchSpy).not.toHaveBeenCalled(); // a null seed issues no read
  });

  it("names the node and says nothing traces to it yet when docs are empty", async () => {
    stubImpact(RESOLVED_NO_DOCS);
    render(<DecisionsPanel seed="scip::x" onFocus={() => {}} />);
    expect(
      await screen.findByText(/No requirements, ADRs, or stories trace to/i),
    ).toBeInTheDocument();
    // The node is named (in the identity header and the empty message both).
    expect(screen.getAllByText("lonely").length).toBeGreaterThan(0);
    // No decisions table is rendered for a doc-less node.
    expect(screen.queryByRole("table", { name: "Linked decisions and docs" })).not.toBeInTheDocument();
  });

  it("renders an honest error panel when the impact read fails", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve({ ok: false, status: 500 } as Response)));
    render(<DecisionsPanel seed="scip::x" onFocus={() => {}} />);
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
  });

  it("paginates the decisions table at 20 rows/page (S-195, FR-UI-11)", async () => {
    // 25 traced docs → the Decisions & docs table pages at the shared 20, never
    // rendering more than one page (the 0.9.2 panel was unpaginated).
    const docs: TraceLink[] = Array.from({ length: 25 }, (_, i) => ({
      symbol: `scip::doc${i}`,
      name: `FR-UI-${String(i).padStart(2, "0")}`,
      kind: "requirement",
      file: null,
      line: null,
      via: "doc_reference",
    }));
    stubImpact({ ...RESOLVED_NO_DOCS, docs });
    render(<DecisionsPanel seed="scip::x" onFocus={() => {}} />);
    const table = await screen.findByRole("table", { name: "Linked decisions and docs" });
    // 20 body rows + the header row — no page renders more than 20 rows.
    expect(within(table).getAllByRole("row").length).toBe(20 + 1);
    expect(screen.getByText(/Showing 1–20 of 25/)).toBeInTheDocument();
  });
});
