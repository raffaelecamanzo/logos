import { describe, expect, it } from "vitest";

import type {
  CoverageCrossReport,
  CrossSymbol,
  HotspotReport,
  TemporalReport,
} from "../../api/types.ts";
import {
  fileLine,
  fileRiskRows,
  fileWeights,
  jitter,
  ownershipRows,
  pctBp,
  plotX,
  plotY,
  radiusFor,
  rankByUrgency,
  scatterPoints,
  severity,
  trustScoreBp,
  weightOf,
} from "./analyticsModel.ts";

const sym = (over: Partial<CrossSymbol>): CrossSymbol => ({
  symbol: over.symbol ?? `scip::${over.name ?? "x"}`,
  name: over.name ?? "x",
  file: over.file ?? "src/x.rs",
  start_line: over.start_line ?? 1,
  end_line: over.end_line ?? 2,
  reachable_from_test: over.reachable_from_test ?? false,
  runtime_exec_bp: over.runtime_exec_bp ?? null,
  quadrant: over.quadrant ?? null,
});

const cross = (symbols: CrossSymbol[], hasFresh = true): CoverageCrossReport => ({
  head_sha: hasFresh ? "head" : null,
  config_hash: hasFresh ? "cfg" : null,
  has_fresh_coverage: hasFresh,
  symbols,
  totals: { q1: 0, q2: 0, q3: 0, q4: 0, na_runtime: 0, total: symbols.length },
  notice: hasFresh ? null : "no coverage",
});

describe("pctBp", () => {
  it("reprojects basis points to a one-decimal percent and clamps out-of-range", () => {
    expect(pctBp(7350)).toBe("73.5%");
    expect(pctBp(0)).toBe("0.0%");
    expect(pctBp(10000)).toBe("100.0%");
    expect(pctBp(12000)).toBe("100.0%"); // clamped
    expect(pctBp(-5)).toBe("0.0%"); // clamped
  });
});

describe("fileLine", () => {
  it("formats file:line, file, or null — never a fabricated :0", () => {
    expect(fileLine("a.rs", 12)).toBe("a.rs:12");
    expect(fileLine("a.rs", null)).toBe("a.rs");
    expect(fileLine(null, 12)).toBeNull();
    expect(fileLine(null, null)).toBeNull();
  });
});

describe("severity ordering", () => {
  it("ranks disagreements highest: Q1 > Q2 > Q3 > Q4 > n/a", () => {
    expect(severity("q1")).toBeGreaterThan(severity("q2"));
    expect(severity("q2")).toBeGreaterThan(severity("q3"));
    expect(severity("q3")).toBeGreaterThan(severity("q4"));
    expect(severity("q4")).toBeGreaterThan(severity(null));
    expect(severity(null)).toBe(0);
  });
});

describe("weights", () => {
  const hotspots = {
    files: [
      { path: "hot.rs", score: 40 },
      { path: "warm.rs", score: 8 },
    ],
  } as unknown as HotspotReport;

  it("maps file → score and floors an unranked file at 1 (never weighted to zero)", () => {
    const w = fileWeights(hotspots);
    expect(weightOf(w, "hot.rs")).toBe(40);
    expect(weightOf(w, "cold.rs")).toBe(1); // floored, never erased
  });

  it("an absent hotspot report yields an empty map (every weight floors to 1)", () => {
    const w = fileWeights(null);
    expect(weightOf(w, "anything.rs")).toBe(1);
  });
});

describe("trustScoreBp", () => {
  it("is the weighted Q4 share over PLACED symbols; n/a is excluded from the denominator", () => {
    const symbols = [
      sym({ name: "a", file: "a.rs", reachable_from_test: true, runtime_exec_bp: 9000, quadrant: "q4" }),
      sym({ name: "b", file: "b.rs", reachable_from_test: true, runtime_exec_bp: 9000, quadrant: "q4" }),
      sym({ name: "c", file: "c.rs", reachable_from_test: true, runtime_exec_bp: 9000, quadrant: "q4" }),
      sym({ name: "d", file: "d.rs", reachable_from_test: false, runtime_exec_bp: 5000, quadrant: "q1" }),
      // an n/a symbol must NOT dilute the score
      sym({ name: "e", file: "e.rs", runtime_exec_bp: null, quadrant: null }),
    ];
    // equal weights (all unranked → 1): 3 trust / 4 placed = 7500 bp
    expect(trustScoreBp(symbols, fileWeights(null))).toBe(7500);
  });

  it("is null when no symbol is placed (no fresh coverage) — never a fabricated 0%", () => {
    const symbols = [sym({ name: "a", runtime_exec_bp: null, quadrant: null })];
    expect(trustScoreBp(symbols, fileWeights(null))).toBeNull();
  });
});

describe("scatterPoints — the n/a rule", () => {
  it("plots one point per PLACED symbol and excludes every n/a (unplaced) symbol", () => {
    const report = cross([
      sym({ name: "placed", reachable_from_test: true, runtime_exec_bp: 8000, quadrant: "q4" }),
      sym({ name: "na", runtime_exec_bp: null, quadrant: null }),
    ]);
    const points = scatterPoints(report, fileWeights(null));
    expect(points).toHaveLength(1);
    expect(points[0].name).toBe("placed");
    expect(points[0].x).toBe(1); // reachable → right column
    expect(points[0].y).toBeCloseTo(0.8); // 8000 bp / 10000
    expect(points[0].q).toBe("q4");
  });
});

describe("plot placement is deterministic (no Math.random)", () => {
  it("jitter is a stable hash in [-0.5, 0.5)", () => {
    expect(jitter(0)).toBe(jitter(0));
    expect(jitter(3)).toBeGreaterThanOrEqual(-0.5);
    expect(jitter(3)).toBeLessThan(0.5);
  });

  it("executed symbols sit in the top band, unexecuted in the bottom band", () => {
    const executed = { x: 1, y: 0.5, w: 1, q: "q4" as const, name: "e", loc: "" };
    const unexecuted = { x: 1, y: 0, w: 1, q: "q2" as const, name: "u", loc: "" };
    expect(plotY(executed, 0)).toBeGreaterThan(0.5);
    expect(plotY(unexecuted, 0)).toBeLessThan(0.5);
  });

  it("reachable symbols plot to the right column, unreachable to the left", () => {
    const right = plotX({ x: 1, y: 0.8, w: 1, q: "q4", name: "r", loc: "" }, 0);
    const left = plotX({ x: 0, y: 0, w: 1, q: "q3", name: "l", loc: "" }, 0);
    expect(right).toBeGreaterThan(0.5);
    expect(left).toBeLessThan(0.5);
  });

  it("radius scales with weight but stays bounded [3, 14]", () => {
    expect(radiusFor(0)).toBeGreaterThanOrEqual(3);
    expect(radiusFor(100000)).toBeLessThanOrEqual(14);
    expect(radiusFor(25)).toBeGreaterThan(radiusFor(1));
  });
});

describe("rankByUrgency", () => {
  it("orders most-dangerous-first (weight × severity), with canonical order as the tiebreak", () => {
    const report = cross([
      sym({ name: "trust", file: "a.rs", runtime_exec_bp: 9000, reachable_from_test: true, quadrant: "q4" }),
      sym({ name: "falsegreen", file: "a.rs", runtime_exec_bp: 5000, reachable_from_test: false, quadrant: "q1" }),
      sym({ name: "gap", file: "a.rs", runtime_exec_bp: 0, reachable_from_test: false, quadrant: "q3" }),
    ]);
    const ranked = rankByUrgency(report, fileWeights(null));
    expect(ranked.map((r) => r.symbol.name)).toEqual(["falsegreen", "gap", "trust"]);
  });
});

describe("fileRiskRows — the temporal join and the n/a rule", () => {
  const report = {
    files: [
      { path: "ranked-with-temporal.rs", score: 10, churn_commits: 3, co_change_count: 1, defect_commits: 2, complexity: 5, coverage: { state: "n/a", coverage_bp: null } },
      { path: "ranked-no-temporal.rs", score: 8, churn_commits: 1, co_change_count: 0, defect_commits: 0, complexity: 2, coverage: { state: "n/a", coverage_bp: null } },
    ],
  } as unknown as HotspotReport;
  const temporal = {
    files: [
      { path: "ranked-with-temporal.rs", lines_added: 30, lines_deleted: 9, last_change_age_days: 4, ownership_dispersion_bp: 6000, change_entropy_bp: 0 },
    ],
  } as unknown as TemporalReport;

  it("preserves the hotspot-board order (the spine) and joins temporal by path", () => {
    const rows = fileRiskRows(report, temporal);
    expect(rows.map((r) => r.path)).toEqual([
      "ranked-with-temporal.rs",
      "ranked-no-temporal.rs",
    ]);
    expect(rows[0].churn).toEqual({ added: 30, deleted: 9 });
    expect(rows[0].ageDays).toBe(4);
  });

  it("renders a ranked file with no temporal join as n/a (null), never a fabricated zero", () => {
    const rows = fileRiskRows(report, temporal);
    expect(rows[1].churn).toBeNull();
    expect(rows[1].ageDays).toBeNull();
  });

  it("ownershipRows keeps only files with a real ownership/entropy signal", () => {
    expect(ownershipRows(temporal).map((f) => f.path)).toEqual(["ranked-with-temporal.rs"]);
  });
});
