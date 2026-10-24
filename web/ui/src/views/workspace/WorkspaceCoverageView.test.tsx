/*
 * Workspace coverage (S-740, FR-CV-16, UAT-CV-05 step 6).
 *
 * The payload is the one `web/tests/workspace_api.rs` produced over its
 * three-member fixture (`fixtures/workspaceCoverage.real.json`, the temporary
 * path replaced): `covered` — an LCOV snapshot over two files, one changed
 * since, and a custom command that ran; `idle` — nothing ingested, its custom
 * command failed; `broken` — a member nothing can open.
 *
 * The assertions are the acceptance criteria: a never-run member reads "never
 * run", not 0 %; an unopenable member is a named row that hides no other; the
 * figure row states how many members have fresh coverage out of all members;
 * each member row links to that member's Coverage page. And the page has no Run
 * button.
 */

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { WorkspaceCoverageAnswer, WorkspaceRoster } from "../../api/index.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import {
  memberCoverage,
  WORKSPACE_COVERAGE_ABSENCE,
  WORKSPACE_COVERAGE_TEXT,
} from "../../copy/workspaceCoverage.copy.ts";
import { expectOneWidgetStack, widgetTitled } from "../../test/widgetStack.ts";
import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../../workspace/scope.ts";
import real from "./fixtures/workspaceCoverage.real.json";
import { memberCoverageHref, WorkspaceCoverageView } from "./WorkspaceCoverageView.tsx";

const REAL = real as unknown as WorkspaceCoverageAnswer;

const ROSTER: WorkspaceRoster = { workspace: "measured", default: "covered", members: ["covered", "idle", "broken"] };

/** One hour after the fixture's `covered` run ended. */
const NOW = 1_760_000_000 + 3600;

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

/** Stub the roster probe (`probe` 404 ⇒ single-root) and the one read; return the
 *  URLs fetched. */
function stub(answer: WorkspaceCoverageAnswer, probe = 200): () => string[] {
  const calls: string[] = [];
  const json = (body: unknown, code = 200) =>
    Promise.resolve({ ok: code === 200, status: code, json: () => Promise.resolve(body) } as Response);
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      calls.push(url);
      if (url.startsWith("/api/v1/workspace/roster")) return json(ROSTER, probe);
      if (url.startsWith("/api/v1/workspace/coverage")) return json(answer);
      return json({});
    }),
  );
  return () => calls;
}

async function mount(answer: WorkspaceCoverageAnswer = REAL) {
  const calls = stub(answer);
  const utils = render(
    <WorkspaceProvider>
      <WorkspaceCoverageView nowUnix={NOW} />
    </WorkspaceProvider>,
  );
  await screen.findByRole("heading", { name: /^Members$/ });
  return { ...utils, calls };
}

/** The table row whose first cell names `member`. */
function memberRow(member: string): HTMLElement {
  const link = within(widgetTitled(document.body, "Members")).getByRole("link", { name: member });
  const row = link.closest("tr");
  expect(row).not.toBeNull();
  return row as HTMLElement;
}

/** The cells of `row` after the member column: Coverage, Fresh files, Tools, Last run. */
function figureCells(row: HTMLElement): string[] {
  return within(row)
    .getAllByRole("cell")
    .slice(1)
    .map((c) => c.textContent ?? "");
}

/** `answer` with its roll-up fields replaced. */
function withCoverage(patch: Partial<WorkspaceCoverageAnswer["coverage"]>): WorkspaceCoverageAnswer {
  return { ...REAL, coverage: { ...REAL.coverage, ...patch } };
}

describe("the member roll-up (FR-CV-16)", () => {
  it("states the covered member's own percentage, fresh files of total, tool outcomes and last run", async () => {
    await mount();
    const row = memberRow("covered");
    // The percentage is printed beside the bar, not only as the meter's fallback
    // text, which a browser does not paint.
    const coverage = within(row).getAllByRole("cell")[1];
    expect(coverage.querySelector("meter")?.getAttribute("value")).toBe("5000");
    expect(Array.from(coverage.querySelectorAll("span.num")).map((s) => s.textContent)).toEqual(["50.0%"]);
    expect(figureCells(row).slice(1)).toEqual([
      WORKSPACE_COVERAGE_TEXT.freshFiles(1, 2),
      WORKSPACE_COVERAGE_TEXT.tools(1, 0, 1),
      "1h ago",
    ]);
  });

  it("reads never run for a member with nothing ingested — never 0 %", async () => {
    await mount();
    const row = memberRow("idle");
    const [coverage, , tools] = figureCells(row);
    expect(coverage).toBe(WORKSPACE_COVERAGE_TEXT.neverRun);
    expect(row.textContent).not.toMatch(/0(\.0)?\s*%/);
    // Its failed tool is still stated: a run that ingested nothing is not no run.
    expect(tools).toBe(WORKSPACE_COVERAGE_TEXT.tools(0, 1, 1));
  });

  it("states a member whose every covered file changed as stale, not as a percentage", async () => {
    const [covered, ...rest] = REAL.coverage.members;
    const moved = {
      ...covered,
      result: { ...covered.result!, coverage: "stale" as const, overall_coverage_bp: null, fresh_files: 0 },
    };
    await mount(withCoverage({ members: [moved, ...rest] }));
    expect(figureCells(memberRow("covered"))[0]).toBe(WORKSPACE_COVERAGE_TEXT.stale);
  });

  it("states a member whose report matched no indexed file as such, not as stale", async () => {
    const [covered, ...rest] = REAL.coverage.members;
    const none = {
      ...covered,
      result: { ...covered.result!, coverage: "unmatched" as const, overall_coverage_bp: null, fresh_files: 0, total_files: 0 },
    };
    await mount(withCoverage({ members: [none, ...rest] }));
    expect(figureCells(memberRow("covered"))[0]).toBe(WORKSPACE_COVERAGE_TEXT.unmatched);
  });

  it("names a fresh member with no percentage in words, and a member with no run yet", async () => {
    const [covered, idle, broken] = REAL.coverage.members;
    const uncovered = { ...covered, result: { ...covered.result!, overall_coverage_bp: null, last_run: null } };
    await mount(withCoverage({ members: [uncovered, idle, broken] }));
    const [coverage, , , lastRun] = figureCells(memberRow("covered"));
    expect(coverage).toBe(WORKSPACE_COVERAGE_TEXT.noCoveredLine);
    expect(lastRun).toBe(WORKSPACE_COVERAGE_TEXT.noRun);
  });

  it("sorts a member with no percentage below every member that has one", async () => {
    await mount();
    const members = widgetTitled(document.body, "Members");
    const order = () =>
      within(members)
        .getAllByRole("row")
        .slice(1)
        .map((r) => within(r).getAllByRole("link")[0].textContent);
    fireEvent.click(within(members).getByRole("button", { name: /^Coverage/ }));
    expect(order().at(-1)).toBe("covered");
    fireEvent.click(within(members).getByRole("button", { name: /^Coverage/ }));
    expect(order()[0]).toBe("covered");
  });

  it("keeps an unopenable member as a named row with its reason, beside the others", async () => {
    await mount();
    const row = memberRow("broken");
    expect(figureCells(row)).toEqual(Array(4).fill(WORKSPACE_COVERAGE_TEXT.notRead));
    expect(within(row).getByText(/unable to open database file/)).toBeInTheDocument();
    // It hides neither other member.
    expect(memberRow("covered")).toBeInTheDocument();
    expect(memberRow("idle")).toBeInTheDocument();
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText(WORKSPACE_COVERAGE_TEXT.unread(1), { exact: false })).toBeInTheDocument();
  });

  it("links each member row to that member's Coverage page", async () => {
    await mount();
    for (const member of ["covered", "idle", "broken"]) {
      const link = within(memberRow(member)).getByRole("link", { name: member });
      expect(link.getAttribute("href")).toBe(`/coverage?repo=${member}`);
    }
    expect(memberCoverageHref("svc/a b")).toBe("/coverage?repo=svc%2Fa%20b");
  });

  it("states its denominator: members with fresh coverage of all, and the never-run count", async () => {
    await mount();
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText(WORKSPACE_COVERAGE_TEXT.denominator(1, 3, 1))).toBeInTheDocument();
    expect(WORKSPACE_COVERAGE_TEXT.denominator(1, 3, 1)).toBe("Coverage from 1 of 3 members; 1 member never run.");
    expect(within(members).getByText(WORKSPACE_COVERAGE_TEXT.perMember)).toBeInTheDocument();
  });

  it("states the figures of the payload, not a constant: two of four members fresh", async () => {
    await mount(withCoverage({ members_total: 4, members_fresh: 2, members_never_run: 0 }));
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText("Coverage from 2 of 4 members; 0 members never run.")).toBeInTheDocument();
  });

  it("names the absence when no member could be read", async () => {
    await mount(withCoverage({ members_read: 0, members_fresh: 0, members_never_run: 0 }));
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText(WORKSPACE_COVERAGE_ABSENCE.noneRead)).toBeInTheDocument();
    expect(within(members).queryByText(/^Coverage from/)).toBeNull();
  });

  it("offers no Run button: a run is started from the member's own page", async () => {
    await mount();
    // The page's only buttons are the table's sort headers.
    expect(screen.getAllByRole("button").map((b) => b.textContent?.replace("↕", "").trim())).toEqual([
      "Member",
      "Coverage",
      "Fresh files",
      "Tools",
      "Last run",
    ]);
    expect(screen.queryByRole("button", { name: /run coverage|cancel/i })).toBeNull();
    expect(screen.getByText(WORKSPACE_COVERAGE_TEXT.noRunHere)).toBeInTheDocument();
  });
});

describe("the frame", () => {
  it("heads the page with the workspace and how many members it answers for", async () => {
    await mount();
    expect(screen.getByText("measured")).toBeInTheDocument();
    expect(screen.getByText(WORKSPACE_COVERAGE_TEXT.heading(3), { exact: false })).toBeInTheDocument();
  });

  it("renders its widget in one stack, with its catalogue entry", async () => {
    const { container } = await mount();
    expectOneWidgetStack(container);
    expectWidgetCopy(widgetTitled(container, "Members"), memberCoverage);
  });

  it("reads the roll-up unscoped, once, even with a member selected", async () => {
    setScopedMember("idle");
    const { calls } = await mount();
    expect(calls().filter((u) => u.startsWith("/api/v1/workspace/coverage"))).toEqual(["/api/v1/workspace/coverage"]);
  });

  it("says this is not a workspace under a single-root serve", async () => {
    stub(REAL, 404);
    render(
      <WorkspaceProvider>
        <WorkspaceCoverageView />
      </WorkspaceProvider>,
    );
    expect(await screen.findByText(/Not a workspace/)).toBeInTheDocument();
  });
});
