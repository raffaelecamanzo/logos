/*
 * The accessible node/edge data-table (S-186, FR-UI-08, FR-UI-23) — retained as
 * an in-SPA WCAG 2.1 AA affordance, no longer a no-JS fallback. It lists every
 * rendered node and edge so the information (not just the picture) is reachable by
 * keyboard and screen reader, and every row traces to a graph-elements field —
 * nothing fabricated (NFR-RA-05). Built on the design-system `DataTable` (sortable,
 * captioned), so it inherits the system's a11y semantics.
 */

import { Badge, Card, DataTable, DEFAULT_TABLE_PAGE_SIZE, type Column } from "../../components/index.ts";
import type { GraphElementEdge, GraphElementNode } from "../../api/types.ts";
import { layerLabel, prettify, type LoadedSet } from "./graphModel.ts";
import styles from "./GraphTable.module.css";

interface NodeRow {
  id: string;
  layer: string;
  label: string;
  kind: string;
}

interface EdgeRow {
  key: string;
  from: string;
  edge: string;
  to: string;
}

function nodeRows(set: LoadedSet): NodeRow[] {
  return Object.values(set.nodes).map((n: GraphElementNode) => ({
    id: n.id,
    layer: layerLabel(n.layer),
    label: n.label,
    kind: n.kind ?? "cluster",
  }));
}

function edgeRows(set: LoadedSet): EdgeRow[] {
  const labelOf = (id: string) => set.nodes[id]?.label ?? id;
  return set.edges.map((e: GraphElementEdge, i) => ({
    key: `${e.source}->${e.target}:${e.edge_type ?? ""}:${i}`,
    from: labelOf(e.source),
    edge: e.edge_type ? prettify(e.edge_type) : "rollup",
    to: labelOf(e.target),
  }));
}

// Every column is sortable (the legacy node/edge tables sorted on any header);
// the badge columns sort on their raw string, not the rendered chip.
const NODE_COLUMNS: Column<NodeRow>[] = [
  { key: "layer", header: "Layer", cell: (r) => r.layer, sortValue: (r) => r.layer },
  { key: "label", header: "Node", mono: true, cell: (r) => r.label, sortValue: (r) => r.label },
  {
    key: "kind",
    header: "Kind",
    cell: (r) => <Badge tone="muted">{r.kind}</Badge>,
    sortValue: (r) => r.kind,
  },
];

const EDGE_COLUMNS: Column<EdgeRow>[] = [
  { key: "from", header: "From", mono: true, cell: (r) => r.from, sortValue: (r) => r.from },
  {
    key: "edge",
    header: "Edge",
    cell: (r) => <Badge tone="muted">{r.edge}</Badge>,
    sortValue: (r) => r.edge,
  },
  { key: "to", header: "To", mono: true, cell: (r) => r.to, sortValue: (r) => r.to },
];

interface GraphTableProps {
  loaded: LoadedSet;
  /** Label of the currently locked node; when set, the tables show its 1-hop neighbourhood. */
  hoodOf?: string;
}

export function GraphTable({ loaded, hoodOf }: GraphTableProps) {
  const nodes = nodeRows(loaded);
  const edges = edgeRows(loaded);
  const title = hoodOf
    ? `1-hop neighbourhood of ${hoodOf}`
    : "Graph nodes & edges (accessible table)";
  const description = hoodOf
    ? `Nodes and edges directly connected to ${hoodOf}, for keyboard and screen-reader traversal.`
    : "The interactive graph's nodes and edges, listed for keyboard and screen-reader access.";
  return (
    <Card title={title}>
      <p className="muted">{description}</p>
      <DataTable
        caption="Graph nodes"
        captionVisible
        columns={NODE_COLUMNS}
        rows={nodes}
        rowKey={(r) => r.id}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
        empty="No graph elements yet."
      />
      {edges.length > 0 && (
        <>
          <hr className={styles.separator} aria-hidden="true" />
          <DataTable
            caption="Graph edges"
            captionVisible
            columns={EDGE_COLUMNS}
            rows={edges}
            rowKey={(r) => r.key}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        </>
      )}
    </Card>
  );
}
