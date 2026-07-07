/*!
 * graph.js — Logos interactive whole-graph canvas + exploration controls.
 *
 * Project-authored, Apache-2.0-licensed (the Logos project license, see ../../LICENSE
 * and web/assets/VENDOR.md). Names no external origin: it fetches only the
 * same-origin `/api/graph` JSON elements endpoint and drives the already-vendored
 * Apache ECharts global (the slim graph-only build, ADR-29 / HF-1). Loaded
 * `defer`-ed as progressive enhancement under the self-only CSP
 * (`default-src 'self'`) — with JavaScript disabled the Graph view degrades to the
 * server-rendered node/edge data-table twin (FR-UI-08, ADR-29).
 *
 * Responsibilities (S-085 canvas):
 *   - Open on the whole graph by default, or a seed-scoped neighbourhood when the
 *     mount carries a `data-seed`.
 *   - Force-directed layout the user can pan, zoom (`roam`), and drag.
 *   - Visible-element cap + level-of-detail: the server bounds the element set
 *     and reports `elided_*`; this renders the honest "N more not shown" notice,
 *     hides labels when zoomed out, and admits a node's neighbours via the
 *     explicit "Expand neighbours" control (S-119).
 *   - Layer-colored nodes (brand tokens); edges typed by line style + color.
 *
 * Interaction model (S-119): hovering a node no longer dims the graph (it keeps
 * the lightweight tooltip only); adjacency emphasis is reserved for the *locked*
 * selection. Clicking a node locks it (persistent ring + adjacency emphasis +
 * Decisions-panel refresh); clicking it again unlocks, clicking another switches.
 * Neighbour expansion is the explicit control above — not the click — and `+`/`−`
 * buttons drive the same zoom accumulator scroll/pinch feed.
 *
 * Exploration controls (S-086), layered over the same canvas — every one is
 * client-side over the same-origin `/api/graph` feed; none mutates a store:
 *   - Neighbour-focus mode: narrow the canvas to a node + its neighbourhood
 *     (fetched seed-scoped), with the Expand-neighbours control to grow outward
 *     and a reset back to the whole graph.
 *   - Layer filters (code/docs/artifacts) and edge-type filters: server-side
 *     *re-budgeting* params on `/api/graph` (S-122, FR-UI-15) — toggling one
 *     re-fetches the current scope on the Focus/Reset path so the server re-spends
 *     the visible-element budget over the remaining scope (previously-elided nodes
 *     backfill the freed slots), rather than subtractively hiding the loaded set.
 *     A depth filter bounding hops from the focus node stays client-side.
 *   - Structured + relational query (S-120, FR-UI-14, ADR-35): a real whole-graph
 *     query over the same-origin read-only `/api/query` endpoint (field filters +
 *     callers/callees/impact verbs), in place of the old visible-only locate;
 *     ranked hits are listed and a selected hit is centered + locked.
 *   - Click-to-focus from Decisions-panel nodes (any `[data-graph-focus]`).
 *
 * The toolbar (`#graph-controls`) is server-rendered `hidden`; this script reveals
 * it once the canvas boots, so no inert controls ever show without scripting.
 */
(function () {
  "use strict";

  // Brand tokens (mirrors of logos.css §1.2 — ECharts paints to <canvas>, so it
  // cannot read CSS custom properties; the few it needs are reproduced here).
  var COLOR = {
    code: "#2563eb", // --so-blue    (code symbols)
    doc: "#16a34a", // --so-green     (docs)
    artifact: "#d97706", // --so-amber (config/artifacts)
    // The selection accent is drawn as a RING (border), never a fill swap, so a
    // selected/located node keeps its layer color (CR-030, FR-UI-08). `seed` and
    // `located` share the one red ring color.
    seed: "#da291c", // --so-red       (selection ring — focus/seed node)
    located: "#da291c", // --so-red    (selection ring — search-to-locate node)
    edge: "#9ca3af", // a neutral gray fallback that reads on the subtle bg without colliding with any node hue
    label: "#3d3935", // --so-merlin
    nodeBorder: "#ffffff", // white rim lifts each node off the graph
    tooltipBg: "#3d3935", // --so-merlin
  };

  // Per-layer fill, looked up by the wire `layer` (snake_case from /api/graph).
  function layerColor(layer) {
    return COLOR[layer] || COLOR.code;
  }

  // Edge line style by relationship kind (frontend-design §4.4: "edges typed by
  // line style"). Unmapped kinds fall back to a solid hairline. ECharts supports
  // exactly solid / dashed / dotted for `lineStyle.type`.
  var EDGE_STYLE = {
    calls: "solid",
    imports: "dashed",
    contains: "solid",
    references: "dotted",
    type_uses: "dotted",
    implements: "solid",
    extends: "solid",
    instantiates: "solid",
    routes_to: "dashed",
    doc_reference: "dotted",
    traces_to: "dashed",
    accesses: "dashed",
    artifact_ref: "dashed",
    artifact_binding: "dashed",
    forbidden_dependency: "solid",
  };

  // Edge color by relationship kind (CR-030, FR-UI-08): every relationship type
  // gets a distinct, mutually-legible hue so the typed graph reads apart at a
  // glance — `implements` (violet) is clearly different from `extends` (fuchsia).
  // The hues deliberately AVOID the node-layer colors (blue #2563eb / green
  // #16a34a / amber #d97706) so an edge never blends into the nodes it connects.
  // The typed line style (EDGE_STYLE) is retained as a redundant second channel,
  // so the two kinds that share a hue family by design (doc_reference / traces_to,
  // both purple) still separate by dash pattern — every (color, style) pair is
  // unique. These hexes mirror the `.legend-line--*` swatch colors in logos.css
  // (ECharts paints to <canvas> and can't read CSS vars). Unmapped kinds fall back
  // to the neutral edge gray.
  var EDGE_COLOR = {
    calls: "#3d3935", // charcoal
    imports: "#57534e", // warm gray
    contains: "#78716c", // taupe
    references: "#a8a29e", // light taupe
    type_uses: "#0891b2", // cyan
    implements: "#7c3aed", // violet
    extends: "#c026d3", // fuchsia (clearly apart from implements' violet)
    instantiates: "#4f46e5", // indigo
    accesses: "#0e7490", // dark cyan
    routes_to: "#db2777", // pink
    doc_reference: "#9333ea", // purple
    traces_to: "#9333ea", // purple (paired with doc_reference; differs by line style)
    artifact_ref: "#92400e", // brown
    artifact_binding: "#be123c", // crimson
    forbidden_dependency: "#da291c", // --so-red (a rules violation — meaning, not decoration)
  };

  // Edge color looked up by the wire `edge_type`, defaulting to the neutral
  // hairline for any kind not in the typed palette.
  function edgeColor(type) {
    return EDGE_COLOR[type] || COLOR.edge;
  }

  // Node sizing — a base size scaled gently by degree so hubs read larger; the
  // focus/located node is bumped so it never gets lost in a dense neighbourhood.
  var NODE_BASE = 12;
  var NODE_MAX = 34;
  var NODE_FOCUS = 26;

  // The minimum rendered diameter (px) a base node may shrink to at the zoom-out
  // floor (S-126/CR-032, FR-UI-15, ADR-36). The camera-only zoom-out floor is
  // DERIVED from this: ECharts renders a node at `symbolSize × zoom`, so a base
  // node (NODE_BASE px at home zoom 1) is NODE_BASE × zoom px, and the floor zoom
  // that holds it at MIN_NODE_PX is MIN_NODE_PX / NODE_BASE. Keeping every node ≥
  // this size keeps it legible and clickable however far the camera pulls back —
  // replacing the old fixed 0.2 floor that let a base node collapse to ~2.4px.
  var MIN_NODE_PX = 7; // within the ~6–8px legibility target

  // Level-of-detail: labels are hidden below this absolute zoom (a dense,
  // zoomed-out graph shows shape, not text; detail appears on zoom-in), and also
  // hidden from the start when the visible set is large.
  var LABEL_ZOOM_THRESHOLD = 0.9;
  var LABEL_NODE_LIMIT = 60;

  // Canvas zoom bounds — shared by the series `scaleLimit` and the clamp on the
  // tracked absolute zoom so the two never drift apart. `SCALE_MIN` is the
  // camera-only zoom-out FLOOR: no longer a fixed 0.2 (which shrank a base node to
  // ~2.4px and clamped silently with no feedback) but DERIVED from MIN_NODE_PX via
  // computeScaleMin, and recomputed on resize (recomputeScaleMin) so the `−`/`+`
  // buttons, scroll/pinch, and the series `scaleLimit` share ONE floor and never
  // drift (S-126/CR-032, ADR-36). Kept a mutable `var` for exactly that recompute.
  var SCALE_MAX = 5;
  var SCALE_MIN = computeScaleMin();
  // Tolerance for the "at a zoom limit" comparison — the clamps below land the
  // tracked zoom exactly on a bound, so the epsilon only guards float drift (S-126).
  var ZOOM_EPSILON = 1e-6;

  // Derive the camera-only zoom-out floor from the minimum node pixel size: a base
  // node renders at NODE_BASE × zoom, so the floor zoom that keeps it at MIN_NODE_PX
  // is MIN_NODE_PX / NODE_BASE. Clamped at/below home zoom (1) so home stays a valid
  // position to zoom out from even if the node tokens ever change. Pure + unit-
  // coverable — the single source every zoom clamp and the series scaleLimit read.
  function computeScaleMin() {
    return Math.min(MIN_NODE_PX / NODE_BASE, 1);
  }
  // Per-press zoom factor for the `+`/`−` buttons (S-119): one `+` press is a
  // 1.4× zoom-in, one `−` press its reciprocal — matched to a comfortable
  // scroll-wheel step so the buttons feel like the scroll/pinch they complement.
  var ZOOM_STEP = 1.4;

  // ── Zoom-driven density level-of-detail (S-123, ADR-36, FR-UI-15) ─────────────
  // The visible-element budget (the existing `/api/graph` `cap` parameter) is
  // *driven by zoom*, quantized into a few discrete thresholds — retiring the old
  // static default-cap-only model where the client forwarded the server's one
  // resolved cap forever. The server already degree-ranks the in-scope set and
  // truncates to `cap`, so the cap IS a level-of-detail ladder over the degree
  // ranking (S-085): a small cap keeps only the highest-degree hub nodes; a larger
  // cap admits progressively lower-degree tiers. Zoom owns *breadth*; the per-node
  // "Expand neighbours" drill-down (S-119) owns *local depth* and composes on top.
  //
  // Each tier is `{ below, cap }`: the first tier whose `below` exceeds the tracked
  // absolute zoom wins (ascending, the last entry catches the top). The home zoom
  // (1) is the FLOOR — the opening view is the baseline level of detail, and zooming
  // OUT never drops below it (it just shrinks the camera, like a map already showing
  // the whole earth: you see it smaller, the detail does not change). Detail is added
  // only on zoom *in* (the budget widens to admit lower-degree tiers). This retires
  // the earlier "zoom out collapses to the hubs" reshape (the cap=60 far-out tier),
  // which the user found jarring — zoom is camera distance, not content selection.
  // "A few discrete thresholds" (ADR-36 / the risk-table mitigation) — not a
  // continuous map — so a drift in scale only crosses a boundary occasionally.
  var ZOOM_CAP_LADDER = [
    { below: 1.5, cap: 250 }, //         home overview AND anything zoomed out — the baseline floor
    { below: 3.0, cap: 500 }, //         zoomed in — lower-degree tiers admitted
    { below: Infinity, cap: 1000 }, //   fully zoomed in — the broadest budget
  ];
  // Debounce window for a zoom-crossing re-fetch: a flick across several tiers or a
  // pinch that overshoots must settle to ONE re-fetch, never a fetch storm (the
  // ADR-36 negative-consequence mitigation, mirrored from the S-085 cap rationale).
  var ZOOM_REFETCH_DEBOUNCE_MS = 250;

  // The cap budget for a tracked absolute zoom — the LOD ladder lookup. Pure: the
  // single source mapping zoom → the existing `cap` parameter (unit-coverable).
  function capForZoom(zoom) {
    for (var i = 0; i < ZOOM_CAP_LADDER.length; i++) {
      if (zoom < ZOOM_CAP_LADDER[i].below) {
        return ZOOM_CAP_LADDER[i].cap;
      }
    }
    return ZOOM_CAP_LADDER[ZOOM_CAP_LADDER.length - 1].cap;
  }

  // ── Semantic cluster zoom (S-124, ADR-36, FR-UI-15) ───────────────────────────
  // S-124 originally drove a Google-Maps-style module → file → symbol altitude ladder
  // off the SAME zoom accumulator: zooming out swapped the symbol graph for file- then
  // module-rollup views (the `/api/graph` `granularity` views, ADR-34). In dogfooding
  // this read as broken — zooming out to "see the whole graph" silently replaced the
  // node set (symbol ids → file paths → module keys), dropping the selection and
  // reshaping the canvas, when the user only wanted the camera to pull back.
  //
  // The ladder is now FLAT: zoom never changes the semantic tier — it stays `symbol`
  // (the visualization view) at every zoom level, so zoom is pure camera distance and
  // the node set is stable. The rollup views remain implemented server-side (the
  // `granularity` param still works) but are no longer driven by zoom; a future
  // explicit tier control could surface them without re-coupling them to the camera.
  var GRANULARITY_LADDER = [
    { below: Infinity, granularity: "symbol" }, // every zoom level — zoom is camera-only
  ];

  // Human-facing tier names for the legend's tier indicator (S-124).
  var TIER_LABEL = { module: "Modules", file: "Files", symbol: "Symbols" };

  // The semantic tier for a tracked absolute zoom — the cluster-zoom ladder lookup.
  // Pure: the single source mapping zoom → the `granularity` parameter.
  function granularityForZoom(zoom) {
    for (var i = 0; i < GRANULARITY_LADDER.length; i++) {
      if (zoom < GRANULARITY_LADDER[i].below) {
        return GRANULARITY_LADDER[i].granularity;
      }
    }
    return GRANULARITY_LADDER[GRANULARITY_LADDER.length - 1].granularity;
  }

  // Name the current tier in the legend's tier indicator (S-124). Tolerant of the
  // element being absent (no-JS / partial DOM): a missing indicator is not updated.
  function setTierIndicator(granularity) {
    var el = document.getElementById("graph-tier-name");
    if (el) {
      el.textContent = TIER_LABEL[granularity] || TIER_LABEL.symbol;
    }
  }

  // The manual `cap` override, read from the page URL's existing `?cap=` parameter
  // (FR-UI-08; the same parameter the server honours). An explicit `?cap=` is a
  // genuine user intent signal — distinct from the server's always-present resolved
  // default on `data-cap` — so it is read straight from the URL the canvas opened
  // on. `0` means "no override", in which case the zoom ladder drives the budget.
  // A URL `cap` of 0 (or absent) yields 0 = "no override": a zero budget has no
  // meaningful server-side effect, so it is treated as absent and the zoom ladder
  // drives the budget. (Keep 0 as the sentinel — `endpoint`'s `if (cap)` guard and
  // the override checks all rely on 0 being falsy.)
  function manualCapFromUrl() {
    var m = /[?&]cap=(\d+)/.exec(window.location.search || "");
    return m ? Number(m[1]) : 0;
  }

  // The budget at home zoom: a manual override pins it, else the LOD ladder's home
  // tier (capForZoom at zoom 1). Used by every scope change that returns the canvas
  // to home zoom, so the S-123 zoom↔budget lockstep is set in exactly one place.
  function budgetAtHomeZoom() {
    return state.capOverride || capForZoom(1);
  }

  // ── Exploration state (S-086) ────────────────────────────────────────────────
  // The single source of truth for which loaded elements are visible. `applyView`
  // recomputes the rendered series from it after any control change; focus/reset
  // re-fetch the element set, the filters only re-derive what is already loaded.
  var state = {
    seed: "", // the server scope the canvas opened on ("" = whole graph)
    cap: 0, // the *effective* visible-element budget forwarded to `/api/graph`:
    //         the zoom-derived LOD cap (capForZoom) at rest, or `capOverride` when
    //         a manual `?cap=` override is in force (S-123). Replaces the old
    //         static "forward the server's one resolved cap forever" model.
    capOverride: 0, // a manual `?cap=` override from the URL; 0 = none → zoom drives it
    granularity: "symbol", // the active semantic tier (S-127): "symbol"/"file"/"module",
    //                        owned by the explicit Symbols/Files/Modules control
    //                        (setGranularity) and camera-INDEPENDENT — zoom never changes
    //                        it (CR-032). "symbol" = the visualization view (the default).
    focusId: null, // the focus-mode node id (null = whole graph / unfocused)
    lockedId: null, // the explicitly *locked* selection (S-119): a click locks it
    //                 (persistent ring + adjacency emphasis + panel refresh),
    //                 a re-click unlocks, a click elsewhere switches.
    locatedId: null, // the search-to-locate / transient highlight node id
    layers: { code: true, doc: true, artifact: true }, // layer toggle → shown?
    edgeTypes: {}, // edge_type → shown? (populated from the loaded elements)
    intent: false, // the off-by-default "Intent / governing docs" overlay (S-129,
    //                 FR-UI-16, ADR-37): when true the fetch carries `?intent=1` to
    //                 activate S-128's bounded intent-overlay budget (governing-doc
    //                 nodes adjacent to the visible code survive the code-degree cap)
    //                 and the doc layer + DocReference/TracesTo edge types are driven
    //                 on. Default false ⇒ the param is omitted ⇒ the fetch is
    //                 byte-identical to today (the structural graph is unchanged).
    depth: 0, // hops from the focus node to keep; 0 = unlimited
    elidedNodes: 0, // nodes the server held back past the cap (the "N more" count)
    elidedEdges: 0, // edges the server held back past the cap
  };

  // The master loaded element set (everything fetched so far, deduped by id).
  // Filters/focus/locate derive the rendered series from this without re-fetching.
  var loaded = {
    nodes: {}, // id → { id, label, kind, layer }
    edges: [], // [{ source, target, edge_type }], deduped by edgeKey
    edgeKeys: {}, // edgeKey → true
  };

  // Live zoom tracked across roam events (ECharts reports a per-gesture scale
  // factor, not an absolute) so label level-of-detail can gate on absolute zoom.
  var zoomLevel = 1;
  var labelsShown = true;
  // The last zoom limit announced on the status line ("out" | "in" | null), so
  // sitting at a limit (which fires repeated roam events) does not re-spam it —
  // the message is set once on the transition into a limit (S-126).
  var zoomLimitNotified = null;
  // Set true only while a `+`/`−` button dispatches its programmatic `graphRoam`,
  // so the `graphroam` event handler skips the gesture the button already counted.
  var programmaticRoam = false;
  // Pending zoom→budget re-fetch timer (S-123): a tier crossing schedules a single
  // debounced re-fetch; a fresh crossing within the window cancels and reschedules.
  var budgetRefetchTimer = null;
  // Monotonic generation for the async budget re-fetch: bumped per commit and on
  // every view reset, so an in-flight fetch whose generation is stale drops its
  // result instead of clobbering a newer budget/scope (review: in-flight race).
  var budgetGen = 0;

  function init() {
    var mount = document.getElementById("graph-canvas");
    if (!mount) {
      return; // not the Graph view
    }
    // Progressive enhancement: if the vendored library somehow failed to load,
    // collapse the empty canvas and surface an honest notice. (The <noscript>
    // twin only renders with scripting *disabled*, so it cannot cover this
    // scripting-on-but-library-missing case — say so rather than show a blank box.)
    if (!window.echarts || typeof window.echarts.init !== "function") {
      mount.classList.add("graph-canvas-unavailable");
      var notice = document.getElementById("graph-cap-notice");
      if (notice) {
        notice.textContent = "Interactive graph unavailable — the graph library did not load.";
        notice.hidden = false;
      }
      return;
    }

    state.seed = mount.getAttribute("data-seed") || "";
    // S-123: the budget is zoom-driven, not the server's one static `data-cap`. A
    // manual `?cap=` override (if present in the URL) wins; otherwise the LOD ladder
    // sets the opening budget at the home zoom (the accumulator boots at 1). The
    // server still exposes its resolved cap on `data-cap` for the no-JS twin, but
    // the interactive canvas now derives the budget from zoom.
    state.capOverride = manualCapFromUrl();
    state.cap = state.capOverride || capForZoom(zoomLevel);
    // S-127/CR-032: the semantic tier boots at `symbol` (the flat ladder's only entry,
    // so granularityForZoom(1) === "symbol") — the opening fetch carries no `granularity`
    // param and is byte-identical to the pre-rollup visualization snapshot. The tier is
    // camera-INDEPENDENT thereafter: zoom never changes it; only the explicit
    // Symbols/Files/Modules control (setGranularity) does.
    state.granularity = granularityForZoom(zoomLevel);
    setTierIndicator(state.granularity);
    // Arriving via `?seed=` opens directly in focus mode on that node.
    state.focusId = state.seed || null;

    fetchElements(state.seed, state.cap)
      .then(function (data) {
        setLoaded(data);
        var chart = render(mount);
        applyCapNotice(data);
        wireNodeClick(chart);
        setupControls(chart);
        // applyView → rebuildSeries commits the series and clears the busy overlay
        // (Issue 3): the overlay is dropped the moment the data is on the canvas, not
        // held through the force-layout settle (which is interactive feedback, not a
        // freeze). See `rebuildSeries`.
        applyView(chart);
      })
      .catch(function () {
        // A failed fetch is reported in the notice line, never silently swallowed.
        var notice = document.getElementById("graph-cap-notice");
        if (notice) {
          notice.textContent = "Could not load the graph elements.";
          notice.hidden = false;
        }
      });
  }

  // ── Data ──────────────────────────────────────────────────────────────────

  function endpoint(seed, cap) {
    var params = [];
    if (seed) {
      params.push("seed=" + encodeURIComponent(seed));
    }
    if (cap) {
      params.push("cap=" + encodeURIComponent(cap));
    }
    // S-122: the layer/edge-type filters are server-side *re-budgeting* params
    // now, not client-side hides — every fetch (boot, focus, reset, expand, and
    // the toggle re-fetch) carries the current selection so the server re-spends
    // the visible-element budget over the remaining scope. Omitted when nothing is
    // deselected (all-on ⇒ no param ⇒ the whole, unfiltered scope).
    var layers = activeLayers();
    if (layers !== null) {
      params.push("layers=" + encodeURIComponent(layers.join(",")));
    }
    var edges = activeEdgeTypes();
    if (edges !== null) {
      params.push("edge_types=" + encodeURIComponent(edges.join(",")));
    }
    // S-124: carry the semantic cluster-zoom tier so the server reads the matching
    // rollup view. Omitted at the default `symbol` tier so the request stays
    // byte-identical to the pre-S-124 visualization fetch (server default = symbol).
    if (state.granularity && state.granularity !== "symbol") {
      params.push("granularity=" + encodeURIComponent(state.granularity));
    }
    // S-129 (FR-UI-16, ADR-37): the off-by-default "Intent / governing docs" toggle
    // activates S-128's bounded intent-overlay budget across the API boundary via
    // `?intent=1` (truthy 1/true/on/yes — see web/src/lib.rs::parse_intent). The
    // overlay admits the governing-doc nodes adjacent to the kept code anchors up to
    // a separate reserved budget, computed outside the structural degree ranking, so
    // the DocReference/TracesTo edges and their doc nodes survive the code-degree cap.
    // Omitted when off so the request is byte-identical to today (the no-JS twin
    // always omits it — the overlay is the scripted enhancement only, ADR-29).
    if (state.intent) {
      params.push("intent=1");
    }
    return "/api/graph" + (params.length ? "?" + params.join("&") : "");
  }

  // The enabled layers as a wire list, or `null` when every layer is on (so the
  // request omits the filter and the server returns the whole scope). An explicit
  // empty array (all three deselected) is sent as `layers=` so the server honestly
  // returns the empty graph rather than defaulting back to all layers.
  function activeLayers() {
    var all = ["code", "doc", "artifact"];
    var on = all.filter(function (l) {
      return state.layers[l] !== false;
    });
    return on.length === all.length ? null : on;
  }

  // The enabled edge types as a wire list, or `null` when none are known yet or
  // every known type is on (omit the filter). Once the user deselects at least one
  // type, the enabled subset is sent (possibly empty — "hide every edge"), so the
  // server re-budgets the degree ranking over the remaining edge structure.
  function activeEdgeTypes() {
    var keys = Object.keys(state.edgeTypes);
    if (!keys.length) {
      return null;
    }
    var anyOff = keys.some(function (t) {
      return state.edgeTypes[t] === false;
    });
    if (!anyOff) {
      return null;
    }
    return keys.filter(function (t) {
      return state.edgeTypes[t] !== false;
    });
  }

  function fetchElements(seed, cap) {
    // Every graph fetch funnels through here, so the busy overlay is reference-counted
    // here (Issue 3): a fetch — the network round-trip plus the server re-budgeting
    // compute — can leave the canvas blank for a beat on a large graph. We raise the
    // count once and release it once when THIS fetch settles, whatever its outcome:
    // resolved, rejected, or later dropped by a generation guard downstream. Pairing
    // the release with the fetch (not with a commit) is what makes the overlay
    // leak-proof — a result the caller discards still releases its slot. `release`
    // runs exactly once (the two-arg `then` is an ES5-safe `finally`).
    setBusy(true);
    var released = false;
    function release(passthrough, isError) {
      if (!released) {
        released = true;
        setBusy(false);
      }
      if (isError) {
        throw passthrough;
      }
      return passthrough;
    }
    // Same-origin only; no credentials, no external origin (NFR-SE-01).
    return fetch(endpoint(seed, cap), {
      headers: { Accept: "application/json" },
    })
      .then(function (resp) {
        if (!resp.ok) {
          throw new Error("graph elements " + resp.status);
        }
        return resp.json();
      })
      .then(
        function (json) {
          return release(json, false);
        },
        function (err) {
          return release(err, true);
        }
      );
  }

  function edgeKey(e) {
    return e.source + "->" + e.target + ":" + e.edge_type;
  }

  // Swap the "Decisions & docs" panel to reflect the selected node (S-116). The
  // swap is delegated to htmx (`htmx.ajax`) — the same same-origin fragment-swap
  // mechanism the live-search boxes use — so this canvas script assembles no
  // markup itself: it points htmx at the read-only `/api/impact` endpoint and
  // htmx fetches the server-rendered fragment (seed-scoped; an empty seed renders
  // the opening prompt) and replaces `#decisions-panel`'s content. Best-effort:
  // htmx does not swap on a non-2xx response, so a failed fetch leaves the
  // existing panel in place rather than blanking it or fabricating one.
  function updateDecisions(seed) {
    if (!window.htmx || typeof window.htmx.ajax !== "function") {
      return; // htmx always loads before this script; degrade quietly if not
    }
    if (!document.getElementById("decisions-panel")) {
      return;
    }
    var url = "/api/impact" + (seed ? "?seed=" + encodeURIComponent(seed) : "");
    window.htmx.ajax("GET", url, { target: "#decisions-panel", swap: "innerHTML" });
  }

  // Replace the master loaded set with a freshly-fetched snapshot (focus/reset).
  function setLoaded(data) {
    loaded.nodes = {};
    loaded.edges = [];
    loaded.edgeKeys = {};
    mergeLoaded(data);
  }

  // Merge a fetched snapshot into the master set, deduping nodes by id and edges
  // by their (source,target,edge_type) key. Returns the count of newly-added
  // nodes and edges ({ nodes, edges }) so the expand control can draw down the
  // "N more not shown" counter by exactly what it admitted.
  function mergeLoaded(data) {
    var addedNodes = 0;
    var addedEdges = 0;
    (data.nodes || []).forEach(function (n) {
      if (!loaded.nodes[n.id]) {
        loaded.nodes[n.id] = {
          id: n.id,
          label: n.label,
          kind: n.kind,
          layer: n.layer,
        };
        addedNodes++;
      }
    });
    (data.edges || []).forEach(function (e) {
      var key = edgeKey(e);
      if (!loaded.edgeKeys[key]) {
        loaded.edgeKeys[key] = true;
        loaded.edges.push({ source: e.source, target: e.target, edge_type: e.edge_type });
        addedEdges++;
      }
    });
    return { nodes: addedNodes, edges: addedEdges };
  }

  // Re-fetch the current scope with the active layer/edge filters applied
  // server-side (S-122). This is the layer/edge toggle's replacement for the old
  // subtractive client-side hide: it reuses the Focus/Reset fetch path
  // (`fetchElements` → `setLoaded`), so the server re-spends the visible-element
  // budget over the remaining scope and previously-elided nodes backfill the freed
  // slots — the graph stays full, not merely smaller. The honest "N more not
  // shown" notice re-bases on the filtered snapshot's own elided counts. The
  // locked selection is kept across the re-fetch where it survives the filter
  // (matching the Focus/Reset reshuffle); `applyView` drops its adjacency emphasis
  // if the locked node was filtered out.
  function refetchWithFilters(chart) {
    // The current server scope: the focus node when focused, else the whole graph.
    var scope = state.focusId || "";
    fetchElements(scope, state.cap)
      .then(function (data) {
        setLoaded(data);
        refreshEdgeFilters();
        applyView(chart);
        applyCapNotice(data);
      })
      .catch(function () {
        notice("Could not apply the filter.");
      });
  }

  // The doc-layer / intent-edge-type selections the overlay forced on, captured at
  // activation so the OFF transition (and Reset) can restore them — the intent
  // overlay is reversible and must not permanently discard a prior deselection
  // (review-fix S-129). `null` whenever the overlay is off.
  var intentSavedSelection = null;

  // Turn the documentation intent overlay on or off (S-129, FR-UI-16, ADR-37). The
  // overlay is activated server-side by the `?intent=1` param `endpoint()` now
  // carries when `state.intent` is set (S-128's bounded budget). Turning it ON also
  // *drives the existing layers/edge_types params* (FR-UI-16 part 1): the doc layer
  // and the DocReference/TracesTo edge types are forced on so a prior layer/edge
  // deselection cannot hide the very intent edges the overlay surfaces. Either way it
  // re-fetches through the shared server-side re-budgeting path (refetchWithFilters),
  // so the "N more not shown" notice re-bases on the new snapshot's own elided counts
  // — which S-128 widens to account for the overlay scope (NFR-CC-04). Turning it OFF
  // drops `?intent=1`, restores the layer/edge selections activation forced on, and
  // re-fetches, returning the canvas to today's graph.
  function setIntent(chart, on) {
    if (on) {
      forceIntentOn();
    } else {
      disableIntentOverlay();
    }
    refetchWithFilters(chart);
    notice(
      on
        ? "Intent / governing docs layer shown — DocReference/TracesTo edges and their governing-doc nodes are surfaced."
        : "Intent / governing docs layer hidden."
    );
  }

  // Turn the intent overlay ON and drive the layers/edge_types it depends on
  // (FR-UI-16 part 1) — the shared intent-on state-driving for BOTH the toolbar
  // toggle (setIntent) and the Decisions-panel pivot (pivotToGraph), so the two
  // activate the overlay identically. Idempotent: a second activation while the
  // overlay is already on is a no-op that PRESERVES the selection saved at the
  // original activation (so a pivot mid-overlay never overwrites the OFF-transition
  // restore target). The caller re-fetches afterwards (the state change is inert
  // until the next /api/graph request carries `?intent=1`).
  function forceIntentOn() {
    if (state.intent) {
      return; // already on — keep the selection saved at the first activation
    }
    state.intent = true;
    // Remember the pre-overlay doc-layer / intent-edge-type selections so the OFF
    // transition (and Reset) can restore them — the overlay is a clean, reversible
    // layer and must not silently discard a prior deselection (review-fix S-129).
    intentSavedSelection = {
      docLayer: state.layers.doc,
      docReference: state.edgeTypes.doc_reference,
      tracesTo: state.edgeTypes.traces_to,
    };
    // Drive the doc layer + DocReference/TracesTo edge types on (FR-UI-16 part 1)
    // so a prior layer/edge deselection cannot hide the intent edges the overlay
    // surfaces. The edge-type checkboxes follow state.edgeTypes via the post-fetch
    // refreshEdgeFilters rebuild; only the fixed-enum doc layer box is synced here.
    state.layers.doc = true;
    state.edgeTypes.doc_reference = true;
    state.edgeTypes.traces_to = true;
    syncDocLayerCheckbox(true);
  }

  // Pivot the canvas onto a governing-doc node from the Decisions & docs panel
  // (S-130, FR-UI-16, FR-NV-10, ADR-37). The pivot REUSES the existing seed-scoped
  // fetch + locked-selection path (`focusOn`) with the intent overlay forced ON, so
  // the documentation node's DocReference/TracesTo intent edges (FR-DG-04) are KEPT
  // — without intent the seed-scoped fetch on a doc node carries no overlay budget
  // and the rationale edges that make a governing-doc pivot meaningful could be
  // dropped (S-128). No new endpoint, query verb, or write path: the pivot is the
  // intent toggle + the existing focus path composed (read-only, same-origin,
  // ADR-28). An edgeless doc node simply seeds to its own small/empty neighbourhood
  // via focusOn's honest snapshot — never an error (NFR-CC-04, FR-NV-09).
  function pivotToGraph(chart, id) {
    if (!id) {
      return;
    }
    forceIntentOn(); // keep the doc node's intent edges via S-128's `?intent=1` budget
    syncIntentToggle(true); // the pivot is not the checkbox's own change event — sync it
    focusOn(chart, id); // the single seed-scoped fetch (now carrying ?intent=1) + lock
  }

  // Reflect the doc-layer checkbox state (the layer toggles are a fixed enum, not
  // rebuilt from the loaded set, so their DOM must be synced explicitly — unlike the
  // edge-type boxes, which refreshEdgeFilters rebuilds from state.edgeTypes). Safe
  // when the control is absent (no-JS / partial DOM): a missing box is not toggled.
  function syncDocLayerCheckbox(checked) {
    forEach(document.querySelectorAll(".graph-layer"), function (box) {
      if (box.value === "doc") {
        box.checked = checked;
      }
    });
  }

  // Reflect the #graph-intent toolbar checkbox. The toggle's own change event sets
  // it, but the Decisions-panel pivot (pivotToGraph, S-130) turns intent on WITHOUT
  // the checkbox being its trigger — so it must sync the box explicitly, keeping the
  // toolbar honest about the active overlay. Safe when the control is absent (no-JS /
  // partial DOM): a missing toggle is simply not touched.
  function syncIntentToggle(checked) {
    var box = document.getElementById("graph-intent");
    if (box) {
      box.checked = checked;
    }
  }

  // Restore an intent edge type to its pre-overlay value, dropping the key entirely
  // when it was unseen before (undefined) so refreshEdgeFilters re-seeds it from the
  // loaded set on the re-fetch as it naturally would have.
  function restoreEdgeType(type, prev) {
    if (prev === undefined) {
      delete state.edgeTypes[type];
    } else {
      state.edgeTypes[type] = prev;
    }
  }

  // Turn the intent overlay OFF and RESTORE the layer/edge-type selections activation
  // forced on, then sync the toolbar toggle — the single source of truth for disabling
  // the overlay, shared by the toolbar toggle (setIntent) and a tier switch
  // (setGranularity). Restoring the saved selection keeps the overlay a clean,
  // reversible layer: turning it off by EITHER path never silently discards a prior
  // doc-layer / intent-edge deselection (review-fix F-01). An edge type unseen before
  // activation (undefined) is dropped so refreshEdgeFilters re-seeds it from the loaded
  // set on the re-fetch. Pure state + DOM sync; the caller re-fetches.
  function disableIntentOverlay() {
    state.intent = false;
    if (intentSavedSelection) {
      state.layers.doc = intentSavedSelection.docLayer;
      restoreEdgeType("doc_reference", intentSavedSelection.docReference);
      restoreEdgeType("traces_to", intentSavedSelection.tracesTo);
      syncDocLayerCheckbox(state.layers.doc);
      intentSavedSelection = null;
    }
    syncIntentToggle(false);
  }

  // ── Render ──────────────────────────────────────────────────────────────────

  function render(mount) {
    var chart = window.echarts.init(mount, null, { renderer: "canvas" });
    chart.setOption(baseOption());
    // Keep the canvas crisp when the viewport (or sidebar collapse) resizes.
    window.addEventListener("resize", function () {
      chart.resize();
      // S-126: recompute the camera-only zoom-out floor and re-apply it to the live
      // series `scaleLimit` so the `−`/`+` buttons, scroll/pinch, and the series
      // bound stay on ONE floor across a resize (ADR-36 — they must never drift).
      recomputeScaleMin(chart);
    });
    // ECharts reports a per-gesture scale factor on zoom roam; accumulate it to
    // an absolute zoom and gate label level-of-detail on crossing the threshold.
    // The `+`/`−` buttons (S-119) drive the same accumulator via `zoomBy`, which
    // updates it directly; the guard below stops the programmatic roam those
    // buttons dispatch from being double-counted here.
    chart.on("graphroam", function (params) {
      if (programmaticRoam) {
        return; // a button-driven roam already updated the accumulator
      }
      if (params && params.zoom) {
        // Clamp to the canvas bounds (scaleLimit, below): over-scrolling at a
        // zoom limit must not drift the tracked zoom away from what is rendered,
        // or label level-of-detail would stay out of sync with the real zoom.
        zoomLevel = Math.max(SCALE_MIN, Math.min(SCALE_MAX, zoomLevel * params.zoom));
        applyZoomLOD(chart);
        // S-126: scroll/pinch shares the floor with the buttons — surface the same
        // zoom-limit feedback (disable `−`/`+` + the message) when a gesture lands
        // on the floor or the ceiling, rather than clamping silently.
        updateZoomLimitFeedback();
        // S-123: scroll/pinch feeds the same accumulator the `+`/`−` buttons do, so
        // crossing a density tier re-budgets the visible set (debounced).
        maybeRefetchForZoom(chart);
      }
    });
    return chart;
  }

  // Toggle the node labels on or off when the tracked absolute zoom crosses the
  // level-of-detail threshold — shared by the roam event and the `+`/`−` buttons
  // so both keep label detail in lockstep with the real zoom.
  function applyZoomLOD(chart) {
    var show = zoomLevel >= LABEL_ZOOM_THRESHOLD;
    if (show !== labelsShown) {
      labelsShown = show;
      chart.setOption({ series: [{ label: { show: show } }] });
    }
  }

  // The canvas centre in pixel space — the zoom ORIGIN every programmatic `graphRoam`
  // must carry (the `+`/`−` buttons and the post-rebuild zoom restore). ECharts' own
  // scroll/pinch gestures pass the cursor as the origin; a *dispatched* graphRoam
  // carries NONE unless we supply it, and the roam helper then computes the pan shift
  // as `(originX − x) × (zoom − 1)` → `NaN` when originX is undefined, which corrupts
  // the view transform and sets the centre to `[NaN, NaN]`, collapsing the whole graph
  // into a clump (the "`−` button zooms out too narrow" defect — verified against the
  // rendered coordinate-system centre, not an internal flag). Anchoring the origin at
  // the canvas centre makes a button press zoom about the middle of the view, matching
  // a centred gesture. Defensive fallback to (0,0) only if the mount is somehow absent
  // (no canvas → the dispatch is moot anyway).
  function roamOrigin() {
    var mount = document.getElementById("graph-canvas");
    return {
      originX: mount ? mount.clientWidth / 2 : 0,
      originY: mount ? mount.clientHeight / 2 : 0,
    };
  }

  // Zoom the canvas by `factor` from the `+`/`−` buttons (S-119), driving the same
  // accumulator scroll/pinch feed. The target is clamped to the canvas scale
  // bounds (so the buttons never zoom past `scaleLimit`), then ECharts is rolled
  // to it by dispatching its native `graphRoam` action with the *applied* relative
  // factor about the canvas centre (roamOrigin — a dispatched roam carries no origin
  // of its own; omitting it NaN-corrupts the view centre) — guarded so the resulting
  // `graphroam` event does not double-count what we already accumulated here. A press
  // already at a bound is a no-op.
  function zoomBy(chart, factor) {
    var target = Math.max(SCALE_MIN, Math.min(SCALE_MAX, zoomLevel * factor));
    if (target === zoomLevel) {
      return; // already at the bound — nothing to do
    }
    var applied = target / zoomLevel;
    var origin = roamOrigin();
    programmaticRoam = true;
    chart.dispatchAction({ type: "graphRoam", zoom: applied, originX: origin.originX, originY: origin.originY });
    programmaticRoam = false;
    zoomLevel = target;
    applyZoomLOD(chart);
    // S-126: surface the zoom-limit feedback (disable `−`/`+` + the "maximum zoom
    // reached" message) the moment a press lands on the floor or the ceiling.
    updateZoomLimitFeedback();
    // S-123: the buttons feed the same accumulator as scroll/pinch, so a press that
    // crosses a density tier re-budgets the visible set (debounced) just the same.
    maybeRefetchForZoom(chart);
  }

  // ── Zoom-limit feedback + the recomputable floor (S-126, CR-032, FR-UI-15) ─────
  // Honest zoom-limit feedback: at the zoom-out floor the `−` control is disabled
  // and a "maximum zoom-out reached" message is surfaced; the zoom-in ceiling
  // mirrors it on `+`. A control at its limit is visibly disabled rather than a
  // silently dead button (NFR-CC-04 honest-feedback discipline; verified against
  // rendered state — the button's `disabled` and the on-screen message — not an
  // internal flag, the lesson of the busy-overlay regression).
  function updateZoomLimitFeedback() {
    var atFloor = zoomLevel <= SCALE_MIN + ZOOM_EPSILON;
    var atCeiling = zoomLevel >= SCALE_MAX - ZOOM_EPSILON;
    setZoomDisabled("graph-zoom-out", atFloor);
    setZoomDisabled("graph-zoom-in", atCeiling);
    var limit = atFloor ? "out" : atCeiling ? "in" : null;
    if (limit === zoomLimitNotified) {
      return; // already announced this limit (or already clear) — no re-spam
    }
    zoomLimitNotified = limit;
    if (limit === "out") {
      notice("Maximum zoom-out reached — the whole graph is in view; zoom in to reveal more detail.");
    } else if (limit === "in") {
      notice("Maximum zoom-in reached — fully zoomed in.");
    } else {
      clearZoomLimitNotice();
    }
  }

  // Toggle a zoom control's `disabled` state, tolerant of a missing button (no-JS /
  // partial DOM): an absent control is simply not toggled.
  function setZoomDisabled(id, disabled) {
    var btn = document.getElementById(id);
    if (btn) {
      btn.disabled = disabled;
    }
  }

  // Clear the status line only when it still holds one of OUR zoom-limit messages,
  // so leaving a limit removes the stale message without clobbering an unrelated
  // notice (a lock/expand/query message takes precedence).
  function clearZoomLimitNotice() {
    var el = document.getElementById("graph-controls-notice");
    if (el && el.textContent.indexOf("Maximum zoom-") === 0) {
      el.textContent = "";
    }
  }

  // Recompute the camera-only zoom-out floor and re-apply it everywhere it is read
  // (S-126): the `−`/`+` buttons and scroll/pinch clamp against `SCALE_MIN`, and the
  // series `scaleLimit` carries it — recomputing in one place and re-applying it keeps
  // the three on ONE floor across a resize (ADR-36). The floor derives from constant
  // tokens today, so this is the *defensive* path for a future viewport/DPR-aware
  // MIN_NODE_PX: only when the floor genuinely moves do we push a fresh series option
  // (and clamp the tracked zoom up if the raised floor now sits above it). A no-op
  // resize must NOT re-push the series option — that could perturb the force layout
  // for nothing. Button state is refreshed unconditionally (it can change between
  // resizes via a programmatic zoom), independent of whether the floor moved.
  function recomputeScaleMin(chart) {
    var next = computeScaleMin();
    if (next !== SCALE_MIN) {
      SCALE_MIN = next;
      chart.setOption({ series: [{ scaleLimit: { min: SCALE_MIN, max: SCALE_MAX } }] });
      if (zoomLevel < SCALE_MIN) {
        zoomLevel = SCALE_MIN;
        applyZoomLOD(chart);
      }
    }
    updateZoomLimitFeedback();
  }

  // ── Zoom → budget re-fetch (S-123) ────────────────────────────────────────────
  // The tracked absolute zoom drives the effective `cap`. When a zoom gesture
  // crosses into a tier whose cap differs from the committed budget, schedule a
  // single debounced re-fetch of the CURRENT scope at the new cap. A manual `?cap=`
  // override pins the budget — zoom then only pans/zooms + drives label LOD, it
  // does not re-budget (the explicit user cap wins; zoom = breadth still composes
  // with it as plain navigation). Invariant (no override): `state.cap` always
  // equals `capForZoom(zoomLevel)` at rest, so this fires only on a real crossing
  // and never thrashes.
  function maybeRefetchForZoom(chart) {
    // Zoom drives the density budget ONLY — never the semantic tier (CR-032/S-127). The
    // S-124 zoom→tier coupling that swapped the node id-space out from under a zoom-out
    // is retired: the tier is camera-independent and owned by the explicit
    // Symbols/Files/Modules control (setGranularity). The CURRENT tier is carried
    // through unchanged, so a zoom crossing re-budgets the cap WITHIN the active tier
    // (a Files view stays Files as the camera moves). A manual `?cap=` override pins the
    // budget (capForZoom is ignored over it).
    var targetCap = state.capOverride ? state.capOverride : capForZoom(zoomLevel);
    var targetGran = state.granularity;
    if (targetCap === state.cap) {
      return; // still within the committed density tier — nothing to do (tier is camera-independent)
    }
    scheduleBudgetRefetch(chart, targetCap, targetGran);
  }

  // Debounce a budget/tier re-fetch: a flick across several tiers or an overshooting
  // pinch collapses to ONE re-fetch once the gesture settles (no fetch storm).
  function scheduleBudgetRefetch(chart, targetCap, targetGran) {
    if (budgetRefetchTimer) {
      clearTimeout(budgetRefetchTimer);
    }
    budgetRefetchTimer = setTimeout(function () {
      budgetRefetchTimer = null;
      commitBudget(chart, targetCap, targetGran);
    }, ZOOM_REFETCH_DEBOUNCE_MS);
  }

  // Commit a new zoom-derived budget: re-fetch the CURRENT scope (whole graph or
  // the focused neighbourhood) at `targetCap`. The server degree-ranks+truncates
  // to it, so the direction of the change decides how the loaded set moves:
  //   - widening (zoom in, larger cap) MERGES the new snapshot — the stable degree
  //     ranking makes it a superset, so this admits exactly the newly-revealed
  //     lower-degree tier while keeping the locked selection and any expanded
  //     neighbours (zoom = breadth composing with Expand = local depth);
  //   - narrowing (zoom out, smaller cap) REPLACES with the smaller snapshot, so
  //     the graph honestly collapses to the highest-degree hubs.
  // Either way the "N more not shown" notice re-bases on the new budget's own
  // elided counts (NFR-CC-04 — honest across the budget change), and the canvas
  // stays at the user's zoom (the series replace reset roam; we restore it).
  function commitBudget(chart, targetCap, targetGran) {
    // `widening` and `tierChanged` read the OLD committed state — they MUST be read
    // before the reassignments below, or the direction/tier checks would read equal.
    var widening = targetCap > state.cap;
    var tierChanged = targetGran !== state.granularity;
    state.cap = targetCap;
    state.granularity = targetGran;
    // NOTE (S-127/CR-032): `commitBudget` is reached ONLY from the zoom path
    // (maybeRefetchForZoom → scheduleBudgetRefetch), which now always passes
    // `targetGran = state.granularity` — so `tierChanged` is structurally always false
    // here and the branch below is unreachable from any current caller. It is retained
    // (rather than deleted) as the defensive handler for a hypothetical future
    // tier-changing re-budget caller; the deliberate tier switch goes through
    // `setGranularity` instead (which also syncs the toolbar select, a step this dormant
    // branch omits). Zoom can never change the tier.
    // A semantic-tier change (S-124) swaps the node id-space — symbol ids ⇄ file
    // paths / module keys — so the loaded set cannot be MERGED across it, and a
    // symbol scope cannot carry to a rollup tier (it would not resolve → an honest
    // empty snapshot). The altitude change therefore re-fetches the WHOLE graph at
    // the new tier and clears the scope/selection/filters, which the per-tier id
    // space invalidates anyway; that whole-tier transition IS "a cluster expands
    // into its member symbols on zoom-in". Within ONE tier the S-123 budget
    // merge/replace-by-direction is unchanged.
    if (tierChanged) {
      clearScopeForTierChange();
      setTierIndicator(state.granularity);
    }
    var scope = tierChanged ? "" : state.focusId || "";
    var gen = ++budgetGen; // this commit's generation; a later commit/reset supersedes it
    fetchElements(scope, targetCap)
      .then(function (data) {
        if (gen !== budgetGen) {
          return; // a newer re-budget (or a view reset) superseded this fetch — drop it
        }
        if (widening && !tierChanged) {
          mergeLoaded(data);
        } else {
          setLoaded(data);
        }
        refreshEdgeFilters();
        rebuildSeries(chart, false); // keep the user's zoom — zoom drove this fetch
        applyCapNotice(data);
      })
      .catch(function () {
        /* a re-budget is best-effort; the existing canvas stays usable */
      });
  }

  // Clear the scope, selection, and per-tier filter state on a semantic-tier change
  // (S-124): the new tier's node id-space differs, so a symbol scope, a locked
  // selection, and the edge-type set (rebuilt from the new tier's edges) cannot
  // carry across. The layer toggles persist (code/doc/artifact apply at any tier).
  // Mirrors the relevant resets of `resetToWholeGraph` minus the zoom/cap reset —
  // a tier change keeps the user's zoom (it drove the change).
  function clearScopeForTierChange() {
    updateDecisions("");
    state.focusId = null;
    state.lockedId = null;
    state.locatedId = null;
    state.depth = 0;
    state.edgeTypes = {};
    setExpandEnabled(false);
    var depthSel = document.getElementById("graph-depth");
    if (depthSel) {
      depthSel.value = "0";
    }
  }

  // ── Explicit semantic-tier control (S-127, CR-032, FR-UI-15, FR-DB-05, ADR-34) ────
  // Switch the semantic tier from the deliberate Symbols/Files/Modules control. This
  // selects the existing server-side `granularity` rollup view — `file`/`module` render
  // the file/module clusters, `symbol` the visualization view (ADR-34) — INDEPENDENTLY
  // of zoom: CR-032 retired the S-124 zoom→tier coupling, so the dormant rollup tiers
  // are reachable ONLY through this control, never the camera. A switch swaps the node
  // id-space (symbol ids ⇄ file paths / module keys), so the prior scope, locked
  // selection, and per-tier edge-type set cannot carry — it REUSES the existing
  // tier-change scope-clear path (clearScopeForTierChange: no stale ring, no orphaned
  // lock), re-fetches the WHOLE graph at the new tier and the home-zoom budget, and
  // returns the canvas to home zoom (applyView) so the zoom↔cap invariant holds. The
  // legend names the active tier (setTierIndicator) and the "N more not shown" notice
  // re-bases on the new tier's snapshot (applyCapNotice, NFR-CC-04). The rollup views
  // exclude documentation files/modules and are metric-neutral server-side (FR-DG-06,
  // ADR-34, FR-QM-08); the path stays read-only and same-origin (ADR-28).
  function setGranularity(chart, tier) {
    if (!TIER_LABEL[tier] || tier === state.granularity) {
      return; // unknown tier, or already on it — a no-op (no needless re-fetch)
    }
    // A deliberate tier switch returns to home zoom — abandon any pending zoom→budget
    // re-fetch so a stale tier-crossing cannot fire against the new tier (mirrors the
    // rebuildSeries reset guard); the gen bump below supersedes any in-flight one.
    if (budgetRefetchTimer) {
      clearTimeout(budgetRefetchTimer);
      budgetRefetchTimer = null;
    }
    state.granularity = tier;
    clearScopeForTierChange(); // swap the id-space: clear scope/selection/filters cleanly
    // A tier switch disables the intent overlay. The overlay admits governing-doc
    // nodes adjacent to visible code SYMBOLS (S-128's `?intent=1` budget), which is
    // undefined at the file/module rollup tiers (no code symbols there; the rollup
    // views exclude doc files/modules, FR-DG-06) — so the new-tier fetch must not carry
    // a stale `?intent=1`. disableIntentOverlay() also RESTORES the pre-intent doc-layer
    // / edge-type selections (not merely drops them), so a doc-layer deselection made
    // before the overlay survives the tier switch — moot at file/module tiers (docs
    // excluded) and honoured back at Symbols (review-fix F-01).
    disableIntentOverlay();
    setTierIndicator(state.granularity); // name the active tier in the legend
    syncTierControl(state.granularity); // keep the toolbar select honest (programmatic callers)
    state.cap = budgetAtHomeZoom();
    var gen = ++budgetGen; // this switch's generation; supersedes any in-flight re-budget
    fetchElements("", state.cap)
      .then(function (data) {
        if (gen !== budgetGen) {
          return; // a newer re-budget / view reset superseded this fetch — drop it
        }
        setLoaded(data);
        refreshEdgeFilters();
        applyView(chart); // home zoom; the granularity is preserved (this control owns it)
        applyCapNotice(data); // the "N more not shown" notice re-bases on the new tier (NFR-CC-04)
      })
      .catch(function () {
        notice("Could not switch to the " + TIER_LABEL[tier] + " tier.");
      });
    notice(
      "Tier: " +
        TIER_LABEL[tier] +
        " — " +
        (tier === "symbol"
          ? "showing individual symbols (the default visualization view)."
          : "clustering by " + (tier === "file" ? "file" : "module") + ".")
    );
  }

  // Reflect the #graph-tier toolbar select. The select's own change event sets it, but
  // a programmatic tier change (Reset / a focus that returns to the symbol tier) must
  // sync it explicitly so the toolbar stays honest about the active tier. Safe when the
  // control is absent (no-JS / partial DOM): a missing select is simply not touched.
  function syncTierControl(tier) {
    var sel = document.getElementById("graph-tier");
    if (sel) {
      sel.value = tier;
    }
  }

  // The full chart option at boot. The graph series is rebuilt wholesale on every
  // view change (see `applyView`), so its static styling lives in `seriesSkeleton`
  // and is reproduced on each apply. Tooltip rendered on-canvas (richText) so no
  // markup is ever assembled from data.
  function baseOption() {
    var s = seriesSkeleton();
    s.data = [];
    s.links = [];
    return {
      backgroundColor: "transparent",
      tooltip: {
        show: true,
        renderMode: "richText",
        backgroundColor: COLOR.tooltipBg,
        borderWidth: 0,
        padding: [6, 10],
        textStyle: { color: "#ffffff", fontSize: 12 },
        formatter: tooltipText,
      },
      animationDurationUpdate: 400,
      series: [s],
    };
  }

  // The static styling of the graph series (everything except data/links/label),
  // reproduced on each apply because the series is replaced wholesale. Force
  // layout floats and settles; roam = pan + zoom; draggable nodes; a white rim +
  // soft shadow lift each node.
  //
  // S-119 loosens the force layout (more repulsion, longer edges, less gravity)
  // so the graph opens *sparser* and nodes are easier to pick out and click. The
  // hover adjacency-*dim* is gone: `emphasis.focus` is no longer `"adjacency"` and
  // there is no `blur`, so hovering never fades the rest of the graph — it keeps
  // only the lightweight tooltip + a gentle scale. Adjacency emphasis is instead
  // applied to the *locked* selection by `buildSeries` (per-element opacity keyed
  // on `state.lockedId`), so it persists and is driven by an explicit click.
  function seriesSkeleton() {
    return {
      type: "graph",
      layout: "force",
      roam: true,
      draggable: true,
      scaleLimit: { min: SCALE_MIN, max: SCALE_MAX },
      zoom: 1,
      force: {
        repulsion: 420,
        edgeLength: [120, 260],
        gravity: 0.035,
        friction: 0.18,
        layoutAnimation: true,
      },
      itemStyle: {
        borderColor: COLOR.nodeBorder,
        borderWidth: 1.5,
        shadowColor: "rgba(61,57,53,0.18)",
        shadowBlur: 4,
      },
      lineStyle: { color: COLOR.edge, width: 2, opacity: 0.7, curveness: 0.08 },
      emphasis: {
        focus: "none",
        scale: true,
        label: { show: true, fontWeight: 600 },
        lineStyle: { width: 3, opacity: 1 },
      },
      edgeSymbol: ["none", "arrow"],
      edgeSymbolSize: 6,
      label: labelStyle(true),
    };
  }

  // The node-label style; `show` is the level-of-detail gate computed per apply.
  function labelStyle(show) {
    return {
      show: show,
      position: "right",
      distance: 4,
      fontFamily: "JetBrains Mono, ui-monospace, monospace",
      fontSize: 11,
      color: COLOR.label,
      formatter: function (p) {
        return p.data && p.data.displayLabel ? p.data.displayLabel : p.name;
      },
    };
  }

  function tooltipText(p) {
    if (!p || !p.data) {
      return "";
    }
    if (p.dataType === "edge") {
      return p.data.edge_type ? prettify(p.data.edge_type) : "";
    }
    var label = p.data.displayLabel || p.name || "";
    var kind = p.data.kind ? prettify(p.data.kind) : "";
    return kind ? label + "\n" + kind : label;
  }

  // ── View computation ──────────────────────────────────────────────────────────

  function applyView(chart) {
    // A scope/set change (filter/focus/reset/depth/expand) rebuilds the series and
    // returns the canvas to home zoom — `rebuildSeries(true)` resets the accumulator
    // with it. Selection-only changes (lock/unlock/locate) use `restyleSeries`
    // instead, which preserves positions and the camera (see there).
    rebuildSeries(chart, true);
  }

  // Update only the visual encoding of the CURRENT node/edge set (lock, unlock,
  // locate) without replacing the series, so the force-layout positions AND the
  // user's current pan/zoom survive: selecting a node just restyles it (ring +
  // adjacency dim) and pins it to centre via `buildSeries`, rather than re-running
  // the whole layout and snapping the camera home (Issue: a lock used to reshuffle
  // the entire graph and fling the clicked node away). Only valid where the visible
  // node SET is unchanged — set-changing paths still take the full `applyView`
  // rebuild so stale nodes are removed (a plain merge cannot remove them).
  function restyleSeries(chart) {
    chart.setOption({ series: [buildSeries()] });
  }

  // Rebuild the graph series wholesale (the ECharts idiom for *removing* elements —
  // a plain merge would leave stale tail items). `replaceMerge` resets ECharts'
  // roam transform to zoom 1, so the tracked absolute zoom must follow or label
  // level-of-detail and the zoom→budget ladder would desync from the real canvas:
  //   - resetZoom=true (a view change): return the accumulator to home (1). When no
  //     manual override is in force, re-derive the committed budget from the home
  //     zoom too, so the S-123 invariant `state.cap === capForZoom(zoomLevel)` holds
  //     and the next zoom gesture re-budgets from a consistent baseline (no thrash).
  //   - resetZoom=false (a zoom-driven budget re-fetch): keep the accumulator where
  //     the user zoomed and restore the canvas roam to it, so re-budgeting does not
  //     snap the view back to home.
  function rebuildSeries(chart, resetZoom) {
    if (resetZoom) {
      // A view change returns to home zoom — abandon any pending OR in-flight
      // zoom→budget re-fetch so a stale tier-crossing (its debounce timer or an
      // already-dispatched fetch) cannot fire against this new baseline and
      // silently re-budget after the reset (review: stale-timer / in-flight race).
      if (budgetRefetchTimer) {
        clearTimeout(budgetRefetchTimer);
        budgetRefetchTimer = null;
      }
      budgetGen++;
      zoomLevel = 1;
      if (!state.capOverride) {
        state.cap = capForZoom(zoomLevel);
      }
      // S-127/CR-032: a view change returns to home ZOOM but the semantic tier must
      // SURVIVE a view rebuild — the tier is decoupled from the camera and owned by the
      // explicit Symbols/Files/Modules control, so a lock/depth/expand/filter at a
      // file/module tier must NOT snap back to Symbols. (Before the decoupling this line
      // re-derived the tier from the zoom-driven ladder; with the tier camera-independent
      // that would silently revert a deliberate rollup view on the next filter/depth
      // toggle.) The deliberate tier changes set state.granularity explicitly: focus/reset
      // return to the default `symbol` tier (a scope reset legitimately does), and
      // setGranularity owns every explicit switch. Zoom never derives it
      // (maybeRefetchForZoom carries the current tier through), so no spurious tier
      // crossing can fire after this reset.
    }
    chart.setOption({ series: [buildSeries()] }, { replaceMerge: ["series"] });
    if (!resetZoom && zoomLevel !== 1) {
      // The series replace reset roam to the skeleton's `zoom: 1`, so the tracked
      // absolute `zoomLevel` is the correct *relative* factor to roll back to from
      // 1 (graphRoam multiplies the current scale). Dispatched about the canvas centre
      // (roamOrigin) — a programmatic roam with no origin NaN-corrupts the view centre,
      // the same defect the `−` button hit. Guarded so the resulting `graphroam` event
      // is not double-counted into the accumulator.
      var origin = roamOrigin();
      programmaticRoam = true;
      chart.dispatchAction({ type: "graphRoam", zoom: zoomLevel, originX: origin.originX, originY: origin.originY });
      programmaticRoam = false;
    }
    // S-126: a view change returns to home zoom (1) — neither floor nor ceiling — so
    // re-enable both zoom controls and drop any stale zoom-limit message; a
    // zoom-driven re-fetch keeps the user's zoom, so this just reaffirms the state.
    updateZoomLimitFeedback();
  }

  function buildSeries() {
    var vis = visibleNodeIds();
    var degree = degreeMap(vis);
    // The canvas centre, in the layout pixel space the force simulation runs in
    // (Issue: the selection "flies away"). The focused/locked/located node is pinned
    // here so a re-layout — a lock, an expand, a focus — can never fling it out of
    // the visible area; it anchors at centre and its neighbourhood arranges around it.
    var mount = document.getElementById("graph-canvas");
    var center = mount ? [mount.clientWidth / 2, mount.clientHeight / 2] : [0, 0];
    // Adjacency emphasis for the *locked* selection only (S-119): when a node is
    // locked and visible, its neighbourhood (itself + directly-connected visible
    // nodes/edges) stays bright and everything else dims — the persistent,
    // click-driven replacement for the old hover adjacency-dim. `null` when no
    // node is locked (or the locked node is filtered out), so nothing dims.
    var adj =
      state.lockedId && vis[state.lockedId]
        ? adjacencySet(state.lockedId, vis)
        : null;
    var data = [];
    Object.keys(loaded.nodes).forEach(function (id) {
      if (vis[id]) {
        data.push(nodeOption(loaded.nodes[id], degree[id] || 0, adj, center));
      }
    });
    var links = [];
    loaded.edges.forEach(function (e) {
      // Edge-type filtering is server-side now (S-122): the loaded edge set already
      // excludes deselected types, so only the depth-visibility gate remains.
      if (vis[e.source] && vis[e.target]) {
        links.push(linkOption(e, adj));
      }
    });
    // Level-of-detail: labels show when the visible set is small enough to stay
    // legible, or once the user has zoomed in past the threshold.
    labelsShown = data.length <= LABEL_NODE_LIMIT || zoomLevel >= LABEL_ZOOM_THRESHOLD;
    var series = seriesSkeleton();
    series.label = labelStyle(labelsShown);
    series.data = data;
    series.links = links;
    return series;
  }

  // Which loaded node ids are visible. The layer/edge-type filters are now applied
  // server-side (S-122) — the loaded set already excludes the deselected layers and
  // edge types — so the only client-side narrowing left here is the focus-mode
  // depth bound (a hop limit over the loaded neighbourhood, S-086).
  function visibleNodeIds() {
    var depthSet =
      state.focusId && state.depth > 0
        ? nodeIdsWithinDepth(state.focusId, state.depth)
        : null;
    var ids = {};
    Object.keys(loaded.nodes).forEach(function (id) {
      if (!depthSet || depthSet[id] === true) {
        ids[id] = true;
      }
    });
    return ids;
  }

  // Degree of each visible node counting edges between two visible endpoints —
  // drives node sizing so hubs read larger in the rendered view. Edge-type
  // filtering is server-side now (S-122), so the loaded edge set is already the
  // enabled one.
  function degreeMap(vis) {
    var deg = {};
    loaded.edges.forEach(function (e) {
      if (vis[e.source] && vis[e.target]) {
        deg[e.source] = (deg[e.source] || 0) + 1;
        deg[e.target] = (deg[e.target] || 0) + 1;
      }
    });
    return deg;
  }

  function nodeOption(n, degree, adj, center) {
    var isFocus = n.id === state.focusId || n.id === state.seed;
    var isLocked = n.id === state.lockedId;
    var isLocated = n.id === state.locatedId;
    var isSelected = isLocked || isLocated;
    var isPinned = isFocus || isSelected;
    var size = Math.min(NODE_BASE + degree * 1.6, NODE_MAX);
    if (isPinned) {
      size = Math.max(size, NODE_FOCUS);
    }
    var opt = {
      id: n.id,
      name: n.id, // ECharts resolves links by node name → keep it the unique id
      displayLabel: n.label,
      kind: n.kind,
      layer: n.layer,
      symbolSize: size,
      // Always the layer fill — selection is a ring, not a fill swap (CR-030), so
      // a selected/located node keeps its layer color (blue/green/amber).
      itemStyle: { color: layerColor(n.layer) },
      // Pin the selection to the canvas centre so a re-layout never flings it out of
      // view (Issue: "the selected node tends to move out of the visible area"). The
      // force layout honours `fixed` + `x`/`y`: the focused/locked/located node holds
      // centre while its neighbours settle around it; every other node is explicitly
      // unpinned (`fixed: false`) so switching or clearing the selection releases the
      // prior anchor (a merge would otherwise keep the stale `fixed` flag).
      fixed: isPinned,
    };
    if (isPinned && center) {
      opt.x = center[0];
      opt.y = center[1];
    }
    if (isFocus || isSelected) {
      // The focused, locked-selected, or search-located node draws a persistent
      // red ring (paired with the size bump) so it pops while its layer color
      // survives under the ring — color is never the only signal (FR-UI-08,
      // S-119, frontend-design §7).
      opt.itemStyle.borderColor = COLOR.located;
      opt.itemStyle.borderWidth = 3;
    }
    // Adjacency emphasis (S-119): dim any node outside the locked node's
    // neighbourhood. The locked node, its neighbours, and an unrelated located
    // node stay fully opaque so the selection's neighbourhood reads clearly.
    if (adj && !adj[n.id] && !isSelected) {
      opt.itemStyle.opacity = 0.2;
    }
    return opt;
  }

  function linkOption(e, adj) {
    var forbidden = e.edge_type === "forbidden_dependency";
    // Adjacency emphasis (S-119): when a node is locked, an edge stays bright only
    // if it touches the locked node; every other edge dims so the locked node's
    // own connections stand out.
    var dimmed =
      adj && e.source !== state.lockedId && e.target !== state.lockedId;
    return {
      source: e.source,
      target: e.target,
      edge_type: e.edge_type,
      lineStyle: {
        // Colored by relationship type (CR-030); the typed line style is retained
        // as the redundant second channel. A visible ~2px base width keeps every
        // edge legible; a forbidden edge stays the heaviest/most opaque so a rules
        // violation still dominates.
        color: edgeColor(e.edge_type),
        type: EDGE_STYLE[e.edge_type] || "solid",
        width: forbidden ? 2.6 : 2,
        opacity: dimmed ? 0.1 : forbidden ? 0.95 : 0.82,
        curveness: 0.08,
      },
    };
  }

  // The locked node plus its directly-connected neighbours over the *visible* edge
  // set — the set kept bright while the rest of the graph dims (adjacency emphasis
  // for the locked selection, S-119). The returned id→true map always contains the
  // locked id itself. Edge-type filtering is server-side now (S-122), so the loaded
  // edge set is already the enabled one.
  function adjacencySet(lockedId, vis) {
    var set = {};
    set[lockedId] = true;
    loaded.edges.forEach(function (e) {
      if (!vis[e.source] || !vis[e.target]) {
        return;
      }
      if (e.source === lockedId) {
        set[e.target] = true;
      } else if (e.target === lockedId) {
        set[e.source] = true;
      }
    });
    return set;
  }

  // Breadth-first node ids within `depth` hops of `rootId` (undirected, so a
  // neighbourhood grows along edges in either direction). Returns an id→true map.
  function nodeIdsWithinDepth(rootId, depth) {
    var seen = {};
    seen[rootId] = true;
    var frontier = [rootId];
    var d = 0;
    // Adjacency from the loaded edge set (both directions).
    while (frontier.length && d < depth) {
      var next = [];
      for (var i = 0; i < frontier.length; i++) {
        var cur = frontier[i];
        loaded.edges.forEach(function (e) {
          // Edge-type filtering is server-side now (S-122): the loaded edge set is
          // already the enabled one, so every loaded edge is a real hop.
          if (e.source === cur && !seen[e.target]) {
            seen[e.target] = true;
            next.push(e.target);
          } else if (e.target === cur && !seen[e.source]) {
            seen[e.source] = true;
            next.push(e.source);
          }
        });
      }
      frontier = next;
      d++;
    }
    return seen;
  }

  // ── Level-of-detail notice ────────────────────────────────────────────────────

  // Record the server's elided counts from a fresh whole-graph/focus snapshot,
  // then render the "N more not shown" notice from them. Splitting record from
  // render lets the expand control re-render the same notice after it admits
  // previously-elided neighbours (see `decreaseElided` / `renderCapNotice`).
  function applyCapNotice(data) {
    state.elidedNodes = Number(data.elided_nodes) || 0;
    state.elidedEdges = Number(data.elided_edges) || 0;
    renderCapNotice();
  }

  // Draw down the tracked elided counts by what a click just admitted. The
  // admitted neighbours are drawn from the elided set, so the counter shrinks by
  // exactly that many; clamped at zero so it never goes negative if the server's
  // snapshot count and the merged delta drift (NFR-CC-04: never overstate what
  // stays hidden — but never silently truncate it either).
  function decreaseElided(nodes, edges) {
    state.elidedNodes = Math.max(0, state.elidedNodes - nodes);
    state.elidedEdges = Math.max(0, state.elidedEdges - edges);
  }

  // Render the "N more not shown" notice from the tracked elided counts. Hidden
  // once nothing remains hidden, so a fully-expanded graph shows no stale count.
  function renderCapNotice() {
    var notice = document.getElementById("graph-cap-notice");
    if (!notice) {
      return;
    }
    var nodes = state.elidedNodes;
    var edges = state.elidedEdges;
    if (nodes === 0 && edges === 0) {
      notice.hidden = true;
      return;
    }
    // At least one side is non-zero here (the 0/0 case returned above), so this
    // satisfies `elementPhrase`'s precondition — one source for the node/edge phrase.
    notice.textContent =
      elementPhrase(nodes, edges) +
      " not shown — lock a node and use “Expand neighbours” to reveal more.";
    notice.hidden = false;
  }

  // Lock-on-click (S-119): a canvas node click is now an explicit *selection*, not
  // an expansion. Clicking a node locks it — a persistent ring, adjacency emphasis
  // (`buildSeries` dims the rest), and a Decisions-panel refresh (S-116) — and
  // enables the "Expand neighbours" control for it. Clicking the locked node again
  // unlocks it (clears the ring/emphasis and the panel); clicking another switches
  // the lock. Expansion itself moved to that control (see `expandNeighbours`).
  function wireNodeClick(chart) {
    chart.on("click", function (params) {
      if (params.dataType !== "node" || !params.data) {
        return;
      }
      var id = params.data.id || params.name;
      if (state.lockedId === id) {
        unlockSelection(chart);
      } else {
        lockSelection(chart, id);
      }
    });
  }

  // Lock `id` as the selection: persist the ring + adjacency emphasis (via
  // `applyView`), refresh the Decisions panel (S-116), and enable the Expand
  // control. Switching from another locked node just re-points the lock.
  function lockSelection(chart, id) {
    state.lockedId = id;
    updateDecisions(id);
    setExpandEnabled(true);
    restyleSeries(chart); // selection-only: keep positions + camera, pin the lock to centre
    notice("Locked " + labelFor(id) + " — use “Expand neighbours” to grow it, or click it again to unlock.");
  }

  // Unlock the current selection: clear the ring/adjacency emphasis, reset the
  // Decisions panel to its opening prompt (S-116), and disable the Expand control.
  function unlockSelection(chart) {
    state.lockedId = null;
    // Also clear any located/focused ring (focusOn sets `locatedId` alongside the
    // lock): unlocking is a clean slate, so the persistent red ring must go too —
    // otherwise unlocking a focused node would leave its ring behind.
    state.locatedId = null;
    updateDecisions("");
    setExpandEnabled(false);
    restyleSeries(chart); // selection-only: clearing the lock keeps positions + camera
    notice("Selection unlocked.");
  }

  // Enable/disable the "Expand neighbours" control — it acts on the *locked* node,
  // so it is inert until a node is locked. Tolerant of the button being absent
  // (no-JS / partial DOM): a missing control is simply not toggled.
  function setExpandEnabled(enabled) {
    var btn = document.getElementById("graph-expand");
    if (btn) {
      btn.disabled = !enabled;
    }
  }

  // Expand neighbours (S-119, reusing the S-117 additive-expand path unchanged —
  // only re-triggered from the explicit control instead of a node click): pull the
  // locked node's neighbours from the same same-origin endpoint (seed-scoped) and
  // merge only elements not already drawn, admitting previously-elided neighbours
  // even when the whole-graph load already hit the visible-element cap. The
  // interaction is *additive* and always gives visible feedback:
  //   - new neighbours arrived → re-derive the view around the grown set (newly-
  //     arrived edge types join the edge-type filter, S-086), draw the
  //     "N more not shown" counter down by exactly what was admitted, and report
  //     the gain on the status line;
  //   - none remained → highlight the locked node and say so, rather than a silent
  //     no-op (FR-NV-09 — never leave the control looking dead).
  function expandNeighbours(chart, id) {
    if (!id) {
      notice("Lock a node first, then expand its neighbours.");
      return;
    }
    // Expand fetches at the *current* zoom-derived budget (`state.cap`) by design
    // (S-123: zoom = breadth, Expand = local depth) — zooming in first raises the
    // cap and admits more neighbours per expand.
    fetchElements(id, state.cap)
      .then(function (data) {
        var added = mergeLoaded(data);
        if (added.nodes > 0 || added.edges > 0) {
          // In focus mode, re-anchor the depth measurement to the expanded node
          // so the expand visibly grows outward — otherwise a depth limit would
          // re-hide the neighbours we just merged. Harmless when depth is
          // unlimited (0).
          if (state.focusId) {
            state.focusId = id;
          }
          refreshEdgeFilters();
          applyView(chart);
          // The admitted elements came from the elided set — shrink the
          // "N more not shown" counter by exactly that many and re-render it.
          decreaseElided(added.nodes, added.edges);
          renderCapNotice();
          notice("Expanded " + labelFor(id) + " — added " + elementPhrase(added.nodes, added.edges) + ".");
        } else {
          // Nothing new to admit: give explicit, visible feedback (highlight the
          // node + an honest status) instead of doing nothing (FR-NV-09).
          highlight(chart, id);
          notice("“" + labelFor(id) + "” is already fully expanded — no further neighbours to show.");
        }
      })
      .catch(function () {
        /* expansion is best-effort; the existing canvas stays usable */
      });
  }

  // "X node(s) and Y edge(s)" for the expand status line; omits a zero side. The
  // caller only invokes this when at least one side is non-zero.
  function elementPhrase(nodes, edges) {
    var parts = [];
    if (nodes > 0) {
      parts.push(nodes + (nodes === 1 ? " node" : " nodes"));
    }
    if (edges > 0) {
      parts.push(edges + (edges === 1 ? " edge" : " edges"));
    }
    return parts.join(" and ");
  }

  // ── Exploration controls (S-086) ─────────────────────────────────────────────

  // Wire the toolbar to the canvas and reveal it. Layer toggles are server-
  // rendered (fixed enum); edge-type toggles are built from the loaded elements.
  function setupControls(chart) {
    var controls = document.getElementById("graph-controls");
    if (!controls) {
      return;
    }
    refreshEdgeFilters();

    forEach(controls.querySelectorAll(".graph-layer"), function (box) {
      box.addEventListener("change", function () {
        state.layers[box.value] = box.checked;
        // S-122: a layer toggle re-fetches with the server-side re-budgeting filter
        // (so the freed budget backfills from the remaining layers) instead of
        // subtractively hiding the already-truncated client set.
        refetchWithFilters(chart);
      });
    });

    // S-129: the off-by-default "Intent / governing docs" toggle. Server-rendered
    // unchecked (so the canvas opens byte-identical to today); toggling it drives
    // the intent overlay on/off through the same server-side re-budgeting fetch.
    var intentToggle = document.getElementById("graph-intent");
    if (intentToggle) {
      intentToggle.addEventListener("change", function () {
        setIntent(chart, intentToggle.checked);
      });
    }

    // S-127: the explicit Symbols/Files/Modules semantic-tier control. It selects the
    // existing server-side `granularity` rollup view INDEPENDENTLY of zoom (CR-032
    // decoupled the tier from the camera) — choosing Files/Modules renders the
    // file/module clusters, Symbols the visualization view. Switching swaps the node
    // id-space and clears the prior scope/selection cleanly (setGranularity).
    var tierSelect = document.getElementById("graph-tier");
    if (tierSelect) {
      tierSelect.addEventListener("change", function () {
        setGranularity(chart, tierSelect.value);
      });
    }

    var depth = document.getElementById("graph-depth");
    if (depth) {
      depth.addEventListener("change", function () {
        state.depth = Number(depth.value) || 0;
        applyView(chart);
      });
    }

    // Structured + relational whole-graph query (S-120, replacing the old
    // visible-only locate): the Query button (and Enter in any query text field)
    // builds the same-origin /api/query URL and lists the ranked hits.
    var queryBtn = document.getElementById("graph-query-btn");
    if (queryBtn) {
      queryBtn.addEventListener("click", function () {
        runQuery(chart);
      });
    }
    forEach(
      controls.querySelectorAll(".graph-query-text, .graph-query-file, .graph-query-target"),
      function (input) {
        input.addEventListener("keydown", function (evt) {
          if (evt.key === "Enter") {
            evt.preventDefault();
            runQuery(chart);
          }
        });
      }
    );

    // Expand neighbours (S-119): the additive expand of the *locked* node. Starts
    // disabled (server-rendered `disabled`) and is enabled on lock — clicking it
    // runs the reused S-117 path on `state.lockedId`.
    var expand = document.getElementById("graph-expand");
    if (expand) {
      expand.addEventListener("click", function () {
        expandNeighbours(chart, state.lockedId);
      });
    }

    // Zoom buttons (S-119): `+`/`−` drive the same zoom accumulator that scroll /
    // pinch feed, clamped to the canvas scale bounds. Scroll/pinch keep working.
    var zoomIn = document.getElementById("graph-zoom-in");
    if (zoomIn) {
      zoomIn.addEventListener("click", function () {
        zoomBy(chart, ZOOM_STEP);
      });
    }
    var zoomOut = document.getElementById("graph-zoom-out");
    if (zoomOut) {
      zoomOut.addEventListener("click", function () {
        zoomBy(chart, 1 / ZOOM_STEP);
      });
    }

    var reset = document.getElementById("graph-reset");
    if (reset) {
      reset.addEventListener("click", function () {
        resetToWholeGraph(chart);
      });
    }

    // Click-to-focus / pivot-to-graph: an element carrying `data-graph-pivot` (the
    // Decisions-panel governing-doc pivot action, S-130) re-seeds the canvas on that
    // doc node KEEPING its intent edges; an element carrying `data-graph-focus` (the
    // Decisions-panel nodes, DSM cycle links) focuses the canvas without keeping the
    // intent overlay. The pivot is checked first (the more specific affordance). Both
    // keep a real `href` for the no-JS path, so we preventDefault only when scripting
    // is live and the canvas is present.
    document.addEventListener("click", function (evt) {
      var pivot = closestWithAttr(evt.target, "data-graph-pivot");
      if (pivot) {
        evt.preventDefault();
        pivotToGraph(chart, pivot.getAttribute("data-graph-pivot"));
        return;
      }
      var link = closestWithAttr(evt.target, "data-graph-focus");
      if (!link) {
        return;
      }
      evt.preventDefault();
      focusOn(chart, link.getAttribute("data-graph-focus"));
    });

    controls.hidden = false;
  }

  // Walk up from `el` to the nearest ancestor (inclusive) carrying `attr`, or null.
  // Shared by the click-to-focus and pivot-to-graph delegation so a click on a child
  // of the affordance (an inner badge/span) still resolves to the affordance element.
  function closestWithAttr(el, attr) {
    while (el && el.getAttribute) {
      if (el.getAttribute(attr)) {
        return el;
      }
      el = el.parentNode;
    }
    return null;
  }

  // Rebuild the edge-type checkboxes from the edge types currently loaded,
  // preserving any on/off choices the user already made and defaulting newly-seen
  // types to shown. The checkbox set is the UNION of the loaded types and the types
  // already known in `state.edgeTypes`: under server-side filtering (S-122) a
  // deselected type vanishes from the loaded edge set, so seeding only from
  // `loaded.edges` would drop its checkbox and strand it off — keeping the known
  // keys lets the user re-enable a type they just hid.
  function refreshEdgeFilters() {
    var container = document.getElementById("graph-edge-filters");
    if (!container) {
      return;
    }
    var types = {};
    Object.keys(state.edgeTypes).forEach(function (t) {
      types[t] = true;
    });
    loaded.edges.forEach(function (e) {
      // A rollup-cluster edge (module/file tier, S-124) carries no `edge_type` — it
      // aggregates mixed kinds, so it has no per-type filter checkbox; skip it.
      if (e.edge_type) {
        types[e.edge_type] = true;
      }
    });
    // Clear via the DOM, never innerHTML — no markup is ever assembled from data.
    while (container.firstChild) {
      container.removeChild(container.firstChild);
    }
    Object.keys(types)
      .sort()
      .forEach(function (type) {
        if (state.edgeTypes[type] === undefined) {
          state.edgeTypes[type] = true;
        }
        var label = document.createElement("label");
        var box = document.createElement("input");
        box.type = "checkbox";
        box.className = "graph-edge";
        box.value = type;
        box.checked = state.edgeTypes[type] !== false;
        box.addEventListener("change", function () {
          state.edgeTypes[type] = box.checked;
          var chart = window.echarts.getInstanceByDom(
            document.getElementById("graph-canvas")
          );
          if (chart) {
            // S-122: an edge-type toggle re-fetches with the server-side
            // re-budgeting filter (the degree ranking re-spends over the remaining
            // edge structure) instead of subtractively hiding loaded edges.
            refetchWithFilters(chart);
          }
        });
        label.appendChild(box);
        label.appendChild(document.createTextNode(" " + prettify(type)));
        container.appendChild(label);
      });
  }

  function prettify(type) {
    return String(type).replace(/_/g, " ");
  }

  // ── Focus mode ────────────────────────────────────────────────────────────────

  // Narrow the canvas to `id` and its neighbourhood: fetch the seed-scoped
  // element set (same-origin), swap it in, re-derive, and highlight the focus
  // node. The depth/layer/edge filters then refine this neighbourhood further.
  function focusOn(chart, id) {
    if (!id) {
      return;
    }
    // Reflect the focused node in the Decisions panel (S-116).
    updateDecisions(id);
    // A scope change opens at home zoom (applyView resets it), so fetch the
    // neighbourhood at the home-zoom budget — keeping `state.cap` in lockstep with
    // the zoom the canvas is about to reset to (S-123 invariant).
    state.cap = budgetAtHomeZoom();
    // S-124: focusing a symbol returns to the home (symbol) tier — the seed `id` is a
    // symbol id that only resolves in the symbol/visualization view, and home zoom is
    // the symbol tier. (Reached from Decisions-panel / query links, all symbol ids.)
    // S-127: a focus that returns to the symbol tier from a file/module rollup must sync
    // the explicit tier control too, so the toolbar stays honest about the active tier.
    state.granularity = "symbol";
    setTierIndicator(state.granularity);
    syncTierControl(state.granularity);
    fetchElements(id, state.cap)
      .then(function (data) {
        state.focusId = id;
        state.locatedId = id;
        // Focusing a node from a Decisions-panel/DSM link is also a *selection*:
        // lock it so the persistent ring + adjacency emphasis and the enabled
        // "Expand neighbours" control match the panel the focus just refreshed.
        state.lockedId = id;
        setExpandEnabled(true);
        setLoaded(data);
        refreshEdgeFilters();
        applyView(chart);
        // Re-base the "N more not shown" counter on the focus snapshot's own
        // elided counts (like resetToWholeGraph) — otherwise it would keep the
        // whole-graph counts and the expand control would draw down a number that
        // is meaningless for the focused neighbourhood (S-117 honesty, NFR-CC-04).
        applyCapNotice(data);
        highlight(chart, id);
        var count = (data.nodes || []).length;
        notice(
          "Focused on " +
            labelFor(id) +
            " — " +
            count +
            (count === 1 ? " node" : " nodes") +
            " in neighbourhood. Reset to return to the whole graph."
        );
      })
      .catch(function () {
        notice("Could not focus on that node.");
      });
  }

  // Reset returns to the whole graph and clears every control back to its
  // default (all layers, all edge types, unlimited depth, no focus/highlight).
  function resetToWholeGraph(chart) {
    // No node is selected on the whole graph — clear the Decisions panel to its
    // opening prompt eagerly (like focusOn), so the panel stays honest the moment
    // reset is pressed even if the graph reload below fails (S-116).
    updateDecisions("");
    // Clear the layer/edge filter state BEFORE the fetch: `endpoint` reads it to
    // build the re-budgeting params (S-122), so reset must drop them first or the
    // "whole graph" reload would still carry the old filters. Depth and the layer
    // checkboxes reset here too so the toolbar matches the unfiltered fetch.
    state.depth = 0;
    state.layers = { code: true, doc: true, artifact: true };
    state.edgeTypes = {};
    // Reset is the whole-graph default, which is intent OFF (S-129): clear the
    // overlay state and its saved selection so the reload is byte-identical to
    // today, and uncheck the toggle below so the control matches (review-fix:
    // otherwise Reset left the toggle visually off while fetches still carried
    // `?intent=1`, breaking the off ⇒ byte-identical invariant).
    state.intent = false;
    intentSavedSelection = null;
    var depthSel = document.getElementById("graph-depth");
    if (depthSel) {
      depthSel.value = "0";
    }
    forEach(document.querySelectorAll(".graph-layer"), function (box) {
      box.checked = true;
    });
    var intentToggle = document.getElementById("graph-intent");
    if (intentToggle) {
      intentToggle.checked = false;
    }
    // Reset returns to home zoom, so re-budget to the home-zoom cap too (S-123):
    // the whole graph reopens at the same density the canvas first booted on.
    state.cap = budgetAtHomeZoom();
    // S-124: reset also returns to the home (symbol) tier — the whole graph reopens
    // at the same altitude the canvas first booted on. S-127: sync the explicit tier
    // control to Symbols too, so Reset returns the toolbar (not just the canvas) to the
    // default tier.
    state.granularity = granularityForZoom(1);
    setTierIndicator(state.granularity);
    syncTierControl(state.granularity);
    fetchElements("", state.cap)
      .then(function (data) {
        state.focusId = null;
        state.locatedId = null;
        // Reset clears the locked selection too: no ring, no adjacency emphasis,
        // and the "Expand neighbours" control falls back to disabled (S-119).
        state.lockedId = null;
        setExpandEnabled(false);
        setLoaded(data);
        refreshEdgeFilters();
        applyView(chart);
        applyCapNotice(data);
        notice("Showing the whole graph.");
      })
      .catch(function () {
        notice("Could not reload the whole graph.");
      });
  }

  // ── Structured + relational query (S-120, FR-UI-14, ADR-35) ──────────────────
  // Replaces the old visible-only substring locate with a real *whole-graph* query
  // served by the same-origin, read-only /api/query endpoint: field filters
  // (a search term refined by kind/layer/file) and relational verbs (callers /
  // callees / impact of a symbol). The endpoint composes the engine's existing
  // read-only search + navigation read-models — this script only builds the URL,
  // lists the ranked hits, and centers/locks a selection. Results can include
  // nodes not currently on the canvas; selecting one brings it into view and locks
  // it via `focusOn` (the S-119 locked-selection mechanism — state.lockedId).

  // Build the /api/query URL from the query group. A chosen relational verb takes
  // precedence (verb + target); otherwise it is a field-filter query (a search
  // term plus optional kind/layer/file). Only non-empty inputs are sent.
  function queryUrl() {
    var params = [];
    function add(id, key) {
      var el = document.getElementById(id);
      var value = el && el.value ? el.value.trim() : "";
      if (value) {
        params.push(key + "=" + encodeURIComponent(value));
      }
    }
    var verbSel = document.getElementById("graph-query-verb");
    var verb = verbSel && verbSel.value ? verbSel.value : "";
    if (verb) {
      params.push("verb=" + encodeURIComponent(verb));
      add("graph-query-target", "target");
    } else {
      add("graph-query-text", "q");
      add("graph-query-kind", "kind");
      add("graph-query-layer", "layer");
      add("graph-query-file", "file");
    }
    return "/api/query" + (params.length ? "?" + params.join("&") : "");
  }

  // Run the query: fetch the ranked hits same-origin (no credentials, no external
  // origin — NFR-SE-01) and render them. A failed fetch is reported, never
  // silently swallowed.
  function runQuery(chart) {
    fetch(queryUrl(), { headers: { Accept: "application/json" } })
      .then(function (resp) {
        if (!resp.ok) {
          throw new Error("query " + resp.status);
        }
        return resp.json();
      })
      .then(function (data) {
        renderQueryResults(chart, data);
      })
      .catch(function () {
        notice("The query could not be run.");
      });
  }

  // List the ranked hits and center/lock the top one. The results list is built
  // with DOM methods only — never innerHTML, so no markup is assembled from data
  // (NFR-SE-01). Each row is a button that focuses+locks its node on click
  // (`focusOn` → state.lockedId), bringing a whole-graph hit that is not currently
  // on the canvas into view. An empty result renders the server's honest
  // "no matches" note, never an error (FR-NV-09, NFR-CC-04).
  function renderQueryResults(chart, data) {
    var box = document.getElementById("graph-query-results");
    if (box) {
      while (box.firstChild) {
        box.removeChild(box.firstChild);
      }
    }
    var hits = (data && data.hits) || [];
    if (!hits.length) {
      var message = (data && data.note) || "No matches.";
      if (box) {
        var empty = document.createElement("p");
        empty.className = "muted graph-query-empty";
        empty.textContent = message;
        box.appendChild(empty);
      }
      notice(message);
      return;
    }
    if (box) {
      var list = document.createElement("ul");
      list.className = "graph-query-hits";
      hits.forEach(function (hit) {
        var item = document.createElement("li");
        var btn = document.createElement("button");
        btn.type = "button";
        btn.className = "graph-query-hit";
        btn.setAttribute("data-query-id", hit.id);
        var label =
          hit.rank +
          ". " +
          (hit.label || hit.id) +
          " · " +
          prettify(hit.kind || "") +
          (hit.file ? " · " + hit.file : "");
        btn.textContent = label;
        btn.addEventListener("click", function () {
          focusOn(chart, hit.id);
        });
        item.appendChild(btn);
        list.appendChild(item);
      });
      box.appendChild(list);
    }
    var total = Number(data.total) || hits.length;
    var summary = hits.length + (hits.length === 1 ? " match" : " matches");
    if (total > hits.length) {
      summary += " (showing " + hits.length + " of " + total + ")";
    }
    notice(summary + " — select a result to center and lock it.");
    // Center + highlight the top-ranked hit immediately (the listed hits are
    // "highlighted/centered"); selecting any row re-centers and locks it.
    focusOn(chart, hits[0].id);
  }

  // Highlight one node (the located/focused target): re-style it with the located
  // ring, then dispatch ECharts' built-in highlight so it scales/bolds — the
  // canvas equivalent of "center + highlight". Since S-119 set `emphasis.focus` to
  // "none" (hover no longer dims), this lifts only the target node; fading of the
  // rest is the *locked* adjacency dim in `buildSeries`, not this dispatch.
  function highlight(chart, id) {
    state.locatedId = id;
    // Selection-only restyle: the located node's ring + centre-pin apply over the
    // layout the preceding focus/expand already committed, without a second
    // re-layout or camera snap.
    restyleSeries(chart);
    chart.dispatchAction({ type: "downplay", seriesIndex: 0 });
    chart.dispatchAction({ type: "highlight", seriesIndex: 0, name: id });
  }

  // ── Helpers ───────────────────────────────────────────────────────────────────

  function labelFor(id) {
    var n = loaded.nodes[id];
    return n && n.label ? n.label : id;
  }

  function notice(message) {
    var el = document.getElementById("graph-controls-notice");
    if (el) {
      el.textContent = message || "";
    }
  }

  // The canvas busy overlay (Issue 3), reference-counted over outstanding fetches.
  // A plain boolean broke when fetches overlapped: an in-flight fetch dropped by the
  // generation guard (`commitBudget`) never cleared the flag, so the spinner could
  // stay up forever (dogfood report — "the spinner keeps staying on the screen"). The
  // counter makes the overlay show iff at least ONE fetch is outstanding: every
  // `fetchElements` raises it once and releases it once when it settles (success,
  // error, OR a superseded/dropped result), so it can never leak or clear early. The
  // overlay therefore covers exactly the network + server re-budget window, never the
  // interactive force-layout settle. Visual feedback only — the a11y announcement
  // rides the polite `#graph-controls-notice` text, so the overlay is `aria-hidden`.
  // Safe before the element exists (no-op).
  var pendingFetches = 0;
  function setBusy(on) {
    pendingFetches = on ? pendingFetches + 1 : Math.max(0, pendingFetches - 1);
    var el = document.getElementById("graph-canvas-busy");
    if (el) {
      el.hidden = pendingFetches === 0;
    }
  }

  function forEach(list, fn) {
    Array.prototype.forEach.call(list, fn);
  }

  // ── Boot ──────────────────────────────────────────────────────────────────

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
