import { describe, expect, it } from "vitest";

import type {
  EvolutionPoint,
  EvolutionReport,
  MetricSnapshot,
  MetricValue,
  ScanResult,
  StatusInfo,
} from "../../api/types.ts";
import {
  aggregateSignal,
  applicableCount,
  dimensionDetails,
  gateFigure,
  lowestDimension,
  metricRows,
  optDelta,
  optSignal,
  passFloor,
  shortSha,
  snapshotStaleness,
  type DimensionDetail,
} from "./healthModel.ts";
import realOffenders from "./fixtures/worst-offenders.real.json";
import { UNKNOWN_AGE_AHEAD_OF_NOW } from "../dashboard/dashboardModel.ts";

function mv(n: number): MetricValue {
  return { raw: n, normalized: n };
}

function snapshot(over: Partial<MetricSnapshot> = {}): MetricSnapshot {
  return {
    modularity: mv(0.9),
    modularity_not_applicable: null,
    acyclicity: mv(0.8),
    depth: mv(0.7),
    equality: mv(0.6),
    redundancy: mv(0.5),
    nesting: mv(0.9),
    conciseness: mv(0.8),
    cohesion: mv(0.6),
    focus: mv(0.6),
    uniqueness: mv(0.7),
    thresholds_hash: "abc123",
    node_count: 120,
    edge_count: 240,
    function_count: 90,
    test_function_count: 12,
    empty: false,
    aggregate_signal: 8000,
    ...over,
  };
}

function scan(over: Partial<ScanResult> = {}): ScanResult {
  return {
    signal: 8000,
    freshness: "",
    metrics: snapshot(),
    worst_offenders: { recorded: true, nesting: [], conciseness: [], cohesion: [], focus: [], uniqueness: [] },
    warnings: [],
    ...over,
  };
}

// `last_full_index_at` defaults to the day BEFORE `point()`'s instant, so
// `status(true)` is the ordinary case — indexed, with a snapshot the graph has
// not moved past. Before S-436 the field was unread and defaulted to `null`;
// `null` is now the indeterminate arm, and `status(true, { … })` overrides it.
const DEFAULT_SNAPSHOT_AT = 1_726_704_000; // 2024-09-19T00:00:00Z
const DEFAULT_INDEX_AT = DEFAULT_SNAPSHOT_AT - 86_400; // a day earlier
// A `now` far enough past both that no arm here depends on the wall clock.
const NOW = 1_800_000_000; // 2027-01-15T08:00:00Z

function status(indexed: boolean, over: Partial<StatusInfo> = {}): StatusInfo {
  return {
    indexed,
    file_count: 1,
    node_count: 1,
    edge_count: 1,
    db_path: "",
    db_size_bytes: 0,
    last_full_index_at: String(DEFAULT_INDEX_AT),
    last_sync_at: null,
    graph_revision: 1,
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

function point(over: Partial<EvolutionPoint> = {}): EvolutionPoint {
  return {
    snapshot_id: 1,
    created_at: DEFAULT_SNAPSHOT_AT,
    commit_sha: null,
    signal: 8000,
    signal_delta: null,
    ...over,
  };
}

function evolution(...points: EvolutionPoint[]): EvolutionReport {
  return { snapshots: points, warnings: [] };
}

describe("metricRows", () => {
  it("projects the ten metrics in canonical order", () => {
    const rows = metricRows(snapshot());
    expect(rows.map((r) => r.name)).toEqual([
      "Modularity", "Acyclicity", "Depth", "Equality", "Redundancy",
      "Nesting", "Conciseness", "Cohesion", "Focus", "Uniqueness",
    ]);
  });
  it("carries an ADR-21 drop-out through as null (never a fabricated zero)", () => {
    const rows = metricRows(snapshot({ cohesion: null, focus: null }));
    expect(rows.find((r) => r.name === "Cohesion")?.value).toBeNull();
    expect(rows.find((r) => r.name === "Focus")?.value).toBeNull();
    expect(rows.find((r) => r.name === "Uniqueness")?.value).toEqual(mv(0.7));
  });
  it("marks a CR-156 Modularity drop-out with its reason while keeping the computed pair", () => {
    const na = { edges: 3, min_edges: 5, reason: "3 of 5 dependency edges — too few for community structure" };
    const rows = metricRows(snapshot({ modularity: mv(0), modularity_not_applicable: na }));
    const modularity = rows.find((r) => r.name === "Modularity");
    expect(modularity?.notApplicable).toBe("3 of 5 dependency edges — too few for community structure");
    expect(modularity?.value).toEqual(mv(0));
    // Only Modularity carries the reason; an applicable dimension carries none.
    expect(rows.filter((r) => r.notApplicable !== null).map((r) => r.name)).toEqual(["Modularity"]);
    expect(metricRows(snapshot()).every((r) => r.notApplicable === null)).toBe(true);
  });
});

describe("applicableCount / lowestDimension (FR-QM-14, FR-UI-43)", () => {
  it("counts the dimensions in the geometric mean: drop-outs and CR-156 excluded", () => {
    expect(applicableCount(metricRows(snapshot()))).toBe(10);
    expect(applicableCount(metricRows(snapshot({ cohesion: null, focus: null })))).toBe(8);
    const na = { edges: 3, min_edges: 5, reason: "3 of 5 dependency edges — too few for community structure" };
    expect(applicableCount(metricRows(snapshot({ cohesion: null, modularity_not_applicable: na })))).toBe(8);
  });
  it("names the lowest applicable dimension, the first in canonical order on a tie", () => {
    // Redundancy 0.5 is the lowest of the fixture.
    expect(lowestDimension(metricRows(snapshot()))?.name).toBe("Redundancy");
    // Equality and Cohesion tie at 0.3: the canonically earlier one wins.
    expect(lowestDimension(metricRows(snapshot({ equality: mv(0.3), cohesion: mv(0.3) })))?.name).toBe("Equality");
  });
  it("never names a dimension that is out of the mean, however low its computed value", () => {
    const na = { edges: 3, min_edges: 5, reason: "3 of 5 dependency edges — too few for community structure" };
    const rows = metricRows(snapshot({ modularity: mv(0), modularity_not_applicable: na }));
    expect(lowestDimension(rows)?.name).toBe("Redundancy");
  });
  it("names none when every applicable dimension scores a full 1, or none applies", () => {
    const full = snapshot({
      modularity: mv(1), acyclicity: mv(1), depth: mv(1), equality: mv(1), redundancy: mv(1),
      nesting: mv(1), conciseness: mv(1), cohesion: null, focus: mv(1), uniqueness: mv(1),
    });
    expect(lowestDimension(metricRows(full))).toBeNull();
    expect(lowestDimension([])).toBeNull();
  });
});

describe("passFloor / gateFigure (BR-10)", () => {
  it("is baseline − ε, read from the payload", () => {
    expect(passFloor({ baseline_signal: 8165, epsilon: 1 })).toBe(8164);
    expect(passFloor({ baseline_signal: 8165, epsilon: 2.5 })).toBe(8162.5);
  });
  it("is null with no baseline to compare against", () => {
    expect(passFloor({ baseline_signal: null, epsilon: 1 })).toBeNull();
  });
  it("prints an integer as-is and a float to two places at most", () => {
    expect(gateFigure(8164)).toBe("8164");
    expect(gateFigure(1.0)).toBe("1");
    expect(gateFigure(8162.5)).toBe("8162.5");
    expect(gateFigure(0.123)).toBe("0.12");
  });
});

describe("dimensionDetails (FR-UI-43)", () => {
  const OFFENDER_BACKED = ["Nesting", "Conciseness", "Cohesion", "Focus", "Uniqueness"];
  const byName = (dims: DimensionDetail[], name: string) => dims.find((d) => d.name === name)!;

  it("enumerates all ten dimensions from the Quality signal table's own list, in its order", () => {
    const s = scan();
    expect(dimensionDetails(s).map((d) => d.key)).toEqual(metricRows(s.metrics).map((r) => r.key));
    expect(dimensionDetails(s)).toHaveLength(10);
  });
  it("joins the five offender-backed dimensions to their worst offenders", () => {
    const s = scan();
    s.worst_offenders.nesting = [{ name: "deep_fn", file: "src/a.rs", line: 42, detail: "nesting depth 6" }];
    const dims = dimensionDetails(s);
    expect(byName(dims, "Nesting").offenders).toHaveLength(1);
    expect(byName(dims, "Nesting").offenders[0].name).toBe("deep_fn");
  });
  it("marks the other five as unlisted — a named absence, never an empty list read as clean", () => {
    for (const recorded of [true, false]) {
      const s = scan();
      s.worst_offenders.recorded = recorded;
      const unlisted = dimensionDetails(s).filter((d) => d.offenderState === "unlisted").map((d) => d.name);
      expect(unlisted).toEqual(["Modularity", "Acyclicity", "Depth", "Equality", "Redundancy"]);
    }
  });
  it("keeps an n/a dimension's value null", () => {
    const dims = dimensionDetails(scan({ metrics: snapshot({ cohesion: null }) }));
    expect(byName(dims, "Cohesion").value).toBeNull();
  });
  it("carries a CR-156 Modularity drop-out's reason beside its computed value", () => {
    const na = { edges: 3, min_edges: 5, reason: "3 of 5 dependency edges — too few for community structure" };
    const m = byName(dimensionDetails(scan({ metrics: snapshot({ modularity: mv(0), modularity_not_applicable: na }) })), "Modularity");
    expect(m.notApplicable).toBe(na.reason);
    expect(m.value).toEqual(mv(0));
    expect(m.offenderState).toBe("unlisted");
  });

  // CR-162 / S-499: the offender state comes from `recorded`, never from `[]`.
  describe("offender state", () => {
    const entry = { name: "deep_fn", file: "src/a.rs", line: 42, detail: "nesting depth 6" };
    const backed = (s: ScanResult) =>
      dimensionDetails(s).filter((d) => OFFENDER_BACKED.includes(d.name)).map((d) => d.offenderState);

    it("recorded with entries lists the offenders; the other recorded dimensions are none-flagged", () => {
      const s = scan();
      s.worst_offenders.nesting = [entry];
      expect(backed(s)).toEqual(["listed", "none-flagged", "none-flagged", "none-flagged", "none-flagged"]);
    });
    it("not recorded is never none-flagged — every dimension, whatever the lists hold", () => {
      const s = scan();
      s.worst_offenders.recorded = false;
      expect(backed(s)).toEqual(Array(5).fill("not-recorded"));
      // Even a (contradictory) non-empty list under recorded:false is not presented as recorded.
      s.worst_offenders.nesting = [entry];
      expect(backed(s)[0]).toBe("not-recorded");
    });
    it("a payload without the flag is treated as not recorded, never as a clean result", () => {
      const s = scan();
      delete (s.worst_offenders as Partial<typeof s.worst_offenders>).recorded;
      expect(backed(s)).toEqual(Array(5).fill("not-recorded"));
    });
    it("an n/a dimension keeps its n/a state regardless of the recorded flag", () => {
      for (const recorded of [true, false]) {
        const s = scan({ metrics: snapshot({ cohesion: null }) });
        s.worst_offenders.recorded = recorded;
        expect(byName(dimensionDetails(s), "Cohesion").offenderState).toBe("not-applicable");
      }
    });
  });

  // The real `/api/v1/health` payloads, captured by the Rust guard in web/tests/api_v1.rs.
  describe("over the real /api/v1/health payload", () => {
    it("a scanned fixture's recorded offenders list in persisted order", () => {
      const dims = dimensionDetails(scan({ worst_offenders: realOffenders.recorded }));
      const nesting = byName(dims, "Nesting");
      expect(nesting.offenderState).toBe("listed");
      expect(nesting.offenders.map((o) => o.name)).toEqual([
        "beta_depth_six",
        "gamma_depth_five",
        "alpha_depth_four",
      ]);
      expect(OFFENDER_BACKED.slice(1).map((n) => byName(dims, n).offenderState)).toEqual(Array(4).fill("none-flagged"));
    });
    it("a clean scan is recorded-empty and a never-scanned store is not recorded", () => {
      const states = (w: ScanResult["worst_offenders"]) =>
        OFFENDER_BACKED.map((n) => byName(dimensionDetails(scan({ worst_offenders: w })), n).offenderState);
      expect(states(realOffenders.recordedEmpty)).toEqual(Array(5).fill("none-flagged"));
      expect(states(realOffenders.notRecorded)).toEqual(Array(5).fill("not-recorded"));
    });
  });
});

describe("aggregateSignal", () => {
  it("prefers the scan signal, falls back to the snapshot aggregate, else null", () => {
    expect(aggregateSignal(scan({ signal: 7000 }))).toBe(7000);
    expect(aggregateSignal(scan({ signal: null, metrics: snapshot({ aggregate_signal: 6500 }) }))).toBe(6500);
    expect(aggregateSignal(scan({ signal: null, metrics: snapshot({ aggregate_signal: null }) }))).toBeNull();
  });
});

describe("formatting helpers", () => {
  it("optSignal renders a figure or the empty-graph n/a sentinel", () => {
    expect(optSignal(8000)).toBe("8000");
    expect(optSignal(null)).toBe("n/a");
  });
  it("optDelta signs a positive delta, passes a non-positive, dashes a null", () => {
    expect(optDelta(120)).toBe("+120");
    expect(optDelta(-40)).toBe("-40");
    expect(optDelta(0)).toBe("0");
    expect(optDelta(null)).toBe("—");
  });
  it("shortSha abbreviates to 9 chars, dashes an absent commit", () => {
    expect(shortSha("0123456789abcdef")).toBe("012345678");
    expect(shortSha(null)).toBe("—");
  });
});

// CR-135 §3.2: `status.indexed` gates the STALENESS of a populated signal, never
// the CAUSE of an absent one — `signalAbsence` still owns that question and is
// untouched (§3.3).
describe("snapshotStaleness", () => {
  it("is null while the graph is still indexed — the ordinary case cannot reach the branch", () => {
    expect(snapshotStaleness(status(true), evolution(point()))).toBeNull();
  });

  it("dates the stale label by the LAST snapshot in the series, not the first", () => {
    const stale = snapshotStaleness(
      status(false),
      // Oldest-first, as `governance::evolution` emits it: the tail is the row
      // `latest_metric_snapshot` reads (both order by `id`).
      evolution(
        point({ snapshot_id: 1, created_at: 1_726_704_000 }),
        point({ snapshot_id: 2, created_at: 1_758_240_000 }),
      ),
    );
    expect(stale).toEqual({ date: "2025-09-19" });
  });

  it("leaves the date null rather than fabricating one when the series carries no point", () => {
    expect(snapshotStaleness(status(false), evolution())).toEqual({ date: null });
  });

  it("leaves the date null for a timestamp that is not a representable instant", () => {
    expect(snapshotStaleness(status(false), evolution(point({ created_at: 8.64e15 })))).toEqual({ date: null });
  });

  // `getTime()` is NaN only OUTSIDE the ±8.64e15 ms range. Inside it, a year
  // outside 0000-9999 makes `toISOString()` emit the expanded `±YYYYYY-MM-DD`
  // form, and slicing 10 characters off that yields `"+010000-01"` — a garbled
  // fragment that is neither a date nor null. The boundary is pinned on both
  // sides so the guard cannot be widened or dropped silently.
  it("leaves the date null for an instant outside the ordinary 0000-9999 calendar range", () => {
    // The last second that still formats as an ordinary YYYY-MM-DD.
    expect(snapshotStaleness(status(false), evolution(point({ created_at: 253_402_300_799 })))).toEqual({
      date: "9999-12-31",
    });
    // One second later: year 10000, expanded form.
    expect(snapshotStaleness(status(false), evolution(point({ created_at: 253_402_300_800 })))).toEqual({
      date: null,
    });
    // Year 0000 is ordinary and stays a date; the second before it is year -1.
    expect(snapshotStaleness(status(false), evolution(point({ created_at: -62_167_219_200 })))).toEqual({
      date: "0000-01-01",
    });
    expect(snapshotStaleness(status(false), evolution(point({ created_at: -62_167_219_201 })))).toEqual({
      date: null,
    });
    // The shape this guard exists to prevent must never reach a caller.
    const all = [253_402_300_800, -62_167_219_201, 1_789_860_888_449].map(
      (created_at) => snapshotStaleness(status(false), evolution(point({ created_at })))?.date,
    );
    expect(all.every((d) => d === null)).toBe(true);
  });

  // ── S-436: the graph has moved past the snapshot ──────────────────────────
  // The question CR-135 §3.2 left open when it scoped S-422 to the de-index
  // case. The discriminant is the TIMESTAMP PAIR: `created_at` against the
  // later of `last_full_index_at` / `last_sync_at`. The count comparison the
  // Sprint 72 review proposed is rejected in `healthModel.ts` beside the
  // discriminant — the snapshot's `node_count` is production-scoped and
  // `StatusInfo`'s is the whole graph, so it fires on every project with tests.

  it("reports a snapshot the graph has moved past when a full index ran after it", () => {
    const after = { last_full_index_at: String(DEFAULT_SNAPSHOT_AT + 86_400) };
    expect(snapshotStaleness(status(true, after), evolution(point()), NOW)).toEqual({
      date: "2024-09-19",
      cause: "moved-past",
    });
  });

  it("takes the LATER of last_full_index_at and last_sync_at, so a sync after the snapshot counts", () => {
    // The full index PRECEDES the snapshot; only the incremental sync follows
    // it. Reading `last_full_index_at` alone — the order `freshnessStatement`
    // prefers for its own question — would miss this and report the ordinary case.
    const synced = { last_sync_at: String(DEFAULT_SNAPSHOT_AT + 60) };
    expect(snapshotStaleness(status(true, synced), evolution(point()), NOW)).toEqual({
      date: "2024-09-19",
      cause: "moved-past",
    });
  });

  it("is the ordinary case when the index and the snapshot are simultaneous — strictly after, never equal", () => {
    const same = { last_full_index_at: String(DEFAULT_SNAPSHOT_AT) };
    expect(snapshotStaleness(status(true, same), evolution(point()), NOW)).toBeNull();
    // Asserted as the VALUE, not merely as non-null: a boundary that starts
    // classifying one second's difference as indeterminate rather than
    // moved-past would satisfy `not.toBeNull()` while being wrong.
    const oneLater = { last_full_index_at: String(DEFAULT_SNAPSHOT_AT + 1) };
    expect(snapshotStaleness(status(true, oneLater), evolution(point()), NOW)).toEqual({
      date: "2024-09-19",
      cause: "moved-past",
    });
  });

  it("dates the moved-past label by the LAST snapshot in the series, not the first", () => {
    const after = { last_full_index_at: String(1_758_240_000 + 1) };
    const moved = snapshotStaleness(
      status(true, after),
      evolution(
        point({ snapshot_id: 1, created_at: DEFAULT_SNAPSHOT_AT }),
        point({ snapshot_id: 2, created_at: 1_758_240_000 }),
      ),
      NOW,
    );
    expect(moved).toEqual({ date: "2025-09-19", cause: "moved-past" });
  });

  // KNOWN OVER-REPORT, recorded as a test so the label cannot later be read as
  // precision it does not have. An index that ran after the snapshot may have
  // changed NOTHING — same files, same graph, same figures — and this reports
  // `moved-past` just the same. It detects DISAGREEMENT, never currency. The
  // over-report is the safe direction (NFR-RA-05 prefers it to false assurance)
  // and the rendered wording claims only that the graph has moved past the
  // snapshot, never that the figures are stale.
  it("over-reports a re-index that changed nothing — a KNOWN over-report, never a staleness proof", () => {
    // Identical graph either side of the re-index: the counts are the same, the
    // signal is the same, only the index clock moved.
    const unchanged = status(true, {
      last_full_index_at: String(DEFAULT_SNAPSHOT_AT + 3_600),
      node_count: 120,
      edge_count: 240,
    });
    const moved = snapshotStaleness(unchanged, evolution(point({ signal: 8000 })), NOW);
    expect(moved).toEqual({ date: "2024-09-19", cause: "moved-past" });
    // Stated as the fact it is: the comparison establishes disagreement between
    // two clocks, and nothing whatever about whether the figures changed.
    expect(moved?.cause).toBe("moved-past");
  });

  // ── S-436: the indeterminate arm ──────────────────────────────────────────
  // `last_sync_at` is a FILE MTIME, so a stamp ahead of now is routine on a
  // copied tree, an NFS mount or a restored backup — the real-world shape,
  // exercised here rather than a contrived one.

  it("is indeterminate for a future-dated last_sync_at, reusing S-433's wording verbatim", () => {
    const ahead = { last_sync_at: String(NOW + 86_400) };
    expect(snapshotStaleness(status(true, ahead), evolution(point()), NOW)).toEqual({
      date: null,
      cause: "indeterminate",
      detail: "the last index or sync is at an unknown age (recorded ahead of now — check the clock)",
    });
  });

  // The same sentence, pinned against S-433's exported constant rather than a
  // second copy of the words: if the Dashboard rewords this condition, the
  // Health page moves with it or this fails. One product, one wording.
  it("takes the ahead-of-now phrase from S-433's constant, not a restatement of it", () => {
    const ahead = { last_full_index_at: String(NOW + 1) };
    const result = snapshotStaleness(status(true, ahead), evolution(point()), NOW);
    expect(result?.detail).toBe(`the last index or sync is ${UNKNOWN_AGE_AHEAD_OF_NOW}`);
  });

  it("is indeterminate when a future stamp sits beside a usable one — a clock that lies once is not trusted twice", () => {
    // `last_full_index_at` alone would say "moved past"; the future `last_sync_at`
    // says the clock cannot be trusted, and picking the reassuring reading of an
    // untrustworthy pair is exactly what NFR-RA-05 forbids.
    const mixed = {
      last_full_index_at: String(DEFAULT_SNAPSHOT_AT + 3_600),
      last_sync_at: String(NOW + 3_600),
    };
    expect(snapshotStaleness(status(true, mixed), evolution(point()), NOW)?.cause).toBe("indeterminate");
  });

  it("is indeterminate when neither an index nor a sync time is recorded", () => {
    const none = { last_full_index_at: null, last_sync_at: null };
    expect(snapshotStaleness(status(true, none), evolution(point()), NOW)).toEqual({
      date: null,
      cause: "indeterminate",
      detail: "no index or sync time is recorded",
    });
  });

  it("treats a present-but-unparseable stamp as absent, the contract parseSecs already has", () => {
    const junk = { last_full_index_at: "not-a-timestamp", last_sync_at: null };
    expect(snapshotStaleness(status(true, junk), evolution(point()), NOW)?.detail).toBe(
      "no index or sync time is recorded",
    );
  });

  it("is indeterminate when nothing dates the snapshot, even with a usable index time", () => {
    const after = { last_full_index_at: String(NOW - 60) };
    const undated = {
      date: null,
      cause: "indeterminate",
      detail: "the last snapshot carries no date to compare against",
    };
    // No point at all…
    expect(snapshotStaleness(status(true, after), evolution(), NOW)).toEqual(undated);
    // …and a point whose instant is not a representable calendar date.
    expect(snapshotStaleness(status(true, after), evolution(point({ created_at: 8.64e15 })), NOW)).toEqual(
      undated,
    );
    expect(
      snapshotStaleness(status(true, after), evolution(point({ created_at: 253_402_300_800 })), NOW),
    ).toEqual(undated);
  });

  it("never renders a date on the indeterminate arm — `date` is null by type, not by convention", () => {
    const shapes = [
      status(true, { last_sync_at: String(NOW + 1) }),
      status(true, { last_full_index_at: null, last_sync_at: null }),
    ];
    for (const st of shapes) {
      const result = snapshotStaleness(st, evolution(point()), NOW);
      expect(result?.cause).toBe("indeterminate");
      expect(result?.date).toBeNull();
    }
  });

  // ── S-436: the two conditions compose, de-index first ─────────────────────

  it("still reports de-indexed, not moved-past, when a de-indexed graph was also indexed after the snapshot", () => {
    const after = { last_full_index_at: String(DEFAULT_SNAPSHOT_AT + 86_400) };
    // Byte-identical to S-422's value: a date and nothing else, no `cause`.
    expect(snapshotStaleness(status(false, after), evolution(point()), NOW)).toEqual({ date: "2024-09-19" });
  });

  it("still reports de-indexed when the de-indexed graph's timestamps are indeterminate", () => {
    const ahead = { last_full_index_at: null, last_sync_at: String(NOW + 86_400) };
    expect(snapshotStaleness(status(false, ahead), evolution(point()), NOW)).toEqual({ date: "2024-09-19" });
  });

  // ── S-436: the mutation guard, by name ────────────────────────────────────
  // The AC names this mutation explicitly: restoring `if (status.indexed) return
  // null` as snapshotStaleness's SOLE condition must fail a test by name. Every
  // input below satisfies `status.indexed === true`, so that mutation returns
  // null for all of them and this test fails on its first assertion.
  it("mutation guard: `if (status.indexed) return null` as the sole condition is not the discriminant", () => {
    const movedPast = status(true, { last_full_index_at: String(DEFAULT_SNAPSHOT_AT + 1) });
    expect(snapshotStaleness(movedPast, evolution(point()), NOW)).not.toBeNull();

    const futureStamp = status(true, { last_sync_at: String(NOW + 1) });
    expect(snapshotStaleness(futureStamp, evolution(point()), NOW)).not.toBeNull();

    const noStamp = status(true, { last_full_index_at: null, last_sync_at: null });
    expect(snapshotStaleness(noStamp, evolution(point()), NOW)).not.toBeNull();

    const undatedSnapshot = status(true, { last_full_index_at: String(NOW - 60) });
    expect(snapshotStaleness(undatedSnapshot, evolution(), NOW)).not.toBeNull();
  });
});
