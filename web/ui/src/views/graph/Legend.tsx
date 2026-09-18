/*
 * The graph legend (S-186, FR-UI-08, frontend-design §4.4/§7) — documents the
 * node-layer colors and the edge-type colors + line styles so the typed,
 * multi-layer canvas stays legible and color is never the only signal. Pure
 * presentation over a fixed enumeration; no data, so it carries no fetch. The
 * swatch hues mirror the canvas palette (graphModel) exactly.
 */

import { EDGE_COLOR, EDGE_STYLE, prettify } from "./graphModel.ts";
import styles from "./GraphView.module.css";

/** Each layer paired with its label and the module swatch class (CSP-clean — no
 *  inline style; the canvas reads the matching hue from graphModel). */
const LAYERS: ReadonlyArray<[string, string, string]> = [
  ["code", "Code", styles.legendDotCode],
  ["doc", "Docs", styles.legendDotDoc],
  ["artifact", "Artifacts", styles.legendDotArtifact],
];

/** Structural edge rows shown in the legend (the doc-intent edges sit apart). */
const STRUCTURAL_EDGES = [
  "calls",
  "imports",
  "contains",
  "references",
  "type_uses",
  "implements",
  "extends",
  "instantiates",
  "accesses",
  "routes_to",
  "artifact_ref",
  "artifact_binding",
  "forbidden_dependency",
];

/** The documentation→code intent edges, named apart as rationale (S-129). */
const INTENT_EDGES = ["doc_reference", "traces_to"];

function dashFor(type: string): string {
  const style = EDGE_STYLE[type];
  if (style === "dashed") return "6 4";
  if (style === "dotted") return "2 3";
  return "0";
}

/** One legend row: the edge's hue + line style beside its name. Exported so the
 *  app-level service map (S-250) documents its cross-service relation arms in the
 *  SAME legend grammar, from the same palette, rather than inventing a second one.
 *
 *  `dash` and `label` are overrides for a row that documents a channel OTHER than
 *  the edge type — the service map's provenance section (S-419, CR-132 AC3),
 *  whose two rows share one arm's hue and differ only in stroke. They are
 *  overrides on this component rather than a second row component next to it
 *  because a hand-mirrored twin is exactly how two legends drift apart: the swatch
 *  geometry, the hue lookup and the `aria-hidden` treatment must stay one
 *  definition, or the provenance swatch stops matching the arm swatch beside it. */
export function EdgeRow({
  type,
  dash,
  label,
}: {
  type: string;
  /** An explicit SVG `stroke-dasharray`, replacing the type's own line style. */
  dash?: string;
  /** Row text, replacing the prettified type token. */
  label?: string;
}) {
  return (
    <li className={styles.legendRow}>
      <svg width="22" height="8" aria-hidden="true" className={styles.legendLine}>
        <line
          x1="1"
          y1="4"
          x2="21"
          y2="4"
          stroke={EDGE_COLOR[type] ?? "#9ca3af"}
          strokeWidth="2"
          strokeDasharray={dash ?? dashFor(type)}
        />
      </svg>
      <span>{label ?? prettify(type)}</span>
    </li>
  );
}

export function Legend() {
  return (
    <details className={styles.legend} open>
      <summary>Legend</summary>
      <div className={styles.legendBody}>
        <span className={styles.legendHeading}>Layers</span>
        <ul className={styles.legendList}>
          {LAYERS.map(([slug, label, dotClass]) => (
            <li className={styles.legendRow} key={slug}>
              <span className={`${styles.legendDot} ${dotClass}`} aria-hidden="true" />
              <span>{label}</span>
            </li>
          ))}
          <li className={styles.legendRow}>
            <span className={`${styles.legendDot} ${styles.legendDotSelected}`} aria-hidden="true" />
            <span>Selected (ring)</span>
          </li>
        </ul>
        <span className={styles.legendHeading}>Edges</span>
        <ul className={styles.legendList}>
          {STRUCTURAL_EDGES.map((t) => (
            <EdgeRow type={t} key={t} />
          ))}
        </ul>
        <span className={styles.legendHeading}>Intent / governing docs</span>
        <ul className={styles.legendList}>
          {INTENT_EDGES.map((t) => (
            <EdgeRow type={t} key={t} />
          ))}
        </ul>
        <p className={styles.legendNote}>
          Edge line style (solid / dashed / dotted) repeats the type as a second channel. The
          intent edges are off by default — enable “Intent / governing docs”.
        </p>
      </div>
    </details>
  );
}
