import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { HealthModel, MetricSnapshot, MetricValue } from "../../api/types.ts";
import { Badge, type BadgeTone, Callout, type CalloutTone } from "../../components/index.ts";
import { HealthView } from "./HealthView.tsx";

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
  gate: { passed: true, saved: false, signal: 8000, baseline_signal: 7800, test_function_count: 12, threshold: null, epsilon: 0, freshness: "", message: "", warnings: [] },
  scan: {
    signal: 8000,
    freshness: "",
    metrics: metrics(),
    worst_offenders: {
      nesting: [{ name: "deep_fn", file: "src/a.rs", line: 42, detail: "nesting depth 6" }],
      conciseness: [],
      cohesion: [],
      focus: [],
      uniqueness: [],
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

// The tone matchers both tone pins share. Hash-agnostic: render a reference
// element of the intended tone and compare class names, never a literal
// CSS-module hash (the `ScoreBar` tone test's shape). One matcher rather than a
// hand-mirrored twin per spec, so the stale pin and the ordinary pin cannot
// drift apart in what they mean by "this tone".
function chipClass(tone: BadgeTone) {
  const { container } = render(<Badge tone={tone}>REF</Badge>);
  return within(container).getByText("REF").className;
}

function bandClass(tone: CalloutTone) {
  const { container } = render(
    <Callout label="Gate" tone={tone}>
      <span>ref</span>
    </Callout>,
  );
  return container.querySelector("section")?.className ?? "";
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
beforeEach(() => vi.clearAllMocks());

describe("HealthView migration (S-187, FR-UI-04 / FR-UI-21)", () => {
  it("leads with the gate verdict band (PASS + current vs baseline)", async () => {
    stub(HEALTH);
    render(<HealthView />);
    expect(await screen.findByText("PASS")).toBeInTheDocument();
    expect(screen.getByText(/current 8000 vs baseline 7800/i)).toBeInTheDocument();
  });

  it("names a missing baseline honestly in the gate band", async () => {
    const m = clone();
    m.gate.baseline_signal = null;
    stub(m);
    render(<HealthView />);
    expect(await screen.findByText(/current 8000 vs baseline no baseline/i)).toBeInTheDocument();
  });

  it("renders the quality grid, aggregate, and the folded structural drill-downs", async () => {
    stub(HEALTH);
    render(<HealthView />);
    // The aggregate signal.
    expect(await screen.findByText("/ 10000")).toBeInTheDocument();
    // The accessible metric grid.
    const grid = screen.getByRole("table", { name: "Quality metrics" });
    expect(within(grid).getByText("Modularity")).toBeInTheDocument();
    expect(within(grid).getByText("Uniqueness")).toBeInTheDocument();
    // The Nesting drill-down is open with its worst offender.
    expect(screen.getByText(/1 flagged/i)).toBeInTheDocument();
    const offenders = screen.getByRole("table", { name: "Worst offenders" });
    expect(within(offenders).getByText("deep_fn")).toBeInTheDocument();
    expect(within(offenders).getByText("src/a.rs")).toBeInTheDocument();
    // The applicable-but-unflagged dimensions show the honest "none flagged" middle state.
    expect(screen.getAllByText(/none flagged/i).length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText(/No offenders flagged within thresholds/i).length).toBeGreaterThanOrEqual(1);
    // The aggregate-scope provenance (FR-QM-14) traces to the read-model.
    expect(screen.getByText(/90 production functions/i)).toBeInTheDocument();
    expect(screen.getByText(/thresholds abc123/i)).toBeInTheDocument();
    // Every structural drill-down is rendered open (no-JS readable).
    const details = document.querySelectorAll("details");
    expect(details.length).toBe(5);
    expect([...details].every((d) => d.hasAttribute("open"))).toBe(true);
    // The non-gated tier points to Files & Risk (no second copy of that table).
    expect(screen.getByRole("link", { name: /Files & Risk/i })).toHaveAttribute("href", "/files");
  });

  it("renders an ADR-21 metric drop-out as a muted n/a, never a fabricated zero", async () => {
    const m = clone();
    m.scan.metrics.cohesion = null;
    m.scan.metrics.focus = null;
    stub(m);
    render(<HealthView />);
    const grid = await screen.findByRole("table", { name: "Quality metrics" });
    // The Cohesion + Focus rows render n/a badges (no 0.00 figure fabricated).
    expect(within(grid).getAllByText("n/a").length).toBeGreaterThanOrEqual(2);
    // The Cohesion drill-down explains the drop-out, with no offenders table.
    expect(screen.getAllByText(/no applicable construct in this codebase/i).length).toBeGreaterThanOrEqual(2);
  });

  it("renders a CR-156 Modularity drop-out as not applicable with its reason and m-of-5 count", async () => {
    const m = clone();
    m.scan.metrics.modularity = mv(0);
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
    // Every other row still renders its score.
    const acyclicity = within(grid).getByText("Acyclicity").closest("tr") as HTMLElement;
    expect(within(acyclicity).queryByText("not applicable")).toBeNull();
  });

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

  // FR-EH-04 (CR-130): the gate and quality cards are gated on scan-derived data,
  // so on a populated graph their absence means "no scan has run" — never "empty
  // graph", which their own condition does not establish.
  it("names `logos scan` when the graph is populated but no scan has run (FR-EH-04)", async () => {
    const m = clone();
    m.status.indexed = true;
    m.gate.signal = null;
    m.scan.metrics.empty = true;
    m.evolution.snapshots = [];
    stub(m);
    render(<HealthView />);
    // Gate band + Quality signal card both state the absence they can establish…
    expect(await screen.findByText(/n\/a — no scan has been run/i)).toBeInTheDocument();
    expect(screen.getByText(/No scan has been run yet/i)).toBeInTheDocument();
    expect(screen.getByText(/No snapshots yet/i)).toBeInTheDocument();
    // …and all three name the command that produces the missing figure.
    expect(screen.getAllByText("logos scan").length).toBe(3);
    // Neither names a step that cannot change what it reports, nor blames the graph.
    expect(screen.queryByText("logos index")).not.toBeInTheDocument();
    expect(screen.queryByText(/empty graph/i)).not.toBeInTheDocument();
  });

  // Indexed AND scanned, but the production metric graph scored nothing (every
  // indexed symbol is test scope, FR-QM-08). `status.indexed` is a whole-graph fact
  // and cannot see this; `evolution.snapshots` is what establishes a scan ran.
  it("names no command when a scan has run but scored no production functions (FR-EH-04)", async () => {
    const m = clone();
    m.status.indexed = true;
    m.gate.signal = null;
    m.scan.metrics.empty = true;
    // A snapshot EXISTS — a scan did run — but it carries no aggregate signal.
    m.evolution.snapshots = [
      { snapshot_id: 1, created_at: 100, commit_sha: null, signal: null, signal_delta: null },
    ];
    stub(m);
    render(<HealthView />);
    // Both the gate band and the Quality signal card say it — one classification.
    expect((await screen.findAllByText(/no production functions to score/i)).length).toBe(2);
    // The false claim the old discriminant would have made, and the no-op remedy.
    expect(screen.queryByText(/no scan has been run/i)).not.toBeInTheDocument();
    expect(screen.queryByText("logos scan")).not.toBeInTheDocument();
    expect(screen.queryByText("logos index")).not.toBeInTheDocument();
  });

  // ── CR-135 §3.2: a populated signal over a de-indexed graph ────────────────
  // The figures survive a de-index, so without this they render as a confident
  // CURRENT verdict for a graph that no longer exists (reproduced during S-406's
  // review as finding #9). They are labelled, not suppressed: genuine history.
  it("labels both cards as describing a graph no longer indexed, dated, naming `logos index` (CR-135)", async () => {
    const m = clone();
    m.status.indexed = false; // de-indexed AFTER the scan that recorded the snapshot
    m.evolution.snapshots[1].created_at = 1_758_240_000; // 2025-09-19 UTC
    stub(m);
    render(<HealthView />);
    // Both cards carry the same single sentence — one classification, two cards.
    expect((await screen.findAllByText(/no longer indexed/i)).length).toBe(2);
    expect(screen.getAllByText(/Describes the snapshot of 2025-09-19/i).length).toBe(2);
    // …each naming the command that does change what is reported (FR-EH-04).
    expect(screen.getAllByText("logos index").length).toBe(2);
    // The figures are still there — labelled, never discarded.
    expect(screen.getByText("STALE")).toBeInTheDocument();
    expect(screen.getByText(/PASS · signal 8000 vs baseline 7800/)).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Quality metrics" })).toBeInTheDocument();
    // …but the unqualified current-verdict wording is gone.
    expect(screen.queryByText(/current 8000 vs baseline/i)).not.toBeInTheDocument();
  });

  // A stale verdict keeps the figures it recorded, and a FAIL recorded before the
  // de-index is still a FAIL. Nothing pinned this: hardcoding "PASS" in the stale
  // band left the whole suite green.
  it("carries the recorded FAIL verdict, not a PASS, when the stale snapshot failed (CR-135)", async () => {
    const m = clone();
    m.status.indexed = false;
    m.gate.passed = false;
    m.gate.signal = 7000;
    stub(m);
    render(<HealthView />);
    expect(await screen.findByText(/FAIL · signal 7000 vs baseline 7800/)).toBeInTheDocument();
    expect(screen.queryByText(/PASS · signal/)).not.toBeInTheDocument();
  });

  // The undated fallback was pinned at the model level but never in rendered DOM,
  // so its wording could be changed freely with the suite green. It is reachable
  // whenever the series carries no point to date the label by.
  it("renders the undated fallback rather than a fabricated date when nothing dates the snapshot", async () => {
    const m = clone();
    m.status.indexed = false;
    m.evolution.snapshots = []; // populated signal, but no point to date it by
    stub(m);
    render(<HealthView />);
    expect((await screen.findAllByText(/Describes the last recorded snapshot/i)).length).toBe(2);
    // Still labelled, still dateless, still naming the step — never a made-up date.
    expect(screen.getAllByText(/no longer indexed/i).length).toBe(2);
    expect(screen.getAllByText("logos index").length).toBe(2);
    expect(screen.queryByText(/Describes the snapshot of/i)).not.toBeInTheDocument();
  });

  // The tone is the contract this view got WRONG on the way in, so it is pinned
  // rather than left to the eye: `Callout` documents "signal — red (GATE/FAIL,
  // STALE, …)", `Badge` documents "red — fail / error / stale", and the SPA's
  // three other STALE chips are all red. Hash-agnostic, like ScoreBar's tone
  // test: render a reference chip of the intended tone and compare class names,
  // never a literal CSS-module hash.
  it("renders the STALE band and chip in the signal/red tone, not the in-flight orange (CR-135)", async () => {
    const m = clone();
    m.status.indexed = false;
    stub(m);
    render(<HealthView />);
    const chip = await screen.findByText("STALE");

    expect(chip.className).toBe(chipClass("red"));
    expect(chip.className).not.toBe(chipClass("orange"));
    expect(chip.closest("section")?.className).toBe(bandClass("signal"));
    expect(chip.closest("section")?.className).not.toBe(bandClass("warm"));
  });

  // The other half of the same contract: the staleness branch must not leak into
  // the ordinary case. Same populated payload, `indexed` true.
  it("renders both cards exactly as today while the graph is still indexed (CR-135)", async () => {
    stub(HEALTH); // `status.indexed` is true
    render(<HealthView />);
    const chip = await screen.findByText("PASS");
    expect(chip).toBeInTheDocument();
    expect(screen.getByText(/current 8000 vs baseline 7800/i)).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Quality metrics" })).toBeInTheDocument();
    // "Exactly as today" includes the TONE, not only the words. A passing gate
    // is the green chip in the pass band; hardcoding either to the stale
    // branch's signal/red left every assertion above green, so the tone the
    // ordinary case carries is pinned with the same matcher the stale pin uses.
    expect(chip.className).toBe(chipClass("green"));
    expect(chip.className).not.toBe(chipClass("red"));
    expect(chip.closest("section")?.className).toBe(bandClass("pass"));
    expect(chip.closest("section")?.className).not.toBe(bandClass("signal"));
    // None of the staleness wording reaches an indexed project.
    expect(screen.queryByText("STALE")).not.toBeInTheDocument();
    expect(screen.queryByText(/no longer indexed/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/Describes the snapshot of/i)).not.toBeInTheDocument();
    expect(screen.queryByText("logos index")).not.toBeInTheDocument();
  });

  // ── S-436: a snapshot the graph has MOVED PAST ────────────────────────────
  // The question CR-135 §3.2 left open. `status.indexed` is TRUE here, so before
  // S-436 this rendered a green PASS and the literal words "current 8000 vs
  // baseline 7800" over figures describing a graph the store no longer holds.
  it("labels both cards as a snapshot the graph has moved past, dated, naming `logos scan` (S-436)", async () => {
    const m = clone();
    m.status.indexed = true; // still indexed — this is NOT the de-index branch
    m.evolution.snapshots[1].created_at = 1_758_240_000; // 2025-09-19 UTC
    m.status.last_full_index_at = String(1_758_240_000 + 86_400); // indexed a day later
    stub(m);
    render(<HealthView />);
    // One classification, two cards — the same discipline the de-index arm has.
    expect((await screen.findAllByText(/the graph has been indexed or synced since/i)).length).toBe(2);
    expect(screen.getAllByText(/Describes the snapshot of 2025-09-19/i).length).toBe(2);
    // …naming the step that changes what IS reported: a new snapshot, not a new index.
    expect(screen.getAllByText("logos scan").length).toBe(2);
    expect(screen.queryByText("logos index")).not.toBeInTheDocument();
    // The figures stay — labelled, never discarded.
    expect(screen.getByText("MOVED PAST")).toBeInTheDocument();
    expect(screen.getByText(/PASS · signal 8000 vs baseline 7800/)).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Quality metrics" })).toBeInTheDocument();
    // …but the unqualified current-verdict wording is gone. This is the exact
    // string the defect rendered.
    expect(screen.queryByText(/current 8000 vs baseline/i)).not.toBeInTheDocument();
    // It claims what the comparison establishes and no more: the graph moved on.
    // It does not claim the figures are stale, and it says the over-report out loud.
    expect(screen.queryByText("STALE")).not.toBeInTheDocument();
    expect(screen.getAllByText(/an index that changed nothing would read the same way/i).length).toBe(2);
  });

  // The de-index branch is UNCHANGED and the two conditions COMPOSE: a graph
  // that is both de-indexed and indexed-since still reports de-indexed.
  it("still reports de-indexed, not moved past, when both conditions hold (S-436 / CR-135 §3.2)", async () => {
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

  // ── S-436: the indeterminate arm ──────────────────────────────────────────
  // `last_sync_at` is a FILE MTIME, so a stamp ahead of now is routine on a
  // copied tree or a restored backup — the real shape, not a contrived one.
  it("renders neither `current` nor a date when a future-dated last_sync_at makes the comparison indeterminate (S-436)", async () => {
    const m = clone();
    m.status.indexed = true;
    m.status.last_sync_at = String(Math.floor(Date.now() / 1000) + 86_400);
    stub(m);
    render(<HealthView />);
    // S-433's wording for this condition, reused rather than restated.
    expect(
      (await screen.findAllByText(/at an unknown age \(recorded ahead of now — check the clock\)/i)).length,
    ).toBe(2);
    expect(screen.getByText("UNVERIFIED")).toBeInTheDocument();
    // Neither `current`…
    expect(screen.queryByText(/current 8000 vs baseline/i)).not.toBeInTheDocument();
    // …nor a stale date, of either arm's wording.
    expect(screen.queryByText(/Describes the snapshot of/i)).not.toBeInTheDocument();
    expect(screen.queryByText("STALE")).not.toBeInTheDocument();
    expect(screen.queryByText("MOVED PAST")).not.toBeInTheDocument();
    // No command: none of the three missing facts is fixed by running one.
    expect(screen.queryByText("logos scan")).not.toBeInTheDocument();
    expect(screen.queryByText("logos index")).not.toBeInTheDocument();
    // The figures are still there.
    expect(screen.getByText(/PASS · signal 8000 vs baseline 7800/)).toBeInTheDocument();
  });

  it("renders the indeterminate band muted, not the signal/red the two established causes carry (S-436)", async () => {
    const m = clone();
    m.status.last_full_index_at = null;
    m.status.last_sync_at = null;
    stub(m);
    render(<HealthView />);
    const chip = await screen.findByText("UNVERIFIED");
    // Nothing is established, so a red chip would overclaim in the other
    // direction — muted is the tone the absence branch already uses. Same
    // hash-agnostic matcher the two established causes are pinned with.
    expect(chip.className).toBe(chipClass("muted"));
    expect(chip.className).not.toBe(chipClass("red"));
    expect(chip.closest("section")?.className).toBe(bandClass("muted"));
    expect(chip.closest("section")?.className).not.toBe(bandClass("pass"));
    expect(screen.getAllByText(/no index or sync time is recorded/i).length).toBe(2);
  });

  // The third indeterminate sentence. The other two are pinned in rendered DOM
  // above; without this one, `INDETERMINATE.undatedSnapshot`'s wording could be
  // changed — or its cause special-cased away in `StaleNote` — with the suite
  // green. That gap has already bitten this file once, for the undated de-index
  // fallback, which is why every arm is now pinned where it is RENDERED.
  it("renders the undated-snapshot indeterminate sentence, the third arm, in the DOM (S-436)", async () => {
    const m = clone();
    m.status.indexed = true;
    m.status.last_full_index_at = "150"; // usable, and after the snapshots it has none of
    m.evolution.snapshots = []; // a populated signal with no point to date it by
    stub(m);
    render(<HealthView />);
    const notes = await screen.findAllByText(/the last snapshot carries no date to compare against/i);
    expect(notes.length).toBe(2);
    expect(screen.getByText("UNVERIFIED")).toBeInTheDocument();
    // Neither `current` nor a date, and no command — the arm's whole contract.
    expect(screen.queryByText(/current 8000 vs baseline/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/Describes the snapshot of/i)).not.toBeInTheDocument();
    // "Names no command" is asserted of the NOTE, not of the page: with no
    // snapshots the evolution card legitimately names `logos scan` for its own,
    // different absence, and a page-wide matcher would read that as this arm's.
    for (const note of notes) expect(note.querySelector("code")).toBeNull();
    // …and the other two indeterminate sentences are NOT the one rendered.
    expect(screen.queryByText(/no index or sync time is recorded/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/recorded ahead of now/i)).not.toBeInTheDocument();
  });

  it("keeps a distinct state naming `logos index` for a genuinely empty graph (FR-EH-04)", async () => {
    const m = clone();
    m.status.indexed = false;
    m.gate.signal = null;
    m.scan.metrics.empty = true;
    m.evolution.snapshots = [];
    stub(m);
    render(<HealthView />);
    // Both the gate band and the Quality signal card say it, each naming `logos index`.
    expect(await screen.findByText(/n\/a — nothing indexed yet/i)).toBeInTheDocument();
    expect(screen.getAllByText(/nothing indexed yet/i).length).toBe(2);
    expect(screen.getAllByText("logos index").length).toBe(2);
    expect(screen.queryByText(/no scan has been run/i)).not.toBeInTheDocument();
  });
});
