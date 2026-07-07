/*
 * The Decisions & docs panel (S-186, FR-DG-02, FR-NV-10) — the doc-graph
 * Requirement/ADR/Story (and doc-section) links a change to the selected symbol
 * may oblige updating. Re-homed onto the shared data-access layer: it reads the
 * new `/api/v1/impact` JSON read-model via `useApiResource` (a clean second
 * consumer of the shared hook) and renders through the design-system components.
 *
 * Honest states (NFR-CC-04, FR-NV-09): with no node selected it shows the opening
 * prompt; a selected node that traces to nothing shows a node-named "nothing
 * traces here yet"; a failed read renders an honest error panel. A node row is a
 * focus action that pivots the canvas onto it.
 */

import { AsyncResource, fetchImpact, useApiResource } from "../../api/index.ts";
import type { ImpactResult, TraceLink } from "../../api/types.ts";
import { Badge, Button, Card, DataTable, DEFAULT_TABLE_PAGE_SIZE, type Column } from "../../components/index.ts";
import { layerLabel, prettify } from "./graphModel.ts";
import styles from "./GraphView.module.css";

export interface DecisionsPanelProps {
  /** The selected (locked/focused) node id, or `null` when nothing is selected. */
  seed: string | null;
  /** Pivot the canvas onto a traced node. */
  onFocus: (id: string) => void;
}

export function DecisionsPanel({ seed, onFocus }: DecisionsPanelProps) {
  if (!seed) {
    return (
      <Card title="Decisions & docs">
        <p className="muted">
          Lock a symbol to see the requirements, ADRs, and stories it traces to.
        </p>
      </Card>
    );
  }
  return <DecisionsForNode seed={seed} onFocus={onFocus} />;
}

function DecisionsForNode({ seed, onFocus }: { seed: string; onFocus: (id: string) => void }) {
  const resource = useApiResource<ImpactResult>(() => fetchImpact(seed), [seed]);
  return (
    <Card title="Decisions & docs">
      <AsyncResource resource={resource} loadingLabel="Loading decisions…">
        {(impact) => <DecisionsBody impact={impact} seed={seed} onFocus={onFocus} />}
      </AsyncResource>
    </Card>
  );
}

const COLUMNS = (onFocus: (id: string) => void): Column<TraceLink>[] => [
  {
    key: "kind",
    header: "Kind",
    cell: (d) => <Badge tone="muted">{d.kind}</Badge>,
    sortValue: (d) => d.kind,
  },
  {
    key: "node",
    header: "Node",
    cell: (d) => (
      <Button variant="ghost" size="sm" onClick={() => onFocus(d.symbol)}>
        <span className="mono">{d.name}</span>
      </Button>
    ),
    sortValue: (d) => d.name,
  },
  { key: "via", header: "Via", mono: true, cell: (d) => prettify(d.via) },
];

function DecisionsBody({
  impact,
  seed,
  onFocus,
}: {
  impact: ImpactResult;
  seed: string;
  onFocus: (id: string) => void;
}) {
  const resolved = impact.resolved;
  const docs = impact.docs;
  return (
    <>
      {resolved && (
        <header className={styles.identity}>
          <span className="mono">{resolved.name}</span> <Badge tone="muted">{resolved.kind}</Badge>{" "}
          <Badge tone="muted">{layerLabel(layerFromKind(resolved.kind))} layer</Badge>
          {resolved.file && (
            <span className="muted mono">
              {" "}
              {resolved.file}
              {resolved.line ? `:${resolved.line}` : ""}
            </span>
          )}
        </header>
      )}
      {docs.length === 0 ? (
        <p className="muted">
          No requirements, ADRs, or stories trace to{" "}
          <span className="mono">{resolved?.name ?? seed}</span> yet.
        </p>
      ) : (
        <DataTable
          caption="Linked decisions and docs"
          captionVisible
          columns={COLUMNS(onFocus)}
          rows={docs}
          rowKey={(d) => d.symbol}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      )}
    </>
  );
}

/** Derive the presentation layer from a kind for the identity badge (heuristic). */
function layerFromKind(kind: string): "code" | "doc" | "artifact" {
  if (/requirement|adr|story|doc/i.test(kind)) return "doc";
  if (/config|shell|route|artifact/i.test(kind)) return "artifact";
  return "code";
}
