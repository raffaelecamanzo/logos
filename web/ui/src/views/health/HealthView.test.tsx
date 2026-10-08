import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { HealthModel, MetricSnapshot, MetricValue } from "../../api/types.ts";
import { Badge, type BadgeTone } from "../../components/index.ts";
import { expectWidgetCopy, readerText } from "../../copy/expectWidgetCopy.ts";
import {
  DIMENSION_COPY,
  NO_APPLICABLE_CONSTRUCT,
  READING_ABSENCE,
  gate as gateCopy,
  qualitySignal as qualitySignalCopy,
  signalTrend as signalTrendCopy,
} from "../../copy/health.copy.ts";
import { removeHiddenWidgetEntry } from "../../test/hiddenWidgets.ts";
import { HealthView } from "./HealthView.tsx";
import { widgetsIn, widgetTitle, widgetTitled } from "../../test/widgetStack.ts";
import { metricRows, type DimensionKey } from "./healthModel.ts";
import realOffenders from "./fixtures/worst-offenders.real.json";

function mv(n: number): MetricValue {
  return { raw: n, normalized: n };
}

function metrics(over: Partial<MetricSnapshot> = {}): MetricSnapshot {
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

const HEALTH: HealthModel = {
  status: { indexed: true, file_count: 1, node_count: 1, edge_count: 1, db_path: "", db_size_bytes: 0, last_full_index_at: "50", last_sync_at: null, graph_revision: 1, refs_total: 0, refs_resolved: 0, refs_unresolved: 0, resolution_coverage: 0, total_line_count: null, source_line_count: null, test_line_count: null, freshness: "", warnings: [] },
  gate: { passed: true, saved: false, signal: 8000, baseline_signal: 7800, test_function_count: 12, threshold: null, epsilon: 1, freshness: "", message: "", warnings: [] },
  scan: {
    signal: 8000,
    freshness: "",
    metrics: metrics(),
    worst_offenders: {
      recorded: true,
      nesting: [{ name: "deep_fn", file: "src/a.rs", line: 42, detail: "nesting depth 6" }],
      conciseness: [],
      cohesion: [],
      focus: [],
      uniqueness: [],
      acyclicity: [],
      depth: [],
      equality: [],
      redundancy: [],
    },
    warnings: [],
  },
  evolution: {
    snapshots: [
      { snapshot_id: 1, created_at: 100, commit_sha: "0123456789ab", signal: 7800, signal_delta: null },
      { snapshot_id: 2, created_at: 200, commit_sha: null, signal: 8000, signal_delta: 200 },
    ],
    warnings: [],
  },
};

function clone(): HealthModel {
  return JSON.parse(JSON.stringify(HEALTH)) as HealthModel;
}

function stub(model: HealthModel) {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      expect(url).toBe("/api/v1/health");
      return Promise.resolve({ ok: true, json: () => Promise.resolve(model) } as Response);
    }),
  );
}

// The tone matcher every chip pin shares. Hash-agnostic: render a reference
// element of the intended tone and compare class names, never a literal
// CSS-module hash (the `ScoreBar` tone test's shape). One matcher rather than a
// hand-mirrored twin per spec, so the stale pin and the ordinary pin cannot
// drift apart in what they mean by "this tone".
function chipClass(tone: BadgeTone) {
  const { container } = render(<Badge tone={tone}>REF</Badge>);
  return within(container).getByText("REF").className;
}

const DIMENSION_TITLES = [
  "Modularity", "Acyclicity", "Depth", "Equality", "Redundancy",
  "Nesting", "Conciseness", "Cohesion", "Focus", "Uniqueness",
];
const OFFENDER_BACKED = [
  "Acyclicity", "Depth", "Equality", "Redundancy",
  "Nesting", "Conciseness", "Cohesion", "Focus", "Uniqueness",
];
/** The four CR-209 added — rendered exactly like the five CR-005 lists. */
const CR209 = ["Acyclicity", "Depth", "Equality", "Redundancy"];

/** Every rendered widget, in document order. */
const frames = () => widgetsIn(document.body);

/** The one widget with this title. */
const widget = (title: string) => widgetTitled(document.body, title);

/** A part of a widget: the copy element of that name (what, why), else the
 *  frame part (title, figure, evidence). */
function part(frame: Element, name: string): HTMLElement | null {
  return (
    frame.querySelector<HTMLElement>(`[data-widget-copy="${name}"]`) ??
    frame.querySelector<HTMLElement>(`[data-widget-part="${name}"]`)
  );
}

/** Text a reader sees in an element — `expectWidgetCopy`'s own reading — or "". */
function seen(el: Element | null): string {
  return el === null ? "" : readerText(el);
}

/** The titles of the widgets whose text names `command`, as a word, in page
 *  order. Since CR-206 a command is named inside an absence or not-current
 *  sentence, never as a text node of its own, so an exact-text query would find
 *  nothing and pass over nothing. */
function widgetsNaming(command: string): string[] {
  const named = new RegExp(`\\b${command.replace(/ /g, "\\s+")}\\b`);
  return frames()
    .filter((w) => named.test(seen(w)))
    .map(widgetTitle);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
beforeEach(() => vi.clearAllMocks());

describe("HealthView on the widget frame (S-615, FR-UI-43)", () => {
  it("renders Gate, Quality signal, the ten dimensions and Signal trend in ONE WidgetStack", async () => {
    stub(HEALTH);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    expect(document.querySelectorAll("[data-widget-stack]")).toHaveLength(1);
    const stack = document.querySelector("[data-widget-stack]")!;
    // Each widget is its own child of the one stack, so the stack owns every gap.
    const children = [...stack.children];
    expect(children).toHaveLength(13);
    expect(children.map((c) => c.querySelectorAll("[data-widget]").length)).toEqual(Array(13).fill(1));
    expect(frames().map(widgetTitle)).toEqual(["Gate", "Quality signal", ...DIMENSION_TITLES, "Signal trend"]);
  });

  it("enumerates 10 of 10 dimension widgets from the Quality signal table's own list, in its order", async () => {
    stub(HEALTH);
    render(<HealthView />);
    const grid = await screen.findByRole("table", { name: "Quality metrics" });
    const tableOrder = within(grid)
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("cell")[0].textContent);
    const widgetOrder = frames().map(widgetTitle).filter((t) => DIMENSION_TITLES.includes(t));
    expect(widgetOrder).toHaveLength(10);
    expect(widgetOrder).toEqual(tableOrder);
    expect(widgetOrder).toEqual(metricRows(HEALTH.scan.metrics).map((r) => r.name));
  });

  it("every widget passes the message standard on the populated fixture", async () => {
    stub(HEALTH);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    expectWidgetCopy(widget("Gate"), gateCopy);
    expectWidgetCopy(widget("Quality signal"), qualitySignalCopy);
    expectWidgetCopy(widget("Signal trend"), signalTrendCopy);
    for (const row of metricRows(HEALTH.scan.metrics)) {
      expectWidgetCopy(widget(row.name), DIMENSION_COPY[row.key]);
    }
  });
});

describe("Gate widget (CR-203 item 12)", () => {
  it("PASS: the verdict against the baseline with ε's pass condition, and no dimension named", async () => {
    stub(HEALTH);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    const gate = widget("Gate");
    expect(seen(part(gate, "figure"))).toBe("PASS · signal 8000 vs baseline 7800; passes at ≥ 7799 (ε = 1)");
    expect(seen(gate)).not.toContain("Lowest-scoring dimension");
    // The verdict chip is the title row's one badge, in the pass tone.
    const chip = within(part(gate, "title")!).getByText("PASS");
    expect(chip.className).toBe(chipClass("green"));
    expect(chip.className).not.toBe(chipClass("red"));
    // "baseline" and ε are glossed where they first appear: the figure.
    const figure = part(gate, "figure")!;
    expect([...figure.querySelectorAll("dfn")].map((d) => d.getAttribute("data-term"))).toEqual(["baseline", "epsilon"]);
  });

  it("reads ε from GateResult.epsilon rather than restating it", async () => {
    const m = clone();
    m.gate.epsilon = 2.5;
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    expect(seen(part(widget("Gate"), "figure"))).toBe("PASS · signal 8000 vs baseline 7800; passes at ≥ 7797.5 (ε = 2.5)");
  });

  it("FAIL: names the lowest-scoring dimension as a fact under the figure, and no action (CR-206)", async () => {
    const m = clone();
    m.gate.passed = false;
    m.gate.signal = 7700;
    m.scan.metrics.depth = mv(0.2); // the lowest applicable dimension
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    const gate = widget("Gate");
    expect(seen(part(gate, "figure"))).toBe(
      "FAIL · signal 7700 vs baseline 7800; passes at ≥ 7799 (ε = 1)Lowest-scoring dimension: Depth.",
    );
    expect(within(part(gate, "title")!).getByText("FAIL").className).toBe(chipClass("red"));
    expectWidgetCopy(gate, gateCopy);
    const notes = [...part(gate, "figure")!.querySelectorAll("[data-figure-note]")].map(seen);
    expect(notes).toEqual(["Lowest-scoring dimension: Depth."]);
    expect(seen(gate)).not.toContain("logos gate --save");
  });

  it("states a missing baseline as n/a and an informational pass, with no fabricated pass floor", async () => {
    // `baseline_signal: null` is both "no baseline saved" and "a baseline with no
    // signal" (SIGNAL_OR_BASELINE_ABSENT); the figure claims neither.
    const m = clone();
    m.gate.baseline_signal = null;
    m.gate.message = "no baseline saved — informational pass (save one with `gate --save`)";
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    expect(seen(part(widget("Gate"), "figure"))).toBe("PASS · signal 8000 vs baseline n/a; informational pass");
  });

  // FR-GV-10: after a `[metric_thresholds]` change the read-only verdict cannot
  // compare against the old baseline and passes informationally — the signal may
  // sit far below the old floor, so the floor must not be shown as a condition held.
  it("never shows a pass floor the gate did not apply: an incomparable baseline is an informational pass", async () => {
    for (const message of [
      "baseline thresholds differ — informational pass (re-save with `gate --save`)",
      "baseline recorded under different metric semantics — informational pass (re-save with `gate --save`)",
    ]) {
      cleanup();
      const m = clone();
      m.gate.signal = 7000; // below 7800 − 1, yet passed: no comparison was made
      m.gate.message = message;
      stub(m);
      render(<HealthView />);
      await screen.findByText("Signal evolution");
      const gate = widget("Gate");
      const figure = seen(part(gate, "figure"));
      expect(figure).toMatch(/^PASS · signal 7000 vs baseline 7800; not compared, informational pass/);
      expect(figure).not.toMatch(/passes at/);
      expect(figure).toContain("the next logos gate run saves the current score as the new baseline");
      expectWidgetCopy(gate, gateCopy);
    }
  });

  it("keeps the pass floor for a compared verdict, whatever the server's message", async () => {
    const m = clone();
    m.gate.message = "signal 8000 holds the baseline 7800 (ε 1)";
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    expect(seen(part(widget("Gate"), "figure"))).toBe("PASS · signal 8000 vs baseline 7800; passes at ≥ 7799 (ε = 1)");
  });

});

/** A payload whose every applicable dimension scores a full 1 — no dimension to start with. */
function allFull(): HealthModel {
  const m = clone();
  for (const key of ["modularity", "acyclicity", "depth", "equality", "redundancy", "nesting", "conciseness", "focus", "uniqueness"] as const) {
    m.scan.metrics[key] = { raw: 0, normalized: 1 };
  }
  m.scan.metrics.cohesion = null;
  m.scan.worst_offenders = realOffenders.recordedEmpty;
  return m;
}

describe("no dimension below a full score", () => {
  it("a Gate FAIL names no dimension it cannot point to", async () => {
    const m = allFull();
    m.gate.passed = false;
    m.gate.signal = 7700;
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    const figure = seen(part(widget("Gate"), "figure"));
    expect(figure).toBe("FAIL · signal 7700 vs baseline 7800; passes at ≥ 7799 (ε = 1)");
    expect(figure).not.toContain("Lowest-scoring dimension");
  });
});

describe("Quality signal widget (CR-203 items 13 and 20)", () => {
  it("states n / 10000 as the geometric mean of the k applicable dimensions, with the scope line", async () => {
    stub(HEALTH);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    const q = widget("Quality signal");
    expect(seen(part(q, "figure"))).toBe(
      "8000 / 10000, geometric mean of the 10 applicable dimensions90 production functions scored · 12 test functions excluded",
    );
    expect(within(part(q, "figure")!).getByText("90 production functions scored · 12 test functions excluded")).toBeInTheDocument();
  });

  it("counts only the applicable dimensions in k", async () => {
    const m = clone();
    m.scan.metrics.cohesion = null;
    m.scan.metrics.focus = null;
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    expect(seen(part(widget("Quality signal"), "figure"))).toMatch(/^8000 \/ 10000, geometric mean of the 8 applicable dimensions/);
  });

  it("explains the thresholds fingerprint: [metric_thresholds], and that a change re-baselines the gate", async () => {
    stub(HEALTH);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    const evidence = part(widget("Quality signal"), "evidence")!;
    const disclosure = evidence.querySelector("details")!;
    expect(seen(disclosure.querySelector("summary"))).toBe("Thresholds fingerprint abc123");
    const body = seen(disclosure.querySelector("p"));
    expect(body).toContain("[metric_thresholds]");
    expect(body).toContain(".logos/rules.toml");
    // FR-GV-10: the persisting gate re-baselines by itself — no manual save is asked for.
    expect(body).toContain("re-baselines the gate");
    expect(body).toContain("the next logos gate run saves the new score as the baseline by itself");
    expect(body).not.toContain("--save");
    // The internal term is glossed in the disclosure too.
    expect(disclosure.querySelector('dfn[data-term="baseline"]')).not.toBeNull();
  });

  it("renders no separate Aggregate scope card", async () => {
    stub(HEALTH);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    expect(screen.queryByText(/Aggregate scope/i)).toBeNull();
    expect(screen.queryByText(/^Aggregate/)).toBeNull();
    expect(frames().map(widgetTitle)).not.toContain("Aggregate scope");
  });

  it("renders a CR-156 Modularity drop-out as not applicable with its reason and m-of-5 count", async () => {
    const m = clone();
    m.scan.metrics.modularity = { raw: -0.5, normalized: 0 };
    m.scan.metrics.edge_count = 3;
    m.scan.metrics.modularity_not_applicable = { edges: 3, min_edges: 5, reason: "3 of 5 dependency edges — too few for community structure" };
    stub(m);
    render(<HealthView />);
    const grid = await screen.findByRole("table", { name: "Quality metrics" });
    const row = within(grid).getByText("Modularity").closest("tr");
    expect(row).not.toBeNull();
    const cells = within(row as HTMLElement);
    // The score cell names the drop-out and its evidence — never a 0 score bar.
    expect(cells.getByText("not applicable")).toBeInTheDocument();
    expect(cells.getByText("3 of 5 dependency edges — too few for community structure")).toBeInTheDocument();
    expect(cells.queryByRole("meter")).toBeNull();
    // The computed pair is persisted, not hidden: Normalized and Raw still show it.
    expect(cells.getByText("0.00")).toBeInTheDocument();
    expect(cells.getByText("-0.50")).toBeInTheDocument();
    // Every other row still renders its score.
    const acyclicity = within(grid).getByText("Acyclicity").closest("tr") as HTMLElement;
    expect(within(acyclicity).queryByText("not applicable")).toBeNull();
    // …and k drops to 9: Modularity is out of the mean.
    expect(seen(part(widget("Quality signal"), "figure"))).toMatch(/geometric mean of the 9 applicable dimensions/);
  });

  it("sorts a CR-156 not-applicable Modularity with the unscored rows, never by its computed value", async () => {
    const user = userEvent.setup();
    const m = clone();
    // A high computed value that would rank last ascending if it were scored.
    m.scan.metrics.modularity = mv(0.95);
    m.scan.metrics.modularity_not_applicable = { edges: 4, min_edges: 5, reason: "4 of 5 dependency edges — too few for community structure" };
    stub(m);
    render(<HealthView />);
    const grid = await screen.findByRole("table", { name: "Quality metrics" });
    await user.click(within(grid).getByRole("button", { name: /Score/ }));
    const firstRow = within(grid).getAllByRole("row")[1];
    expect(within(firstRow).getByText("Modularity")).toBeInTheDocument();
  });
});

describe("Dimension widgets (CR-203 items 14–19)", () => {
  it("each carries its score bar, normalized value and raw value with its unit", async () => {
    const m = clone();
    // Raw values in their own units, as a real scan records them.
    m.scan.metrics.acyclicity = { raw: 2, normalized: 0.33 };
    m.scan.metrics.depth = { raw: 1, normalized: 0.89 };
    m.scan.metrics.nesting = { raw: 0.1, normalized: 0.9 };
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    for (const row of metricRows(m.scan.metrics)) {
      const figure = part(widget(row.name), "figure")!;
      expect(within(figure).getByRole("meter"), row.name).toBeInTheDocument();
      expect(seen(figure), row.name).toContain(row.value!.normalized.toFixed(2));
    }
    expect(seen(part(widget("Nesting"), "figure"))).toContain("10.0% of production functions are deeply nested");
    expect(seen(part(widget("Acyclicity"), "figure"))).toContain("2 dependency cycles");
    expect(seen(part(widget("Depth"), "figure"))).toContain("longest chain of 1 unit, each cycle counted as one");
    // Equality's raw value is a Gini coefficient, glossed at its first use.
    expect(part(widget("Equality"), "figure")!.querySelector('dfn[data-term="gini"]')).not.toBeNull();
  });

  it("never rounds a non-zero share to 0.0%: one offender among thousands is <0.1%", async () => {
    const m = clone();
    m.scan.metrics.function_count = 2500;
    m.scan.metrics.nesting = { raw: 1 / 2500, normalized: 1 - 1 / 2500 };
    m.scan.metrics.redundancy = { raw: 0, normalized: 1 };
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    const nesting = seen(part(widget("Nesting"), "figure"));
    expect(nesting).toContain("<0.1% of production functions are deeply nested");
    expect(nesting).not.toContain("0.0%");
    expect(within(part(widget("Nesting"), "title")!).getByText("1 flagged")).toBeInTheDocument();
    // A genuine zero still reads as one.
    expect(seen(part(widget("Redundancy"), "figure"))).toContain("0.0% of production functions are dead or duplicated");
  });

  it("Modularity alone states that it has no list, never an empty table", async () => {
    for (const recorded of [true, false]) {
      cleanup();
      const m = clone();
      m.scan.worst_offenders = recorded ? realOffenders.recordedEmpty : realOffenders.notRecorded;
      stub(m);
      render(<HealthView />);
      await screen.findByText("Signal evolution");
      // No list, so no offender badge: the title never claims a clean result
      // ("none flagged") or a recording state (CR-162, NFR-CC-04).
      expect(seen(part(widget("Modularity"), "title"))).toBe("Modularity");
      const evidence = part(widget("Modularity"), "evidence")!;
      expect(within(evidence).queryByRole("table")).toBeNull();
      expect(evidence.querySelector('[data-offender-state="unlisted"]')).not.toBeNull();
      expect(seen(evidence)).toMatch(/^No list of units/);
      // Modularity has no per-unit attribution: it says so and points nowhere.
      expect(within(evidence).queryByRole("link")).toBeNull();
      expect(evidence.querySelector("code")).toBeNull();
      expect(seen(evidence)).toMatch(/as a whole/);
      // CR-209 AC-4: the four it added are lists now, never "unlisted".
      expect(document.querySelectorAll('[data-offender-state="unlisted"]')).toHaveLength(1);
      for (const name of CR209) {
        expect(seen(widget(name)), name).not.toMatch(/No list of|unlisted/i);
      }
    }
    // The Architecture view is hidden (CR-208): no Health text names it or its matrix.
    expect(document.querySelector('a[href^="/architecture"]')).toBeNull();
    expect(seen(document.body)).not.toMatch(/Architecture|dependency matrix/i);
  });

  it("renders the four CR-209 lists with the same badge and table as the five (CR-209 AC-4)", async () => {
    const m = clone();
    m.scan.worst_offenders = {
      ...realOffenders.recordedEmpty,
      acyclicity: [
        { name: "ping", file: "src/alpha/mod.rs", line: 1, detail: "2 symbols across 2 directories: src/alpha, src/beta" },
      ],
      depth: [
        { name: "api", file: "", line: null, detail: "api → core → util (3 directories)" },
        { name: "cli", file: "", line: null, detail: "cli → util (2 directories)" },
      ],
      equality: [{ name: "parse", file: "src/p.rs", line: 9, detail: "complexity 31" }],
      redundancy: [{ name: "gone", file: "src/g.rs", line: 4, detail: "dead, duplicate" }],
    };
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    const expected: Record<string, string[][]> = {
      Acyclicity: [["ping", "src/alpha/mod.rs", "1", "2 symbols across 2 directories: src/alpha, src/beta"]],
      Depth: [
        ["api", "", "", "api → core → util (3 directories)"],
        ["cli", "", "", "cli → util (2 directories)"],
      ],
      Equality: [["parse", "src/p.rs", "9", "complexity 31"]],
      Redundancy: [["gone", "src/g.rs", "4", "dead, duplicate"]],
    };
    for (const [name, rows] of Object.entries(expected)) {
      const w = widget(name);
      expect(within(part(w, "title")!).getByText(`${rows.length} flagged`), name).toBeInTheDocument();
      const table = within(w).getByRole("table", { name: "Worst offenders" });
      const cells = within(table)
        .getAllByRole("row")
        .slice(1)
        .map((r) => within(r).getAllByRole("cell").map((c) => c.textContent));
      expect(cells, name).toEqual(rows);
    }
  });

  it("renders a list its snapshot predates (`unrecorded`) as not recorded, never as none flagged", async () => {
    const m = clone();
    m.scan.worst_offenders = { ...realOffenders.recordedEmpty, unrecorded: ["acyclicity", "redundancy"] };
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    for (const name of ["Acyclicity", "Redundancy"]) {
      const w = widget(name);
      expect(within(part(w, "title")!).getByText("not recorded"), name).toBeInTheDocument();
      expect(seen(w), name).not.toMatch(/none flagged/i);
    }
    for (const name of ["Depth", "Equality", "Nesting"]) {
      expect(within(part(widget(name), "title")!).getByText("none flagged"), name).toBeInTheDocument();
    }
  });

  it("no dimension widget carries an action line, at any score, listed or not (CR-206)", async () => {
    const m = clone();
    m.scan.metrics.acyclicity = mv(1);
    m.scan.worst_offenders = realOffenders.recorded;
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    for (const row of metricRows(m.scan.metrics)) {
      const w = widget(row.name);
      expectWidgetCopy(w, DIMENSION_COPY[row.key]);
      expect(seen(w), row.name).not.toMatch(/\[metric_thresholds\] in \.logos\/rules\.toml|What you can do/);
    }
    // Modularity's named absence is evidence, not an action.
    expect(seen(part(widget("Modularity"), "evidence"))).toMatch(/^No list of units/);
  });

  it("renders an ADR-21 drop-out as a stated n/a, never a fabricated zero", async () => {
    const m = clone();
    m.scan.metrics.cohesion = null;
    m.scan.metrics.focus = null;
    stub(m);
    render(<HealthView />);
    const grid = await screen.findByRole("table", { name: "Quality metrics" });
    // The Cohesion + Focus rows render n/a badges (no 0.00 figure fabricated).
    expect(within(grid).getAllByText("n/a").length).toBeGreaterThanOrEqual(2);
    for (const [name, key] of [["Cohesion", "cohesion"], ["Focus", "focus"]] as const) {
      const w = widget(name);
      expect(seen(part(w, "figure"))).toBe(NO_APPLICABLE_CONSTRUCT);
      expect(w.querySelector("[data-widget-absence]")).not.toBeNull();
      expect(within(w).queryByRole("meter")).toBeNull();
      expect(within(w).queryByRole("table")).toBeNull();
      expectWidgetCopy(w, DIMENSION_COPY[key]);
    }
  });

  it("renders a CR-156 Modularity drop-out as not applicable with its reason — no score bar", async () => {
    const m = clone();
    m.scan.metrics.modularity = { raw: -0.5, normalized: 0 };
    m.scan.metrics.modularity_not_applicable = { edges: 3, min_edges: 5, reason: "3 of 5 dependency edges — too few for community structure" };
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    const w = widget("Modularity");
    const figure = part(w, "figure")!;
    expect(within(figure).getByText("not applicable")).toBeInTheDocument();
    expect(within(figure).getByText("3 of 5 dependency edges — too few for community structure")).toBeInTheDocument();
    expect(within(figure).queryByRole("meter")).toBeNull();
    // The reason stands in place of a score — no normalized figure beside it — and
    // the computed raw value is still stated.
    const lines = [...figure.querySelector("[data-dimension]")!.children].map((line) => seen(line));
    expect(lines).toEqual([
      "not applicable 3 of 5 dependency edges — too few for community structure",
      "Q -0.50 (Newman's modularity, from −0.5 to 1)",
    ]);
    expectWidgetCopy(w, DIMENSION_COPY.modularity);
  });
});

// S-499 / CR-162: the five offender-backed widgets render the persisted offenders
// in three honest states, decided by the `recorded` flag — a snapshot that
// recorded nothing is never presented as a clean result.
describe("HealthView offender states (S-499, FR-QM-15 / NFR-CC-04)", () => {
  const NONE_FLAGGED = /No offenders flagged within thresholds/i;
  const NOT_RECORDED = /Offenders were not recorded for this snapshot/i;

  it("renders recorded-empty as 'No offenders flagged' in every applicable widget — and only then", async () => {
    const m = clone();
    m.scan.worst_offenders = realOffenders.recordedEmpty;
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    for (const name of OFFENDER_BACKED) {
      const w = widget(name);
      expect(seen(part(w, "evidence"))).toMatch(NONE_FLAGGED);
      expect(seen(w)).not.toMatch(NOT_RECORDED);
      expect(within(w).queryByRole("table")).toBeNull();
      expect(within(part(w, "title")!).getByText("none flagged")).toBeInTheDocument();
    }
  });

  it("renders a not-recorded snapshot as 'not recorded' in every widget, never as a clean result", async () => {
    const m = clone();
    m.scan.worst_offenders = realOffenders.notRecorded;
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    for (const name of OFFENDER_BACKED) {
      const w = widget(name);
      expect(seen(part(w, "evidence"))).toMatch(NOT_RECORDED);
      expect(seen(w)).not.toMatch(NONE_FLAGGED);
      expect(seen(w)).not.toMatch(/none flagged/i);
      expect(within(w).queryByRole("table")).toBeNull();
      expect(within(part(w, "title")!).getByText("not recorded")).toBeInTheDocument();
      // The absence names the command that records them (CR-206).
      expect(seen(part(w, "evidence"))).toBe("Offenders were not recorded for this snapshot; run logos scan to record them.");
      const key = name.toLowerCase() as DimensionKey;
      expectWidgetCopy(w, DIMENSION_COPY[key]);
    }
  });

  it("treats a payload without the recorded flag as not recorded", async () => {
    const m = clone();
    delete (m.scan.worst_offenders as Partial<typeof m.scan.worst_offenders>).recorded;
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    for (const name of OFFENDER_BACKED) {
      expect(seen(part(widget(name), "evidence"))).toMatch(NOT_RECORDED);
    }
  });

  it("keeps an n/a dimension's n/a rendering whether or not offenders were recorded", async () => {
    for (const recorded of [true, false]) {
      cleanup();
      const m = clone();
      m.scan.worst_offenders = recorded ? realOffenders.recordedEmpty : realOffenders.notRecorded;
      m.scan.metrics.cohesion = null;
      m.scan.metrics.focus = null;
      stub(m);
      render(<HealthView />);
      await screen.findByText("Signal evolution");
      for (const name of ["Cohesion", "Focus"]) {
        const w = widget(name);
        expect(seen(w)).toMatch(/no applicable construct in this codebase/i);
        expect(seen(w)).not.toMatch(NOT_RECORDED);
        expect(seen(w)).not.toMatch(NONE_FLAGGED);
        // The badge too: an n/a dimension must never read "none flagged" (a clean result).
        const title = part(w, "title")!;
        expect(within(title).getByText("n/a")).toBeInTheDocument();
        expect(seen(title)).not.toMatch(/none flagged|not recorded/i);
      }
      for (const name of ["Nesting", "Conciseness", "Uniqueness"]) {
        expect(seen(part(widget(name), "evidence"))).toMatch(recorded ? NONE_FLAGGED : NOT_RECORDED);
      }
    }
  });

  it("renders the real scanned payload's offenders in persisted order", async () => {
    const m = clone();
    m.scan.worst_offenders = realOffenders.recorded;
    stub(m);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    const nesting = widget("Nesting");
    const table = within(nesting).getByRole("table", { name: "Worst offenders" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    // Deepest first — persisted order, neither declaration nor name order.
    expect(rows.map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual([
      "beta_depth_six",
      "gamma_depth_five",
      "alpha_depth_four",
    ]);
    expect(within(rows[0]).getByText("src/lib.rs")).toBeInTheDocument();
    expect(within(rows[0]).getByText("nesting depth 6")).toBeInTheDocument();
    expect(within(part(nesting, "title")!).getByText("3 flagged")).toBeInTheDocument();
    // CR-209: Equality lists the one function above the mean complexity.
    const equality = widget("Equality");
    expect(within(part(equality, "title")!).getByText("1 flagged")).toBeInTheDocument();
    expect(within(equality).getByText("complexity 7")).toBeInTheDocument();
    // The other seven recorded dimensions flagged nothing.
    for (const name of OFFENDER_BACKED.filter((n) => n !== "Nesting" && n !== "Equality")) {
      expect(seen(part(widget(name), "evidence")), name).toMatch(NONE_FLAGGED);
    }
  });
});

describe("Signal trend widget", () => {
  it("renders the evolution trend table oldest-first with signed deltas", async () => {
    stub(HEALTH);
    render(<HealthView />);
    const trend = await screen.findByRole("table", { name: "Signal evolution" });
    expect(within(trend).getByText("012345678")).toBeInTheDocument(); // abbreviated sha
    expect(within(trend).getByText("+200")).toBeInTheDocument(); // signed delta
    expect(within(trend).getAllByText("—").length).toBeGreaterThanOrEqual(1); // delta-less point + commit-less snapshot
  });

  it("paginates the signal trend at 20 rows/page (shared page size, FR-UI-11)", async () => {
    const m = clone();
    m.evolution.snapshots = Array.from({ length: 30 }, (_, i) => ({
      snapshot_id: i + 1,
      created_at: (i + 1) * 100,
      commit_sha: null,
      signal: 7000 + i,
      signal_delta: i === 0 ? null : 1,
    }));
    stub(m);
    render(<HealthView />);
    const trend = await screen.findByRole("table", { name: "Signal evolution" });
    expect(within(trend).getAllByRole("row").length).toBe(20 + 1); // 20 body rows + header
    expect(screen.getByText(/Showing 1–20 of 30/)).toBeInTheDocument();
  });
});

// S-612 (FR-UI-41): the Non-gated tier callout is hidden through the register —
// a static pointer to a view the sidebar already lists. Files & Risk and
// `logos hotspots` still serve the per-file detail it pointed at.
describe("Non-gated tier (S-612, FR-UI-41)", () => {
  it("renders no Non-gated tier callout", async () => {
    stub(HEALTH);
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    expect(screen.queryByText("Non-gated tier")).toBeNull();
    // No widget links to Files & Risk: Equality lists its own functions (CR-209).
    expect(screen.queryAllByRole("link", { name: /Files & Risk/i })).toEqual([]);
  });

  it("renders the Non-gated tier callout again when its register entry is removed", async () => {
    const restore = removeHiddenWidgetEntry("non-gated-tier");
    try {
      stub(HEALTH);
      render(<HealthView />);
      expect(await screen.findByText("Non-gated tier")).toBeInTheDocument();
      // It points to Files & Risk (no second copy of that table).
      const links = screen.getAllByRole("link", { name: /Files & Risk/i });
      expect(links.some((l) => l.closest("[data-widget]") === null && l.getAttribute("href") === "/files")).toBe(true);
    } finally {
      restore();
    }
  });
});

// FR-EH-04 (CR-130): the Gate and Quality signal widgets are gated on scan-derived
// data, so on a populated graph their absence means "no scan has run" — never
// "empty graph", which their own condition does not establish.
describe("absent readings (FR-EH-04, CR-130)", () => {
  function absent(indexed: boolean, snapshots: HealthModel["evolution"]["snapshots"]): HealthModel {
    const m = clone();
    m.status.indexed = indexed;
    m.gate.signal = null;
    m.scan.metrics.empty = true;
    m.evolution.snapshots = snapshots;
    return m;
  }

  it("names `logos scan` when the graph is populated but no scan has run", async () => {
    stub(absent(true, []));
    render(<HealthView />);
    await screen.findAllByText(READING_ABSENCE.unscanned);
    // Gate and Quality signal state the one absence they can establish, in the figure row…
    for (const title of ["Gate", "Quality signal"]) {
      expect(seen(part(widget(title), "figure"))).toBe(READING_ABSENCE.unscanned);
      // Pinned by its claim, not by the catalogue that wrote it: no scan, never the index.
      expect(seen(part(widget(title), "figure"))).toMatch(/no scan has been run/);
      expect(seen(part(widget(title), "figure"))).not.toMatch(/nothing indexed/);
      // The absence names the command that changes it (CR-206).
      expect(seen(part(widget(title), "figure"))).toMatch(/; run logos scan\.$/);
    }
    expectWidgetCopy(widget("Gate"), gateCopy);
    expectWidgetCopy(widget("Quality signal"), qualitySignalCopy);
    expectWidgetCopy(widget("Signal trend"), signalTrendCopy);
    // …and all three name the command that produces the missing figure, the
    // empty trend in its own absence sentence.
    expect(widgetsNaming("logos scan")).toEqual(["Gate", "Quality signal", "Signal trend"]);
    expect(seen(part(widget("Signal trend"), "figure"))).toBe("No snapshots yet; run logos scan to record the first.");
    // No dimension widget without a figure, no step that cannot change the readout,
    // no blame on the graph, and no centred EmptyState inside a widget.
    expect(frames().map(widgetTitle)).toEqual(["Gate", "Quality signal", "Signal trend"]);
    expect(widgetsNaming("logos index")).toEqual([]);
    expect(screen.queryByText(/empty graph/i)).not.toBeInTheDocument();
    expect(document.querySelectorAll("[data-widget-absence]")).toHaveLength(3);
  });

  // Indexed AND scanned, but the production metric graph scored nothing (every
  // indexed symbol is test scope, FR-QM-08). `status.indexed` is a whole-graph fact
  // and cannot see this; `evolution.snapshots` is what establishes a scan ran.
  it("names no command when a scan has run but scored no production functions", async () => {
    stub(absent(true, [{ snapshot_id: 1, created_at: 100, commit_sha: null, signal: null, signal_delta: null }]));
    render(<HealthView />);
    expect((await screen.findAllByText(READING_ABSENCE["no-production-scope"])).length).toBe(2);
    expect(READING_ABSENCE["no-production-scope"]).toMatch(/no production functions to score/);
    expectWidgetCopy(widget("Gate"), gateCopy);
    expectWidgetCopy(widget("Quality signal"), qualitySignalCopy);
    // The false claim the old discriminant would have made, and the no-op remedy.
    expect(screen.queryByText(/no scan has been run/i)).not.toBeInTheDocument();
    expect(widgetsNaming("logos scan")).toEqual([]);
    expect(widgetsNaming("logos index")).toEqual([]);
  });

  it("keeps a distinct state naming `logos index` for a genuinely empty graph", async () => {
    stub(absent(false, []));
    render(<HealthView />);
    expect((await screen.findAllByText(READING_ABSENCE.unindexed)).length).toBe(2);
    for (const title of ["Gate", "Quality signal"]) {
      expect(seen(part(widget(title), "figure"))).toMatch(/nothing indexed/);
      expect(seen(part(widget(title), "figure"))).toMatch(/; run logos index, then logos scan\.$/);
    }
    expectWidgetCopy(widget("Gate"), gateCopy);
    expectWidgetCopy(widget("Quality signal"), qualitySignalCopy);
    expect(widgetsNaming("logos index")).toEqual(["Gate", "Quality signal"]);
    expect(screen.queryByText(/no scan has been run/i)).not.toBeInTheDocument();
  });
});

// ── CR-135 §3.2 / S-436: a populated signal the graph no longer matches ──────
// The figures survive a de-index and a re-index, so without this they render as a
// confident CURRENT verdict. They are labelled, not suppressed: genuine history.
describe("not-current readings (CR-135, S-436)", () => {
  it("labels both widgets as describing a graph no longer indexed, dated, naming `logos index`", async () => {
    const m = clone();
    m.status.indexed = false; // de-indexed AFTER the scan that recorded the snapshot
    m.evolution.snapshots[1].created_at = 1_758_240_000; // 2025-09-19 UTC
    stub(m);
    render(<HealthView />);
    // Both widgets carry the same single sentence — one classification, two widgets.
    expect((await screen.findAllByText(/no longer indexed/i)).length).toBe(2);
    expect(screen.getAllByText(/Describes the snapshot of 2025-09-19/i).length).toBe(2);
    // …each naming the command that does change what is reported (FR-EH-04), in
    // that sentence (CR-206); the dimension widgets point to the Gate for why.
    expect(widgetsNaming("logos index")).toEqual(["Gate", "Quality signal"]);
    expect(screen.getAllByText(/; run logos index before reading them as current\.$/).length).toBe(2);
    expectWidgetCopy(widget("Gate"), gateCopy);
    expectWidgetCopy(widget("Quality signal"), qualitySignalCopy);
    // The figures are still there — labelled, never discarded — under a red STALE chip.
    const chip = within(part(widget("Gate"), "title")!).getByText("STALE");
    expect(chip.className).toBe(chipClass("red"));
    expect(chip.className).not.toBe(chipClass("orange"));
    expect(seen(part(widget("Gate"), "figure"))).toMatch(/^PASS · signal 8000 vs baseline 7800/);
    expect(screen.getByRole("table", { name: "Quality metrics" })).toBeInTheDocument();
    // …but nothing asserts they are current: no green PASS chip.
    expect(within(part(widget("Gate"), "title")!).queryByText("PASS")).toBeNull();
  });

  // CR-135: one classification for the whole page. A dimension widget scored from
  // the same not-current snapshot says so and points to the Gate, whose sentence
  // names the command — it states no remedy of its own.
  it("carries the not-current reading into every dimension widget, pointing to the Gate", async () => {
    for (const [indexed, command] of [[false, "logos index"], [true, "logos scan"]] as const) {
      cleanup();
      const m = clone();
      m.scan.worst_offenders = realOffenders.recorded; // Nesting would otherwise act on 3 listed
      m.status.indexed = indexed;
      m.evolution.snapshots[1].created_at = 1_758_240_000;
      m.status.last_full_index_at = String(1_758_240_000 + 86_400); // moved past when still indexed
      stub(m);
      render(<HealthView />);
      await screen.findByText("Signal evolution");
      for (const row of metricRows(m.scan.metrics)) {
        const w = widget(row.name);
        expect(seen(part(w, "figure")), row.name).toContain("not established as a current reading — see the Gate for why");
        expectWidgetCopy(w, DIMENSION_COPY[row.key]);
      }
      expect(widgetsNaming(command)).toEqual(["Gate", "Quality signal"]);
    }
  });

  // A stale verdict keeps the figures it recorded, and a FAIL recorded before the
  // de-index is still a FAIL — but it is not acted on as one.
  it("carries the recorded FAIL verdict, not a PASS, when the stale snapshot failed", async () => {
    const m = clone();
    m.status.indexed = false;
    m.gate.passed = false;
    m.gate.signal = 7000;
    stub(m);
    render(<HealthView />);
    await screen.findAllByText(/no longer indexed/i);
    expect(seen(part(widget("Gate"), "figure"))).toMatch(/^FAIL · signal 7000 vs baseline 7800/);
    expect(seen(part(widget("Gate"), "figure"))).toMatch(/run logos index before reading them as current/);
    expect(seen(part(widget("Gate"), "figure"))).not.toMatch(/Lowest-scoring dimension/);
  });

  // CRA-02: only a CURRENT FAIL names its lowest-scoring dimension. Each stale
  // arm, not only the de-index above, keeps the FAIL figure and drops the fact.
  it.each([
    ["moved past", (m: HealthModel) => {
      m.evolution.snapshots[1].created_at = 1_758_240_000;
      m.status.last_full_index_at = String(1_758_240_000 + 86_400);
    }, /the graph has been indexed or synced since/],
    ["indeterminate", (m: HealthModel) => {
      m.status.last_full_index_at = null;
      m.status.last_sync_at = null;
    }, /no index or sync time is recorded/],
  ] as const)("names no lowest-scoring dimension on a FAIL whose snapshot is %s (HF-1 review)", async (_name, stale, note) => {
    const m = clone();
    m.gate.passed = false;
    m.gate.signal = 7700;
    m.scan.metrics.depth = mv(0.2);
    stale(m);
    stub(m);
    render(<HealthView />);
    await screen.findAllByText(note);
    const figure = seen(part(widget("Gate"), "figure"));
    expect(figure).toMatch(/^FAIL · signal 7700 vs baseline 7800/);
    expect(figure).not.toMatch(/Lowest-scoring dimension/);
  });

  it("renders the undated fallback rather than a fabricated date when nothing dates the snapshot", async () => {
    const m = clone();
    m.status.indexed = false;
    m.evolution.snapshots = []; // populated signal, but no point to date it by
    stub(m);
    render(<HealthView />);
    expect((await screen.findAllByText(/Describes the last recorded snapshot/i)).length).toBe(2);
    expect(screen.getAllByText(/no longer indexed/i).length).toBe(2);
    expect(screen.queryByText(/Describes the snapshot of/i)).not.toBeInTheDocument();
  });

  it("renders both widgets exactly as current while the graph is still indexed", async () => {
    stub(HEALTH); // `status.indexed` is true
    render(<HealthView />);
    await screen.findByText("Signal evolution");
    expect(screen.queryByText("STALE")).not.toBeInTheDocument();
    expect(screen.queryByText(/no longer indexed/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/Describes the snapshot of/i)).not.toBeInTheDocument();
    expect(widgetsNaming("logos index")).toEqual([]);
  });

  // S-436: `status.indexed` is TRUE here, so before S-436 this rendered a green PASS
  // over figures describing a graph the store no longer holds.
  it("labels both widgets as a snapshot the graph has moved past, dated, naming `logos scan`", async () => {
    const m = clone();
    m.status.indexed = true; // still indexed — this is NOT the de-index branch
    m.evolution.snapshots[1].created_at = 1_758_240_000; // 2025-09-19 UTC
    m.status.last_full_index_at = String(1_758_240_000 + 86_400); // indexed a day later
    stub(m);
    render(<HealthView />);
    expect((await screen.findAllByText(/the graph has been indexed or synced since/i)).length).toBe(2);
    expect(screen.getAllByText(/Describes the snapshot of 2025-09-19/i).length).toBe(2);
    // …naming the step that changes what IS reported: a new snapshot, not a new index
    // (Gate, Quality signal, and the ten dimension widgets scored from it).
    expect(widgetsNaming("logos scan")).toEqual(["Gate", "Quality signal"]);
    expect(screen.getAllByText(/Run logos scan to record a snapshot of the graph as it stands now\.$/).length).toBe(2);
    expect(widgetsNaming("logos index")).toEqual([]);
    expectWidgetCopy(widget("Gate"), gateCopy);
    expectWidgetCopy(widget("Quality signal"), qualitySignalCopy);
    expect(within(part(widget("Gate"), "title")!).getByText("MOVED PAST").className).toBe(chipClass("red"));
    expect(seen(part(widget("Gate"), "figure"))).toMatch(/^PASS · signal 8000 vs baseline 7800/);
    // It claims what the comparison establishes and no more, and says the over-report out loud.
    expect(screen.queryByText("STALE")).not.toBeInTheDocument();
    expect(screen.getAllByText(/an index that changed nothing would read the same way/i).length).toBe(2);
  });

  it("still reports de-indexed, not moved past, when both conditions hold", async () => {
    const m = clone();
    m.status.indexed = false;
    m.status.last_full_index_at = String(1_000_000_000_000); // long after the snapshot
    stub(m);
    render(<HealthView />);
    expect((await screen.findAllByText(/no longer indexed/i)).length).toBe(2);
    expect(screen.getByText("STALE")).toBeInTheDocument();
    expect(screen.queryByText("MOVED PAST")).not.toBeInTheDocument();
    expect(screen.queryByText(/has been indexed or synced since/i)).not.toBeInTheDocument();
  });

  // `last_sync_at` is a FILE MTIME, so a stamp ahead of now is routine on a copied
  // tree or a restored backup — the real shape, not a contrived one.
  it("renders neither a current verdict nor a date when a future-dated last_sync_at makes the comparison indeterminate", async () => {
    const m = clone();
    m.status.indexed = true;
    m.status.last_sync_at = String(Math.floor(Date.now() / 1000) + 86_400);
    stub(m);
    render(<HealthView />);
    // S-433's wording for this condition, reused rather than restated.
    expect(
      (await screen.findAllByText(/at an unknown age \(recorded ahead of now — check the clock\)/i)).length,
    ).toBe(2);
    const chip = within(part(widget("Gate"), "title")!).getByText("UNVERIFIED");
    // Nothing is established, so a red chip would overclaim in the other direction.
    expect(chip.className).toBe(chipClass("muted"));
    expect(chip.className).not.toBe(chipClass("red"));
    expect(screen.queryByText(/Describes the snapshot of/i)).not.toBeInTheDocument();
    expect(screen.queryByText("STALE")).not.toBeInTheDocument();
    // No command: none of the three missing facts is fixed by running one.
    expect(widgetsNaming("logos scan")).toEqual([]);
    expect(widgetsNaming("logos index")).toEqual([]);
    // The figures are still there.
    expect(seen(part(widget("Gate"), "figure"))).toMatch(/^PASS · signal 8000 vs baseline 7800/);
  });

  it("renders the no-index-time indeterminate sentence", async () => {
    const m = clone();
    m.status.last_full_index_at = null;
    m.status.last_sync_at = null;
    stub(m);
    render(<HealthView />);
    expect((await screen.findAllByText(/no index or sync time is recorded/i)).length).toBe(2);
    expect(screen.getByText("UNVERIFIED")).toBeInTheDocument();
    // The third stale variant passes the message standard on both widgets too.
    expectWidgetCopy(widget("Gate"), gateCopy);
    expectWidgetCopy(widget("Quality signal"), qualitySignalCopy);
  });

  // The third indeterminate sentence, pinned where it is RENDERED, so its wording
  // cannot change — or its cause be special-cased away — with the suite green.
  it("renders the undated-snapshot indeterminate sentence, the third arm, in the DOM", async () => {
    const m = clone();
    m.status.indexed = true;
    m.status.last_full_index_at = "150"; // usable, and after the snapshots it has none of
    m.evolution.snapshots = []; // a populated signal with no point to date it by
    stub(m);
    render(<HealthView />);
    const notes = await screen.findAllByText(/the last snapshot carries no date to compare against/i);
    expect(notes.length).toBe(2);
    expect(screen.getByText("UNVERIFIED")).toBeInTheDocument();
    expect(screen.queryByText(/Describes the snapshot of/i)).not.toBeInTheDocument();
    // "Names no command" holds of the two widgets; the empty trend legitimately
    // names `logos scan` for its own, different absence.
    expect(widgetsNaming("logos scan")).toEqual(["Signal trend"]);
    expect(widgetsNaming("logos index")).toEqual([]);
    expect(screen.queryByText(/no index or sync time is recorded/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/recorded ahead of now/i)).not.toBeInTheDocument();
  });
});
