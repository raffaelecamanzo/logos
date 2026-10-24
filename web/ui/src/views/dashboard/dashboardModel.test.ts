import { describe, expect, it } from "vitest";

import type { SecurityGateBand, SecuritySummary, StatusInfo } from "../../api/types.ts";
import {
  bandOf,
  fmtInt,
  freshnessStatement,
  gateStatement,
  humanizeAge,
  newestScanStatement,
  pctBp,
  scannersFailedStatement,
  securityState,
  testCodeStatement,
} from "./dashboardModel.ts";

/** A minimal StatusInfo for the freshness tests. */
function status(over: Partial<StatusInfo>): StatusInfo {
  return {
    indexed: true,
    file_count: 0,
    node_count: 0,
    edge_count: 0,
    db_path: "",
    db_size_bytes: 0,
    last_full_index_at: null,
    last_sync_at: null,
    graph_revision: 0,
    refs_total: 0,
    refs_resolved: 0,
    refs_unresolved: 0,
    resolution_coverage: 0,
    total_line_count: null,
    source_line_count: null,
    test_line_count: null,
    freshness: "",
    warnings: [],
    ...over,
  };
}

describe("bandOf — BR-34 advisory quality bands", () => {
  it("maps the four band thresholds (red → orange → lime → green)", () => {
    expect(bandOf(4999)).toEqual({ label: "Poor", tone: "poor" });
    expect(bandOf(5000)).toEqual({ label: "Average", tone: "average" });
    expect(bandOf(6999)).toEqual({ label: "Average", tone: "average" });
    expect(bandOf(7000)).toEqual({ label: "Good", tone: "good" });
    expect(bandOf(8499)).toEqual({ label: "Good", tone: "good" });
    expect(bandOf(8500)).toEqual({ label: "Excellent", tone: "excellent" });
    expect(bandOf(10_000)).toEqual({ label: "Excellent", tone: "excellent" });
  });
});

describe("fmtInt — integer digit-grouping", () => {
  it("groups thousands and leaves sub-1000 figures unchanged", () => {
    expect(fmtInt(0)).toBe("0");
    expect(fmtInt(42)).toBe("42");
    expect(fmtInt(1000)).toBe("1,000");
    expect(fmtInt(54_321)).toBe("54,321");
    expect(fmtInt(1_234_567)).toBe("1,234,567");
  });
});

describe("pctBp — basis-point reprojection", () => {
  it("formats one decimal and clamps out-of-range", () => {
    expect(pctBp(8500)).toBe("85.0%");
    expect(pctBp(0)).toBe("0.0%");
    expect(pctBp(10_000)).toBe("100.0%");
    expect(pctBp(20_000)).toBe("100.0%");
    expect(pctBp(-5)).toBe("0.0%");
  });
});

describe("humanizeAge", () => {
  /**
   * The CLI's two shipped degradations, quoted verbatim from
   * `logos-core/src/governance/readout.rs::render_age` (S-314, `37909c9c`) —
   * the AC is *matching wording*, so these are literals here rather than an
   * import: a silent edit to either phrase has to fail this file.
   */
  const AHEAD_OF_NOW = "at an unknown age (recorded ahead of now — check the clock)";
  const IMPLAUSIBLY_OLD =
    "at an unknown age (the recorded time is implausibly old — check the store)";
  /** The degradation threshold: a century, mirroring `IMPLAUSIBLE_AGE_SECS`. */
  const CENTURY_SECS = 100 * 365 * 24 * 60 * 60;

  it("buckets an ordinary past age, each unit pinned on both sides of its threshold", () => {
    expect(humanizeAge(1000, 1000)).toBe("just now");
    expect(humanizeAge(1000 + 59, 1000)).toBe("just now");
    expect(humanizeAge(1000 + 60, 1000)).toBe("1m ago");
    expect(humanizeAge(1090, 1000)).toBe("1m ago");
    expect(humanizeAge(1000 + 3_599, 1000)).toBe("59m ago");
    expect(humanizeAge(1000 + 3_600, 1000)).toBe("1h ago");
    expect(humanizeAge(4700, 1000)).toBe("1h ago");
    expect(humanizeAge(1000 + 86_399, 1000)).toBe("23h ago");
    expect(humanizeAge(1000 + 86_400, 1000)).toBe("1d ago");
    expect(humanizeAge(1000 + 90_000, 1000)).toBe("1d ago");
    // One second under two days. The days bucket is the only one whose other
    // probes all sit far from an integer boundary, so without this the branch
    // survives both `Math.floor` -> `Math.round` and a `86_400` -> `86_399`
    // divisor slip: at 1.04 and at exactly 36500 days, every one of those
    // spellings floors to the same answer.
    expect(humanizeAge(1000 + 172_799, 1000)).toBe("1d ago");
    expect(humanizeAge(1000 + 172_800, 1000)).toBe("2d ago");
  });

  it("names a timestamp ahead of now instead of clamping the interval to zero", () => {
    // This is the test a mutation restoring `Math.max(0, now - then)` fails:
    // the clamp renders every case below as "just now".
    expect(humanizeAge(999, 1000)).toBe(AHEAD_OF_NOW); // one second ahead
    expect(humanizeAge(1000, 5000)).toBe(AHEAD_OF_NOW);
    expect(humanizeAge(0, 4_000_000_000)).toBe(AHEAD_OF_NOW); // a future file mtime
  });

  it("names an implausibly old timestamp instead of counting tens of thousands of days", () => {
    expect(humanizeAge(CENTURY_SECS + 1, 0)).toBe(IMPLAUSIBLY_OLD);
    expect(humanizeAge(Number.MAX_SAFE_INTEGER, 0)).toBe(IMPLAUSIBLY_OLD);
    // The boundary itself still renders: a century is merely stale, past it is wrong.
    expect(humanizeAge(CENTURY_SECS, 0)).toBe("36500d ago");
  });

  it("renders neither a figure nor 'just now' for either degradation", () => {
    for (const rendered of [humanizeAge(1000, 5000), humanizeAge(CENTURY_SECS + 1, 0)]) {
      expect(rendered).not.toMatch(/\d/);
      expect(rendered).not.toContain("ago");
      expect(rendered).not.toContain("just now");
    }
  });
});

describe("freshnessStatement", () => {
  const CAVEAT = "reflects the last index, not unsaved edits";
  it("prefers the full-index timestamp", () => {
    const s = status({ last_full_index_at: "900", last_sync_at: "950" });
    expect(freshnessStatement(s, 1000)).toBe(`Indexed 1m ago — ${CAVEAT}`);
  });
  it("falls back to the sync timestamp", () => {
    const s = status({ last_full_index_at: null, last_sync_at: "940" });
    expect(freshnessStatement(s, 1000)).toBe(`Last synced 1m ago — ${CAVEAT}`);
  });
  it("is age-free but honest when no timestamp is recorded", () => {
    expect(freshnessStatement(status({}), 1000)).toBe(`Index present — ${CAVEAT}`);
  });
  it("treats a non-numeric timestamp field as absent", () => {
    const s = status({ last_full_index_at: "not-a-number" });
    expect(freshnessStatement(s, 1000)).toBe(`Index present — ${CAVEAT}`);
  });
  it("carries the ahead-of-now degradation into the freshness line", () => {
    // `last_sync_at` is a file mtime: a future stamp is routine on a copied
    // tree, an NFS mount or a restored backup.
    const s = status({ last_full_index_at: null, last_sync_at: "5000" });
    expect(freshnessStatement(s, 1000)).toBe(
      `Last synced at an unknown age (recorded ahead of now — check the clock) — ${CAVEAT}`,
    );
  });
});

// ── The Security widget's decisions (S-789) ──────────────────────────────────

const ZERO = { critical: 0, high: 0, medium: 0, low: 0, info: 0, unrated: 0, total: 0 };

/** A scanned summary, with `over` applied. */
function summary(over: Partial<SecuritySummary> = {}): SecuritySummary {
  return {
    scan: "fresh",
    tally: { counted: { ...ZERO, high: 2, total: 2 }, excluded: { not_fresh: 0, suppressed: 0, test: 0 } },
    newest_scan_at: 1_000,
    scanners_failed: 0,
    scanners_total: 1,
    consent: { consented: ["semgrep"], pending: [] },
    gate: null,
    ...over,
  };
}

describe("securityState (S-789)", () => {
  it("is a fault whenever the read failed, whatever else the payload holds", () => {
    expect(securityState({ summary: null, error: "boom" })).toEqual({ kind: "fault", error: "boom" });
    expect(securityState({ summary: summary(), error: "boom" }).kind).toBe("fault");
  });

  it("is a fault, not an empty count, when the payload carries neither a summary nor an error", () => {
    expect(securityState({ summary: null, error: null }).kind).toBe("fault");
  });

  it("is never_scanned when a scanner is consented to, and no_consent when none is", () => {
    const never = (consented: string[]) =>
      securityState({ summary: summary({ scan: "never_run", tally: null, consent: { consented, pending: [] } }), error: null });
    expect(never(["semgrep"]).kind).toBe("never_scanned");
    expect(never([]).kind).toBe("no_consent");
  });

  it("reads a missing tally as never scanned, never as zero findings", () => {
    expect(securityState({ summary: summary({ scan: "fresh", tally: null }), error: null }).kind).not.toBe("scanned");
  });

  it("carries the counted severities for a stored report, stale or fresh", () => {
    for (const scan of ["fresh", "stale"] as const) {
      const state = securityState({ summary: summary({ scan }), error: null });
      expect(state.kind).toBe("scanned");
      if (state.kind === "scanned") expect(state.counted.high).toBe(2);
    }
  });
});

describe("the Security widget's statements (S-789)", () => {
  it("states N of M scanners, agreeing in number", () => {
    expect(scannersFailedStatement(summary({ scanners_failed: 1, scanners_total: 3 }))).toBe("1 of 3 scanners");
    expect(scannersFailedStatement(summary({ scanners_failed: 0, scanners_total: 1 }))).toBe("0 of 1 scanner");
  });

  it("states the newest scan as a relative age, or that none is stored", () => {
    expect(newestScanStatement(summary({ newest_scan_at: 1_000 }), 1_000 + 2 * 86_400)).toBe("2d ago");
    expect(newestScanStatement(summary({ newest_scan_at: null }), 5_000)).toBe("none stored");
    // An age that cannot be established is named, never rendered as "just now".
    expect(newestScanStatement(summary({ newest_scan_at: 9_000 }), 1_000)).toMatch(/unknown age/);
  });

  const band = (over: Partial<SecurityGateBand>): SecurityGateBand => ({
    passed: true,
    checked_rules: 1,
    required_scanners: [],
    missing_scans: [],
    violations: [],
    notes: [],
    ...over,
  });

  it("has no gate statement without a [security] table", () => {
    expect(gateStatement(null)).toBeNull();
  });

  it("states a passing gate with its rule count, and a failing one with every reason", () => {
    expect(gateStatement(band({ passed: true, checked_rules: 1 }))).toBe("passes 1 rule");
    expect(gateStatement(band({ passed: true, checked_rules: 3 }))).toBe("passes 3 rules");
    const miss = { scanner: "bandit", state: "never_run" as const, message: "" };
    const violation = { rule: "r", rule_type: "constraint", severity: "error", file: "", node_id: null, message: "" };
    expect(gateStatement(band({ passed: false, missing_scans: [miss] }))).toBe("fails: 1 required scan missing");
    expect(gateStatement(band({ passed: false, violations: [violation, violation], missing_scans: [miss] }))).toBe(
      "fails: 2 findings fail a rule, 1 required scan missing",
    );
    expect(gateStatement(band({ passed: false }))).toBe("fails");
    // Each reason alone, and each in the plural.
    expect(gateStatement(band({ passed: false, violations: [violation] }))).toBe("fails: 1 finding fail a rule");
    expect(gateStatement(band({ passed: false, violations: [violation, violation] }))).toBe("fails: 2 findings fail a rule");
    expect(gateStatement(band({ passed: false, missing_scans: [miss, miss] }))).toBe("fails: 2 required scans missing");
  });

  it("states how many more findings lie in test code, and nothing when none do", () => {
    const tally = (test: number) => ({ counted: { ...ZERO }, excluded: { not_fresh: 0, suppressed: 0, test } });
    expect(testCodeStatement(tally(0))).toBeNull();
    expect(testCodeStatement(tally(3))).toBe("3 more in test code");
    expect(testCodeStatement(tally(1_234))).toBe("1,234 more in test code");
  });
});
