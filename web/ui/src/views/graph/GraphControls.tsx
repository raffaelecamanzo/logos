/*
 * The graph exploration controls (S-186, FR-UI-08/FR-UI-15/FR-UI-16). Re-homed
 * into React over the `/api/v1/graph` feed: layer filters, the documentation-intent
 * overlay toggle, the semantic-tier select, a focus-depth bound, edge-type filters,
 * zoom buttons, the locked-node Expand control, and reset. Every control is a
 * client-side action over the read-only feed — none mutates a store (ADR-28).
 * Layer/edge/intent/tier changes re-fetch with the server-side re-budgeting params
 * (S-122); depth is a client-side hop bound; zoom drives the canvas camera.
 */

import { Button, SelectField } from "../../components/index.ts";
import type { GraphGranularity, GraphLayer } from "../../api/types.ts";
import { prettify } from "./graphModel.ts";
import styles from "./GraphView.module.css";

export interface EdgeFilter {
  type: string;
  on: boolean;
}

export interface GraphControlsProps {
  layers: Record<GraphLayer, boolean>;
  onToggleLayer: (layer: GraphLayer) => void;
  intent: boolean;
  onToggleIntent: (on: boolean) => void;
  granularity: GraphGranularity;
  onGranularity: (tier: GraphGranularity) => void;
  depth: number;
  onDepth: (depth: number) => void;
  edgeFilters: EdgeFilter[];
  onToggleEdge: (type: string) => void;
  canExpand: boolean;
  onExpand: () => void;
  onReset: () => void;
  onZoomIn: () => void;
  onZoomOut: () => void;
  /** The status-line message (lock/expand/query feedback). */
  notice: string;
}

const LAYERS: ReadonlyArray<[GraphLayer, string]> = [
  ["code", "Code"],
  ["doc", "Docs"],
  ["artifact", "Artifacts"],
];

export function GraphControls(props: GraphControlsProps) {
  return (
    <div className={styles.controls} role="group" aria-label="Graph exploration controls">
      <fieldset className={styles.controlGroup}>
        <legend className={styles.controlLabel}>Layers</legend>
        {LAYERS.map(([layer, label]) => (
          <label key={layer} className={styles.check}>
            <input
              type="checkbox"
              checked={props.layers[layer]}
              onChange={() => props.onToggleLayer(layer)}
            />{" "}
            {label}
          </label>
        ))}
      </fieldset>

      <fieldset className={styles.controlGroup}>
        <legend className={styles.controlLabel}>Intent</legend>
        <label className={styles.check}>
          <input
            type="checkbox"
            checked={props.intent}
            onChange={(e) => props.onToggleIntent(e.target.checked)}
          />{" "}
          Intent / governing docs
        </label>
      </fieldset>

      <div className={styles.controlGroup}>
        <SelectField
          label="Tier"
          value={props.granularity}
          onChange={(e) => props.onGranularity(e.target.value as GraphGranularity)}
        >
          <option value="symbol">Symbols</option>
          <option value="file">Files</option>
          <option value="module">Modules</option>
        </SelectField>
      </div>

      <div className={styles.controlGroup}>
        <SelectField
          label="Depth"
          value={String(props.depth)}
          onChange={(e) => props.onDepth(Number(e.target.value) || 0)}
        >
          <option value="1">1 hop</option>
          <option value="2">2 hops</option>
          <option value="3">3 hops</option>
          <option value="0">All</option>
        </SelectField>
      </div>

      {props.edgeFilters.length > 0 && (
        <fieldset className={styles.controlGroup}>
          <legend className={styles.controlLabel}>Edges</legend>
          <div className={styles.edgeFilters}>
            {props.edgeFilters.map((f) => (
              <label key={f.type} className={styles.check}>
                <input type="checkbox" checked={f.on} onChange={() => props.onToggleEdge(f.type)} />{" "}
                {prettify(f.type)}
              </label>
            ))}
          </div>
        </fieldset>
      )}

      <div className={styles.controlGroup} role="group" aria-label="Zoom">
        <span className={styles.controlLabel}>Zoom</span>
        <Button size="sm" variant="secondary" aria-label="Zoom out" onClick={props.onZoomOut}>
          −
        </Button>
        <Button size="sm" variant="secondary" aria-label="Zoom in" onClick={props.onZoomIn}>
          +
        </Button>
      </div>

      <Button size="sm" variant="secondary" disabled={!props.canExpand} onClick={props.onExpand}>
        Expand neighbours
      </Button>
      <Button size="sm" variant="secondary" onClick={props.onReset}>
        Reset to whole graph
      </Button>

      <p className={styles.notice} role="status" aria-live="polite">
        {props.notice}
      </p>
    </div>
  );
}
