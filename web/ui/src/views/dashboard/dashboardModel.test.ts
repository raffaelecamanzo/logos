import { describe, expect, it } from "vitest";

import type { StatusInfo } from "../../api/types.ts";
import {
  bandOf,
  freshnessStatement,
  humanizeAge,
  pctBp,
  snippetOf,
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
  it("buckets the age and saturates a future timestamp to 'just now'", () => {
    expect(humanizeAge(1000, 1000)).toBe("just now");
    expect(humanizeAge(1090, 1000)).toBe("1m ago");
    expect(humanizeAge(4700, 1000)).toBe("1h ago");
    expect(humanizeAge(1000 + 90_000, 1000)).toBe("1d ago");
    expect(humanizeAge(1000, 5000)).toBe("just now"); // clock skew
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
});

describe("snippetOf", () => {
  it("takes the first prose paragraph, stripping structure and links", () => {
    const body = "# Title\n\nThe [project](/x) does **things** well.\n\nSecond paragraph.";
    expect(snippetOf(body)).toBe("The project does things well.");
  });
  it("skips a fenced code block before the prose", () => {
    const body = "```\ncode();\n```\n\nReal prose here.";
    expect(snippetOf(body)).toBe("Real prose here.");
  });
  it("returns an empty string for a prose-less body (caller falls back honestly)", () => {
    expect(snippetOf("# Only a heading\n")).toBe("");
    expect(snippetOf("")).toBe("");
  });
  it("truncates at a word boundary with an ellipsis", () => {
    const long = `${"word ".repeat(150)}`.trim();
    const out = snippetOf(long);
    expect(out.endsWith("…")).toBe(true);
    expect([...out].length).toBeLessThanOrEqual(481);
  });
  it("skips a setext-underlined title and leads with the body prose", () => {
    expect(snippetOf("Title\n===\n\nReal prose.")).toBe("Real prose.");
  });
  it("strips a leading list / quote / ordered marker from the first prose line", () => {
    expect(snippetOf("- A bullet of prose.")).toBe("A bullet of prose.");
    expect(snippetOf("> A quoted line.")).toBe("A quoted line.");
    expect(snippetOf("1. An ordered item.")).toBe("An ordered item.");
  });
  it("skips a thematic break (HR) before the prose", () => {
    expect(snippetOf("---\n\nProse after a rule.")).toBe("Prose after a rule.");
  });
  it("skips a ~~~ fenced block as well as a ``` one", () => {
    expect(snippetOf("~~~\ncode\n~~~\n\nProse here.")).toBe("Prose here.");
  });
});
