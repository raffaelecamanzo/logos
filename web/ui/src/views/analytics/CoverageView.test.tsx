import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { CoverageJob, CoverageModel, CoverageToolStatus } from "../../api/types.ts";
import {
  COVERAGE_CONSENT_TEXT,
  COVERAGE_OFFER_TEXT,
  COVERAGE_PREPARE_CONSENT_TEXT,
  COVERAGE_RUN_TEXT,
  COVERAGE_VIEW_ABSENCE,
  coverageTools,
  perFileCoverage,
  untestedHotspots,
} from "../../copy/coverageView.copy.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import { installEventSources } from "../../test/eventSource.ts";
import { expectOneWidgetStack, widgetTitle } from "../../test/widgetStack.ts";
import { CoverageView } from "./CoverageView.tsx";
import {
  CANCELLED_END,
  COMPLETED_END,
  CONSENT_FIRST,
  DEPENDENCY_FIRST,
  EVENTS,
  GO_CANCELLED,
  PREPARABLE_TOOLS,
  RUNNING,
  TOOLS,
  TOOLS_AFTER,
} from "./fixtures/coverageRun.ts";

const STATUS = { indexed: true, file_count: 9, node_count: 99, edge_count: 80, db_path: ".logos/graph.db", db_size_bytes: 12288, last_full_index_at: "1719600000", last_sync_at: null, graph_revision: 7, refs_total: 120, refs_resolved: 118, refs_unresolved: 2, resolution_coverage: 0.983, total_line_count: null, source_line_count: null, test_line_count: null, freshness: "fresh", warnings: [] };

const UNTESTED_REPORT = {
  tier: "temporal (non-gated, advisory)",
  defect_label: "heuristic",
  head_sha: "head",
  config_hash: "cfg",
  limit: 20,
  ranked_files: 1,
  files: [
    { path: "src/untested.rs", score: 15, churn_rank: 1, churn_commits: 4, complexity_rank: 1, complexity: 9, co_change_count: 0, defect_commits: 0, coverage: { state: "n/a", coverage_bp: null } },
  ],
  degraded: null,
  notice: null,
  untested: true,
  production_scope: false,
  coverage_basis: "coverage",
  coverage_label: null,
};

function populated(): CoverageModel {
  return {
    status: STATUS,
    coverage: {
      head_sha: "abc123",
      config_hash: "cfg",
      formats: ["lcov"],
      report_count: 1,
      total_files: 3,
      fresh_files: 2,
      stale_files: 1,
      freshness_bp: 6667,
      overall_coverage_bp: 7300,
      files: [
        { path: "src/fresh.rs", freshness: "fresh", coverage_bp: 8200, instrumented_lines: 100, covered_lines: 82 },
        { path: "src/stale.rs", freshness: "stale", coverage_bp: null, instrumented_lines: 0, covered_lines: 0 },
      ],
      notice: null,
      current_head: "abc123",
      head_stale: false,
      staleness_prompt: null,
    },
    untested: UNTESTED_REPORT as CoverageModel["untested"],
  };
}

function empty(): CoverageModel {
  const m = populated();
  m.coverage.notice = "no coverage ingested";
  return m;
}

function stubFetch(byUrl: () => unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve({ ok: true, json: () => Promise.resolve(byUrl()) } as Response)),
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("CoverageView (S-188, FR-UI-11)", () => {
  it("renders the coverage status verdict and the untested-hotspots table", async () => {
    stubFetch(populated);
    render(<CoverageView />);
    await screen.findByText("src/untested.rs");
    expect(screen.getByRole("status")).toHaveTextContent("2/3 files fresh");
  });

  it("renders a fresh file as a <meter> + percent and a stale file as a STALE label (never a shifted number)", async () => {
    stubFetch(populated);
    render(<CoverageView />);
    await screen.findByText("src/fresh.rs");
    // the fresh row carries a native meter (accessible bar) anchored to the bp value
    const meter = screen.getByRole("meter");
    expect(meter).toHaveAttribute("value", "8200");
    // the percent shows (in the meter fallback + the figure span)
    expect(screen.getAllByText("82.0%").length).toBeGreaterThanOrEqual(1);
    // the stale row is a label, not a number
    expect(screen.getByText("STALE")).toBeInTheDocument();
  });

  it("shows the honest ingest empty state when no coverage exists", async () => {
    stubFetch(empty);
    render(<CoverageView />);
    expect((await screen.findAllByText(/No coverage ingested/)).length).toBe(2);
    // Both widgets name the ingest command in their absence sentence (CR-206).
    const absences = [...document.querySelectorAll("[data-widget-absence]")].map((a) => a.textContent);
    expect(absences).toEqual([COVERAGE_VIEW_ABSENCE.notIngested, COVERAGE_VIEW_ABSENCE.notIngested]);
    expect(COVERAGE_VIEW_ABSENCE.notIngested).toMatch(/; run logos coverage ingest <report> /);
  });
});

// ── S-617 (CR-203, FR-UI-39/40): each widget explains itself in each state ───

function allFresh(): CoverageModel {
  const m = populated();
  m.coverage.stale_files = 0;
  m.coverage.files = m.coverage.files.filter((f) => f.freshness === "fresh");
  m.untested = { ...m.untested, files: [] };
  return m;
}

function notRanked(): CoverageModel {
  const m = allFresh();
  m.untested = { ...m.untested, ranked_files: 0, files: [], degraded: "NotGit", notice: "not a git repository: churn unavailable" } as CoverageModel["untested"];
  return m;
}

const STATES = [
  {
    name: "populated (an untested file, a stale file)",
    model: populated,
    expected: [
      "Untested hotspots",
      "Per-file coverage",
    ],
  },
  {
    name: "all fresh, nothing untested",
    model: allFresh,
    expected: [
      "Untested hotspots",
      "Per-file coverage",
    ],
  },
  {
    name: "no coverage ingested",
    model: empty,
    expected: [
      "Untested hotspots",
      "Per-file coverage",
    ],
  },
  {
    // Review fix (S-617): no history to rank → nothing measured, never "nothing untested".
    name: "nothing ranked (no git history)",
    model: notRanked,
    expected: [
      "Untested hotspots",
      "Per-file coverage",
    ],
  },
] as const;

describe("Coverage widgets explain themselves (S-617, FR-UI-39/40)", () => {
  it.each(STATES)("$name: one stack, each widget its catalogue entry", async ({ model, expected }) => {
    stubFetch(model);
    const { container } = render(<CoverageView />);
    await screen.findByRole("heading", { name: "Per-file coverage" });
    const widgets = expectOneWidgetStack(container);
    expect(widgets.map(widgetTitle)).toEqual([...expected]);
    expectWidgetCopy(widgets[0], untestedHotspots);
    expectWidgetCopy(widgets[1], perFileCoverage);
  });

  it("states an un-ingested report in each widget's figure row, never as a centred empty state", async () => {
    stubFetch(empty);
    const { container } = render(<CoverageView />);
    await screen.findByRole("heading", { name: "Per-file coverage" });
    expect(container.querySelectorAll("[data-widget-absence]")).toHaveLength(2);
    expect(container.querySelector('[class*="empty"]')).toBeNull();
  });

  it("states the stale files with their denominator in the figure row", async () => {
    stubFetch(populated);
    render(<CoverageView />);
    const heading = await screen.findByRole("heading", { name: "Per-file coverage" });
    const figure = heading.closest("[data-widget]")!.querySelector('[data-widget-part="figure"]');
    expect(figure).toHaveTextContent("73.0% of lines covered · 1 of 3 files stale");
  });
});

it("names the ranking command in its own nothing-ranked absence, when the read-model has no notice (HF-1 review)", async () => {
  stubFetch(() => {
    const m = notRanked();
    m.untested = { ...m.untested, notice: null, degraded: null } as CoverageModel["untested"];
    return m;
  });
  render(<CoverageView />);
  const heading = await screen.findByRole("heading", { name: "Untested hotspots" });
  expect(heading.closest("[data-widget]")!.querySelector("[data-widget-absence]")?.textContent).toBe(
    "No files ranked yet, so no untested file can be named; run logos hotspots to rank the files from the git history.",
  );
});

it("states the read-model's notice alone when nothing was ranked: a degraded history is not fixed by the ranking command (HF-1 review)", async () => {
  stubFetch(notRanked);
  render(<CoverageView />);
  const heading = await screen.findByRole("heading", { name: "Untested hotspots" });
  const widget = heading.closest("[data-widget]")!;
  // Not a git repository: re-running `logos hotspots` returns the same degraded
  // answer, so the absence names no command (R3; FR-UI-39).
  expect(widget.querySelector("[data-widget-absence]")?.textContent).toBe("not a git repository: churn unavailable");
});

// ── S-739 (FR-CV-15, FR-CV-13, FR-UI-05 as modified): Run coverage ───────────

/** `model` as the page reads it: the status carries the coverage runner. */
function withRunner(model: CoverageModel, tools: CoverageToolStatus[] = TOOLS, consented: string[] = []): CoverageModel {
  return { ...model, coverage: { ...model.coverage, runner: { consent: { consented, pending: [], command_changed: false }, tools } } };
}

interface Call {
  method: string;
  url: string;
  body: string | null;
  intent: string | null;
}

interface Reply {
  status?: number;
  body: unknown;
}

/** Stubs fetch: `GET /api/v1/coverage` answers `pages` in turn (the last one
 *  again; every read after the first waits for `hold` when one is given),
 *  `GET /api/v1/coverage/run` answers `{ job: current }`, and each POST answers
 *  the next of `posts`. Every call is recorded. */
function stubRun({
  pages,
  current = null,
  posts = [],
  hold,
}: {
  pages: CoverageModel[];
  current?: CoverageJob | null;
  posts?: Reply[];
  hold?: Promise<void>;
}): Call[] {
  const calls: Call[] = [];
  let page = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init: RequestInit = {}) => {
      const call: Call = {
        method: init.method ?? "GET",
        url,
        body: typeof init.body === "string" ? init.body : null,
        intent: new Headers(init.headers).get("x-logos-intent"),
      };
      calls.push(call);
      let reply: Reply;
      if (call.method === "POST") reply = posts.shift() ?? { status: 500, body: { error: "no reply staged" } };
      else if (url.startsWith("/api/v1/coverage/run")) reply = { body: { job: current } };
      else {
        const read = page++;
        reply = { body: pages[Math.min(read, pages.length - 1)] };
        if (read > 0) await hold;
      }
      const status = reply.status ?? 200;
      return {
        ok: status >= 200 && status < 300,
        status,
        json: () => Promise.resolve(structuredClone(reply.body)),
        text: () => Promise.resolve(JSON.stringify(reply.body)),
      } as Response;
    }),
  );
  return calls;
}

const pageReads = (calls: Call[]) => calls.filter((c) => c.method === "GET" && c.url === "/api/v1/coverage").length;
/** Each listed tool and what its State cell reads: the run is the table's (S-803). */
const toolStates = () =>
  within(within(toolsWidget()).getByRole("table", { name: "Coverage tools" }))
    .getAllByRole("row")
    .slice(1)
    .map((row) => {
      const cells = row.querySelectorAll("td");
      return `${cells[0].textContent} ${cells[2].textContent}`;
    });
const liveLine = (container: HTMLElement) => container.querySelector('[aria-live="polite"]')?.textContent ?? "";
const toolsWidget = () => screen.getByRole("heading", { name: "Coverage tools" }).closest("[data-widget]") as HTMLElement;

describe("Run coverage on the Coverage page (S-739)", () => {
  it("leads with the Coverage tools widget, which explains itself", async () => {
    stubRun({ pages: [withRunner(populated())] });
    const { container } = render(<CoverageView />);
    await screen.findByRole("heading", { name: "Coverage tools" });
    const widgets = expectOneWidgetStack(container);
    expect(widgets.map(widgetTitle)).toEqual(["Coverage tools", "Untested hotspots", "Per-file coverage"]);
    expectWidgetCopy(widgets[0], coverageTools);
  });

  it("asks the execution consent on first use, then starts with exactly what it showed and streams each tool", async () => {
    const sources = installEventSources();
    const calls = stubRun({
      pages: [withRunner(populated())],
      posts: [{ status: 409, body: CONSENT_FIRST }, { body: { joined: false, job: { ...RUNNING, events: [] } } }],
    });
    const { container } = render(<CoverageView />);
    await userEvent.click(await within(await findToolsWidget()).findByRole("button", { name: COVERAGE_RUN_TEXT.run }));

    // The dialog names the tools that will run and the command as shown; nothing started.
    const dialog = await screen.findByRole("dialog", { name: COVERAGE_CONSENT_TEXT.title });
    expect(within(dialog).getByRole("list", { name: COVERAGE_CONSENT_TEXT.tools })).toHaveTextContent("vitestgo");
    expect(within(dialog).getByLabelText(COVERAGE_CONSENT_TEXT.command).textContent).toBe("make cov\\u{202e}");
    expect(sources).toHaveLength(0);

    await userEvent.click(within(dialog).getByRole("button", { name: COVERAGE_CONSENT_TEXT.accept }));
    const posts = calls.filter((c) => c.method === "POST");
    expect(posts).toHaveLength(2);
    expect(posts.every((p) => p.url === "/api/v1/coverage/run" && p.intent === "test-intent-token")).toBe(true);
    expect(JSON.parse(posts[0].body ?? "")).toEqual({ consent: [] });
    // The accepted start echoes the exact command text, never the escaped display.
    expect(JSON.parse(posts[1].body ?? "")).toEqual({ consent: ["vitest", "go", "custom"], refresh_cmd: "make cov‮" });
    expect(screen.queryByRole("dialog")).toBeNull();

    await waitFor(() => expect(sources).toHaveLength(1));
    expect(toolStates()).toEqual(["vitest queued", "maven not installed", "go queued", "custom queued"]);
    expect(screen.queryByRole("list", { name: "Coverage run progress" })).toBeNull();
    expect(within(toolsWidget()).getByRole("button", { name: COVERAGE_RUN_TEXT.cancel })).toBeInTheDocument();
    act(() => {
      for (const step of EVENTS.slice(0, 3)) sources[0].emit("step", step);
    });
    expect(toolStates()).toEqual(["vitest ran — tests failed", "maven not installed", "go queued", "custom queued"]);
    expect(liveLine(container)).toContain("vitest: ran — tests failed.");
    act(() => sources[0].emit("step", EVENTS[7]));
    expect(toolStates()).toEqual(["vitest ran — tests failed", "maven not installed", "go running", "custom queued"]);
  });

  it("starts nothing and records nothing when the dialog is cancelled", async () => {
    const sources = installEventSources();
    const calls = stubRun({ pages: [withRunner(populated())], posts: [{ status: 409, body: CONSENT_FIRST }] });
    render(<CoverageView />);
    await userEvent.click(await within(await findToolsWidget()).findByRole("button", { name: COVERAGE_RUN_TEXT.run }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: COVERAGE_CONSENT_TEXT.decline }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
    expect(sources).toHaveLength(0);
  });

  it("follows a run started in another tab from the moment the page opens", async () => {
    const sources = installEventSources();
    stubRun({ pages: [withRunner(populated())], current: RUNNING });
    render(<CoverageView />);
    await waitFor(() => expect(sources).toHaveLength(1));
    expect(sources[0].url).toBe("/api/v1/coverage/run");
    expect(toolStates()).toEqual(["vitest ran — tests failed", "maven not installed", "go running", "custom queued"]);
    expect(within(toolsWidget()).queryByRole("button", { name: COVERAGE_RUN_TEXT.run })).toBeNull();
  });

  it("joins the run going on a second click from a tab that had not seen it", async () => {
    const sources = installEventSources();
    stubRun({ pages: [withRunner(populated())], posts: [{ body: { joined: true, job: RUNNING } }] });
    const { container } = render(<CoverageView />);
    await userEvent.click(await within(await findToolsWidget()).findByRole("button", { name: COVERAGE_RUN_TEXT.run }));
    await waitFor(() => expect(liveLine(container)).toContain(COVERAGE_RUN_TEXT.joined));
    expect(sources).toHaveLength(1);
    expect(toolStates()).toContain("go running");
  });

  it("cancels the run: the live line says so, the page reloads, and the rows read their stored state again", async () => {
    const sources = installEventSources();
    const calls = stubRun({ pages: [withRunner(populated())], current: RUNNING, posts: [{ body: { job: RUNNING } }] });
    const { container } = render(<CoverageView />);
    await waitFor(() => expect(sources).toHaveLength(1));
    await userEvent.click(within(toolsWidget()).getByRole("button", { name: COVERAGE_RUN_TEXT.cancel }));
    const cancel = calls.find((c) => c.method === "POST");
    expect(cancel).toMatchObject({ url: "/api/v1/coverage/run/cancel", intent: "test-intent-token" });
    act(() => {
      sources[0].emit("step", GO_CANCELLED);
      sources[0].emit("end", { job: CANCELLED_END });
    });
    await waitFor(() => expect(liveLine(container)).toContain(COVERAGE_RUN_TEXT.cancelled(["vitest"])));
    expect(sources[0].closed).toBe(true);
    await waitFor(() => expect(pageReads(calls)).toBe(2));
    // Once the reload has landed (HF-6) the live states give way to the stored ones.
    expect(toolStates()).toEqual(["vitest never run", "maven not installed", "go never run", "custom never run"]);
    expect(within(toolsWidget()).getByRole("button", { name: COVERAGE_RUN_TEXT.run })).toBeInTheDocument();
  });

  it("refreshes its figures when the run ends, and reads each tool's new state in words", async () => {
    const sources = installEventSources();
    const before = withRunner(empty());
    const after = withRunner(populated(), TOOLS_AFTER, ["vitest", "go", "custom"]);
    const calls = stubRun({ pages: [before, after], current: RUNNING });
    const { container } = render(<CoverageView />);
    await waitFor(() => expect(sources).toHaveLength(1));
    expect(screen.getAllByText(/No coverage ingested/)).toHaveLength(2);
    act(() => {
      for (const step of EVENTS.slice(8)) sources[0].emit("step", step);
      sources[0].emit("end", { job: COMPLETED_END });
    });
    await screen.findByText("2/3 files fresh", { exact: false });
    expect(pageReads(calls)).toBe(2);
    expect(liveLine(container)).toContain(COVERAGE_RUN_TEXT.completed(2, 1, 1));
    const table = within(toolsWidget()).getByRole("table", { name: "Coverage tools" });
    const stateOf = (tool: string) => within(table).getByText(tool, { selector: "td" }).closest("tr")?.querySelectorAll("td")[2].textContent;
    expect(stateOf("vitest")).toBe("ran — tests failed");
    expect(stateOf("maven")).toBe("not installed");
    expect(stateOf("custom")).toMatch(/^failed/);
    // A not-installed tool and a failing suite never render as a percentage.
    for (const tool of ["vitest", "maven"]) expect(stateOf(tool)).not.toMatch(/%/);
  });

  it("shows the run ended only once the reload has answered, never over the figures read before it (HF-6)", async () => {
    const sources = installEventSources();
    let release = () => undefined as void;
    const hold = new Promise<void>((done) => {
      release = done;
    });
    const before = withRunner(empty());
    const after = withRunner(populated(), TOOLS_AFTER, ["vitest", "go", "custom"]);
    const calls = stubRun({ pages: [before, after], current: RUNNING, hold });
    const { container } = render(<CoverageView />);
    await waitFor(() => expect(sources).toHaveLength(1));
    act(() => {
      for (const step of EVENTS.slice(8)) sources[0].emit("step", step);
      sources[0].emit("end", { job: COMPLETED_END });
    });
    await waitFor(() => expect(pageReads(calls)).toBe(2));
    await new Promise((done) => setTimeout(done, 50));
    // The second read is asked for and unanswered: the run still reads running,
    // not finished over the page's pre-run figures.
    expect(within(toolsWidget()).getByRole("button", { name: COVERAGE_RUN_TEXT.cancel })).toBeInTheDocument();
    expect(liveLine(container)).not.toContain(COVERAGE_RUN_TEXT.completed(2, 1, 1));
    expect(screen.getAllByText(/No coverage ingested/)).toHaveLength(2);
    act(() => release());
    await screen.findByText("2/3 files fresh", { exact: false });
    await waitFor(() => expect(liveLine(container)).toContain(COVERAGE_RUN_TEXT.completed(2, 1, 1)));
    expect(within(toolsWidget()).getByRole("button", { name: COVERAGE_RUN_TEXT.run })).toBeInTheDocument();
    expect(pageReads(calls)).toBe(2);
  });

  it("offers Run coverage in the empty state, beside the ingest command", async () => {
    installEventSources();
    stubRun({ pages: [withRunner(empty())], posts: [{ status: 409, body: CONSENT_FIRST }] });
    render(<CoverageView />);
    const callout = (await screen.findByText(COVERAGE_RUN_TEXT.emptyRun, { exact: false })).closest("section") as HTMLElement;
    expect(callout).toHaveTextContent("no data ingested");
    const absences = [...document.querySelectorAll("[data-widget-absence]")].map((a) => a.textContent);
    expect(absences).toEqual([COVERAGE_VIEW_ABSENCE.notIngested, COVERAGE_VIEW_ABSENCE.notIngested]);
    await userEvent.click(within(callout).getByRole("button", { name: COVERAGE_RUN_TEXT.run }));
    expect(await screen.findByRole("dialog", { name: COVERAGE_CONSENT_TEXT.title })).toBeInTheDocument();
  });

  it("says which tools would apply when none is installed, and offers no Run", async () => {
    const missing = TOOLS.map((t) => (t.state === "not_applicable" ? t : { ...t, state: "not_installed" as const }));
    stubRun({ pages: [withRunner(empty(), missing)] });
    render(<CoverageView />);
    const sentence = await screen.findByText(COVERAGE_RUN_TEXT.noneInstalled(["vitest", "maven", "go", "custom"], false));
    expect(sentence).toHaveTextContent("Logos cannot set these up for you: install them yourself.");
    expect(within(sentence.closest("section") as HTMLElement).queryByRole("button")).toBeNull();
    expect(within(toolsWidget()).getByRole("button", { name: COVERAGE_RUN_TEXT.run })).toBeDisabled();
  });

  it("states a refused start with the server's reason", async () => {
    installEventSources();
    stubRun({ pages: [withRunner(populated())], posts: [{ status: 403, body: { error: "the request carries no valid intent token" } }] });
    render(<CoverageView />);
    await userEvent.click(await within(await findToolsWidget()).findByRole("button", { name: COVERAGE_RUN_TEXT.run }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      COVERAGE_RUN_TEXT.requestFailed("the request carries no valid intent token"),
    );
  });
});

describe("a refused request's alert (review)", () => {
  it("is cleared by the next start that is answered", async () => {
    installEventSources();
    stubRun({
      pages: [withRunner(populated())],
      posts: [{ status: 403, body: { error: "the request carries no valid intent token" } }, { body: { joined: false, job: RUNNING } }],
    });
    render(<CoverageView />);
    const run = await within(await findToolsWidget()).findByRole("button", { name: COVERAGE_RUN_TEXT.run });
    await userEvent.click(run);
    await screen.findByRole("alert");
    await userEvent.click(run);
    await waitFor(() => expect(within(toolsWidget()).getByRole("button", { name: COVERAGE_RUN_TEXT.cancel })).toBeInTheDocument());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("is cleared by the next cancel that is answered", async () => {
    installEventSources();
    stubRun({
      pages: [withRunner(populated())],
      current: RUNNING,
      posts: [{ status: 500, body: { error: "the runner is gone" } }, { body: { job: RUNNING } }],
    });
    render(<CoverageView />);
    const cancel = await within(await findToolsWidget()).findByRole("button", { name: COVERAGE_RUN_TEXT.cancel });
    await userEvent.click(cancel);
    expect(await screen.findByRole("alert")).toHaveTextContent("the runner is gone");
    await userEvent.click(cancel);
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });
});

async function findToolsWidget(): Promise<HTMLElement> {
  return (await screen.findByRole("heading", { name: "Coverage tools" })).closest("[data-widget]") as HTMLElement;
}

describe("Install and Add dependency on the Coverage page (S-750, FR-CV-17, FR-CV-18)", () => {
  it("shows a vitest consent with the exact command and files before anything changes, then starts the job on Accept", async () => {
    installEventSources();
    const calls = stubRun({
      pages: [withRunner(empty(), PREPARABLE_TOOLS)],
      posts: [{ status: 409, body: DEPENDENCY_FIRST }, { body: { joined: false, job: RUNNING } }],
    });
    render(<CoverageView />);
    await userEvent.click(await screen.findByRole("button", { name: COVERAGE_OFFER_TEXT.addDependencyLabel("vitest") }));
    const dialog = await screen.findByRole("dialog", { name: COVERAGE_PREPARE_CONSENT_TEXT.dependencyTitle });
    expect(within(dialog).getByLabelText(COVERAGE_PREPARE_CONSENT_TEXT.command).textContent).toBe("pnpm add -D @vitest/coverage-v8@3");
    expect(dialog).toHaveTextContent("It changes package.json, pnpm-lock.yaml.");
    // Before the answer the one POST asked and carried no acceptance.
    const posts = () => calls.filter((c) => c.method === "POST");
    expect(posts().map((c) => [c.url, JSON.parse(c.body ?? "")])).toEqual([["/api/v1/coverage/dependency", { consent: [], tool: "vitest" }]]);
    expect(posts()[0].intent).toBe("test-intent-token");
    await userEvent.click(within(dialog).getByRole("button", { name: COVERAGE_PREPARE_CONSENT_TEXT.acceptDependency }));
    await waitFor(() => expect(posts()).toHaveLength(2));
    expect(JSON.parse(posts()[1].body ?? "")).toEqual({
      consent: ["vitest"],
      install: [],
      dependency: ["pnpm add -D @vitest/coverage-v8@3"],
      tool: "vitest",
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("says the widget can set some tools up when none is installed and Logos offers to", async () => {
    stubRun({ pages: [withRunner(empty(), PREPARABLE_TOOLS)] });
    render(<CoverageView />);
    const sentence = await screen.findByText(COVERAGE_RUN_TEXT.noneInstalled(["cargo-llvm-cov", "vitest", "maven"], true));
    expect(sentence).toHaveTextContent("Install or Add dependency in the Coverage tools widget");
    expect(within(toolsWidget()).getByRole("button", { name: COVERAGE_OFFER_TEXT.installLabel("cargo-llvm-cov") })).toBeEnabled();
  });
});
