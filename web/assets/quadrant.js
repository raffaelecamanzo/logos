// Quadrant 2×2 bootstrap (S-154, CR-040, FR-UI-17, frontend-design §4.12).
//
// Renders the reachability × runtime-coverage cross as a true 2×2 grid with four
// shaded cell regions into `#quadrant-chart`, over the same-origin `/api/quadrant`
// payload. Flipped to the Gartner convention (best top-right, CR-040): X =
// reachability (unreachable left → reachable right), Y = runtime execution
// (0% bottom → executed top). Authored for Logos (Project MIT; see the repository
// LICENSE / VENDOR.md). Names no external origin (NFR-SE-01) — every dependency is
// the vendored uPlot bundle loaded before this script under the self-only CSP.
//
// Progressive enhancement only (ADR-29): with JavaScript disabled, or if uPlot
// or the fetch is unavailable, the server-rendered urgency table below the mount
// is the deterministic, accessible twin — this script simply does not populate
// the chart, and the `<noscript>` note already explains it.
(function () {
  "use strict";

  // The quadrant palette (frontend-design §4.12 / §1.2 tokens) under the CR-040
  // Gartner numbering: the surprising disagreements pop — Q1 red (false-green,
  // worst) and Q2 orange (dead/guarded edge) — while Q4 (trust, green) and Q3
  // (true gap, muted) stay calm. Color is a redundant channel: the cell labels,
  // the server legend, and the table carry the same classification, so the grid
  // is never color-only (a11y).
  var COLORS = {
    q1: "#da291c", // --so-red    (incidental / false-green, worst)
    q2: "#e35205", // --so-orange (dead/guarded test edge)
    q3: "#716b5d", // --so-muted  (true gap)
    q4: "#16a34a", // --so-green  (trust, best)
  };
  // Faint cell-region fills (the shaded 2×2 background), each ~8% of the swatch.
  var CELL_FILLS = {
    tl: "rgba(218, 41, 28, 0.07)", // top-left    Q1 false-green
    tr: "rgba(22, 163, 74, 0.08)", // top-right   Q4 trust
    bl: "rgba(113, 107, 93, 0.06)", // bottom-left Q3 true gap
    br: "rgba(227, 82, 5, 0.07)", // bottom-right Q2 dead edge
  };
  var MERLIN = "#3d3935"; // --so-merlin (axes / gridlines)

  function ready(fn) {
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", fn);
    } else {
      fn();
    }
  }

  // Point radius scales with architectural weight (hotspot rank, FR-GH-06): a
  // sqrt scale so a hot file reads as a larger point — the blast radius — without
  // dwarfing the rest. Bounded so a point is always selectable and never swamps a
  // neighbour. The server legend names this size = blast-radius encoding.
  function radiusFor(weight) {
    var w = weight > 0 ? weight : 1;
    var r = 3 + Math.sqrt(w);
    return Math.max(3, Math.min(r, 14));
  }

  // A small deterministic spread in [-0.5, 0.5) from a point's index, so symbols
  // sharing a cell separate without the non-determinism of Math.random (the table
  // twin remains the deterministic surface, ADR-29; this only declutters pixels).
  function jitter(i) {
    // Knuth multiplicative hash, evaluated in 32-bit (`>>> 0`) as the constant
    // intends — so the product never loses integer precision past 2^53 for a
    // very large symbol index.
    var h = ((i * 2654435761) >>> 0) % 1000;
    return h / 1000 - 0.5;
  }

  // Plotted X: reachability drives the column (left unreachable / right reachable),
  // jittered within the column half so overlapping symbols separate.
  function plotX(p, i) {
    var center = p.x >= 0.5 ? 0.75 : 0.25;
    return center + jitter(i) * 0.36;
  }

  // Plotted Y: executed symbols (fraction > 0) sit in the top band with their
  // height tracking the runtime fraction (more coverage → higher); unexecuted
  // symbols (a measured 0%) sit in the bottom band, jittered. This keeps the
  // binary executed/unexecuted split reading as two clean cells (CR-040 §4.3)
  // while still surfacing the continuous fraction within the executed band.
  function plotY(p, i) {
    if (p.y > 0) return 0.55 + p.y * 0.4; // executed: 0.55 → 0.95
    return 0.27 + jitter(i) * 0.34; // unexecuted band: ~0.10 → 0.44
  }

  function render(mount, points) {
    var uPlot = window.uPlot;
    if (!uPlot || typeof uPlot !== "function") {
      return; // no chart lib → the table twin stands alone (ADR-29).
    }

    // uPlot shares one x array across series; we keep all points in one series
    // and draw them ourselves (per-point color + weight size) via points.show.
    var xs = points.map(function (p, i) { return plotX(p, i); });
    var ys = points.map(function (p, i) { return plotY(p, i); });
    var data = [xs, ys];

    function drawPoints(u, seriesIdx, i0, i1) {
      var ctx = u.ctx;
      ctx.save();
      for (var i = i0; i <= i1; i++) {
        var p = points[i];
        if (p == null) continue;
        var cx = u.valToPos(u.data[0][i], "x", true);
        var cy = u.valToPos(u.data[seriesIdx][i], "y", true);
        ctx.beginPath();
        ctx.globalAlpha = 0.72;
        ctx.fillStyle = COLORS[p.q] || MERLIN;
        ctx.arc(cx, cy, radiusFor(p.w), 0, 2 * Math.PI);
        ctx.fill();
      }
      ctx.restore();
      return null; // we drew the points; uPlot draws none of its own.
    }

    // The four shaded cell regions + meaning labels, drawn behind the points: a
    // vertical rule at x=0.5 (unreachable | reachable) and a horizontal rule at
    // y=0.5 (unexecuted | executed) carve the plot into the flipped 2×2 — Q4 trust
    // top-right, Q1 false-green top-left, Q2 dead edge bottom-right, Q3 true gap
    // bottom-left (CR-040).
    function drawCells(u) {
      var ctx = u.ctx;
      var left = u.bbox.left, right = u.bbox.left + u.bbox.width;
      var top = u.bbox.top, bot = u.bbox.top + u.bbox.height;
      var xMid = u.valToPos(0.5, "x", true);
      var yMid = u.valToPos(0.5, "y", true);
      ctx.save();
      // Shade each cell.
      ctx.fillStyle = CELL_FILLS.tl; ctx.fillRect(left, top, xMid - left, yMid - top);
      ctx.fillStyle = CELL_FILLS.tr; ctx.fillRect(xMid, top, right - xMid, yMid - top);
      ctx.fillStyle = CELL_FILLS.bl; ctx.fillRect(left, yMid, xMid - left, bot - yMid);
      ctx.fillStyle = CELL_FILLS.br; ctx.fillRect(xMid, yMid, right - xMid, bot - yMid);
      // The dividing cross.
      ctx.strokeStyle = MERLIN;
      ctx.globalAlpha = 0.35;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(xMid, top); ctx.lineTo(xMid, bot);
      ctx.moveTo(left, yMid); ctx.lineTo(right, yMid);
      ctx.stroke();
      // Cell meaning labels in each corner (color matches the cell's badge).
      ctx.globalAlpha = 0.85;
      ctx.font = "600 11px system-ui, sans-serif";
      var pad = 6;
      ctx.textBaseline = "top";
      ctx.textAlign = "left";
      ctx.fillStyle = COLORS.q1; ctx.fillText("Q1 false-green", left + pad, top + pad);
      ctx.fillStyle = COLORS.q3; ctx.fillText("Q3 true gap", left + pad, bot - 16);
      ctx.textAlign = "right";
      ctx.fillStyle = COLORS.q4; ctx.fillText("Q4 trust ★", right - pad, top + pad);
      ctx.fillStyle = COLORS.q2; ctx.fillText("Q2 dead edge", right - pad, bot - 16);
      ctx.restore();
    }

    var opts = {
      width: mount.clientWidth || 640,
      height: 360,
      cursor: { y: false },
      legend: { show: false },
      scales: {
        x: { time: false, range: [0, 1] },
        y: { range: [0, 1] },
      },
      axes: [
        {
          stroke: MERLIN,
          grid: { show: false },
          ticks: { show: false },
          label: "Reachability",
          // Category words centered under each column; nothing at the edges.
          splits: function () { return [0.25, 0.75]; },
          values: function (u, vals) {
            return vals.map(function (v) {
              if (v === 0.25) return "unreachable";
              if (v === 0.75) return "reachable";
              return "";
            });
          },
        },
        {
          stroke: MERLIN,
          grid: { show: false },
          ticks: { show: false },
          // Reserve gutter for the widest label so words are never clipped.
          size: 84,
          label: "Runtime executed",
          splits: function () { return [0.25, 0.75]; },
          values: function (u, vals) {
            return vals.map(function (v) {
              if (v === 0.25) return "0% (dead)";
              if (v === 0.75) return "executed";
              return "";
            });
          },
        },
      ],
      series: [
        {},
        {
          label: "symbols",
          stroke: MERLIN,
          paths: function () { return null; }, // points-only (no connecting line)
          points: { show: drawPoints },
        },
      ],
      hooks: { draw: [drawCells] },
    };

    var chart = new uPlot(opts, data, mount);

    // Reflow on resize so the square-ish grid tracks its column width.
    window.addEventListener("resize", function () {
      chart.setSize({ width: mount.clientWidth || 640, height: 360 });
    });
  }

  ready(function () {
    var mount = document.getElementById("quadrant-chart");
    if (!mount) return;
    fetch("/api/quadrant", { headers: { Accept: "application/json" } })
      .then(function (r) { return r.ok ? r.json() : []; })
      .then(function (points) {
        if (Array.isArray(points) && points.length > 0) {
          render(mount, points);
        }
        // An empty payload (no placed symbols) leaves the mount blank; the
        // server-rendered verdict + urgency table already carry the honest state.
      })
      .catch(function () {
        // Network/parse failure → the table twin stands alone (ADR-29).
      });
  });
})();
