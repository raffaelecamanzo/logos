import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FilesModel } from "../../api/types.ts";
import { FilesView } from "./FilesView.tsx";

const STATUS = { indexed: true, file_count: 9, node_count: 99, edge_count: 80, db_path: ".logos/graph.db", db_size_bytes: 12288, last_full_index_at: "1719600000", last_sync_at: null, graph_revision: 7, refs_total: 120, refs_resolved: 118, refs_unresolved: 2, resolution_coverage: 0.983, freshness: "fresh", warnings: [] };

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

describe("FilesView (S-188, FR-UI-11)", () => {
  it("leads with the top hotspot and renders the merged risk table from /api/v1", async () => {
    stubFetch(() => model());
    render(<FilesView />);
    const table = await screen.findByRole("table", { name: "Files ranked by risk" });
    const verdict = screen.getByRole("status");
    expect(verdict).toHaveTextContent("src/hot.rs");
    expect(verdict).toHaveTextContent("score 40");
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

  it("shows the honest empty state naming the command when no hotspots are ranked", async () => {
    stubFetch(() => EMPTY());
    render(<FilesView />);
    expect(await screen.findByText(/No hotspots ranked yet/)).toBeInTheDocument();
    expect(screen.getByText("logos hotspots")).toBeInTheDocument();
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

  it("exposes the data as an accessible <table> (keyboard/screen-reader affordance)", async () => {
    stubFetch(() => model());
    render(<FilesView />);
    const table = await screen.findByRole("table", { name: "Files ranked by risk" });
    expect(table.tagName).toBe("TABLE");
    expect(screen.getAllByRole("table").length).toBeGreaterThanOrEqual(1);
  });
});
