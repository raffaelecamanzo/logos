import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import type { GraphElementEdge, GraphElementNode } from "../../api/types.ts";
import { GraphTable } from "./GraphTable.tsx";
import { loadedFrom } from "./graphModel.ts";

afterEach(cleanup);

const nodes: GraphElementNode[] = [
  { id: "scip::a", label: "alpha", kind: "function", layer: "code" },
  { id: "scip::r", label: "FR-UI-08", kind: "requirement", layer: "doc" },
];
const edges: GraphElementEdge[] = [{ source: "scip::r", target: "scip::a", edge_type: "doc_reference" }];

describe("GraphTable (the in-SPA accessible node/edge affordance, FR-UI-08 / FR-UI-23)", () => {
  it("lists every node and edge, each traced to a graph-elements field", () => {
    render(<GraphTable loaded={loadedFrom(nodes, edges)} />);
    const nodeTable = screen.getByRole("table", { name: "Graph nodes" });
    expect(within(nodeTable).getByText("alpha")).toBeInTheDocument();
    expect(within(nodeTable).getByText("FR-UI-08")).toBeInTheDocument();
    const edgeTable = screen.getByRole("table", { name: "Graph edges" });
    expect(within(edgeTable).getByText("doc reference")).toBeInTheDocument(); // prettified edge type
  });

  it("exposes a keyboard-operable, sortable column header (WCAG 2.1 AA)", async () => {
    render(<GraphTable loaded={loadedFrom(nodes, edges)} />);
    const nodeTable = screen.getByRole("table", { name: "Graph nodes" });
    // The "Kind" column is sortable: its header is a real <button> in a th carrying aria-sort.
    const kindHeader = within(nodeTable).getAllByRole("columnheader").find((h) => /Kind/.test(h.textContent ?? ""))!;
    expect(kindHeader).toHaveAttribute("aria-sort", "none");
    const sortButton = within(kindHeader).getByRole("button");
    await userEvent.click(sortButton); // keyboard-focusable button, sortable
    expect(kindHeader).toHaveAttribute("aria-sort", "ascending");
  });

  it("omits the edge table when there are no edges (no empty twin)", () => {
    render(<GraphTable loaded={loadedFrom(nodes, [])} />);
    expect(screen.getByRole("table", { name: "Graph nodes" })).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: "Graph edges" })).not.toBeInTheDocument();
  });

  it("makes every node column sortable, not just the badge column", async () => {
    render(<GraphTable loaded={loadedFrom(nodes, edges)} />);
    const nodeTable = screen.getByRole("table", { name: "Graph nodes" });
    // The Layer column is now sortable too (legacy parity, FR-UI-11).
    const layerHeader = within(nodeTable).getAllByRole("columnheader").find((h) => /Layer/.test(h.textContent ?? ""))!;
    expect(layerHeader).toHaveAttribute("aria-sort", "none");
    await userEvent.click(within(layerHeader).getByRole("button"));
    expect(layerHeader).toHaveAttribute("aria-sort", "ascending");
  });

  it("paginates the node table at 20 rows/page, slicing the sorted set (FR-UI-11)", () => {
    // 30 nodes → a pager appears and only the first page (20) is rendered.
    const many: GraphElementNode[] = Array.from({ length: 30 }, (_, i) => ({
      id: `scip::n${i}`,
      label: `node-${String(i).padStart(2, "0")}`,
      kind: "function",
      layer: "code",
    }));
    render(<GraphTable loaded={loadedFrom(many, [])} />);
    const nodeTable = screen.getByRole("table", { name: "Graph nodes" });
    expect(within(nodeTable).getAllByRole("row").length).toBe(20 + 1); // 20 body rows + header
    expect(screen.getByRole("navigation", { name: "Table pages" })).toBeInTheDocument();
    expect(screen.getByText(/Showing 1–20 of 30/)).toBeInTheDocument();
  });

  it("renders a visual separator between the nodes and edges tables (S-198, FR-UI-08)", () => {
    const { container } = render(<GraphTable loaded={loadedFrom(nodes, edges)} />);
    // The <hr> divides the two DataTable sections — present when edges exist.
    expect(container.querySelector("hr")).toBeInTheDocument();
  });

  it("omits the separator when there are no edges (S-198, FR-UI-08)", () => {
    const { container } = render(<GraphTable loaded={loadedFrom(nodes, [])} />);
    expect(container.querySelector("hr")).not.toBeInTheDocument();
  });

  it("shows the default card heading when no node is selected", () => {
    render(<GraphTable loaded={loadedFrom(nodes, edges)} />);
    expect(screen.getByRole("heading", { name: /Graph nodes & edges/i })).toBeInTheDocument();
  });

  it("updates the card heading to the 1-hop neighbourhood label when hoodOf is set (S-198)", () => {
    render(<GraphTable loaded={loadedFrom(nodes, edges)} hoodOf="alpha" />);
    expect(
      screen.getByRole("heading", { name: /1-hop neighbourhood of alpha/i })
    ).toBeInTheDocument();
    // Captions are preserved (WCAG 2.1 AA — tables still have their names).
    expect(screen.getByRole("table", { name: "Graph nodes" })).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Graph edges" })).toBeInTheDocument();
  });
});
