/*
 * Workspace security (S-674, FR-SC-16, UAT-SC-03).
 *
 * The payload is the one `web/tests/workspace_api.rs` produced over its
 * three-member fixture (`fixtures/workspaceSecurity.real.json`, the temporary
 * path replaced): `scanned` — a fresh Semgrep report with one suppressed
 * finding and a stale report from an unregistered tool; `idle` — never
 * scanned; `broken` — a member nothing can open.
 *
 * The assertions are the acceptance criteria: a never-scanned member reads
 * "never run", never 0; an unopenable member is a named row that hides no
 * other; the totals say they count only fresh, unsuppressed findings and over
 * which members; each member row links to that member's Security page.
 */

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { WorkspaceRoster, WorkspaceSecurityAnswer } from "../../api/types.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import { FINDING_COUNT_TEXT, OWASP_TEXT } from "../../copy/security.copy.ts";
import {
  memberSecurity,
  WORKSPACE_SECURITY_ABSENCE,
  WORKSPACE_SECURITY_TEXT,
  workspaceOwasp,
} from "../../copy/workspaceSecurity.copy.ts";
import { SCOPED_ONLY_COUNTED, SCOPED_ONLY_EXCLUDED } from "../../test/scopedOnlyProject.ts";
import { expectOneWidgetStack, widgetTitled } from "../../test/widgetStack.ts";
import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../../workspace/scope.ts";
import real from "./fixtures/workspaceSecurity.real.json";
import { memberSecurityHref } from "./memberSecurityRow.tsx";
import { WorkspaceSecurityView } from "./WorkspaceSecurityView.tsx";

const REAL = real as unknown as WorkspaceSecurityAnswer;

const ROSTER: WorkspaceRoster = { workspace: "secure", default: "scanned", members: ["scanned", "idle", "broken"] };

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

/** Stub the roster probe (`probe` 404 ⇒ single-root) and the one read; return the
 *  URLs fetched. */
function stub(answer: WorkspaceSecurityAnswer, probe = 200): () => string[] {
  const calls: string[] = [];
  const json = (body: unknown, code = 200) =>
    Promise.resolve({ ok: code === 200, status: code, json: () => Promise.resolve(body) } as Response);
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      calls.push(url);
      if (url.startsWith("/api/v1/workspace/roster")) return json(ROSTER, probe);
      if (url.startsWith("/api/v1/workspace/security")) return json(answer);
      return json({});
    }),
  );
  return () => calls;
}

async function mount(answer: WorkspaceSecurityAnswer = REAL) {
  const calls = stub(answer);
  const utils = render(
    <WorkspaceProvider>
      <WorkspaceSecurityView />
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

/** The cells of `row` after the member and scanners columns: critical … info. */
function figureCells(row: HTMLElement): string[] {
  return within(row)
    .getAllByRole("cell")
    .slice(2)
    .map((c) => c.textContent ?? "");
}

/** `answer` with its roll-up fields replaced. */
function withSecurity(patch: Partial<WorkspaceSecurityAnswer["security"]>): WorkspaceSecurityAnswer {
  return { ...REAL, security: { ...REAL.security, ...patch } };
}

describe("the member roll-up (FR-SC-16)", () => {
  it("reads never run in every figure cell of a never-scanned member — never 0", async () => {
    await mount();
    expect(figureCells(memberRow("idle"))).toEqual(Array(6).fill(WORKSPACE_SECURITY_TEXT.neverRun));
  });

  it("states the scanned member's fresh, unsuppressed counts and its scanners' freshness", async () => {
    await mount();
    const row = memberRow("scanned");
    // critical, high, medium, low, info, unrated: the suppressed injection and the stale
    // tool's finding are not here.
    expect(figureCells(row)).toEqual(["0", "1", "1", "0", "0", "0"]);
    expect(within(row).getByText(WORKSPACE_SECURITY_TEXT.scannersFresh(1, 2))).toBeInTheDocument();
  });

  it("keeps an unopenable member as a named row with its reason, beside the others", async () => {
    await mount();
    const row = memberRow("broken");
    expect(figureCells(row)).toEqual(Array(6).fill(WORKSPACE_SECURITY_TEXT.notRead));
    expect(within(row).getByText(/unable to open database file/)).toBeInTheDocument();
    // It hides neither other member.
    expect(memberRow("scanned")).toBeInTheDocument();
    expect(memberRow("idle")).toBeInTheDocument();
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText(WORKSPACE_SECURITY_TEXT.unread(1), { exact: false })).toBeInTheDocument();
  });

  it("links each member row to that member's Security page", async () => {
    await mount();
    for (const member of ["scanned", "idle", "broken"]) {
      const link = within(memberRow(member)).getByRole("link", { name: member });
      expect(link.getAttribute("href")).toBe(`/security?repo=${member}`);
    }
    expect(memberSecurityHref("svc/a b")).toBe("/security?repo=svc%2Fa%20b");
  });

  it("says the totals count only fresh, unsuppressed findings, over which members, and what they left out", async () => {
    await mount();
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText(WORKSPACE_SECURITY_TEXT.totals(0, 1, 1, 0, 0, 0))).toBeInTheDocument();
    expect(WORKSPACE_SECURITY_TEXT.totals(0, 1, 1, 0, 0, 0)).toContain("fresh, unsuppressed findings only");
    expect(within(members).getByText(WORKSPACE_SECURITY_TEXT.denominator(1, 3, 1))).toBeInTheDocument();
    expect(within(members).getByText(WORKSPACE_SECURITY_TEXT.excluded(1, 1))).toBeInTheDocument();
  });

  it("puts each severity in its own column and its own place in the totals", async () => {
    // Six distinct figures, so no two columns or sentence slots can be swapped
    // unnoticed (the real payload's critical and low are both 0).
    const distinct = { critical: 3, high: 1, medium: 4, low: 2, info: 5, unrated: 6, total: 21 };
    const members = REAL.security.members.map((m) =>
      m.member === "scanned" && m.result ? { ...m, result: { ...m.result, findings: distinct } } : m,
    );
    await mount(withSecurity({ members, totals: distinct }));
    expect(figureCells(memberRow("scanned"))).toEqual(["3", "1", "4", "2", "5", "6"]);
    const headers = within(widgetTitled(document.body, "Members"))
      .getAllByRole("columnheader")
      .slice(2)
      .map((h) => h.textContent?.replace(/[↕↑↓]/g, "").trim());
    expect(headers).toEqual(["Critical", "High", "Medium", "Low", "Info", "Unrated"]);
    const sentence = within(widgetTitled(document.body, "Members")).getByText(/critical ·/).textContent;
    expect(sentence).toBe("3 critical · 1 high · 4 medium · 2 low · 5 info · 6 unrated — fresh, unsuppressed findings only");
  });

  it("counts a scoped-only member's findings by their own freshness, and states its reports apart (S-760)", async () => {
    const members = REAL.security.members.map((m) =>
      m.member === "scanned" && m.result
        ? {
            ...m,
            result: { ...m.result, scan: "stale" as const, reports_stored: 1, reports_fresh: 0, findings: SCOPED_ONLY_COUNTED, excluded: SCOPED_ONLY_EXCLUDED },
          }
        : m,
    );
    await mount(withSecurity({ members, totals: SCOPED_ONLY_COUNTED, excluded: SCOPED_ONLY_EXCLUDED, reports_stored: 1, reports_fresh: 0 }));
    // The figure the member's own page states, not "no fresh scan".
    expect(figureCells(memberRow("scanned"))).toEqual(["0", "0", "2", "0", "0", "0"]);
    const { critical, high, medium, low, info, unrated } = SCOPED_ONLY_COUNTED;
    const widget = widgetTitled(document.body, "Members");
    expect(within(widget).getByText(FINDING_COUNT_TEXT.totals(critical, high, medium, low, info, unrated))).toBeInTheDocument();
    expect(within(widget).getByText(WORKSPACE_SECURITY_TEXT.reportsFresh(0, 1))).toBeInTheDocument();
  });

  it("states the test-code remainder on the figure row, the payload's own sum, left out of every total (S-796)", async () => {
    await mount(withSecurity({ excluded: { not_fresh: 3, suppressed: 1, test: 7, dev: 0, needs_review: 0 } }));
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText("· 7 in test code, left out of every total above.")).toBeInTheDocument();
    // The totals line itself is unchanged: the remainder is its own line.
    expect(within(members).getByText(WORKSPACE_SECURITY_TEXT.totals(0, 1, 1, 0, 0, 0))).toBeInTheDocument();
  });

  it("states the needs-review remainder on its own line, the payload's own sum, left out of every total (S-817)", async () => {
    await mount(withSecurity({ excluded: { not_fresh: 0, suppressed: 0, test: 0, dev: 0, needs_review: 5 } }));
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText("· 5 more need review, left out of every total above.")).toBeInTheDocument();
    expect(within(members).getByText("· 0 in test code, left out of every total above.")).toBeInTheDocument();
  });

  it("states the dev-only remainder on its own line, the payload's own sum, left out of every total (S-810)", async () => {
    await mount(withSecurity({ excluded: { not_fresh: 0, suppressed: 0, test: 0, dev: 60, needs_review: 0 } }));
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText("· 60 more in dev-only dependencies, left out of every total above.")).toBeInTheDocument();
    expect(within(members).getByText("· 0 in test code, left out of every total above.")).toBeInTheDocument();
  });

  it("states a remainder of none as none, never as nothing said (S-796)", async () => {
    await mount(withSecurity({ excluded: { not_fresh: 0, suppressed: 0, test: 0, dev: 0, needs_review: 0 } }));
    expect(within(widgetTitled(document.body, "Members")).getByText("· 0 in test code, left out of every total above.")).toBeInTheDocument();
  });

  it("puts each left-out figure in its own place, in the spelling the manual gives (S-760)", async () => {
    await mount(withSecurity({ excluded: { not_fresh: 3, suppressed: 1, test: 0, dev: 0, needs_review: 0 } }));
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText("Left out: 3 findings in files changed since the scan, and 1 suppressed.")).toBeInTheDocument();
  });

  it("says the OWASP counts rest on fresh reports only, not on the members counted (S-760)", async () => {
    await mount();
    const owasp = widgetTitled(document.body, "OWASP Top 10");
    expect(
      within(owasp).getByText("Category counts come from fresh reports only: 1 of 2 reports fresh at HEAD."),
    ).toBeInTheDocument();
    expect(within(owasp).queryByText(WORKSPACE_SECURITY_TEXT.denominator(1, 3, 1))).toBeNull();
  });

  it("states how many reports are fresh as its own field beside the totals (S-760)", async () => {
    await mount();
    const members = widgetTitled(document.body, "Members");
    expect(WORKSPACE_SECURITY_TEXT.reportsFresh(1, 2)).toBe("1 of 2 reports fresh at HEAD.");
    expect(within(members).getByText(WORKSPACE_SECURITY_TEXT.reportsFresh(1, 2))).toBeInTheDocument();
    expect(WORKSPACE_SECURITY_TEXT.reportsFresh(0, 1)).toBe("0 of 1 report fresh at HEAD.");
  });

  it("shows a member whose only findings are info, never a row of zeros", async () => {
    const infoOnly = { critical: 0, high: 0, medium: 0, low: 0, info: 2, unrated: 0, total: 2 };
    const members = REAL.security.members.map((m) =>
      m.member === "scanned" && m.result ? { ...m, result: { ...m.result, findings: infoOnly } } : m,
    );
    await mount(withSecurity({ members, totals: infoOnly }));
    expect(figureCells(memberRow("scanned"))).toEqual(["0", "0", "0", "0", "2", "0"]);
    const widget = widgetTitled(document.body, "Members");
    expect(within(widget).getByText(WORKSPACE_SECURITY_TEXT.totals(0, 0, 0, 0, 2, 0))).toBeInTheDocument();
    expect(WORKSPACE_SECURITY_TEXT.totals(0, 0, 0, 0, 2, 0)).toContain("2 info");
  });

  it("shows a member whose only findings are unrated in their own column, never as medium (S-768)", async () => {
    const unratedOnly = { critical: 0, high: 0, medium: 0, low: 0, info: 0, unrated: 3, total: 3 };
    const members = REAL.security.members.map((m) =>
      m.member === "scanned" && m.result ? { ...m, result: { ...m.result, findings: unratedOnly } } : m,
    );
    await mount(withSecurity({ members, totals: unratedOnly }));
    expect(figureCells(memberRow("scanned"))).toEqual(["0", "0", "0", "0", "0", "3"]);
    const sentence = within(widgetTitled(document.body, "Members")).getByText(/critical ·/).textContent;
    expect(sentence).toBe("0 critical · 0 high · 0 medium · 0 low · 0 info · 3 unrated — fresh, unsuppressed findings only");
  });

  it("states the absence when no member has a stored scan, rather than totals of 0", async () => {
    const zero = { critical: 0, high: 0, medium: 0, low: 0, info: 0, unrated: 0, total: 0 };
    await mount(withSecurity({ members_counted: 0, totals: zero }));
    const members = widgetTitled(document.body, "Members");
    expect(within(members).getByText(WORKSPACE_SECURITY_ABSENCE.noneScanned)).toBeInTheDocument();
    expect(within(members).queryByText(WORKSPACE_SECURITY_TEXT.totals(0, 0, 0, 0, 0, 0))).toBeNull();
  });

  it("states that nothing is summed when no member could be read", async () => {
    await mount(withSecurity({ members_read: 0, members_counted: 0, members_never_scanned: 0, owasp: [] }));
    for (const title of ["Members", "OWASP Top 10"]) {
      expect(within(widgetTitled(document.body, title)).getByText(WORKSPACE_SECURITY_ABSENCE.noneRead)).toBeInTheDocument();
    }
  });

  it("offers no scan: a scan is started from a member's own Security page", async () => {
    await mount();
    // A scan action ("Scan", "Run scan", "Install & scan") — not the "Scanners
    // fresh" column's sort control, which this pattern must not match.
    expect(screen.queryByRole("button", { name: /(^|\s)scan\b/i })).toBeNull();
    expect(screen.getByRole("button", { name: /Scanners fresh/ })).toBeInTheDocument();
    expect(screen.getByText(WORKSPACE_SECURITY_TEXT.noScanHere)).toBeInTheDocument();
  });
});

describe("the OWASP roll-up", () => {
  /** The OWASP table row for category `id`. */
  function categoryRow(id: string): HTMLElement {
    const row = within(widgetTitled(document.body, "OWASP Top 10")).getByText(id).closest("tr");
    expect(row).not.toBeNull();
    return row as HTMLElement;
  }

  it("counts a covered category and says in how many members read it is covered", async () => {
    await mount();
    const injection = categoryRow("A05:2025");
    expect(within(injection).getByText("1")).toBeInTheDocument();
    expect(within(injection).getByText(WORKSPACE_SECURITY_TEXT.coveredIn(1, 2))).toBeInTheDocument();
  });

  it("reads a covered category with no finding as 0, not as not covered", async () => {
    await mount();
    // Covered by a fresh scan that found nothing: a clean result, which is not
    // the blind spot "not covered" names (BR-66).
    const accessControl = categoryRow("A01:2025");
    expect(within(accessControl).getByText("0")).toBeInTheDocument();
    expect(within(accessControl).queryByText(WORKSPACE_SECURITY_TEXT.notCovered)).toBeNull();
    expect(within(accessControl).getByText(WORKSPACE_SECURITY_TEXT.coveredIn(1, 2))).toBeInTheDocument();
  });

  it("reads a category no member covers as not covered, never 0", async () => {
    await mount();
    const supplyChain = categoryRow("A03:2025");
    expect(within(supplyChain).getByText(WORKSPACE_SECURITY_TEXT.notCovered)).toBeInTheDocument();
    expect(within(supplyChain).queryByText("0")).toBeNull();
  });

  it("states the counted findings under no category, as the member page does, per edition", async () => {
    await mount();
    const owasp = widgetTitled(document.body, "OWASP Top 10");
    expect(within(owasp).queryByText(/no OWASP category/)).toBeNull();

    cleanup();
    const [current, older] = REAL.security.owasp;
    await mount(withSecurity({ owasp: [{ ...current, unclassified: 3 }, older] }));
    expect(within(widgetTitled(document.body, "OWASP Top 10")).getByText(OWASP_TEXT.unclassified(3))).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "2021" }));
    expect(within(widgetTitled(document.body, "OWASP Top 10")).queryByText(/no OWASP category/)).toBeNull();
  });

  it("switches edition from the same payload, without a refetch", async () => {
    const { calls } = await mount();
    fireEvent.click(screen.getByRole("button", { name: "2021" }));
    expect(within(categoryRow("A03:2021")).getByText("1")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "2021" }).getAttribute("aria-pressed")).toBe("true");
    expect(calls().filter((u) => u.startsWith("/api/v1/workspace/security"))).toHaveLength(1);
  });
});

describe("the frame", () => {
  it("renders its two widgets in one stack, each with its catalogue entry", async () => {
    const { container } = await mount();
    expectOneWidgetStack(container);
    expectWidgetCopy(widgetTitled(container, "Members"), memberSecurity);
    expectWidgetCopy(widgetTitled(container, "OWASP Top 10"), workspaceOwasp);
  });

  it("reads the roll-up unscoped, once", async () => {
    const { calls } = await mount();
    expect(calls().filter((u) => u.startsWith("/api/v1/workspace/security"))).toEqual(["/api/v1/workspace/security"]);
  });

  it("says this is not a workspace under a single-root serve", async () => {
    stub(REAL, 404);
    render(
      <WorkspaceProvider>
        <WorkspaceSecurityView />
      </WorkspaceProvider>,
    );
    expect(await screen.findByText(/Not a workspace/)).toBeInTheDocument();
  });
});
