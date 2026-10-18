import { describe, expect, it } from "vitest";

import type { HotspotReport, TemporalReport } from "../../api/types.ts";
import {
  fileLine,
  fileRiskRows,
  findingsSortValue,
  hasFindingsColumn,
  ownershipRows,
  pctBp,
  securityHref,
} from "./analyticsModel.ts";

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

describe("findings column model (S-671, FR-SC-14)", () => {
  const row = (findings: { count: number; highest: "high" | "low" | null; stale: number } | undefined) =>
    ({ path: "a.rs", findings }) as unknown as Parameters<typeof hasFindingsColumn>[0][number];

  it("shows the column only when some row carries a cell — a scanned zero counts, no scan does not", () => {
    expect(hasFindingsColumn([row(undefined), row(undefined)])).toBe(false);
    expect(hasFindingsColumn([row(undefined), row({ count: 0, highest: null, stale: 0 })])).toBe(true);
    expect(hasFindingsColumn([])).toBe(false);
  });

  it("links to the Security page filtered by the encoded path", () => {
    expect(securityHref("src/a b.rs")).toBe("/security?path=src%2Fa%20b.rs");
  });

  it("sorts the highest severity first, then the count, and a clean file below both", () => {
    const sev = (highest: "high" | "low", count: number) => findingsSortValue({ count, highest, stale: 0 });
    expect(sev("high", 1)).toBeGreaterThan(sev("low", 99));
    expect(sev("high", 2)).toBeGreaterThan(sev("high", 1));
    expect(findingsSortValue({ count: 0, highest: null, stale: 5 })).toBeLessThan(sev("low", 1));
  });
});
