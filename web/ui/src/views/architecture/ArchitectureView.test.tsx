import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ArchitectureModel } from "../../api/types.ts";
import { ArchitectureView } from "./ArchitectureView.tsx";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function stub(model: ArchitectureModel) {
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

function withCycle(): ArchitectureModel {
  return {
    status: INDEXED,
    dsm: {
      granularity: "module",
      rows: [{ name: "api", layer: null }, { name: "db", layer: null }],
      // api → db sits ABOVE the diagonal (0→1) = a back-edge / cycle participant.
      matrix: [
        [0, 4],
        [0, 0],
      ],
      freshness: "fresh",
      warnings: [],
    },
  };
}

describe("ArchitectureView over mocked /api/v1 (S-189, FR-UI-06)", () => {
  it("shows the honest empty state when there are no modules (never a blank)", async () => {
    stub({ status: INDEXED, dsm: { granularity: "module", rows: [], matrix: [], freshness: "fresh", warnings: [] } });
    render(<ArchitectureView />);
    expect(await screen.findByText(/No modules to chart/i)).toBeInTheDocument();
    expect(screen.getByText("logos index")).toBeInTheDocument();
  });

  it("leads with a red cycles verdict and lists the back-edge", async () => {
    stub(withCycle());
    render(<ArchitectureView />);
    // The verdict names the cycle count.
    expect(await screen.findByText(/1 cycle \/ layering-violation edge/i)).toBeInTheDocument();
    // The cycle list renders the From → To participants as focus links.
    const cyclesTable = screen.getByRole("table", { name: "Cycles" });
    expect(within(cyclesTable).getByRole("button", { name: "api" })).toBeInTheDocument();
    expect(within(cyclesTable).getByRole("button", { name: "db" })).toBeInTheDocument();
  });

  it("reads an acyclic graph as muted and shows the no-cycles state", async () => {
    const acyclic = withCycle();
    acyclic.dsm.matrix = [
      [0, 0],
      [3, 0],
    ]; // db → api only (below diagonal) — no back-edge
    stub(acyclic);
    render(<ArchitectureView />);
    // The acyclic state is stated in both the verdict band and the cycles card.
    expect((await screen.findAllByText(/No cycles detected/i)).length).toBeGreaterThan(0);
  });

  it("renders the demoted dependency matrix disclosure", async () => {
    stub(withCycle());
    render(<ArchitectureView />);
    expect(await screen.findByText(/Full dependency matrix · 2 modules/i)).toBeInTheDocument();
  });

  it("paginates the cycles table at 20 rows/page (S-195, FR-UI-11)", async () => {
    // 8 modules with every above-diagonal cell non-zero → 8·7/2 = 28 back-edges,
    // so the previously-unpaginated Cycles table caps at 20 rows on page 1.
    const n = 8;
    const matrix = Array.from({ length: n }, (_, i) =>
      Array.from({ length: n }, (_, j) => (i < j ? 1 : 0)),
    );
    stub({
      status: INDEXED,
      dsm: {
        granularity: "module",
        rows: Array.from({ length: n }, (_, i) => ({ name: `mod${i}`, layer: null })),
        matrix,
        freshness: "fresh",
        warnings: [],
      },
    });
    render(<ArchitectureView />);
    const cyclesTable = await screen.findByRole("table", { name: "Cycles" });
    // 20 body rows + the header row — no page renders more than 20 rows.
    expect(within(cyclesTable).getAllByRole("row").length).toBe(20 + 1);
    expect(screen.getByText(/Showing 1–20 of 28/)).toBeInTheDocument();
  });
});
