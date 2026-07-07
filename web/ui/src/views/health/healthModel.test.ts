import { describe, expect, it } from "vitest";

import type { MetricSnapshot, MetricValue, ScanResult } from "../../api/types.ts";
import {
  aggregateSignal,
  metricRows,
  optDelta,
  optSignal,
  shortSha,
  structuralDetails,
} from "./healthModel.ts";

function mv(n: number): MetricValue {
  return { raw: n, normalized: n };
}

function snapshot(over: Partial<MetricSnapshot> = {}): MetricSnapshot {
  return {
    modularity: mv(0.9),
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
    worst_offenders: { nesting: [], conciseness: [], cohesion: [], focus: [], uniqueness: [] },
    warnings: [],
    ...over,
  };
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
});

describe("structuralDetails", () => {
  it("joins the five structural dimensions to their worst offenders", () => {
    const s = scan();
    s.worst_offenders.nesting = [{ name: "deep_fn", file: "src/a.rs", line: 42, detail: "nesting depth 6" }];
    const dims = structuralDetails(s);
    expect(dims.map((d) => d.name)).toEqual(["Nesting", "Conciseness", "Cohesion", "Focus", "Uniqueness"]);
    expect(dims[0].offenders).toHaveLength(1);
    expect(dims[0].offenders[0].name).toBe("deep_fn");
  });
  it("keeps an n/a dimension's value null", () => {
    const dims = structuralDetails(scan({ metrics: snapshot({ cohesion: null }) }));
    expect(dims.find((d) => d.name === "Cohesion")?.value).toBeNull();
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
