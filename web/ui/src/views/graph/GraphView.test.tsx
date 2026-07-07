import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  GraphElements,
  ImpactResult,
  QueryResponse,
} from "../../api/types.ts";

// Mock the ECharts seam: jsdom has no real canvas, so the canvas is exercised
// through a fake instance that captures its event handlers (so a test can fire a
// node click) and records setOption calls. The pure option-building is tested
// directly in graphOptions.test.ts.
const captured: { handlers: Record<string, (p: unknown) => void>; setOption: ReturnType<typeof vi.fn> } = {
  handlers: {},
  setOption: vi.fn(),
};
vi.mock("./echarts.ts", () => ({
  createGraphChart: () => ({
    setOption: (...args: unknown[]) => captured.setOption(...args),
    on: (event: string, handler: (p: unknown) => void) => {
      captured.handlers[event] = handler;
    },
    dispatchAction: vi.fn(),
    resize: vi.fn(),
    dispose: vi.fn(),
  }),
}));

import { GraphView } from "./GraphView.tsx";

// ── Canned /api/v1 read-models ────────────────────────────────────────────────
const GRAPH: GraphElements = {
  seed: null,
  granularity: "symbol",
  cap: 250,
  total_nodes: 3,
  total_edges: 2,
  elided_nodes: 4,
  elided_edges: 1,
  nodes: [
    { id: "scip::a", label: "alpha", kind: "function", layer: "code" },
    { id: "scip::b", label: "beta", kind: "struct", layer: "code" },
    { id: "scip::r", label: "FR-UI-08", kind: "requirement", layer: "doc" },
  ],
  edges: [
    { source: "scip::a", target: "scip::b", edge_type: "calls" },
    { source: "scip::r", target: "scip::a", edge_type: "doc_reference" },
  ],
  warnings: [],
};

const EMPTY_GRAPH: GraphElements = { ...GRAPH, total_nodes: 0, total_edges: 0, elided_nodes: 0, elided_edges: 0, nodes: [], edges: [] };

// A seed-scoped snapshot the Expand fetch returns: it admits one previously-elided
// neighbour (delta = 1 node, 1 edge) so the "N more not shown" counter draws down.
const EXPANDED: GraphElements = {
  ...GRAPH,
  seed: "scip::a",
  nodes: [...GRAPH.nodes, { id: "scip::d", label: "delta", kind: "function", layer: "code" }],
  edges: [...GRAPH.edges, { source: "scip::a", target: "scip::d", edge_type: "calls" }],
};

const IMPACT: ImpactResult = {
  query: "scip::a",
  resolved: { symbol: "scip::a", name: "alpha", kind: "function", file: "src/a.rs", line: 10 },
  depth: 3,
  upstream_label: "breaks if changed",
  upstream: [],
  downstream_label: "depends on",
  downstream: [],
  docs_label: "documented by",
  docs: [{ symbol: "scip::r", name: "FR-UI-08", kind: "requirement", file: null, line: null, via: "doc_reference" }],
  suggestions: [],
  warnings: [],
};

const QUERY: QueryResponse = {
  mode: "filter",
  query: "“alpha”",
  hits: [{ id: "scip::a", label: "alpha", kind: "function", layer: "code", file: "src/a.rs", line: 10, rank: 1 }],
  total: 1,
  note: null,
  suggestions: [],
  warnings: [],
};

/** Route a stubbed same-origin GET to its canned read-model; record the URLs. A
 *  seed-scoped graph fetch (Expand/focus on `scip::a`) returns the EXPANDED snapshot. */
function stubApi(graph: GraphElements = GRAPH) {
  const urls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      urls.push(url);
      const body = url.startsWith("/api/v1/impact")
        ? IMPACT
        : url.startsWith("/api/v1/query")
          ? QUERY
          : url.includes("seed=scip%3A%3Aa")
            ? EXPANDED
            : graph;
      return Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response);
    }),
  );
  return urls;
}

beforeEach(() => {
  captured.handlers = {};
  captured.setOption.mockClear();
});
afterEach(() => {
  cleanup(); // RTL's auto-cleanup is off under globals:false — unmount explicitly
  vi.unstubAllGlobals();
});

describe("GraphView migration (S-186, FR-UI-08 / FR-UI-21)", () => {
  it("renders the canvas, controls, query bar, decisions prompt, and the accessible table", async () => {
    stubApi();
    render(<GraphView />);

    // The interactive canvas mount (an a11y application region).
    expect(await screen.findByRole("application")).toBeInTheDocument();
    // Controls re-homed into React.
    expect(screen.getByRole("group", { name: /graph exploration controls/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reset to whole graph" })).toBeInTheDocument();
    // The structured/relational query bar.
    expect(screen.getByRole("button", { name: "Query" })).toBeInTheDocument();
    // The Decisions panel opens with the honest prompt (nothing selected yet).
    expect(screen.getByText(/Lock a symbol to see the requirements/i)).toBeInTheDocument();
    // The accessible node/edge data-table lists every node label.
    const table = screen.getByRole("table", { name: "Graph nodes" });
    expect(within(table).getByText("alpha")).toBeInTheDocument();
    expect(within(table).getByText("FR-UI-08")).toBeInTheDocument();
    // The honest "N more not shown" cap notice.
    expect(screen.getByText(/4 nodes and 1 edge not shown/i)).toBeInTheDocument();
  });

  it("shows an honest empty state (not a blank canvas) when the project is unindexed", async () => {
    stubApi(EMPTY_GRAPH);
    render(<GraphView />);
    expect(await screen.findByText(/No graph elements yet/i)).toBeInTheDocument();
    expect(screen.getByText("logos index")).toBeInTheDocument();
    expect(screen.queryByRole("application")).not.toBeInTheDocument();
  });

  it("locks a node on canvas click and loads its Decisions over /api/v1/impact", async () => {
    const urls = stubApi();
    render(<GraphView />);
    await screen.findByRole("application");
    // The click handler is wired in the canvas mount effect — wait for it.
    await waitFor(() => expect(typeof captured.handlers.click).toBe("function"));

    // Simulate a canvas node click through the captured ECharts handler.
    act(() => captured.handlers.click({ dataType: "node", data: { id: "scip::a" } }));

    // The Decisions panel fetches the impact read-model and renders the trace.
    await waitFor(() => expect(urls.some((u) => u.startsWith("/api/v1/impact?seed=scip%3A%3Aa"))).toBe(true));
    expect(await screen.findByRole("table", { name: "Linked decisions and docs" })).toBeInTheDocument();
    expect(screen.getByText(/Locked alpha/i)).toBeInTheDocument();

    // Clicking the same node again UNLOCKS it — the Decisions panel returns to the
    // opening prompt (the toggle half of the lockable-selection acceptance criterion).
    act(() => captured.handlers.click({ dataType: "node", data: { id: "scip::a" } }));
    expect(await screen.findByText(/Lock a symbol to see the requirements/i)).toBeInTheDocument();
    expect(screen.getByText(/Selection unlocked/i)).toBeInTheDocument();
  });

  it("expands a locked node's neighbours and draws the 'N more not shown' counter down", async () => {
    const urls = stubApi();
    render(<GraphView />);
    await screen.findByRole("application");
    await waitFor(() => expect(typeof captured.handlers.click).toBe("function"));

    // Lock a node, then Expand it: the seed-scoped fetch merges one elided neighbour.
    act(() => captured.handlers.click({ dataType: "node", data: { id: "scip::a" } }));
    await userEvent.click(screen.getByRole("button", { name: "Expand neighbours" }));

    await waitFor(() => expect(urls.some((u) => u.includes("seed=scip%3A%3Aa") && !u.includes("impact"))).toBe(true));
    // The merge admitted 1 node + 1 edge, so the elided counter drops 4→3 nodes, 1→0 edges.
    expect(await screen.findByText(/Expanded alpha — added 1 node and 1 edge/i)).toBeInTheDocument();
    expect(screen.getByText(/3 nodes not shown/i)).toBeInTheDocument();
    // The merged neighbour is now in the accessible table.
    expect(within(screen.getByRole("table", { name: "Graph nodes" })).getByText("delta")).toBeInTheDocument();
  });

  it("surfaces an honest notice when an interactive re-fetch fails", async () => {
    // The initial load succeeds; the subsequent filter re-fetch rejects.
    let calls = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        calls += 1;
        if (calls > 1 && url.startsWith("/api/v1/graph")) return Promise.reject(new Error("network down"));
        return Promise.resolve({ ok: true, json: () => Promise.resolve(GRAPH) } as Response);
      }),
    );
    render(<GraphView />);
    await screen.findByRole("application");

    await userEvent.click(screen.getByRole("checkbox", { name: "Docs" }));
    expect(await screen.findByText(/Could not load the graph elements/i)).toBeInTheDocument();
  });

  it("re-fetches with the server-side re-budgeting filter when a layer is toggled", async () => {
    const urls = stubApi();
    render(<GraphView />);
    await screen.findByRole("application");
    urls.length = 0;

    await userEvent.click(screen.getByRole("checkbox", { name: "Docs" }));
    await waitFor(() => expect(urls.some((u) => u.includes("layers=code%2Cartifact"))).toBe(true));
  });

  it("reloads the table to the 1-hop neighbourhood on lock, reverts on unlock (S-198, FR-UI-08)", async () => {
    stubApi();
    render(<GraphView />);
    await screen.findByRole("application");
    await waitFor(() => expect(typeof captured.handlers.click).toBe("function"));

    // Initial table shows the full loaded graph — delta is not in the GRAPH fixture.
    expect(
      within(screen.getByRole("table", { name: "Graph nodes" })).queryByText("delta")
    ).not.toBeInTheDocument();

    // Lock scip::a → reloadTable fetches ?seed=scip::a → EXPANDED fixture contains delta.
    act(() => captured.handlers.click({ dataType: "node", data: { id: "scip::a" } }));

    // Table reloads to the fetched 1-hop neighbourhood (delta was elided from the canvas).
    await waitFor(() =>
      expect(
        within(screen.getByRole("table", { name: "Graph nodes" })).getByText("delta")
      ).toBeInTheDocument()
    );
    // The card heading also reflects the 1-hop scope.
    expect(screen.getByRole("heading", { name: /1-hop neighbourhood of alpha/i })).toBeInTheDocument();

    // Unlock: tableSet is cleared → table reverts to the full currently-loaded set (no delta).
    act(() => captured.handlers.click({ dataType: "node", data: { id: "scip::a" } }));
    await waitFor(() =>
      expect(
        within(screen.getByRole("table", { name: "Graph nodes" })).queryByText("delta")
      ).not.toBeInTheDocument()
    );
    // Heading reverts to the default full-set label.
    expect(screen.getByRole("heading", { name: /Graph nodes & edges/i })).toBeInTheDocument();
  });

  it("populates the table to the 1-hop neighbourhood when a query hit triggers focusOn (S-198)", async () => {
    // focusOn is the second code path that sets tableSet — it reuses the seed-scoped
    // canvas fetch, so no second request is needed for the table.
    stubApi();
    render(<GraphView />);
    await screen.findByRole("application");

    // Querying "alpha" returns 1 hit (QUERY fixture) → auto-calls focusOn("scip::a")
    // → canvas fetches ?seed=scip::a (EXPANDED) → setTableSet(EXPANDED)
    await userEvent.type(screen.getByLabelText("Search"), "alpha");
    await userEvent.click(screen.getByRole("button", { name: "Query" }));

    // Table heading switches to the 1-hop neighbourhood of the focused node
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: /1-hop neighbourhood of alpha/i })
      ).toBeInTheDocument()
    );
    // The fetched neighbourhood (EXPANDED) includes delta, which was not in the initial load
    expect(
      within(screen.getByRole("table", { name: "Graph nodes" })).getByText("delta")
    ).toBeInTheDocument();
  });

  it("runs a whole-graph query and lists the ranked hit", async () => {
    const urls = stubApi();
    render(<GraphView />);
    await screen.findByRole("application");

    await userEvent.type(screen.getByLabelText("Search"), "alpha");
    await userEvent.click(screen.getByRole("button", { name: "Query" }));

    await waitFor(() => expect(urls.some((u) => u.startsWith("/api/v1/query?q=alpha"))).toBe(true));
    expect(await screen.findByText(/1 match/i)).toBeInTheDocument();
    // Query results render as a paginated table (S-197): rank in the "#" column, the
    // node label as a select-to-lock button in the "Name" column (no longer "1. alpha · function").
    const results = screen.getByRole("region", { name: "Query results" });
    expect(within(results).getByRole("button", { name: "alpha" })).toBeInTheDocument();
  });
});
