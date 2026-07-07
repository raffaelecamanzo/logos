/*
 * GraphCanvas (S-186, FR-UI-08) — the imperative-ECharts-in-a-React-component
 * mount, the pattern the spa-frontend names for re-homing the canvas. React owns
 * the surrounding controls/state; this component owns the ECharts instance behind
 * a ref and re-commits the series (built by the pure `graphOptions.buildSeries`)
 * whenever the loaded set or selection changes. A node click is reported up via
 * `onNodeClick` so the parent owns lock/unlock; zoom in/out is exposed via the
 * forwarded ref for the toolbar buttons (scroll/pinch roam natively).
 *
 * Set change vs selection change (the legacy rebuild/restyle split): when the
 * visible node-id set changes the series is replaced (`replaceMerge`) so stale
 * nodes are removed and the layout reflows; a selection-only change restyles in
 * place so force-layout positions and the camera survive.
 */

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
} from "react";

import { createGraphChart, type GraphChartInstance, type GraphEventParams } from "./echarts.ts";
import { buildSeries, labelsVisible, type GraphSelection } from "./graphOptions.ts";
import { prettify, type LoadedSet } from "./graphModel.ts";
import styles from "./GraphView.module.css";

/** Canvas zoom bounds (mirror graph.js): max 5×, floor derived from MIN_NODE_PX/NODE_BASE. */
const SCALE_MAX = 5;
const SCALE_MIN = 7 / 12;
/** Per-press zoom factor for the toolbar `+`/`−` buttons. */
const ZOOM_STEP = 1.4;

/** The imperative handle the toolbar drives. */
export interface GraphCanvasHandle {
  zoomIn(): void;
  zoomOut(): void;
}

export interface GraphCanvasProps {
  /** The accumulated element set to render. */
  loaded: LoadedSet;
  /** The current selection/scope (focus, lock, located, depth, seed). */
  selection: GraphSelection;
  /** Called with a node id when a canvas node is clicked (parent owns lock/unlock). */
  onNodeClick: (id: string) => void;
  /** When true, a busy overlay covers the canvas (a re-fetch is in flight). */
  busy?: boolean;
}

/** The on-canvas tooltip text for a hovered node/edge. */
function tooltipText(p: GraphEventParams): string {
  if (!p?.data) return "";
  if (p.dataType === "edge") return p.data.edge_type ? prettify(p.data.edge_type) : "";
  const label = p.data.displayLabel || p.name || "";
  const kind = p.data.kind ? prettify(p.data.kind) : "";
  return kind ? `${label}\n${kind}` : label;
}

export const GraphCanvas = forwardRef<GraphCanvasHandle, GraphCanvasProps>(function GraphCanvas(
  { loaded, selection, onNodeClick, busy = false },
  ref,
) {
  const mountRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<GraphChartInstance | null>(null);
  const zoomRef = useRef(1);
  // The visible node-id signature at the last commit — decides replace vs restyle.
  const prevSignatureRef = useRef<string>("");
  // Keep the latest click handler reachable from the (once-wired) ECharts listener.
  const clickRef = useRef(onNodeClick);
  clickRef.current = onNodeClick;
  // Same hazard for the roam handler: it is wired once in the mount effect but must
  // call the CURRENT `commit` (which closes over the latest `loaded`/`selection`),
  // not the first render's — otherwise a pan/zoom after any state change would
  // rebuild the canvas from the stale initial snapshot. Held in a ref, refreshed
  // each render below, exactly like `clickRef`.
  const commitRef = useRef<(forceReplace: boolean) => void>(() => {});

  // The canvas centre in layout pixel space — the selection is pinned here so a
  // re-layout never flings it out of view. Zero in a headless test (no layout).
  const center = (): readonly [number, number] => {
    const el = mountRef.current;
    return el ? [el.clientWidth / 2, el.clientHeight / 2] : [0, 0];
  };

  // Instantiate the chart once, wire the click + roam listeners, and tidy up.
  useEffect(() => {
    const el = mountRef.current;
    if (!el) return;
    const chart = createGraphChart(el);
    chartRef.current = chart;
    chart.setOption({
      backgroundColor: "transparent",
      tooltip: {
        show: true,
        renderMode: "richText",
        backgroundColor: "#3d3935",
        borderWidth: 0,
        padding: [6, 10],
        textStyle: { color: "#ffffff", fontSize: 12 },
        formatter: tooltipText,
      },
      animationDurationUpdate: 400,
      series: [],
    });
    chart.on("click", (params) => {
      if (params.dataType !== "node" || !params.data) return;
      const id = params.data.id ?? params.name;
      if (id) clickRef.current(id);
    });
    chart.on("graphroam", (params) => {
      if (params?.zoom) {
        zoomRef.current = Math.max(SCALE_MIN, Math.min(SCALE_MAX, zoomRef.current * params.zoom));
        commitRef.current(false); // the CURRENT commit, not the mount-time closure
      }
    });
    const onResize = () => chart.resize();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      chart.dispose();
      chartRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Commit the series. `forceReplace` (a set change) replaces the series so stale
  // nodes are removed and the layout reflows; otherwise a selection-only restyle
  // keeps positions and the camera. A roam-driven recommit only flips label LOD.
  const commit = (forceReplace: boolean) => {
    const chart = chartRef.current;
    if (!chart) return;
    const visibleSig = signatureOf(loaded, selection);
    const setChanged = forceReplace && visibleSig !== prevSignatureRef.current;
    const series = buildSeries(loaded, selection, center(), zoomRef.current, SCALE_MIN, SCALE_MAX);
    if (setChanged) {
      zoomRef.current = 1; // a view change returns to home zoom
      chart.setOption({ series: [series] }, { replaceMerge: ["series"] });
      prevSignatureRef.current = visibleSig;
    } else {
      chart.setOption({ series: [series] });
    }
  };
  // Refresh the roam handler's view of `commit` every render (see `commitRef`).
  commitRef.current = commit;

  // Re-commit on any loaded-set or selection change. The signature comparison
  // inside `commit` decides replace (set change) vs restyle (selection only).
  useEffect(() => {
    commit(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded, selection]);

  // Zoom the canvas by a relative factor about its centre (the toolbar buttons).
  const zoomBy = (factor: number) => {
    const chart = chartRef.current;
    if (!chart) return;
    const target = Math.max(SCALE_MIN, Math.min(SCALE_MAX, zoomRef.current * factor));
    if (target === zoomRef.current) return;
    const applied = target / zoomRef.current;
    const el = mountRef.current;
    chart.dispatchAction({
      type: "graphRoam",
      zoom: applied,
      originX: el ? el.clientWidth / 2 : 0,
      originY: el ? el.clientHeight / 2 : 0,
    });
    const before = labelsVisible(Object.keys(loaded.nodes).length, zoomRef.current);
    zoomRef.current = target;
    const after = labelsVisible(Object.keys(loaded.nodes).length, zoomRef.current);
    if (before !== after) commit(false); // a label LOD crossing — relabel in place
  };

  useImperativeHandle(ref, () => ({
    zoomIn: () => zoomBy(ZOOM_STEP),
    zoomOut: () => zoomBy(1 / ZOOM_STEP),
  }));

  return (
    <div className={styles.canvasWrap}>
      <div
        ref={mountRef}
        className={styles.canvasMount}
        role="application"
        aria-label="Interactive code graph (an accessible node and edge table is below)"
      />
      {busy && (
        <div role="status" aria-live="polite" className={styles.canvasBusy}>
          <span className="sr-only">Updating the graph…</span>
        </div>
      )}
    </div>
  );
});

/** A signature of the visible node ids — changes iff the rendered node set changes. */
function signatureOf(loaded: LoadedSet, selection: GraphSelection): string {
  // Visible-id derivation matches buildSeries; cheap to recompute for the compare.
  const ids = Object.keys(loaded.nodes);
  if (!selection.focusId || selection.depth <= 0) return ids.sort().join("|");
  // With a depth bound the visible set is narrower; recompute via the model.
  return ids.sort().join("|") + `@${selection.focusId}:${selection.depth}`;
}
