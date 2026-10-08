import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ArchitectureModel } from "../../api/types.ts";
import { cycles, dependencyMatrix } from "../../copy/architecture.copy.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import { removeHiddenWidgetEntry } from "../../test/hiddenWidgets.ts";
import { actionKind, expectOneWidgetStack, widgetTitle } from "../../test/widgetStack.ts";
import { ArchitectureView } from "./ArchitectureView.tsx";
import { MATRIX_MODULE_THRESHOLD } from "./dsmModel.ts";

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
  total_line_count: null,
  source_line_count: null,
  test_line_count: null,
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

  it("renders the demoted dependency matrix disclosure", async () => {
    stub(withCycle());
    render(<ArchitectureView />);
    expect(await screen.findByText(/Full dependency matrix · 2 modules/i)).toBeInTheDocument();
  });

  // S-612 (FR-UI-41): the CYCLES band and the cycle list are hidden through the
  // register. The Dependency matrix stays, and its back-edge cells keep their ↺.
  it("renders no CYCLES band and no cycle list, while the matrix keeps its cycle cells", async () => {
    stub(withCycle());
    render(<ArchitectureView />);
    expect(await screen.findByText(/Full dependency matrix · 2 modules/i)).toBeInTheDocument();
    expect(screen.queryByText("CYCLES")).toBeNull();
    expect(screen.queryByText(/cycle \/ layering-violation edge/i)).toBeNull();
    expect(screen.queryByRole("table", { name: "Cycles" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Cycles" })).toBeNull();
    // The api → db back-edge cell is still outlined and glyphed in the matrix.
    const backEdge = screen.getByTitle(/back-edge: 4 dependency\(ies\) against layer order/);
    expect(backEdge).toHaveTextContent("↺");
  });

  it("leads with the Dependency matrix when the graph is acyclic too", async () => {
    const acyclic = withCycle();
    acyclic.dsm.matrix = [
      [0, 0],
      [3, 0],
    ];
    stub(acyclic);
    render(<ArchitectureView />);
    expect(await screen.findByText(/Full dependency matrix · 2 modules/i)).toBeInTheDocument();
    expect(screen.queryByText(/No cycles detected/i)).toBeNull();
  });

  it("no longer points the collapsed matrix's note at a cycle list", async () => {
    // Past the threshold the disclosure stays collapsed and explains why. It used
    // to call "the cycle list above" the actionable view — a list no longer drawn.
    const n = MATRIX_MODULE_THRESHOLD + 1;
    stub({
      status: INDEXED,
      dsm: {
        granularity: "module",
        rows: Array.from({ length: n }, (_, i) => ({ name: `mod${i}`, layer: null })),
        matrix: Array.from({ length: n }, () => Array.from({ length: n }, () => 0)),
        freshness: "fresh",
        warnings: [],
      },
    });
    render(<ArchitectureView />);
    const note = await screen.findByText(/the matrix is unreadable at this size/i);
    expect(note.textContent).not.toMatch(/cycle list/i);
  });
});

describe("ArchitectureView with the cycles register entry removed (S-612, FR-UI-41)", () => {
  let restore: () => void;
  beforeEach(() => {
    restore = removeHiddenWidgetEntry("architecture-cycles");
  });
  afterEach(() => restore());

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

// ── S-617 (CR-203, FR-UI-39/40): the widgets explain themselves ──────────────

function acyclicModel(): ArchitectureModel {
  const m = withCycle();
  m.dsm.matrix = [
    [0, 0],
    [3, 0],
  ];
  return m;
}

describe("the Architecture widgets explain themselves (S-617, FR-UI-39/40)", () => {
  it.each([
    ["a cycle", withCycle, 1, "act"],
    ["acyclic", acyclicModel, 0, "none"],
  ] as const)("Dependency matrix, %s: its catalogue entry, alone in the view's one stack", async (_n, build, backEdges, kind) => {
    stub(build());
    const { container } = render(<ArchitectureView />);
    await screen.findByText(/Full dependency matrix · 2 modules/i);
    const [matrix, ...rest] = expectOneWidgetStack(container);
    expect(rest).toEqual([]);
    expect(widgetTitle(matrix)).toBe("Dependency matrix");
    expect(actionKind(matrix)).toBe(kind);
    expectWidgetCopy(matrix, dependencyMatrix, { backEdges });
    expect(matrix.querySelector('[data-widget-part="figure"]')).toHaveTextContent(
      // A count of module pairs (cells marked ↺), not of the dependencies in them:
      // the one ↺ cell here holds 4.
      `2 modules · ${backEdges} ${backEdges === 1 ? "module pair" : "module pairs"} against layer order`,
    );
  });

  describe("with the cycles register entry removed", () => {
    let restore: () => void;
    beforeEach(() => {
      restore = removeHiddenWidgetEntry("architecture-cycles");
    });
    afterEach(() => restore());

    it.each([
      ["a cycle", withCycle, 1, "act"],
      ["acyclic", acyclicModel, 0, "none"],
    ] as const)("Cycles, %s: returns already explained, in the same stack", async (_n, build, backEdges, kind) => {
      stub(build());
      const { container } = render(<ArchitectureView />);
      await screen.findByText(/Full dependency matrix · 2 modules/i);
      const widgets = expectOneWidgetStack(container);
      expect(widgets.map(widgetTitle)).toEqual(["Cycles", "Dependency matrix"]);
      expect(actionKind(widgets[0])).toBe(kind);
      expectWidgetCopy(widgets[0], cycles, { backEdges });
    });
  });
});
