import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FilesModel } from "../../api/types.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import { filesAbsence, filesRankedByRisk, ownershipDispersion } from "../../copy/files.copy.ts";
import { FilesView } from "./FilesView.tsx";

const STATUS = { indexed: true, file_count: 9, node_count: 99, edge_count: 80, db_path: ".logos/graph.db", db_size_bytes: 12288, last_full_index_at: "1719600000", last_sync_at: null, graph_revision: 7, refs_total: 120, refs_resolved: 118, refs_unresolved: 2, resolution_coverage: 0.983, total_line_count: null, source_line_count: null, test_line_count: null, freshness: "fresh", warnings: [] };

function model(over: Partial<FilesModel["hotspots"]> = {}): FilesModel {
  return {
    status: STATUS,
    hotspots: {
      tier: "temporal (non-gated, advisory)",
      defect_label: "heuristic",
      head_sha: "head",
      config_hash: "cfg",
      limit: 50,
      ranked_files: 2,
      files: [
        { path: "src/hot.rs", score: 40, churn_rank: 2, churn_commits: 12, complexity_rank: 2, complexity: 30, co_change_count: 3, defect_commits: 1, coverage: { state: "fresh", coverage_bp: 8200 } },
        { path: "src/cold.rs", score: 8, churn_rank: 1, churn_commits: 2, complexity_rank: 1, complexity: 4, co_change_count: 0, defect_commits: 0, coverage: { state: "n/a", coverage_bp: null } },
      ],
      degraded: null,
      notice: null,
      untested: false,
      production_scope: false,
      coverage_basis: "coverage",
      coverage_label: null,
      ...over,
    },
    temporal: {
      head_sha: "head",
      mined_through: "head",
      config_hash: "cfg",
      window_months: 6,
      // src/cold.rs intentionally absent → its churn/age must render n/a.
      files: [
        { path: "src/hot.rs", commit_count: 12, lines_added: 120, lines_deleted: 30, last_change_age_days: 3, age_dispersion_days: 2, ownership_dispersion_bp: 5500, change_entropy_bp: 1200 },
      ],
      degraded: null,
      first_mine: false,
    },
  };
}

const EMPTY = (): FilesModel => {
  const m = model();
  m.hotspots.files = [];
  m.hotspots.ranked_files = 0;
  return m;
};

function stubFetch(byUrl: (url: string) => unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) =>
      Promise.resolve({ ok: true, json: () => Promise.resolve(byUrl(url)) } as Response),
    ),
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/** The widget frame titled `title`. */
function widget(title: string): Element {
  const frame = screen.getByRole("heading", { name: title }).closest("[data-widget]");
  if (!frame) throw new Error(`no widget titled ${title}`);
  return frame;
}

const LONG_A = "logos-core/src/history/temporal/mining/window/commits.rs";
const LONG_B = "logos-core/src/history/spatial/mining/window/commits.rs";
const DEEP = "src/aaa/bbb/ccc/ddd/eee/fff/ggg/hhh/iii/zzz.rs";

describe("FilesView (S-188, FR-UI-11)", () => {
  it("leads with the top hotspot and renders the merged risk table from /api/v1", async () => {
    stubFetch(() => model());
    render(<FilesView />);
    const table = await screen.findByRole("table", { name: "Files ranked by risk" });
    const figure = widget("Files ranked by risk").querySelector('[data-widget-part="figure"]')!;
    expect(figure).toHaveTextContent("2 files ranked");
    expect(figure).toHaveTextContent("top: src/hot.rs, score 40");
    expect(within(table).getByText("src/hot.rs")).toBeInTheDocument();
    expect(screen.getByText(/Defect column: heuristic/)).toBeInTheDocument();
  });

  it("renders an absent temporal join as n/a — never a fabricated zero", async () => {
    stubFetch(() => model());
    render(<FilesView />);
    const table = await screen.findByRole("table", { name: "Files ranked by risk" });
    const coldRow = within(table).getByText("src/cold.rs").closest("tr")!;
    // src/cold.rs has no temporal row → churn (+/−) and age cells are n/a
    expect(within(coldRow).getAllByText("n/a").length).toBeGreaterThanOrEqual(2);
  });

  it("states an empty board in the figure row and names the command that ranks it", async () => {
    stubFetch(() => EMPTY());
    render(<FilesView />);
    const absence = await screen.findByText(filesAbsence.unranked);
    expect(absence).toHaveAttribute("data-widget-absence");
    const w = widget("Files ranked by risk");
    expectWidgetCopy(w, filesRankedByRisk, { ranked: 0, filtered: false, coverageMissing: false });
    expect(w.querySelector('[data-widget-copy="where"]')).toHaveTextContent("logos hotspots");
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("an empty board under a filter keeps the filter switchable and does not ask to rank", async () => {
    const user = userEvent.setup();
    const urls: string[] = [];
    stubFetch((url) => {
      urls.push(url);
      if (!url.includes("untested=true")) return model();
      const m = EMPTY();
      m.hotspots.untested = true;
      return m;
    });
    render(<FilesView />);
    await screen.findByRole("table", { name: "Files ranked by risk" });
    await user.click(screen.getByRole("button", { name: "Untested only" }));
    const absence = await screen.findByText(filesAbsence.filteredOut);
    expect(absence).toHaveAttribute("data-widget-absence");
    const w = widget("Files ranked by risk");
    expectWidgetCopy(w, filesRankedByRisk, { ranked: 0, filtered: true, coverageMissing: false });
    expect(w.querySelector('[data-widget-part="action"]')).toHaveAttribute("data-action-kind", "none");
    // The way back is on the page.
    await user.click(within(w as HTMLElement).getByRole("button", { name: "Show all files" }));
    expect(await screen.findByRole("table", { name: "Files ranked by risk" })).toBeInTheDocument();
  });

  it("states the read-model's own notice as the absence when it has one", async () => {
    stubFetch(() => {
      const m = EMPTY();
      m.hotspots.notice = "First mine: history is being read.";
      return m;
    });
    render(<FilesView />);
    expect(await screen.findByText("First mine: history is being read.")).toHaveAttribute("data-widget-absence");
  });

  it("the untested toggle re-fetches the board with ?untested", async () => {
    const user = userEvent.setup();
    const urls: string[] = [];
    stubFetch((url) => {
      urls.push(url);
      return url.includes("untested") ? model({ untested: true }) : model();
    });
    render(<FilesView />);
    await screen.findByRole("table", { name: "Files ranked by risk" });
    await user.click(screen.getByRole("button", { name: "Untested only" }));
    await waitFor(() => expect(urls.some((u) => u.includes("untested=true"))).toBe(true));
  });

  it("the production-scope toggle re-fetches the board with ?production_scope (CR-076)", async () => {
    const user = userEvent.setup();
    const urls: string[] = [];
    stubFetch((url) => {
      urls.push(url);
      return url.includes("production_scope") ? model({ production_scope: true }) : model();
    });
    render(<FilesView />);
    await screen.findByRole("table", { name: "Files ranked by risk" });
    await user.click(screen.getByRole("button", { name: "Production files only" }));
    await waitFor(() =>
      expect(urls.some((u) => u.includes("production_scope=true"))).toBe(true),
    );
    expect(await screen.findByText(/production files only/)).toBeInTheDocument();
  });

  it("exposes the data as an accessible <table> (keyboard/screen-reader affordance)", async () => {
    stubFetch(() => model());
    render(<FilesView />);
    const table = await screen.findByRole("table", { name: "Files ranked by risk" });
    expect(table.tagName).toBe("TABLE");
    expect(screen.getAllByRole("table").length).toBeGreaterThanOrEqual(1);
  });

  it("explains the risk ranking; with no coverage ingested, the action is the ingest command", async () => {
    stubFetch(() => model());
    render(<FilesView />);
    await screen.findByRole("table", { name: "Files ranked by risk" });
    // src/hot.rs is fresh: one ranked file has a coverage figure.
    expectWidgetCopy(widget("Files ranked by risk"), filesRankedByRisk, { ranked: 2, filtered: false, coverageMissing: false });
    cleanup();

    // No report ingested: the read-model falls back to static reachability.
    stubFetch(() => {
      const m = model({ coverage_basis: "static-reachability", coverage_label: "static reachability, not execution coverage" });
      for (const f of m.hotspots.files) f.coverage = { state: "n/a", coverage_bp: null };
      return m;
    });
    render(<FilesView />);
    await screen.findByRole("table", { name: "Files ranked by risk" });
    const w = widget("Files ranked by risk");
    expectWidgetCopy(w, filesRankedByRisk, { ranked: 2, filtered: false, coverageMissing: true });
    expect(w.querySelector('[data-widget-copy="where"]')).toHaveTextContent("command logos coverage ingest");
  });

  it("never asks to ingest coverage that is ingested, even when every listed file reads n/a", async () => {
    // "Untested only" over an ingested report keeps exactly the files with no
    // fresh coverage, so every remaining cell can read n/a — the report exists.
    stubFetch(() => {
      const m = model({ untested: true, coverage_basis: "coverage" });
      for (const f of m.hotspots.files) f.coverage = { state: "n/a", coverage_bp: null };
      return m;
    });
    render(<FilesView />);
    await screen.findByRole("table", { name: "Files ranked by risk" });
    const w = widget("Files ranked by risk");
    expectWidgetCopy(w, filesRankedByRisk, { ranked: 2, filtered: true, coverageMissing: false });
    expect(w.querySelector('[data-widget-copy="where"]')).toHaveTextContent(/^source code$/);
  });

  it("with coverage, the action is to test or split the top files, in source code", async () => {
    stubFetch(() => model());
    render(<FilesView />);
    await screen.findByRole("table", { name: "Files ranked by risk" });
    expect(widget("Files ranked by risk").querySelector('[data-widget-copy="where"]')).toHaveTextContent(
      /^source code$/,
    );
  });

  it("glosses the Co-change and Defect headers", async () => {
    stubFetch(() => model());
    render(<FilesView />);
    const table = await screen.findByRole("table", { name: "Files ranked by risk" });
    const glossed = [...table.querySelectorAll("th dfn[data-term]")].map((d) => d.getAttribute("data-term"));
    expect(glossed).toEqual(["coChange", "defect"]);
  });

  it("explains ownership dispersion, and names CODEOWNERS when files have several authors", async () => {
    stubFetch(() => model());
    render(<FilesView />);
    await screen.findByRole("table", { name: "Ownership dispersion" });
    const w = widget("Ownership dispersion");
    expectWidgetCopy(w, ownershipDispersion, { multiAuthor: true });
    expect(w.querySelector('[data-widget-copy="where"]')).toHaveTextContent("documentation CODEOWNERS");
    expect(w.querySelector('[data-widget-part="figure"]')).toHaveTextContent("1 of 1 files have more than one author");
  });

  it("reads a single-author history as nothing to do", async () => {
    stubFetch(() => {
      const m = model();
      m.temporal.files[0].ownership_dispersion_bp = 0;
      m.temporal.files[0].change_entropy_bp = 0;
      return m;
    });
    render(<FilesView />);
    await screen.findByRole("table", { name: "Files ranked by risk" });
    const w = widget("Ownership dispersion");
    expectWidgetCopy(w, ownershipDispersion, { multiAuthor: false });
    // Pinned apart from the catalogue: expectWidgetCopy compares against the
    // catalogue's own action, so a wrong branch there would agree with itself.
    expect(w.querySelector('[data-widget-part="action"]')).toHaveAttribute("data-action-kind", "none");
    expect(within(w as HTMLElement).getByText(filesAbsence.singleAuthor)).toHaveAttribute("data-widget-absence");
    expect(screen.queryByRole("table", { name: "Ownership dispersion" })).toBeNull();
  });

  it("stacks both widgets in one WidgetStack", async () => {
    stubFetch(() => model());
    const { container } = render(<FilesView />);
    await screen.findByRole("table", { name: "Files ranked by risk" });
    const stacks = container.querySelectorAll("[data-widget-stack]");
    expect(stacks).toHaveLength(1);
    expect([...stacks[0].children].map((c) => c.querySelector("h3")?.textContent)).toEqual([
      "Files ranked by risk",
      "Ownership dispersion",
    ]);
  });

  it("abbreviates long paths in both tables, distinct where they would collide, full path as name", async () => {
    stubFetch(() => {
      const m = model();
      m.hotspots.files[0].path = LONG_A;
      m.hotspots.files[1].path = LONG_B;
      m.temporal.files[0].path = LONG_A;
      return m;
    });
    render(<FilesView />);
    const risk = await screen.findByRole("table", { name: "Files ranked by risk" });
    const label = (table: HTMLElement, path: string) =>
      within(table).getByRole("cell", { name: path }).querySelector('[aria-hidden="true"]')?.textContent;
    expect(label(risk, LONG_A)).toBe("logos-core/…/temporal/mining/window/commits.rs");
    expect(label(risk, LONG_B)).toBe("logos-core/…/spatial/mining/window/commits.rs");
    // Alone in its table, the same path takes the shortest abbreviation.
    const owners = screen.getByRole("table", { name: "Ownership dispersion" });
    expect(label(owners, LONG_A)).toBe("logos-core/…/window/commits.rs");
    expect(within(owners).getByTitle(LONG_A)).toBeInTheDocument();
  });

  it("labels the top file in the figure exactly as its table row, even where the short label collides", async () => {
    stubFetch(() => {
      const m = model();
      m.hotspots.files[0].path = LONG_A;
      m.hotspots.files[1].path = LONG_B;
      return m;
    });
    render(<FilesView />);
    const table = await screen.findByRole("table", { name: "Files ranked by risk" });
    const figure = widget("Files ranked by risk").querySelector('[data-widget-part="figure"]')!;
    const inFigure = within(figure as HTMLElement).getByTitle(LONG_A);
    expect(inFigure.querySelector('[aria-hidden="true"]')?.textContent).toBe(
      "logos-core/…/temporal/mining/window/commits.rs",
    );
    expect(within(table).getByTitle(LONG_A).querySelector('[aria-hidden="true"]')?.textContent).toBe(
      inFigure.querySelector('[aria-hidden="true"]')?.textContent,
    );
  });

  it("sorts the File column by the full path", async () => {
    const user = userEvent.setup();
    stubFetch(() => {
      const m = model();
      // By full path "src/aaa/…" sorts before "src/b.rs"; by its label
      // "src/…/iii/zzz.rs" it would sort after ("…" is above every ASCII letter).
      m.hotspots.files[0].path = "src/b.rs";
      m.hotspots.files[1].path = DEEP;
      return m;
    });
    render(<FilesView />);
    const table = await screen.findByRole("table", { name: "Files ranked by risk" });
    // The deep path IS abbreviated, so the order below separates the two keys.
    expect(within(table).getByTitle(DEEP).querySelector('[aria-hidden="true"]')).toHaveTextContent("src/…/iii/zzz.rs");
    await user.click(within(table).getByRole("button", { name: "File" }));
    const order = [...table.querySelectorAll("tbody [data-path]")].map((el) => el.getAttribute("data-path"));
    expect(order).toEqual([DEEP, "src/b.rs"]);
  });
});
