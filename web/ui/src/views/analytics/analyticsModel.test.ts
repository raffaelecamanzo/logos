import { describe, expect, it } from "vitest";

import type { HotspotReport, TemporalReport } from "../../api/types.ts";
import {
  fileLine,
  fileRiskRows,
  ownershipRows,
  pctBp,
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
