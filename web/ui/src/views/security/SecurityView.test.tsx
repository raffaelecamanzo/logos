// SecurityView over a mocked `/api/v1/security` (S-670, FR-SC-09): the absent
// states name their fixing commands, no snippet is rendered and a secret-class
// message is explained in words, the edition switch re-classifies without a refetch,
// `?path=` narrows the table, and the detail states reachability honestly.
// Pinned to the real payload `web/tests/api_v1.rs` produced.
import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ImpactResult, ScanJob, SecurityEntryStatus, SecurityFinding, SecurityGateBand, SecurityModel, SecurityPage, SecurityScannerStatus } from "../../api/types.ts";
import { expectWidgetCopy, readerText } from "../../copy/expectWidgetCopy.ts";
import {
  ANCHOR_TEXT,
  CONTEXT_TEXT,
  COPY_TEXT,
  EVIDENCE_TEXT,
  FINDING_COUNT_TEXT,
  FINDINGS_CELL_TEXT,
  FINDINGS_TABLE_TEXT,
  IGNORED_DIRS_LEGEND,
  IGNORED_DIRS_TEXT,
  INSTALL_JOB_TEXT,
  LIVE_STATE_TEXT,
  NVD_KEY_TEXT,
  OWASP_TEXT,
  REDACTED_TEXT,
  REPORTERS_TEXT,
  SCAN_JOB_TEXT,
  SCOPE_TEXT,
  SECURITY_ABSENCE,
  SCANNER_STATE_TEXT,
  SECURITY_GATE_TEXT,
  SUPPRESS_TEXT,
  SUPPRESSIONS_TEXT,
  UNCLASSIFIED_TEXT,
  findingDetail,
  owaspView,
  scannerMatrix,
  securityFindings,
  securityGate,
} from "../../copy/security.copy.ts";
import { SCOPED_ONLY_COUNTED, SCOPED_ONLY_EXCLUDED } from "../../test/scopedOnlyProject.ts";
import { expectOneWidgetStack, widgetTitle, widgetTitled } from "../../test/widgetStack.ts";
import real from "./fixtures/security.real.json";
import scanJob from "./fixtures/scan-job.real.json";
import { FindingDetail } from "./FindingDetail.tsx";
import { SecurityView } from "./SecurityView.tsx";
import styles from "./SecurityView.module.css";

const REAL = real as unknown as SecurityModel & { gate_band: SecurityGateBand };
const PLANTED = "PLANTED-snippet-text-0123456789";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
});

/** The page with `over` applied to its `security` part. */
function model(over: Partial<SecurityPage> = {}): SecurityModel {
  return { status: REAL.status, security: { ...REAL.security, ...over } };
}

const IMPACT: ImpactResult = {
  query: "find",
  resolved: { symbol: "logos . project . src/`lib.rs`/find().", name: "find", kind: "function", file: "src/lib.rs", line: 2 },
  depth: 3,
  upstream_label: "Depends on it",
  upstream: [{ symbol: "s:caller", name: "caller", kind: "function", file: "src/main.rs", line: 4, distance: 1 }],
  downstream_label: "It depends on",
  downstream: [],
  docs_label: "Docs",
  docs: [],
  suggestions: [],
  warnings: [],
};

/** Stubs fetch: `/api/v1/security` answers `page`, `/api/v1/impact` answers
 *  `impact` (default {@link IMPACT}). Returns the URLs fetched, in order. */
function stub(page: SecurityModel, impact: ImpactResult = IMPACT): string[] {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      // The Scanners widget's own job reads (S-673, S-747) are not the page's reads.
      if (url.startsWith("/api/v1/security/scan") || url.startsWith("/api/v1/security/install")) {
        return Promise.resolve({ ok: true, json: () => Promise.resolve({ job: null }) } as Response);
      }
      calls.push(url);
      const body = url.startsWith("/api/v1/impact") ? impact : page;
      return Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response);
    }),
  );
  return calls;
}

async function mount(page: SecurityModel, impact: ImpactResult = IMPACT) {
  const calls = stub(page, impact);
  const view = render(<SecurityView />);
  await screen.findByRole("heading", { name: "Scanners" });
  return { calls, root: view.container };
}

/** A finding with the stored-snapshot fields left as they are. */
function finding(rule: string): SecurityFinding {
  return REAL.security.findings.findings.find((f) => f.rule_id === rule) as SecurityFinding;
}

/** `path:line` as the page spells a finding's location. */
function findingLocationOf(f: SecurityFinding): string {
  return `${f.path ?? f.report_path}:${f.start_line}`;
}

describe("SecurityView over the real payload", () => {
  it("renders one stack of catalogued widgets, the gate band left out without a table", async () => {
    const { root } = await mount(model());
    const widgets = expectOneWidgetStack(root);
    expect(widgets.map(widgetTitle)).toEqual(["Scanners", "Findings", "OWASP Top 10", "Finding detail"]);
    expectWidgetCopy(widgetTitled(root, "Scanners"), scannerMatrix);
    expectWidgetCopy(widgetTitled(root, "Findings"), securityFindings);
    expectWidgetCopy(widgetTitled(root, "OWASP Top 10"), owaspView);
    expectWidgetCopy(widgetTitled(root, "Finding detail"), findingDetail);
  });

  it("states each absent scanner with its fixing command: not installed, never run", async () => {
    const { root } = await mount(model());
    const matrix = widgetTitled(root, "Scanners");
    const rowOf = (name: string) => within(matrix).getByText(name, { selector: "td" }).closest("tr") as HTMLElement;
    expect(readerText(rowOf("trivy"))).toContain("Not installed; run logos security install trivy.");
    expect(readerText(rowOf("codeql"))).toContain("logos security ingest <report>");
    expect(readerText(rowOf("bandit"))).toContain("Installed but never run; run logos security scan --consent.");
    expect(readerText(rowOf("semgrep"))).toContain("2 findings");
  });

  it("reads Scanner · State · Version · Findings · Ignored dirs · Action, the command under the badge of each absent state", async () => {
    const { root } = await mount(model());
    const matrix = widgetTitled(root, "Scanners");
    // A sortable header carries its sort control's text after its name.
    expect(within(matrix).getAllByRole("columnheader").map((h) => h.textContent?.replace(/[^A-Za-z ]+$/, "").trim())).toEqual([
      "Scanner",
      "State",
      "Version",
      "Findings",
      "Ignored dirs",
      "Action",
    ]);
    const cell = (name: string, column: string) => {
      const row = within(matrix).getByText(name, { selector: "td" }).closest("tr") as HTMLElement;
      const at = within(matrix).getAllByRole("columnheader").findIndex((h) => h.textContent?.startsWith(column));
      return row.querySelectorAll("td")[at] as HTMLElement;
    };
    // The fixing command sits in the State cell, beside the badge, as its own muted line.
    expect(cell("trivy", "State").querySelector("p")?.textContent).toBe("Not installed; run logos security install trivy.");
    expect(cell("bandit", "State").querySelector("p")?.textContent).toBe("Installed but never run; run logos security scan --consent.");
    expect(cell("semgrep", "State").querySelector("p"), "a fresh row has nothing to fix").toBeNull();
    // `never run` and `0 findings` stay different words in the Findings cell.
    expect(readerText(cell("bandit", "Findings"))).toBe(FINDINGS_CELL_TEXT.neverRun);
    expect(readerText(cell("semgrep", "Findings"))).toBe("2 findings");
    expect(readerText(cell("trivy", "Findings"))).toBe(FINDINGS_CELL_TEXT.none);
    // The "What it means" and "Copy" columns are gone: no sentence repeats the badge.
    expect(within(matrix).queryByRole("columnheader", { name: /What it means|Copy/ })).toBeNull();
  });

  it("states each installed row's copy and an update, and offers Install and Update on the rows they fix (S-747)", async () => {
    const [semgrep, trivy, gitleaks, osv, bandit, ...rest] = REAL.security.status.scanners;
    const managed = { copy: "managed", binary: "/home/user/.local/share/tools/gitleaks/8.18.4/gitleaks", version: "8.18.4", update_available: "8.19.0" } as const;
    const override = { copy: "override", binary: "/opt/osv-scanner", version: "1.9.2", update_available: null } as const;
    const scanners = [
      semgrep,
      trivy,
      { ...gitleaks, state: "never_run", installed: managed, install: null },
      { ...osv, state: "never_run", installed: override, install: null },
      bandit,
      ...rest,
    ];
    const { root } = await mount(model({ status: { ...REAL.security.status, scanners } as SecurityPage["status"] }));
    const matrix = widgetTitled(root, "Scanners");
    const rowOf = (name: string) => within(matrix).getByText(name, { selector: "td" }).closest("tr") as HTMLElement;
    // The copy in use shows in the Version cell, after the version.
    expect(readerText(rowOf("gitleaks"))).toContain(`8.18.4 · ${COPY_TEXT.managed} ${COPY_TEXT.updateAvailable("8.19.0")}`);
    expect(readerText(rowOf("osv-scanner"))).toContain(`1.9.2 · ${COPY_TEXT.override}`);
    expect(readerText(rowOf("bandit"))).toContain(`· ${COPY_TEXT.path}`);
    expect(within(rowOf("trivy")).getByRole("button", { name: "Install trivy" })).toBeInTheDocument();
    expect(within(rowOf("gitleaks")).getByRole("button", { name: "Update gitleaks" })).toBeInTheDocument();
    // A copy that is not Logos's, or at its pin, has nothing to install or update;
    // a scanner Logos cannot install here offers no Install.
    for (const name of ["osv-scanner", "bandit", "semgrep", "codeql"]) {
      expect(within(rowOf(name)).queryByRole("button"), name).toBeNull();
    }
    // While a default scanner is missing, the title row's action is Install & scan.
    expect(within(widgetTitled(root, "Scanners")).getByRole("button", { name: INSTALL_JOB_TEXT.installAndScan })).toBeInTheDocument();
  });

  it("marks each scanner's coverage of the ignored directories from its kind, with the note as tooltip and name, and a legend of exactly the markers shown (FR-SC-26)", async () => {
    const [semgrep, trivy, gitleaks, osv, bandit, ...rest] = REAL.security.status.scanners;
    const note = (text: string) => text;
    const scanners = [
      { ...semgrep, ignored_paths: "excluded" },
      trivy,
      { ...gitleaks, ignored_paths: "scanned", coverage_kind: "history", coverage_note: note("it reads the committed history, which holds any file committed inside an ignored directory") },
      // The note's words never pick the marker: this kind reads as the whole project.
      { ...osv, ignored_paths: "scanned", coverage_kind: "whole_project", coverage_note: note("exclusion flag not proven by the harness") },
      { ...bandit, ignored_paths: "scanned", coverage_kind: "unproven", coverage_note: note("exclusion flag not proven by the harness") },
      ...rest,
    ];
    const { root } = await mount(model({ status: { ...REAL.security.status, scanners } as SecurityPage["status"] }));
    const matrix = widgetTitled(root, "Scanners");
    expect(within(matrix).getByRole("columnheader", { name: "Ignored dirs" })).toBeInTheDocument();
    const cellOf = (name: string) => {
      const row = within(matrix).getByText(name, { selector: "td" }).closest("tr") as HTMLElement;
      const header = within(matrix).getAllByRole("columnheader").findIndex((h) => h.textContent === "Ignored dirs");
      return row.querySelectorAll("td")[header] as HTMLElement;
    };
    expect(readerText(cellOf("semgrep"))).toBe(IGNORED_DIRS_TEXT.skipped.marker);
    expect(readerText(cellOf("gitleaks"))).toBe(IGNORED_DIRS_TEXT.history.marker);
    expect(readerText(cellOf("osv-scanner"))).toBe(IGNORED_DIRS_TEXT.whole_project.marker);
    expect(readerText(cellOf("bandit"))).toBe(IGNORED_DIRS_TEXT.unproven.marker);
    // Not installed here: no whole-project run to state, never a guess.
    expect(readerText(cellOf("trivy"))).toBe(IGNORED_DIRS_TEXT.none.marker);
    // The full note is the marker's tooltip and its accessible name.
    const marker = within(cellOf("gitleaks")).getByRole("note", {
      name: `${IGNORED_DIRS_TEXT.history.marker}: it reads the committed history, which holds any file committed inside an ignored directory`,
    });
    expect(marker.getAttribute("title")).toBe("it reads the committed history, which holds any file committed inside an ignored directory");
    // The legend lists each marker shown once, in plain words, and no other.
    const legend = within(matrix).getByRole("list", { name: IGNORED_DIRS_LEGEND });
    expect(within(legend).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      `${IGNORED_DIRS_TEXT.skipped.marker}: ${IGNORED_DIRS_TEXT.skipped.means}`,
      `${IGNORED_DIRS_TEXT.unproven.marker}: ${IGNORED_DIRS_TEXT.unproven.means}`,
      `${IGNORED_DIRS_TEXT.history.marker}: ${IGNORED_DIRS_TEXT.history.means}`,
      `${IGNORED_DIRS_TEXT.whole_project.marker}: ${IGNORED_DIRS_TEXT.whole_project.means}`,
      `${IGNORED_DIRS_TEXT.none.marker}: ${IGNORED_DIRS_TEXT.none.means}`,
    ]);
  });

  it("lists a marker in the legend only while a row shows it, and `partly skipped` when gosec was not given every directory", async () => {
    const [semgrep, ...rest] = REAL.security.status.scanners;
    const partial = { ...semgrep, ignored_paths: "scanned", coverage_kind: "partial", coverage_note: "it is not given 2 ignored directories past the 64 KiB argument budget" };
    const installed = (row: (typeof rest)[number]) => ({ ...row, state: "never_run", installed: { copy: "path", binary: "/bin/x", version: "1", update_available: null }, install: null });
    const scanners = [partial, ...rest.map((r) => ({ ...installed(r), ignored_paths: "excluded" }))];
    const { root } = await mount(model({ status: { ...REAL.security.status, scanners } as SecurityPage["status"] }));
    const legend = within(widgetTitled(root, "Scanners")).getByRole("list", { name: IGNORED_DIRS_LEGEND });
    expect(within(legend).getAllByRole("listitem").map((li) => li.querySelector(".mono")?.textContent)).toEqual([
      IGNORED_DIRS_TEXT.skipped.marker,
      IGNORED_DIRS_TEXT.partial.marker,
    ]);
  });

  it("reads a stale row's count, version and failure, and a row without a kind as `scanned` with its note (S-792)", async () => {
    const [semgrep, trivy, gitleaks, osv, ...rest] = REAL.security.status.scanners;
    const installed = (copy: string, version: string, update: string | null) => ({ copy, binary: "/bin/x", version, update_available: update });
    const scanners = [
      // Stale: its count and version come from the stored report, and its failure is still disclosed.
      {
        ...semgrep,
        state: "stale",
        because: "head_moved",
        version: "1.85.0",
        findings: 2,
        suppressed: 0,
        installed: installed("path", "1.90.0", null),
        last_failure: ".logos/security/semgrep.failed.log",
      },
      trivy,
      // A path copy is never offered an Update, whatever a pin says.
      { ...gitleaks, state: "never_run", installed: installed("path", "8.18.4", "9.9.9"), install: null },
      // A server that sent a note and no kind: a neutral marker, its note as tooltip and name.
      { ...osv, ignored_paths: "scanned", coverage_note: "read without a kind" },
      ...rest,
    ];
    const { root } = await mount(model({ status: { ...REAL.security.status, scanners } as SecurityPage["status"] }));
    const matrix = widgetTitled(root, "Scanners");
    const cell = (name: string, column: string) => {
      const row = within(matrix).getByText(name, { selector: "td" }).closest("tr") as HTMLElement;
      const at = within(matrix).getAllByRole("columnheader").findIndex((h) => h.textContent?.startsWith(column));
      return row.querySelectorAll("td")[at] as HTMLElement;
    };
    expect(readerText(cell("semgrep", "Findings"))).toContain("2 findings");
    expect(within(cell("semgrep", "Findings")).getByRole("button", { name: SCANNER_STATE_TEXT.lastFailed })).toBeInTheDocument();
    expect(readerText(cell("semgrep", "Version"))).toContain("1.85.0 · path");
    expect(within(cell("gitleaks", "Action")).queryByRole("button")).toBeNull();
    expect(readerText(cell("osv-scanner", "Ignored dirs"))).toBe(IGNORED_DIRS_TEXT.scanned.marker);
    expect(within(cell("osv-scanner", "Ignored dirs")).getByRole("note").getAttribute("title")).toBe("read without a kind");
    const legend = within(matrix).getByRole("list", { name: IGNORED_DIRS_LEGEND });
    expect(within(legend).getAllByRole("listitem").map((li) => li.querySelector(".mono")?.textContent)).toContain(IGNORED_DIRS_TEXT.scanned.marker);
  });

  it("reads a scanner with nothing to scan as not applicable, and a failed last scan as LAST SCAN FAILED that expands to its cause and log (S-769)", async () => {
    const scanners = REAL.security.status.scanners.map((row) => {
      if (row.scanner === "gosec") {
        return { ...row, state: "not_applicable", reason: "no Go source file (.go) among the files the project does not ignore", installed: null, install: null };
      }
      if (row.scanner === "dependency-check") {
        return { ...row, last_failure: ".logos/security/dependency-check.failed.log", last_failure_cause: "[ERROR] Error updating the NVD Data" };
      }
      return row;
    });
    const { root } = await mount(model({ status: { ...REAL.security.status, scanners } as SecurityPage["status"] }));
    const matrix = widgetTitled(root, "Scanners");
    const cellOf = (name: string, column: string) => {
      const row = within(matrix).getByText(name, { selector: "td" }).closest("tr") as HTMLElement;
      // A sortable header carries its sort control's text after its name.
      const header = within(matrix).getAllByRole("columnheader").findIndex((h) => h.textContent?.startsWith(column));
      return row.querySelectorAll("td")[header] as HTMLElement;
    };
    expect(readerText(cellOf("gosec", "State"))).toBe("not applicableNothing to scan: no Go source file (.go) among the files the project does not ignore.");
    // The label is a collapsed disclosure; the cause and the log are behind it.
    const failed = within(cellOf("dependency-check", "Findings")).getByRole("button", { name: SCANNER_STATE_TEXT.lastFailed });
    expect(failed.getAttribute("aria-expanded")).toBe("false");
    expect(readerText(cellOf("dependency-check", "Findings"))).not.toContain("NVD");
    await userEvent.click(failed);
    expect(failed.getAttribute("aria-expanded")).toBe("true");
    expect(readerText(cellOf("dependency-check", "Findings"))).toContain(
      "[ERROR] Error updating the NVD Data — the output is in .logos/security/dependency-check.failed.log.",
    );
    await userEvent.click(failed);
    expect(readerText(cellOf("dependency-check", "Findings"))).not.toContain("NVD");
    // A row without a failure log says nothing of one.
    expect(within(cellOf("semgrep", "Findings")).queryByRole("button")).toBeNull();
  });

  it("states on Dependency-Check's row alone whether it has an NVD key, never the key (S-794)", async () => {
    for (const [state, text] of [["set", NVD_KEY_TEXT.set], ["not_set", NVD_KEY_TEXT.not_set]] as const) {
      const scanners = REAL.security.status.scanners.map((row) => (row.scanner === "dependency-check" ? { ...row, nvd_api_key: state } : row));
      const { root } = await mount(model({ status: { ...REAL.security.status, scanners } as SecurityPage["status"] }));
      const matrix = widgetTitled(root, "Scanners");
      const scannerCell = (name: string) => within(matrix).getByText(name, { selector: "td" });
      expect(readerText(scannerCell("dependency-check"))).toBe(`dependency-check${text}`);
      const line = within(scannerCell("dependency-check")).getByText(text);
      expect(line.getAttribute("title")).toBe(state === "not_set" ? NVD_KEY_TEXT.hint : null);
      expect(readerText(scannerCell("semgrep"))).toBe("semgrep");
      expect(within(matrix).getAllByText(/NVD key/)).toHaveLength(1);
      cleanup();
    }
  });

  it("starts a row's Install and Update through the install endpoint, with the intent token (S-747)", async () => {
    const sent: { url: string; body: unknown; intent: string | null }[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init: RequestInit = {}) => {
        if ((init.method ?? "GET") === "POST") {
          sent.push({ url, body: JSON.parse(init.body as string), intent: new Headers(init.headers).get("x-logos-intent") });
          return Promise.resolve({ ok: false, status: 409, json: () => Promise.resolve({ error: "consent first", plan: { lines: [], kept: [], scan_consent: [] } }) } as Response);
        }
        const body = url.startsWith("/api/v1/security/") ? { job: null } : model();
        return Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response);
      }),
    );
    const { container } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });
    await userEvent.click(within(widgetTitled(container, "Scanners")).getByRole("button", { name: "Install trivy" }));
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toEqual({ url: "/api/v1/security/install", body: { tools: ["trivy"], update: false }, intent: "test-intent-token" });
  });

  it("renders a report stored under a registry name as its own row, without a key collision", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const lowercase = { scanner: "semgrep", driver: "semgrep", state: "fresh", snapshot_head: "a", version: "0.1", findings: 0, suppressed: 0, installed: null, install: null, last_failure: null } as const;
    const { root } = await mount(model({ status: { ...REAL.security.status, other_reports: [lowercase] } }));
    expect(within(widgetTitled(root, "Scanners")).getAllByText("semgrep", { selector: "td" })).toHaveLength(2);
    expect(errors.mock.calls.flat().join(" ")).not.toMatch(/same key/);
    errors.mockRestore();
  });

  it("states a not-covered category with the command that covers it — never a count", async () => {
    const { root } = await mount(model());
    const owasp = widgetTitled(root, "OWASP Top 10");
    const supplyChain = within(owasp).getByText("A03:2025").closest("tr") as HTMLElement;
    expect(readerText(supplyChain)).toContain("Not covered: no scanner that ran can detect this category; run logos security scan --install --consent.");
    expect(readerText(supplyChain)).not.toMatch(/\b0 findings\b/);
    const injection = within(owasp).getByText("A05:2025").closest("tr") as HTMLElement;
    expect(readerText(injection)).toContain("1 finding");
  });

  it("leads with the findings count", async () => {
    await mount(model());
    const callout = screen.getAllByRole("status").find((el) => el.textContent?.startsWith("SECURITY"));
    expect(callout?.textContent).toContain("2 findings from 1 report");
  });

  it("tells a clean scan from no scan: a stored report with nothing found is not 'no scan yet'", async () => {
    const clean = model({ findings: { ...REAL.security.findings, findings: [], total: 0 } });
    const { root } = await mount(clean);
    const widget = readerText(widgetTitled(root, "Findings"));
    expect(widget).toContain(SECURITY_ABSENCE.clean);
    expect(widget).not.toContain(SECURITY_ABSENCE.noScan);
    expect(screen.queryByRole("heading", { name: "Finding detail" })).toBeNull();
  });

  it("states that no scan has run, with its fixing command, when nothing is stored", async () => {
    const empty = model({
      findings: { ...REAL.security.findings, scanned: [], findings: [], total: 0, notice: "no security report is stored" },
    });
    const { root } = await mount(empty);
    expect(readerText(widgetTitled(root, "Findings"))).toContain(SECURITY_ABSENCE.noScan);
    expect(SECURITY_ABSENCE.noScan).toContain("logos security scan");
    // Nothing stored, so nothing to detail.
    expect(screen.queryByRole("heading", { name: "Finding detail" })).toBeNull();
  });
});

describe("no snippet, and secrets redacted (FR-SC-03, ADR-79 §2, S-796)", () => {
  const WHY = "Redacted: the scanner's message would contain the secret itself";

  it("explains a secret-class message in the detail with its location, and never shows a snippet", async () => {
    const secret = { ...finding("generic.secrets.hardcoded-key"), message: `key=${PLANTED}` };
    // A snippet the server never sends: proof the page renders no such field.
    const withSnippet = { ...finding("rust.lang.security.formatted-sql"), snippet: PLANTED } as SecurityFinding;
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [withSnippet, secret] } }));
    expect(root.textContent).not.toContain(PLANTED);
    const table = within(widgetTitled(root, "Findings")).getByRole("table");
    // No row carries a message — redacted or not (§4.23): it is the detail's.
    expect(table.textContent).not.toContain(WHY);
    expect(table.textContent).not.toContain("Formatted SQL query.");

    await userEvent.click(screen.getByRole("button", { name: /Show generic\.secrets\.hardcoded-key/ }));
    const detail = widgetTitled(root, "Finding detail");
    expect(within(detail).getByText(`${WHY}, at ${findingLocationOf(secret)}`)).toBeInTheDocument();
    expect(root.textContent).not.toContain("[redacted]");
    expect(root.textContent).not.toContain(PLANTED);
    expect(root.querySelector("pre, code[class*='snippet']")).toBeNull();
    // Another finding's detail shows its message, and not the redaction.
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    const other = widgetTitled(root, "Finding detail");
    expect(within(other).getByText("Formatted SQL query.")).toBeInTheDocument();
    expect(other.textContent).not.toContain(WHY);
  });

  it("gives a secret found only in history its location in the explanation", async () => {
    const secret = finding("generic.secrets.hardcoded-key");
    const historical = { ...secret, binding: "historical" as const, path: null, report_path: "legacy/client.py", start_line: 1 };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [historical] } }));
    await userEvent.click(screen.getByRole("button", { name: /Show generic\.secrets\.hardcoded-key/ }));
    expect(within(widgetTitled(root, "Finding detail")).getByText(`${WHY}, at legacy/client.py:1`)).toBeInTheDocument();
  });

  it("reads the explanation for every redacted finding, each with its own location", async () => {
    const secret = finding("generic.secrets.hardcoded-key");
    const second = { ...secret, fingerprint: "second", path: "src/other.rs", report_path: "src/other.rs", start_line: 41 };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [secret, second] } }));
    for (const f of [secret, second]) {
      await userEvent.click(screen.getByRole("button", { name: `Show ${f.rule_id} at ${findingLocationOf(f)}` }));
      expect(within(widgetTitled(root, "Finding detail")).getByText(`${WHY}, at ${findingLocationOf(f)}`)).toBeInTheDocument();
    }
  });

  it("keeps the bare constant out of the copy catalogue, the model and the detail", () => {
    const sources = import.meta.glob(["../../copy/security.copy.ts", "./securityModel.ts", "./FindingDetail.tsx"], {
      query: "?raw",
      import: "default",
      eager: true,
    }) as Record<string, string>;
    expect(Object.keys(sources)).toHaveLength(3);
    for (const [file, text] of Object.entries(sources)) {
      expect(text, file).not.toMatch(/\bREDACTED\b/);
      expect(text, file).not.toContain("[redacted]");
    }
    expect(REDACTED_TEXT.why).toBe(WHY);
  });
});

describe("a secret found only in history (S-767)", () => {
  const COMMIT = "05ea98774379a22652c0c3b1b69faf9506c34b2d";

  it("labels a historical finding and shows the payload's short commit beside its location", async () => {
    const base = finding("generic.secrets.hardcoded-key");
    const anchor = { commit: COMMIT, short_commit: "05ea987", path: "legacy/client.py" };
    const historical: SecurityFinding = {
      ...base,
      binding: "historical",
      report_path: "legacy/client.py",
      path: null,
      start_line: 1,
      anchor,
    };
    const bound: SecurityFinding = { ...finding("rust.lang.security.formatted-sql"), anchor: { ...anchor, path: "src/lib.rs" } };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [bound, historical] } }));
    const table = within(widgetTitled(root, "Findings")).getByRole("table");
    const rowOf = (rule: string) => within(table).getByRole("button", { name: new RegExp(`Show ${rule}`) }).closest("tr") as HTMLElement;

    const row = rowOf("generic\\.secrets\\.hardcoded-key");
    expect(row.textContent).toContain("legacy/client.py:1");
    // The report's path still goes through PathCell: its full path is the tooltip.
    expect(row.querySelector('[data-path="legacy/client.py"]')).toHaveAttribute("title", "legacy/client.py");
    expect(within(row).getByText(ANCHOR_TEXT.historical)).toBeInTheDocument();
    const commit = within(row).getByText(anchor.short_commit);
    expect(commit).toHaveAttribute("title", ANCHOR_TEXT.commit(anchor));
    expect(row.textContent).not.toContain(COMMIT);

    const other = rowOf("rust\\.lang\\.security\\.formatted-sql");
    expect(within(other).getByText(anchor.short_commit)).toBeInTheDocument();
    expect(within(other).queryByText(ANCHOR_TEXT.historical)).toBeNull();
  });

  it("shows no label and no commit for a finding no commit names", async () => {
    const { root } = await mount(model());
    const table = within(widgetTitled(root, "Findings")).getByRole("table");
    expect(within(table).queryByText(ANCHOR_TEXT.historical)).toBeNull();
    expect(table.querySelector("[data-commit]")).toBeNull();
  });
});

describe("suppression", () => {
  const rowOf = (table: HTMLElement, loc: RegExp) => within(table).getByRole("button", { name: loc }).closest("tr") as HTMLElement;

  it("tags a suppressed finding's row, and only that row — its reason is the detail's", async () => {
    const secret = { ...finding("generic.secrets.hardcoded-key"), suppression: { state: "suppressed" as const, reason: "test key", expires: null, by: { fingerprint: finding("generic.secrets.hardcoded-key").fingerprint } } };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [finding("rust.lang.security.formatted-sql"), secret] } }));
    const table = within(widgetTitled(root, "Findings")).getByRole("table");
    expect(within(rowOf(table, /^Unsuppress generic/)).getByText("suppressed")).toBeInTheDocument();
    expect(within(rowOf(table, /^Suppress rust/)).queryByText("suppressed")).toBeNull();
    expect(table.textContent).not.toContain("test key");
    expect(table.querySelector("[data-suppression-reason]")).toBeNull();
  });

  it("states a suppressed finding's state, reason and expiry in the detail, and an expired one's lapse (S-672)", async () => {
    const secret = { ...finding("generic.secrets.hardcoded-key"), suppression: { state: "suppressed" as const, reason: "rotated test key", expires: "2027-01-31", by: { fingerprint: finding("generic.secrets.hardcoded-key").fingerprint } } };
    const sql = { ...finding("rust.lang.security.formatted-sql"), suppression: { state: "expired" as const, reason: "legacy path", expires: "2026-01-31", by: { fingerprint: finding("rust.lang.security.formatted-sql").fingerprint } } };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [sql, secret] } }));
    const table = within(widgetTitled(root, "Findings")).getByRole("table");
    // Either way the entry exists, so the row's action removes it.
    expect(within(rowOf(table, /^Unsuppress generic/)).getByText("suppressed")).toBeInTheDocument();
    expect(within(rowOf(table, /^Unsuppress rust/)).getByText("expired")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Show generic\.secrets\.hardcoded-key/ }));
    const detail = widgetTitled(root, "Finding detail");
    expect(detail.querySelector("[data-suppression-reason]")?.textContent).toBe("rotated test key (until 2027-01-31)");
    expect(within(detail).getByText("suppressed")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    expect(widgetTitled(root, "Finding detail").querySelector("[data-suppression-reason]")?.textContent).toBe("legacy path (expired after 2026-01-31)");
    expect(within(widgetTitled(root, "Finding detail")).getByText("expired")).toBeInTheDocument();
  });

  it("shows no suppression line in the detail of an active finding", async () => {
    const { root } = await mount(model());
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    const detail = widgetTitled(root, "Finding detail");
    expect(within(detail).queryByText("Suppression")).toBeNull();
    expect(detail.querySelector("[data-suppression-reason]")).toBeNull();
  });
});

interface Sent {
  url: string;
  method: string;
  intent: string | null;
  body: string | null;
}

/** Stubs fetch for the triage flow: the page read answers `pages` in turn (the
 *  last one repeats), a triage POST answers `triage`. Returns every request. */
function stubTriage(pages: SecurityModel[], triage: { status: number; body: unknown }): Sent[] {
  const sent: Sent[] = [];
  let reads = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init: RequestInit = {}) => {
      if (url.startsWith("/api/v1/security/scan") || url.startsWith("/api/v1/security/install")) {
        return Promise.resolve({ ok: true, json: () => Promise.resolve({ job: null }) } as Response);
      }
      sent.push({
        url,
        method: init.method ?? "GET",
        intent: new Headers(init.headers).get("x-logos-intent"),
        body: typeof init.body === "string" ? init.body : null,
      });
      const isTriage =
        url.startsWith("/api/v1/security/suppress") || url.startsWith("/api/v1/security/unsuppress") || url.startsWith("/api/v1/security/reconfirm");
      const status = isTriage ? triage.status : 200;
      const body = isTriage ? triage.body : pages[Math.min(reads++, pages.length - 1)];
      return Promise.resolve({
        ok: status >= 200 && status < 300,
        status,
        json: () => Promise.resolve(body),
        text: () => Promise.resolve(JSON.stringify(body)),
      } as Response);
    }),
  );
  return sent;
}

/** Makes the stubbed fetch (a {@link stubTriage}) answer `/api/v1/impact` with {@link IMPACT}, for a test that opens the detail. */
function answerImpact(): void {
  const page = globalThis.fetch;
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) =>
      url.startsWith("/api/v1/impact") ? Promise.resolve({ ok: true, json: () => Promise.resolve(IMPACT) } as Response) : page(url, init),
    ),
  );
}

describe("triage (S-672, FR-SC-12)", () => {
  const sqlLoc = "src/lib.rs:3";

  it("suppresses from the dialog with an intent-guarded POST, then reloads and shows the reason", async () => {
    const sql = finding("rust.lang.security.formatted-sql");
    const after = model({
      findings: { ...REAL.security.findings, findings: [{ ...sql, suppression: { state: "suppressed", reason: "constant query", expires: null, by: { fingerprint: sql.fingerprint } } }] },
    });
    const written = { file: ".logos/security-ignore.toml", entry: { fingerprint: sql.fingerprint, reason: "constant query", expires: null }, entries: 1 };
    const sent = stubTriage([model({ findings: { ...REAL.security.findings, findings: [sql] } }), after], { status: 200, body: written });
    answerImpact();
    const view = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Suppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    const dialog = screen.getByRole("dialog", { name: SUPPRESS_TEXT.title });
    expect(dialog.textContent).toContain(".logos/security-ignore.toml");
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "constant query");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));

    // The reloaded row is tagged, and its reason is the detail's.
    const unsuppress = await screen.findByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` });
    expect(within(unsuppress.closest("tr") as HTMLElement).getByText("suppressed")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: `Show rust.lang.security.formatted-sql at ${sqlLoc}` }));
    expect(widgetTitled(view.container, "Finding detail").querySelector("[data-suppression-reason]")?.textContent).toBe("constant query");
    expect(screen.queryByRole("dialog")).toBeNull();
    const post = sent.find((r) => r.method === "POST");
    expect(post).toEqual({
      url: "/api/v1/security/suppress",
      method: "POST",
      intent: "test-intent-token",
      body: JSON.stringify({ fingerprint: sql.fingerprint, reason: "constant query", expires: null }),
    });
    expect(sent.filter((r) => r.url === "/api/v1/security").length).toBe(2);
    view.unmount();
  });

  it("unsuppresses with an intent-guarded POST and reloads", async () => {
    const sql = { ...finding("rust.lang.security.formatted-sql"), suppression: { state: "suppressed" as const, reason: "r", expires: null, by: { fingerprint: finding("rust.lang.security.formatted-sql").fingerprint } } };
    const written = { file: ".logos/security-ignore.toml", entry: { fingerprint: sql.fingerprint, reason: "r", expires: null }, entries: 0 };
    const sent = stubTriage([model({ findings: { ...REAL.security.findings, findings: [sql] } }), model({ findings: { ...REAL.security.findings, findings: [{ ...sql, suppression: null }] } })], { status: 200, body: written });
    render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    await screen.findByRole("button", { name: `Suppress rust.lang.security.formatted-sql at ${sqlLoc}` });
    expect(sent.find((r) => r.method === "POST")).toMatchObject({
      url: "/api/v1/security/unsuppress",
      intent: "test-intent-token",
      body: JSON.stringify({ fingerprint: sql.fingerprint }),
    });
  });

  it("sends one unsuppress for a double click, and states no error for a removal that succeeded", async () => {
    const sql = { ...finding("rust.lang.security.formatted-sql"), suppression: { state: "suppressed" as const, reason: "r", expires: null, by: { fingerprint: finding("rust.lang.security.formatted-sql").fingerprint } } };
    const page = model({ findings: { ...REAL.security.findings, findings: [sql] } });
    let posts = 0;
    let reads = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init: RequestInit = {}) => {
        if (url.startsWith("/api/v1/security/scan") || url.startsWith("/api/v1/security/install")) {
          return Promise.resolve({ ok: true, json: () => Promise.resolve({ job: null }) } as Response);
        }
        if ((init.method ?? "GET") === "POST") {
          // The first removal succeeds; any second one finds no entry left.
          posts += 1;
          const status = posts === 1 ? 200 : 422;
          const body = posts === 1 ? { file: ".logos/security-ignore.toml", entry: { fingerprint: sql.fingerprint, reason: "r", expires: null }, entries: 0 } : { error: `no entry suppresses "${sql.fingerprint}"` };
          return Promise.resolve({ ok: status === 200, status, json: () => Promise.resolve(body), text: () => Promise.resolve(JSON.stringify(body)) } as Response);
        }
        // The reload's read stays pending, so the old page (and its button) stays.
        reads += 1;
        return reads === 1 ? Promise.resolve({ ok: true, json: () => Promise.resolve(page) } as Response) : new Promise<Response>(() => {});
      }),
    );
    const { container: root } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.dblClick(screen.getByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    await vi.waitFor(() => expect(reads).toBe(2));
    expect(posts).toBe(1);
    expect(within(widgetTitled(root, "Findings")).queryByRole("alert")).toBeNull();
  });

  it("states a refused unsuppress with the server's reason and keeps the row as it was", async () => {
    const sql = { ...finding("rust.lang.security.formatted-sql"), suppression: { state: "suppressed" as const, reason: "r", expires: null, by: { fingerprint: finding("rust.lang.security.formatted-sql").fingerprint } } };
    const refusal = "invalid value for .logos/security-ignore.toml: the file is not in the canonical spelling Logos writes";
    stubTriage([model({ findings: { ...REAL.security.findings, findings: [sql] } })], { status: 422, body: { error: refusal } });
    const { container: root } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    const alert = await within(widgetTitled(root, "Findings")).findByRole("alert");
    expect(alert.textContent).toBe(SUPPRESS_TEXT.removeFailed(refusal));
    expect(screen.getByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` })).toBeInTheDocument();
  });
});

describe("evidence (FR-SC-32, S-818)", () => {
  const sqlLoc = "src/lib.rs:3";
  const sql = finding("rust.lang.security.formatted-sql");
  const by = { fingerprint: sql.fingerprint };
  const changed = [{ symbol: "internal/`cookie.go`/DefaultCookieConfig().", name: "DefaultCookieConfig", change: "changed" as const }];
  const outOfDate = { ...sql, suppression: { state: "out_of_date" as const, reason: "the cookie is Secure by default", expires: null, by, changed } };
  const held = { ...sql, suppression: { state: "suppressed" as const, reason: "the cookie is Secure by default", expires: null, by, evidence: [{ symbol: "s", name: "DefaultCookieConfig" }] } };
  const page = (f: SecurityFinding) => model({ findings: { ...REAL.security.findings, findings: [f] } });
  const written = { file: ".logos/security-ignore.toml", entry: { fingerprint: sql.fingerprint, reason: "r", expires: null }, entries: 1 };

  it("reads an out-of-date verdict as active, naming the changed symbol, with Re-confirm beside Unsuppress", async () => {
    const { root } = await mount(page(outOfDate));
    const table = within(widgetTitled(root, "Findings")).getByRole("table");
    const row = within(table).getByRole("button", { name: `Re-confirm rust.lang.security.formatted-sql at ${sqlLoc}` }).closest("tr") as HTMLElement;
    expect(within(row).getByText("out of date")).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` })).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /^Suppress / })).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    const detail = widgetTitled(root, "Finding detail");
    expect(detail.querySelector("[data-suppression-reason]")?.textContent).toBe("the cookie is Secure by default");
    const evidence = detail.querySelector("[data-suppression-evidence]");
    expect(evidence?.getAttribute("data-suppression-evidence")).toBe("out_of_date");
    expect(evidence?.textContent).toBe(`${EVIDENCE_TEXT.outOfDate}. \`DefaultCookieConfig\` changed`);
    // The wording and the changed symbol are said once in the detail, not twice.
    const text = detail.textContent ?? "";
    expect(text.split(EVIDENCE_TEXT.outOfDate).length - 1).toBe(1);
    expect(text.split("`DefaultCookieConfig` changed").length - 1).toBe(1);
  });

  it("says what a suppression with no evidence rests on, and names the symbols one that has some rests on", async () => {
    const bare = { ...sql, suppression: { state: "suppressed" as const, reason: "r", expires: null, by, evidence: [] } };
    const { root } = await mount(page(bare));
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    expect(widgetTitled(root, "Finding detail").querySelector("[data-suppression-evidence]")?.textContent).toBe(EVIDENCE_TEXT.none);
    cleanup();
    const view = await mount(page(held));
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    expect(widgetTitled(view.root, "Finding detail").querySelector("[data-suppression-evidence]")?.textContent).toBe(
      EVIDENCE_TEXT.resting(["DefaultCookieConfig"]),
    );
  });

  it("re-confirms with an intent-guarded POST of the finding's fingerprint, then reloads the suppressed verdict", async () => {
    const sent = stubTriage([page(outOfDate), page(held)], { status: 200, body: written });
    render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Re-confirm rust.lang.security.formatted-sql at ${sqlLoc}` }));
    const back = await screen.findByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` });
    expect(screen.queryByRole("button", { name: /^Re-confirm / })).toBeNull();
    expect(within(back.closest("tr") as HTMLElement).getByText("suppressed")).toBeInTheDocument();
    expect(sent.find((r) => r.method === "POST")).toEqual({
      url: "/api/v1/security/reconfirm",
      method: "POST",
      intent: "test-intent-token",
      body: JSON.stringify({ fingerprint: sql.fingerprint }),
    });
    expect(sent.filter((r) => r.url === "/api/v1/security").length).toBe(2);
  });

  it("states a refused re-confirm with the server's reason and keeps the row as it was", async () => {
    const refusal = "evidence `DefaultCookieConfig` names no symbol in the index";
    stubTriage([page(outOfDate)], { status: 422, body: { error: refusal } });
    const { container: root } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Re-confirm rust.lang.security.formatted-sql at ${sqlLoc}` }));
    const alert = await within(widgetTitled(root, "Findings")).findByRole("alert");
    expect(alert.textContent).toBe(EVIDENCE_TEXT.reconfirmFailed(refusal));
    expect(screen.getByRole("button", { name: `Re-confirm rust.lang.security.formatted-sql at ${sqlLoc}` })).toBeEnabled();
  });

  it("lists an out-of-date entry with the symbol that changed, and re-confirms a fingerprint entry from there", async () => {
    const entry: SecurityEntryStatus = { entry: { fingerprint: sql.fingerprint, reason: "r", expires: null }, expired: false, orphaned: false, suppressed: 0, should_name: null, out_of_date: changed };
    const path: SecurityEntryStatus = { entry: { path: "src", reason: "r", expires: null }, expired: false, orphaned: false, suppressed: 0, should_name: null, out_of_date: changed };
    const sent = stubTriage([model({ findings: { ...REAL.security.findings, findings: [outOfDate], suppressions: [entry, path] } })], { status: 200, body: written });
    const { container: root } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });
    const widget = widgetTitled(root, SUPPRESSIONS_TEXT.title);
    expect(within(widget).getAllByText("out of date").length).toBe(2);
    expect(widget.textContent).toContain(EVIDENCE_TEXT.entryOutOfDate(changed));
    // Only the fingerprint entry has a fingerprint to re-confirm by.
    const buttons = within(widget).getAllByRole("button", { name: /^Re-confirm the entry/ });
    expect(buttons.map((b) => b.getAttribute("aria-label"))).toEqual([SUPPRESSIONS_TEXT.reconfirm({ fingerprint: sql.fingerprint })]);
    await userEvent.click(buttons[0]);
    await vi.waitFor(() => expect(sent.some((r) => r.url === "/api/v1/security/reconfirm")).toBe(true));
    expect(sent.find((r) => r.method === "POST")?.body).toBe(JSON.stringify({ fingerprint: sql.fingerprint }));
  });

  it("offers the enclosing symbol as evidence, off by default, and sends it only when ticked", async () => {
    const after = page({ ...sql, suppression: held.suppression });
    const sent = stubTriage([page(sql), after], { status: 200, body: written });
    answerImpact();
    render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Suppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    const dialog = screen.getByRole("dialog", { name: SUPPRESS_TEXT.title });
    const box = within(dialog).getByRole("checkbox", { name: new RegExp(EVIDENCE_TEXT.recordEvidence) });
    expect(box).not.toBeChecked();
    expect(dialog.textContent).toContain(EVIDENCE_TEXT.recordEvidenceHint("find"));
    await userEvent.click(box);
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "constant query");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));

    await screen.findByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` });
    expect(sent.find((r) => r.method === "POST")?.body).toBe(
      JSON.stringify({ fingerprint: sql.fingerprint, reason: "constant query", expires: null, evidence: ["logos . project . src/`lib.rs`/find()."] }),
    );
  });

  it("offers no evidence for a finding that binds no symbol", async () => {
    const unbound = { ...sql, context: { ...sql.context, enclosing: { ...sql.context.enclosing, symbol: null, unbound_because: "no_symbol" as const } } };
    stubTriage([page(unbound)], { status: 200, body: written });
    render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });
    await userEvent.click(screen.getByRole("button", { name: `Suppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    expect(within(screen.getByRole("dialog", { name: SUPPRESS_TEXT.title })).queryByRole("checkbox", { name: new RegExp(EVIDENCE_TEXT.recordEvidence) })).toBeNull();
  });
});

describe("path suppressions (S-770)", () => {
  const sqlLoc = "src/lib.rs:3";
  const under = { path: "src" };
  /** Both findings, each suppressed by the one `src` path entry when `entry` holds it. */
  function page(entry: SecurityEntryStatus | null, extra: SecurityEntryStatus[] = []): SecurityModel {
    const by = entry ? { state: "suppressed" as const, reason: "generated", expires: null, by: under } : null;
    const findings = [finding("rust.lang.security.formatted-sql"), finding("generic.secrets.hardcoded-key")].map((f) => ({ ...f, suppression: by }));
    return model({ findings: { ...REAL.security.findings, findings, suppressions: [...(entry ? [entry] : []), ...extra] } });
  }
  const entry: SecurityEntryStatus = { entry: { path: "src", reason: "generated", expires: null }, expired: false, orphaned: false, suppressed: 2, should_name: null, out_of_date: [] };
  const orphan: SecurityEntryStatus = { entry: { path: "tests/fixtures", scanner: "gitleaks", reason: "planted", expires: null }, expired: false, orphaned: true, suppressed: 0, should_name: null, out_of_date: [] };

  it("suppresses everything under a path from the dialog, and Unsuppress sends back the same selector", async () => {
    const written = { file: ".logos/security-ignore.toml", entry: entry.entry, entries: 1 };
    const sent = stubTriage([page(null), page(entry), page(null)], { status: 200, body: written });
    answerImpact();
    const { container: root } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Suppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    const dialog = screen.getByRole("dialog", { name: SUPPRESS_TEXT.title });
    await userEvent.click(within(dialog).getByRole("radio", { name: SUPPRESS_TEXT.scopePath }));
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "generated");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));

    // Both rows the entry covers say so, and the entry is listed with its count.
    const table = await within(widgetTitled(root, "Findings")).findByRole("table");
    await vi.waitFor(() => expect(within(table).getAllByText("suppressed").length).toBe(2));
    await userEvent.click(screen.getByRole("button", { name: `Show rust.lang.security.formatted-sql at ${sqlLoc}` }));
    expect(widgetTitled(root, "Finding detail").querySelector("[data-suppression-reason]")?.textContent).toBe(
      "generated — covers everything under src",
    );
    const suppressions = widgetTitled(root, SUPPRESSIONS_TEXT.title);
    expect(suppressions.textContent).toContain(SUPPRESSIONS_TEXT.figure(1, 2));
    expect(suppressions.textContent).toContain(SUPPRESSIONS_TEXT.count(2));

    await userEvent.click(screen.getByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    await screen.findByRole("button", { name: `Suppress rust.lang.security.formatted-sql at ${sqlLoc}` });
    const posts = sent.filter((r) => r.method === "POST");
    expect(posts.map((r) => [r.url, r.intent, r.body])).toEqual([
      ["/api/v1/security/suppress", "test-intent-token", JSON.stringify({ path: "src", reason: "generated", expires: null })],
      // The removal names exactly the entry that was written: the server's
      // add-then-remove restores the file byte for byte (web/tests/api_v1.rs).
      ["/api/v1/security/unsuppress", "test-intent-token", JSON.stringify({ path: "src" })],
    ]);
    expect(screen.queryByRole("heading", { name: SUPPRESSIONS_TEXT.title })).toBeNull();
  });

  it("disables every row one path entry covers while its removal is in flight", async () => {
    stubTriage([page(entry)], { status: 200, body: {} });
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init: RequestInit = {}) => {
        if ((init.method ?? "GET") === "POST") return new Promise<Response>(() => {});
        const body = url.startsWith("/api/v1/security/scan") || url.startsWith("/api/v1/security/install") ? { job: null } : page(entry);
        return Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response);
      }),
    );
    render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });
    await userEvent.click(screen.getByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    for (const button of screen.getAllByRole("button", { name: /^Unsuppress |^Remove the entry/ })) {
      expect(button).toBeDisabled();
    }
  });

  it("lists a fingerprint entry by its fingerprint, flags it expired with the one it should name, and removes it by fingerprint", async () => {
    const fp = "0123456789abcdef".repeat(4);
    const now = "fedcba9876543210".repeat(4);
    const lapsed: SecurityEntryStatus = { entry: { fingerprint: fp, reason: "rotated", expires: "2026-01-31" }, expired: true, orphaned: false, suppressed: 0, should_name: null, out_of_date: [] };
    const renamed: SecurityEntryStatus = { entry: { fingerprint: now.replace("f", "e"), reason: "old key", expires: null }, expired: false, orphaned: true, suppressed: 0, should_name: now, out_of_date: [] };
    const sent = stubTriage([page(null, [lapsed, renamed])], { status: 200, body: { file: ".logos/security-ignore.toml", entry: lapsed.entry, entries: 1 } });
    const { container: root } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });
    const widget = widgetTitled(root, SUPPRESSIONS_TEXT.title);
    const row = within(widget).getByText("fingerprint 0123456789ab…").closest("tr") as HTMLElement;
    expect(within(row).getByText("expired")).toBeInTheDocument();
    expect(row.textContent).toContain("2026-01-31");
    const orphan = within(widget).getByText(SUPPRESSIONS_TEXT.shouldName(now)).closest("tr") as HTMLElement;
    expect(within(orphan).getByText("orphaned")).toBeInTheDocument();

    await userEvent.click(within(row).getByRole("button", { name: SUPPRESSIONS_TEXT.remove({ fingerprint: fp }) }));
    await vi.waitFor(() => expect(sent.some((r) => r.method === "POST")).toBe(true));
    expect(sent.find((r) => r.method === "POST")?.body).toBe(JSON.stringify({ fingerprint: fp }));
  });

  it("lists an orphaned glob as orphaned with no count, and removes it by its own selector", async () => {
    const sent = stubTriage([page(entry, [orphan])], { status: 200, body: { file: ".logos/security-ignore.toml", entry: orphan.entry, entries: 1 } });
    const { container: root } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });
    const widget = widgetTitled(root, SUPPRESSIONS_TEXT.title);
    const row = within(widget).getByText("everything under tests/fixtures, scanner gitleaks").closest("tr") as HTMLElement;
    expect(within(row).getByText("orphaned")).toBeInTheDocument();
    expect(row.textContent).toContain(SUPPRESSIONS_TEXT.count(0));
    expect(widget.textContent).toContain(SUPPRESSIONS_TEXT.figure(2, 2));

    await userEvent.click(within(row).getByRole("button", { name: SUPPRESSIONS_TEXT.remove({ path: "tests/fixtures", scanner: "gitleaks" }) }));
    await vi.waitFor(() => expect(sent.some((r) => r.method === "POST")).toBe(true));
    expect(sent.find((r) => r.method === "POST")?.body).toBe(JSON.stringify({ path: "tests/fixtures", scanner: "gitleaks" }));
  });
});

describe("rule-wide suppressions (S-814)", () => {
  const sqlLoc = "src/lib.rs:3";
  const wide = { rule: "rust.lang.security.formatted-sql", scanner: "semgrep" };
  const entry: SecurityEntryStatus = { entry: { ...wide, reason: "deploy tool sets it", expires: "2027-01-01" }, expired: false, orphaned: false, suppressed: 1, should_name: null, out_of_date: [] };

  /** The formatted-sql finding suppressed by the rule-wide entry; the other stays active. */
  function page(): SecurityModel {
    const held = { state: "suppressed" as const, reason: "deploy tool sets it", expires: "2027-01-01", by: wide };
    const findings = [{ ...finding("rust.lang.security.formatted-sql"), suppression: held }, finding("generic.secrets.hardcoded-key")];
    return model({ findings: { ...REAL.security.findings, findings, suppressions: [entry] } });
  }

  it("labels the finding the entry suppresses, lists the entry, and removes it by rule and scanner", async () => {
    const sent = stubTriage([page()], { status: 200, body: { file: ".logos/security-ignore.toml", entry: entry.entry, entries: 0 } });
    answerImpact();
    const { container: root } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Show rust.lang.security.formatted-sql at ${sqlLoc}` }));
    expect(widgetTitled(root, "Finding detail").querySelector("[data-suppression-reason]")?.textContent).toBe(
      "deploy tool sets it (until 2027-01-01) — suppressed by a rule-wide entry (rule rust.lang.security.formatted-sql of semgrep, in every file)",
    );
    const suppressions = widgetTitled(root, SUPPRESSIONS_TEXT.title);
    const row = within(suppressions).getByText("rule rust.lang.security.formatted-sql of semgrep, in every file").closest("tr") as HTMLElement;
    expect(row.textContent).toContain(SUPPRESSIONS_TEXT.count(1));

    await userEvent.click(within(row).getByRole("button", { name: SUPPRESSIONS_TEXT.remove(wide) }));
    await vi.waitFor(() => expect(sent.some((r) => r.method === "POST")).toBe(true));
    expect(sent.find((r) => r.method === "POST")?.body).toBe(JSON.stringify(wide));
  });

  it("sends the entry's own selector from the finding's Unsuppress", async () => {
    const sent = stubTriage([page()], { status: 200, body: { file: ".logos/security-ignore.toml", entry: entry.entry, entries: 0 } });
    render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Unsuppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    await vi.waitFor(() => expect(sent.some((r) => r.method === "POST")).toBe(true));
    expect(sent.find((r) => r.method === "POST")?.body).toBe(JSON.stringify(wide));
  });
});

describe("the edition switch (FR-SC-04)", () => {
  it("re-classifies the table and the OWASP view without refetching", async () => {
    const { calls, root } = await mount(model());
    expect(calls).toEqual(["/api/v1/security"]);
    const table = () => within(widgetTitled(root, "Findings")).getByRole("table");
    expect(within(table()).getByText("A05:2025")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "2021" }));
    expect(screen.getByRole("button", { name: "2021" })).toHaveAttribute("aria-pressed", "true");
    expect(within(table()).getByText("A03:2021")).toBeInTheDocument();
    expect(within(table()).queryByText("A05:2025")).toBeNull();
    const owasp = widgetTitled(root, "OWASP Top 10");
    expect(readerText(owasp)).toContain("OWASP Top 10:2021");
    expect(within(owasp).getByText("A01:2021")).toBeInTheDocument();
    expect(within(owasp).queryByText("A01:2025")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "2025" }));
    expect(within(table()).getByText("A05:2025")).toBeInTheDocument();
    expect(calls, "switching edition fetches nothing").toEqual(["/api/v1/security"]);
  });
});

describe("the finding count (S-760)", () => {
  it("states the real payload's count and what it leaves out", async () => {
    const { root } = await mount(model());
    const text = readerText(widgetTitled(root, "Findings"));
    expect(text).toContain(FINDING_COUNT_TEXT.totals(0, 2, 0, 0, 0, 0));
    expect(text).toContain(FINDING_COUNT_TEXT.excluded(0, 0));
  });

  it("puts each severity and each left-out figure in its own place, in the spelling the manual gives", async () => {
    // Six distinct figures and two distinct exclusions, so no slot can be
    // swapped unnoticed; the literals are the definition's wording, not the
    // catalogue's own constants.
    const counted = { critical: 1, high: 2, medium: 3, low: 4, info: 5, unrated: 6, total: 21 };
    const { root } = await mount(model({ counts: { counted, excluded: { not_fresh: 7, suppressed: 9, test: 0, dev: 0, needs_review: 0 } } }));
    const text = readerText(widgetTitled(root, "Findings"));
    expect(text).toContain("1 critical · 2 high · 3 medium · 4 low · 5 info · 6 unrated — fresh, unsuppressed findings only");
    expect(text).toContain("Left out: 7 findings in files changed since the scan, and 9 suppressed.");
  });

  it("reads a scoped-only project's count from its findings, though no report is fresh", async () => {
    const stale = REAL.security.status.scanners.map((s) =>
      s.scanner === "semgrep"
        ? { ...s, state: "stale" as const, because: "scoped_only" as const, snapshot_head: "abc", version: "1.85.0", findings: 2, suppressed: 0 }
        : s,
    );
    const { root } = await mount(
      model({
        status: { ...REAL.security.status, scanners: stale },
        counts: { counted: SCOPED_ONLY_COUNTED, excluded: SCOPED_ONLY_EXCLUDED },
      }),
    );
    const findings = readerText(widgetTitled(root, "Findings"));
    const { critical, high, medium, low, info, unrated } = SCOPED_ONLY_COUNTED;
    expect(findings).toContain(FINDING_COUNT_TEXT.totals(critical, high, medium, low, info, unrated));
    expect(findings).toContain(FINDING_COUNT_TEXT.excluded(0, 0));
    expect(readerText(widgetTitled(root, "Scanners"))).toContain("0 of 9 scanners fresh at HEAD");
  });
});

/** The real page with two findings planted in test code — one by its enclosing
 *  symbol, one by its path — classified under both editions as the SQL finding
 *  is, and counted as the server counts them: apart (`excluded.test`), while the
 *  payload's OWASP counts hold every stored finding. */
function withTestCode(): { page: SecurityModel; inSymbol: SecurityFinding; inPath: SecurityFinding } {
  const sql = finding("rust.lang.security.formatted-sql");
  const inSymbol: SecurityFinding = {
    ...sql,
    fingerprint: "test-symbol",
    path: "src/db.rs",
    report_path: "src/db.rs",
    start_line: 9,
    code_scope: "test",
    code_scope_reason: "test_symbol",
  };
  const inPath: SecurityFinding = {
    ...sql,
    fingerprint: "test-path",
    path: "tests/fixtures/db.rs",
    report_path: "tests/fixtures/db.rs",
    start_line: 4,
    code_scope: "test",
    code_scope_reason: "test_path",
  };
  const planted = [inSymbol, inPath];
  const editions = REAL.security.editions.map((e) => ({
    ...e,
    categories: e.categories.map((c) =>
      c.state.state === "covered" && c.state.findings > 0 && e.findings.some((f) => f.categories.includes(c.id))
        ? { ...c, state: { state: "covered" as const, findings: c.state.findings + planted.length } }
        : c,
    ),
    findings: [...e.findings, ...planted.map((f) => ({ scanner_name: f.scanner_name, fingerprint: f.fingerprint, categories: e.findings.find((x) => x.fingerprint === sql.fingerprint)?.categories ?? [], unclassified: null }))],
  }));
  const page = model({
    findings: { ...REAL.security.findings, findings: [...REAL.security.findings.findings, ...planted], total: REAL.security.findings.total + planted.length },
    editions,
    counts: { counted: REAL.security.counts.counted, excluded: { not_fresh: 0, suppressed: 0, test: planted.length, dev: 0, needs_review: 0 } },
  });
  return { page, inSymbol, inPath };
}

/** The real page with one finding its scanner marks low-confidence planted in
 *  production code, as the server serves it: listed with `needs_review`, counted
 *  apart (`excluded.needs_review`), and in its category's `findings` but not its
 *  `production_findings`. */
function withNeedsReview(): { page: SecurityModel; low: SecurityFinding } {
  const sql = finding("rust.lang.security.formatted-sql");
  const low: SecurityFinding = {
    ...sql,
    fingerprint: "low-confidence",
    rule_id: "rust.lang.security.low-confidence-sql",
    path: "src/db.rs",
    report_path: "src/db.rs",
    start_line: 9,
    confidence: "low",
    needs_review: true,
  };
  const editions = REAL.security.editions.map((e) => ({
    ...e,
    categories: e.categories.map((c) =>
      c.state.state === "covered" && c.state.findings > 0 && e.findings.some((f) => f.categories.includes(c.id))
        ? { ...c, state: { state: "covered" as const, findings: c.state.findings + 1 }, needs_review_findings: 1 }
        : c,
    ),
    findings: [...e.findings, { scanner_name: low.scanner_name, fingerprint: low.fingerprint, categories: e.findings.find((x) => x.fingerprint === sql.fingerprint)?.categories ?? [], unclassified: null }],
  }));
  const page = model({
    findings: { ...REAL.security.findings, findings: [...REAL.security.findings.findings, low], total: REAL.security.findings.total + 1 },
    editions,
    counts: { counted: REAL.security.counts.counted, excluded: { not_fresh: 0, suppressed: 0, test: 0, dev: 0, needs_review: 1 } },
  });
  return { page, low };
}

describe("a finding its scanner marks low-confidence reads needs review (FR-SC-31, S-817)", () => {
  const rows = (root: HTMLElement) =>
    [...within(widgetTitled(root, "Findings")).getByRole("table").querySelectorAll("tbody tr")] as HTMLElement[];
  const scope = () => screen.getByLabelText(SCOPE_TEXT.label) as HTMLSelectElement;

  it("leaves it out of the default list and states how many beside the count, in both widgets", async () => {
    const { page } = withNeedsReview();
    const { root } = await mount(page);
    expect(rows(root)).toHaveLength(2);
    expect(widgetTitled(root, "Findings").querySelector("[data-needs-review]")).toBeNull();
    expect(readerText(widgetTitled(root, "Findings"))).toContain("2 of 3 findings shown");
    for (const title of ["Findings", "OWASP Top 10"]) {
      expect(readerText(widgetTitled(root, title))).toContain("· 1 more needs review");
    }
    // The real payload, with none, says so rather than saying nothing.
    cleanup();
    const plain = await mount(model());
    expect(readerText(widgetTitled(plain.root, "Findings"))).toContain("· 0 more need review");
  });

  it("lists it under All, tagged REVIEW with the scanner's word and what it changes on hover", async () => {
    const { page } = withNeedsReview();
    const { root } = await mount(page);
    await userEvent.selectOptions(scope(), "all");
    expect(rows(root)).toHaveLength(3);
    const tags = [...widgetTitled(root, "Findings").querySelectorAll("[data-needs-review]")] as HTMLElement[];
    expect(tags.map((t) => t.getAttribute("title"))).toEqual([SCOPE_TEXT.reviewReason("low")]);
    expect(within(tags[0]).getByText(SCOPE_TEXT.reviewTag).className).toContain("muted");
    // On a line of its own, so it widens no column (the e2e spec measures the fit).
    expect(tags[0].className).toContain(styles.reviewTag);
    expect(tags[0].closest("tr")?.textContent).toContain("src/db.rs");
    expect(tags[0].querySelector(".sr-only")?.textContent?.trim()).toBe(SCOPE_TEXT.reviewReason("low"));
  });

  it("keeps it out of the OWASP count under Production, and in the payload's own count under All", async () => {
    const { page } = withNeedsReview();
    const { root } = await mount(page);
    const injection = () => readerText(within(widgetTitled(root, "OWASP Top 10")).getByText("A05:2025").closest("tr") as HTMLElement);
    expect(injection()).toContain("1 finding");
    await userEvent.selectOptions(scope(), "all");
    expect(injection()).toContain("2 findings");
    await userEvent.selectOptions(scope(), "test");
    expect(injection()).toContain("0 findings");
  });

  it("states it in the finding's detail, and says nothing for a finding the scanner is sure of", async () => {
    const { page, low } = withNeedsReview();
    const { root } = await mount(page);
    await userEvent.selectOptions(scope(), "all");
    const row = rows(root).find((r) => r.textContent?.includes("low-confidence-sql")) as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: /^Show/ }));
    const detail = widgetTitled(root, "Finding detail");
    expect(readerText(detail)).toContain(SCOPE_TEXT.reviewReason(low.confidence));
    const sure = rows(root).find((r) => !r.textContent?.includes("low-confidence-sql")) as HTMLElement;
    await userEvent.click(within(sure).getByRole("button", { name: /^Show/ }));
    expect(readerText(widgetTitled(root, "Finding detail"))).not.toContain("Needs review");
  });
});

/** The real page with one finding on a dev-only dependency planted, as the
 *  server serves it: listed with `code_scope: "dev"`, counted apart
 *  (`excluded.dev`), and in its category's `findings` but not its
 *  `production_findings`. */
function withDevDependency(): { page: SecurityModel; dev: SecurityFinding } {
  const sql = finding("rust.lang.security.formatted-sql");
  const dev: SecurityFinding = {
    ...sql,
    fingerprint: "dev-only",
    rule_id: "GHSA-dev-only-dependency",
    path: null,
    report_path: "web/package-lock.json",
    start_line: null,
    code_scope: "dev",
    code_scope_reason: "dev_dependency",
  };
  const editions = REAL.security.editions.map((e) => ({
    ...e,
    categories: e.categories.map((c) =>
      c.state.state === "covered" && c.state.findings > 0 && e.findings.some((f) => f.categories.includes(c.id))
        ? { ...c, state: { state: "covered" as const, findings: c.state.findings + 1 }, dev_findings: 1 }
        : c,
    ),
    findings: [...e.findings, { scanner_name: dev.scanner_name, fingerprint: dev.fingerprint, categories: e.findings.find((x) => x.fingerprint === sql.fingerprint)?.categories ?? [], unclassified: null }],
  }));
  const page = model({
    findings: { ...REAL.security.findings, findings: [...REAL.security.findings.findings, dev], total: REAL.security.findings.total + 1 },
    editions,
    counts: { counted: REAL.security.counts.counted, excluded: { not_fresh: 0, suppressed: 0, test: 0, dev: 1, needs_review: 0 } },
  });
  return { page, dev };
}

describe("a finding on a dev-only dependency reads dev (FR-SC-30, S-810)", () => {
  const rows = (root: HTMLElement) =>
    [...within(widgetTitled(root, "Findings")).getByRole("table").querySelectorAll("tbody tr")] as HTMLElement[];
  const scope = () => screen.getByLabelText(SCOPE_TEXT.label) as HTMLSelectElement;

  it("leaves it out of the default list and states how many beside the count, in both widgets", async () => {
    const { page } = withDevDependency();
    const { root } = await mount(page);
    expect(rows(root)).toHaveLength(2);
    expect(widgetTitled(root, "Findings").querySelector('[data-code-scope="dev"]')).toBeNull();
    expect(readerText(widgetTitled(root, "Findings"))).toContain("2 of 3 findings shown");
    for (const title of ["Findings", "OWASP Top 10"]) {
      expect(readerText(widgetTitled(root, title))).toContain("· 1 more in dev-only dependencies");
    }
    // The real payload, with none, says so rather than saying nothing.
    cleanup();
    const plain = await mount(model());
    expect(readerText(widgetTitled(plain.root, "Findings"))).toContain("· 0 more in dev-only dependencies");
  });

  it("lists it under All alone, tagged DEV with its reason on hover, and not under Test code", async () => {
    const { page } = withDevDependency();
    const { root } = await mount(page);
    await userEvent.selectOptions(scope(), "test");
    expect(within(widgetTitled(root, "Findings")).queryByRole("table")).toBeNull();
    expect(readerText(widgetTitled(root, "Findings"))).toContain(SCOPE_TEXT.hidden(3, "test-code"));
    await userEvent.selectOptions(scope(), "all");
    expect(rows(root)).toHaveLength(3);
    const tags = [...widgetTitled(root, "Findings").querySelectorAll('[data-code-scope="dev"]')] as HTMLElement[];
    expect(tags.map((t) => t.getAttribute("title"))).toEqual([SCOPE_TEXT.reason.dev_dependency]);
    expect(within(tags[0]).getByText(SCOPE_TEXT.devTag).className).toContain("muted");
    expect(tags[0].className).toContain(styles.reviewTag);
    expect(tags[0].closest("tr")?.textContent).toContain("web/package-lock.json");
    expect(tags[0].querySelector(".sr-only")?.textContent?.trim()).toBe(SCOPE_TEXT.reason.dev_dependency);
  });

  it("keeps it out of the OWASP count under Production and Test code, and in the payload's own count under All", async () => {
    const { page } = withDevDependency();
    const { root } = await mount(page);
    const injection = () => readerText(within(widgetTitled(root, "OWASP Top 10")).getByText("A05:2025").closest("tr") as HTMLElement);
    expect(injection()).toContain("1 finding");
    await userEvent.selectOptions(scope(), "all");
    expect(injection()).toContain("2 findings");
    await userEvent.selectOptions(scope(), "test");
    expect(injection()).toContain("0 findings");
  });

  it("states it in the finding's detail as the dependency scope", async () => {
    const { page } = withDevDependency();
    const { root } = await mount(page);
    await userEvent.selectOptions(scope(), "all");
    const row = rows(root).find((r) => r.textContent?.includes("GHSA-dev-only-dependency")) as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: /^Show/ }));
    const detail = widgetTitled(root, "Finding detail");
    expect(readerText(detail)).toContain(SCOPE_TEXT.reason.dev_dependency);
    expect(detail.querySelector('[data-code-scope="dev"]')).not.toBeNull();
  });
});

describe("the code scope (FR-SC-28, S-796)", () => {
  const rows = (root: HTMLElement) =>
    [...within(widgetTitled(root, "Findings")).getByRole("table").querySelectorAll("tbody tr")] as HTMLElement[];
  const scope = () => screen.getByLabelText(SCOPE_TEXT.label) as HTMLSelectElement;

  it("lists production by default: no test-code row, no TEST tag, and the control reads Production", async () => {
    const { page } = withTestCode();
    const { root } = await mount(page);
    expect(scope()).toHaveValue("production");
    expect([...scope().options].map((o) => o.textContent)).toEqual(["Production", "Test code", "All"]);
    expect(rows(root)).toHaveLength(2);
    expect(widgetTitled(root, "Findings").querySelector("[data-code-scope]")).toBeNull();
    expect(readerText(widgetTitled(root, "Findings"))).toContain("2 of 4 findings shown");
  });

  it("states the findings the counts leave out beside the count, equal to the payload's own remainder", async () => {
    const { page } = withTestCode();
    const { root } = await mount(page);
    const findings = readerText(widgetTitled(root, "Findings"));
    expect(findings).toContain(`${FINDING_COUNT_TEXT.totals(0, 2, 0, 0, 0, 0)} · 2 in test code`);
    // The real payload, with nothing in test code, says so rather than saying nothing.
    cleanup();
    const plain = await mount(model());
    expect(readerText(widgetTitled(plain.root, "Findings"))).toContain("· 0 in test code");
  });

  it("shows test-code rows under All, each tagged muted TEST with its reason on hover", async () => {
    const { page } = withTestCode();
    const { root } = await mount(page);
    await userEvent.selectOptions(scope(), "all");
    expect(rows(root)).toHaveLength(4);
    const tags = [...widgetTitled(root, "Findings").querySelectorAll("[data-code-scope='test']")] as HTMLElement[];
    expect(tags.map((t) => t.getAttribute("title"))).toEqual([SCOPE_TEXT.reason.test_symbol, SCOPE_TEXT.reason.test_path]);
    for (const tag of tags) {
      const badge = within(tag).getByText(SCOPE_TEXT.tag);
      expect(badge.className).toContain("muted");
      expect(tag.closest("tr")).not.toBeNull();
    }
    // The reason is also text for a reader without a pointer, not the tooltip alone.
    expect(tags.map((t) => t.querySelector(".sr-only")?.textContent?.trim())).toEqual([SCOPE_TEXT.reason.test_symbol, SCOPE_TEXT.reason.test_path]);
    // Each tag sits in its own row: the production rows carry none.
    expect(rows(root).filter((r) => r.querySelector("[data-code-scope]"))).toHaveLength(2);
  });

  it("lists only test code under Test code", async () => {
    const { page, inSymbol, inPath } = withTestCode();
    const { root } = await mount(page);
    await userEvent.selectOptions(scope(), "test");
    expect(rows(root)).toHaveLength(2);
    const text = widgetTitled(root, "Findings").textContent as string;
    expect(text).toContain(inSymbol.path);
    expect(text).toContain(inPath.path);
    expect(text).not.toContain("src/lib.rs");
  });

  it("keeps the scope in the URL, in place, beside the other query pairs", async () => {
    const { page } = withTestCode();
    window.history.replaceState({ keep: 1 }, "", "/security?path=src%2Fa%20b&repo=web");
    const length = window.history.length;
    await mount(page);
    await userEvent.selectOptions(scope(), "all");
    expect(window.location.search).toBe("?path=src%2Fa%20b&repo=web&scope=all");
    await userEvent.selectOptions(scope(), "test");
    expect(window.location.search).toBe("?path=src%2Fa%20b&repo=web&scope=test");
    await userEvent.selectOptions(scope(), "production");
    expect(window.location.search).toBe("?path=src%2Fa%20b&repo=web");
    expect(window.history.length, "the control adds no history entry").toBe(length);
    expect(window.history.state).toEqual({ keep: 1 });
  });

  it("opens on the scope the URL names, and on production for one it does not know", async () => {
    const { page } = withTestCode();
    window.history.replaceState(null, "", "/security?scope=test");
    const first = await mount(page);
    expect(scope()).toHaveValue("test");
    expect(rows(first.root)).toHaveLength(2);
    cleanup();
    window.history.replaceState(null, "", "/security?scope=everything");
    const second = await mount(page);
    expect(scope()).toHaveValue("production");
    expect(rows(second.root)).toHaveLength(2);
  });

  it("says where a deep-linked file's findings went when the scope hides them all", async () => {
    const { page } = withTestCode();
    window.history.replaceState(null, "", "/security?path=tests/fixtures");
    const { root } = await mount(page);
    expect(within(widgetTitled(root, "Findings")).queryByRole("table")).toBeNull();
    expect(readerText(widgetTitled(root, "Findings"))).toContain(SCOPE_TEXT.hidden(1, "production"));
    await userEvent.selectOptions(scope(), "all");
    expect(rows(root)[0].textContent).toContain("tests/fixtures/db.rs");
  });

  it("follows the Files & Risk link for a file's test-code findings", async () => {
    const { page } = withTestCode();
    window.history.replaceState(null, "", "/security?path=tests%2Ffixtures%2Fdb.rs&scope=test");
    const { root } = await mount(page);
    expect(scope()).toHaveValue("test");
    expect(rows(root)).toHaveLength(1);
  });

  it("counts each OWASP category over the scope — the payload's own counts under All — and states the test code", async () => {
    const { page } = withTestCode();
    const { root } = await mount(page);
    const owasp = () => widgetTitled(root, "OWASP Top 10");
    const injection = () => readerText(within(owasp()).getByText("A05:2025").closest("tr") as HTMLElement);
    expect(injection()).toContain("1 finding");
    expect(readerText(owasp())).toContain(`OWASP Top 10:2025 ${FINDING_COUNT_TEXT.testCode(2)}`);
    await userEvent.selectOptions(scope(), "all");
    const payload = page.security.editions[0].categories.find((c) => c.id === "A05:2025")!.state;
    expect(payload).toEqual({ state: "covered", findings: 3 });
    expect(injection()).toContain("3 findings");
    await userEvent.selectOptions(scope(), "test");
    expect(injection()).toContain("2 findings");
  });

  it("states the one test-code figure the server counts, in both widgets, when a test finding is suppressed", async () => {
    const { page, inPath } = withTestCode();
    // One of the two test findings is suppressed: the server counts one (fresh,
    // unsuppressed), though two test-code findings are listed.
    const suppressed = { ...inPath, suppression: { state: "suppressed" as const, reason: "r", expires: null, by: { fingerprint: inPath.fingerprint } } };
    const findings = page.security.findings.findings.map((f) => (f.fingerprint === inPath.fingerprint ? suppressed : f));
    const { root } = await mount({
      ...page,
      security: {
        ...page.security,
        findings: { ...page.security.findings, findings },
        counts: { ...page.security.counts, excluded: { not_fresh: 0, suppressed: 1, test: 1, dev: 0, needs_review: 0 } },
      },
    });
    for (const title of ["Findings", "OWASP Top 10"]) {
      expect(readerText(widgetTitled(root, title))).toContain("· 1 in test code");
      expect(readerText(widgetTitled(root, title))).not.toContain("· 2 in test code");
    }
  });

  it("follows the address when it changes under the page: Back, Forward, or a click on the Security link", async () => {
    const { page } = withTestCode();
    window.history.replaceState(null, "", "/security?scope=test");
    const { root } = await mount(page);
    expect(scope()).toHaveValue("test");
    // The sidebar's click on the active link: `navigate` pushes the bare path and fires popstate.
    act(() => {
      window.history.pushState({}, "", "/security");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    expect(scope()).toHaveValue("production");
    expect(rows(root)).toHaveLength(2);
    act(() => {
      window.history.pushState({}, "", "/security?scope=all");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    expect(scope()).toHaveValue("all");
    expect(rows(root)).toHaveLength(4);
    expect(window.location.search).toBe("?scope=all");
  });

  it("words an empty Test code table for the production findings it hides", async () => {
    const { page } = withTestCode();
    window.history.replaceState(null, "", "/security?scope=test&path=src/lib.rs");
    const { root } = await mount(page);
    expect(within(widgetTitled(root, "Findings")).queryByRole("table")).toBeNull();
    // The production findings the path keeps: the real payload's, none of the planted test code.
    const hidden = page.security.findings.findings.filter((f) => f.code_scope === "production" && f.path === "src/lib.rs").length;
    expect(hidden).toBeGreaterThan(0);
    expect(readerText(widgetTitled(root, "Findings"))).toContain(SCOPE_TEXT.hidden(hidden, "test-code"));
  });

  it("states a test-code finding's reason in its detail, and a production finding's none", async () => {
    const { page, inPath } = withTestCode();
    const { root } = await mount(page);
    await userEvent.selectOptions(scope(), "all");
    await userEvent.click(screen.getByRole("button", { name: `Show ${inPath.rule_id} at tests/fixtures/db.rs:4` }));
    expect(widgetTitled(root, "Finding detail").querySelector("[data-code-scope]")?.textContent).toBe(SCOPE_TEXT.reason.test_path);
    await userEvent.click(screen.getByRole("button", { name: "Show rust.lang.security.formatted-sql at src/lib.rs:3" }));
    expect(widgetTitled(root, "Finding detail").querySelector("[data-code-scope]")).toBeNull();
  });
});

describe("filters", () => {
  it("clears a category filter on an edition switch, so no hidden filter empties the table", async () => {
    const { root } = await mount(model());
    await userEvent.selectOptions(screen.getByLabelText("OWASP category"), "A05:2025");
    expect(readerText(widgetTitled(root, "Findings"))).toContain("1 of 2 findings shown");

    await userEvent.click(screen.getByRole("button", { name: "2021" }));
    expect(screen.getByLabelText("OWASP category")).toHaveValue("");
    expect(readerText(widgetTitled(root, "Findings"))).toContain("2 of 2 findings shown");
  });

  it("narrows the table to a ?path= deep link (Files & Risk, S-671)", async () => {
    window.history.replaceState(null, "", "/security?path=src/other.rs");
    const { root } = await mount(model());
    expect(screen.getByLabelText("Path")).toHaveValue("src/other.rs");
    expect(readerText(widgetTitled(root, "Findings"))).toContain(SECURITY_ABSENCE.noFindingAt("src/other.rs"));
  });

  it("keeps the deep-linked file's findings", async () => {
    window.history.replaceState(null, "", "/security?path=src/lib.rs");
    const { root } = await mount(model());
    expect(within(widgetTitled(root, "Findings")).getAllByRole("button", { name: /^Show .* at src\/lib\.rs:3$/ })).toHaveLength(1);
    expect(readerText(widgetTitled(root, "Findings"))).toContain("2 of 2 findings shown");
  });

  it("narrows by scanner, suppression and a typed path", async () => {
    const { root } = await mount(model());
    const shown = () => readerText(widgetTitled(root, "Findings"));
    await userEvent.selectOptions(screen.getByLabelText("Scanner"), "Semgrep OSS");
    expect(shown()).toContain("2 of 2 findings shown");
    await userEvent.selectOptions(screen.getByLabelText("Suppression"), "suppressed");
    expect(shown()).toContain(SECURITY_ABSENCE.noMatch);
    await userEvent.selectOptions(screen.getByLabelText("Suppression"), "");
    await userEvent.type(screen.getByLabelText("Path"), "src/other.rs");
    expect(shown()).toContain(SECURITY_ABSENCE.noFindingAt("src/other.rs"));
  });

  it("narrows by Reachable, the filter named for the column and its tooltip", async () => {
    const { root } = await mount(model());
    expect(screen.getByLabelText("Reachable")).toHaveAttribute("title", FINDINGS_TABLE_TEXT.reachableTip);
    expect(screen.queryByLabelText("Route reach")).toBeNull();
    expect(within(screen.getByLabelText("Reachable")).getAllByRole("option").map((o) => o.textContent)).toEqual(["Any", "yes", "no", "unknown"]);
    await userEvent.selectOptions(screen.getByLabelText("Reachable"), "unknown");
    expect(readerText(widgetTitled(root, "Findings"))).toContain("2 of 2 findings shown");
    await userEvent.selectOptions(screen.getByLabelText("Reachable"), "reachable");
    expect(readerText(widgetTitled(root, "Findings"))).toContain("0 of 2 findings shown");
  });

  it("narrows by severity", async () => {
    const low = { ...finding("rust.lang.security.formatted-sql"), severity: "low" as const };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [low, finding("generic.secrets.hardcoded-key")] } }));
    await userEvent.selectOptions(screen.getByLabelText("Least severity"), "high");
    expect(readerText(widgetTitled(root, "Findings"))).toContain("1 of 2 findings shown");
  });

  it("lists an unrated finding with its reason and narrows to the unrated ones alone (S-768)", async () => {
    const unrated = {
      ...finding("rust.lang.security.formatted-sql"),
      severity: "unrated" as const,
      severity_reason: "the scanner gave none",
    };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [unrated, finding("generic.secrets.hardcoded-key")] } }));
    const row = screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }).closest("tr") as HTMLElement;
    expect(within(row).getByText("unrated").closest("[title]")).toHaveAttribute("title", "the scanner gave none");
    const options = within(screen.getByLabelText("Least severity")).getAllByRole("option").map((o) => o.textContent);
    expect(options).toEqual(["Any", "critical", "high", "medium", "unrated only", "low", "info"]);
    await userEvent.selectOptions(screen.getByLabelText("Least severity"), "unrated");
    expect(readerText(widgetTitled(root, "Findings"))).toContain("1 of 2 findings shown");
    expect(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ })).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("Least severity"), "medium");
    expect(readerText(widgetTitled(root, "Findings"))).toContain("2 of 2 findings shown");
    await userEvent.selectOptions(screen.getByLabelText("Least severity"), "high");
    expect(readerText(widgetTitled(root, "Findings"))).toContain("1 of 2 findings shown");
    expect(screen.queryByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ })).toBeNull();
  });
});

describe("unclassified findings (FR-SC-04)", () => {
  it("counts them in the OWASP view and says why in the detail, in words", async () => {
    const secret = finding("generic.secrets.hardcoded-key");
    const editions = REAL.security.editions.map((e) =>
      e.edition !== "2025"
        ? e
        : {
            ...e,
            findings: e.findings.map((c) => (c.fingerprint === secret.fingerprint ? { ...c, categories: [], unclassified: "no_cwe" as const } : c)),
          },
    );
    const { root } = await mount(model({ editions }));
    expect(readerText(widgetTitled(root, "OWASP Top 10"))).toContain(OWASP_TEXT.unclassified(1));
    await userEvent.click(screen.getByRole("button", { name: /Show generic\.secrets\.hardcoded-key/ }));
    const detail = widgetTitled(root, "Finding detail");
    expect(readerText(detail)).toContain(UNCLASSIFIED_TEXT.no_cwe);
    expect(readerText(detail)).not.toContain("no_cwe");
    // The 2021 edition still classifies it, so the note goes when the edition does.
    await userEvent.click(screen.getByRole("button", { name: "2021" }));
    expect(readerText(widgetTitled(root, "OWASP Top 10"))).not.toContain(OWASP_TEXT.unclassified(1));
  });
});

describe("the Findings table (S-793, frontend-design §4.23)", () => {
  const LONG = `src/${"deeply-nested-module/".repeat(5)}handler.rs`;
  const tableOf = (root: HTMLElement) => within(widgetTitled(root, "Findings")).getByRole("table");
  const rowOf = (table: HTMLElement, rule: string) =>
    within(table).getByRole("button", { name: new RegExp(`^Show ${rule.replace(/\./g, "\\.")} `) }).closest("tr") as HTMLElement;

  it("reads Severity · Rule · Location · Symbol · Reachable · Scanner · OWASP · Show · Triage, with no message column", async () => {
    const { root } = await mount(model());
    const headers = within(tableOf(root)).getAllByRole("columnheader").map((h) => h.textContent?.replace(/[↕▲▼]/g, "").trim());
    expect(headers).toEqual(["Severity", "Rule", "Location", "Symbol", "Reachable", "Scanner", "OWASP", "Show", "Triage"]);
  });

  it("carries no message in any row — the detail does", async () => {
    const { root } = await mount(model());
    const table = tableOf(root);
    for (const f of REAL.security.findings.findings) {
      expect(table.textContent).not.toContain(f.message);
    }
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    expect(within(widgetTitled(root, "Finding detail")).getByText("Formatted SQL query.")).toBeInTheDocument();
  });

  it("truncates a rule id with the full id as its tooltip and its accessible name", async () => {
    const id = `rules.${"x".repeat(114)}`;
    expect(id).toHaveLength(120);
    const long = { ...finding("rust.lang.security.formatted-sql"), rule_id: id };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [long] } }));
    const cell = within(tableOf(root)).getByText(id);
    expect(cell).toHaveAttribute("title", id);
    expect(cell.className).toContain(styles.rule);
    // The full id is the text, so it is the name; CSS only clips what is drawn.
    expect(within(tableOf(root)).getByRole("cell", { name: new RegExp(`^${id}$`) })).toBeInTheDocument();
  });

  it("shows a location through PathCell with :line — whole when short, abbreviated when long, the full path as title and name", async () => {
    const long = { ...finding("rust.lang.security.formatted-sql"), path: LONG, report_path: LONG, start_line: 42 };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [long, finding("generic.secrets.hardcoded-key")] } }));
    const table = tableOf(root);
    const longRow = rowOf(table, "rust.lang.security.formatted-sql");
    const path = longRow.querySelector(`[data-path="${LONG}"]`) as HTMLElement;
    expect(path).toHaveAttribute("title", LONG);
    expect(path.querySelector("[aria-hidden='true']")?.textContent).toBe("src/…/deeply-nested-module/handler.rs");
    expect(within(longRow).getByRole("cell", { name: new RegExp(`^${LONG} ?:42$`) })).toBeInTheDocument();
    expect(path.closest("td")?.textContent?.endsWith(":42")).toBe(true);

    const short = rowOf(table, "generic.secrets.hardcoded-key").querySelector("[data-path]") as HTMLElement;
    expect(short.textContent).toBe("src/lib.rs");
    expect(short).toHaveAttribute("title", "src/lib.rs");
  });

  it("reads a finding with no path as 'no location', and one with no line as the bare path", async () => {
    const none = { ...finding("rust.lang.security.formatted-sql"), path: null, report_path: null, start_line: null };
    const noLine = { ...finding("generic.secrets.hardcoded-key"), start_line: null };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [none, noLine] } }));
    const table = tableOf(root);
    expect(within(rowOf(table, "rust.lang.security.formatted-sql")).getByText("no location")).toBeInTheDocument();
    expect(rowOf(table, "generic.secrets.hardcoded-key").querySelector("[data-path]")?.nextSibling).toBeNull();
  });

  it("reads the Symbol as kind and short name, the full symbol as tooltip", async () => {
    const { root } = await mount(model());
    const cell = within(rowOf(tableOf(root), "rust.lang.security.formatted-sql")).getByText("find").closest("[title]") as HTMLElement;
    expect(cell.textContent).toBe("function find");
    expect(cell).toHaveAttribute("title", "logos . project . src/`lib.rs`/find().");
  });

  it("reads an unbound Symbol as — with its reason", async () => {
    const sql = finding("rust.lang.security.formatted-sql");
    const unbound: SecurityFinding = {
      ...sql,
      context: { ...sql.context, enclosing: { confidence: "unbound", symbol: null, unbound_because: "not_indexed" }, blast_radius: { confidence: "unbound" } },
    };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [unbound] } }));
    const cell = within(rowOf(tableOf(root), "rust.lang.security.formatted-sql")).getByText("—").closest("[title]") as HTMLElement;
    expect(cell).toHaveAttribute("title", CONTEXT_TEXT.unbound("not_indexed"));
    expect(cell.textContent).toContain(CONTEXT_TEXT.unbound("not_indexed"));
  });

  it("reads Reachable as yes / no / unknown with the route tooltip — and unknown is never no", async () => {
    const sql = finding("rust.lang.security.formatted-sql");
    const reach = (verdict: "reachable" | "unreachable" | "unknown", route: { symbol: string; name: string; hops: number } | null) =>
      ({ ...sql, rule_id: `r.${verdict}`, context: { ...sql.context, reachability: { confidence: "resolved" as const, verdict, route, unresolved_because: null } } }) as SecurityFinding;
    const findings = [reach("reachable", { symbol: "s", name: "GET /users/:id", hops: 2 }), reach("unreachable", null), reach("unknown", null)];
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings } }));
    const table = tableOf(root);
    const cellOf = (verdict: string) => rowOf(table, `r.${verdict}`).querySelector(`[data-reachable="${verdict}"]`) as HTMLElement;
    expect(cellOf("reachable").textContent).toBe("yes");
    expect(cellOf("reachable")).toHaveAttribute("title", CONTEXT_TEXT.reachable("GET /users/:id", 2));
    expect(cellOf("unreachable").textContent).toBe("no");
    expect(cellOf("unknown").textContent).toBe("unknown");
    expect(cellOf("unknown").textContent).not.toBe("no");
    expect(cellOf("unknown")).toHaveAttribute("title", CONTEXT_TEXT.unknown(null));
    // The tooltip is the story's own sentence, not a copy of the constant.
    expect(FINDINGS_TABLE_TEXT.reachableTip).toBe("an HTTP route handler can call this code");
    expect(within(table).getByText("Reachable")).toHaveAttribute("title", "an HTTP route handler can call this code");
  });

  it("reads the Scanner and the OWASP category in their own columns", async () => {
    const { root } = await mount(model());
    const cells = within(rowOf(tableOf(root), "rust.lang.security.formatted-sql")).getAllByRole("cell").map((c) => c.textContent);
    expect(cells[5]).toBe("Semgrep OSS");
    expect(cells[6]).toBe("A05:2025");
  });
});

describe("the empty blast-radius lists (S-793)", () => {
  it("keeps each list's title above its empty sentence", async () => {
    const empty: ImpactResult = { ...IMPACT, upstream_label: "Breaks if changed", upstream: [], downstream_label: "Depends on", downstream: [] };
    const { root } = await mount(model(), empty);
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    const detail = widgetTitled(root, "Finding detail");
    const nothing = await within(detail).findByText("Nothing depends on it.");
    const none = within(detail).getByText("It depends on nothing.");
    expect(nothing.previousElementSibling?.textContent).toBe("Breaks if changed");
    expect(none.previousElementSibling?.textContent).toBe("Depends on");
  });

  it("keeps a non-empty list's title as its table caption, and no empty sentence", async () => {
    const { root } = await mount(model());
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    const detail = widgetTitled(root, "Finding detail");
    await within(detail).findByText("caller");
    expect(within(detail).getByRole("table", { name: "Depends on it" })).toBeInTheDocument();
    expect(within(detail).queryByText("Nothing depends on it.")).toBeNull();
    expect(within(detail).getByText("It depends on nothing.")).toBeInTheDocument();
  });
});

describe("the finding detail (FR-SC-13)", () => {
  it("states unknown reachability as unknown with its reason, never as unreachable, and fetches the impact lists", async () => {
    const { calls, root } = await mount(model());
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    const detail = widgetTitled(root, "Finding detail");
    const reach = detail.querySelector("[data-reachability]") as HTMLElement;
    expect(reach.dataset.reachability).toBe("unknown");
    expect(readerText(reach)).toContain("Unknown: the file's language binds no call across files.");
    expect(readerText(reach)).not.toMatch(/unreachable/i);
    // The fact is named for what it asks, as the filter and the column are (§4.23).
    expect(within(detail).getByText("Reachable from a route")).toBeInTheDocument();
    expect(within(detail).queryByText("Route reach")).toBeNull();
    expect(readerText(detail)).toContain("find");
    // The lists come from impact on the enclosing symbol.
    expect(await within(detail).findByText("caller")).toBeInTheDocument();
    const seeds = calls.filter((u) => u.startsWith("/api/v1/impact?")).map((u) => new URLSearchParams(u.split("?")[1]).get("seed"));
    expect(seeds).toEqual(["logos . project . src/`lib.rs`/find()."]);
  });

  it("states an unrated finding's reason, and no severity line for a rating (S-768)", async () => {
    const unrated = {
      ...finding("rust.lang.security.formatted-sql"),
      severity: "unrated" as const,
      severity_reason: "the scanner gave none",
    };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [unrated, finding("generic.secrets.hardcoded-key")] } }));
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    expect(readerText(widgetTitled(root, "Finding detail"))).toContain("unrated: the scanner gave none");
    await userEvent.click(screen.getByRole("button", { name: /Show generic\.secrets\.hardcoded-key/ }));
    expect(within(widgetTitled(root, "Finding detail")).queryByText("Severity")).toBeNull();
  });

  it("states that the impact lists are unknown when the symbol no longer resolves — never empty", async () => {
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        calls.push(url);
        const body = url.startsWith("/api/v1/impact")
          ? { ...IMPACT, resolved: null, upstream: [], downstream: [], warnings: ["symbol not found"] }
          : model();
        return Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response);
      }),
    );
    const { container } = render(<SecurityView />);
    await userEvent.click(await screen.findByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    const detail = widgetTitled(container, "Finding detail");
    expect(await within(detail).findByText(CONTEXT_TEXT.impactUnresolved)).toBeInTheDocument();
    expect(within(detail).getByText("symbol not found")).toBeInTheDocument();
    expect(within(detail).queryByText("Nothing depends on it.")).toBeNull();
  });

  it("states the blast radius from the payload's sizes", async () => {
    const { root } = await mount(model());
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    expect(readerText(widgetTitled(root, "Finding detail"))).toContain(CONTEXT_TEXT.blast(0, 0));
  });

  it("states why a finding has no enclosing symbol, and asks impact nothing", async () => {
    const sql = finding("rust.lang.security.formatted-sql");
    const unbound: SecurityFinding = {
      ...sql,
      context: { ...sql.context, enclosing: { confidence: "unbound", symbol: null, unbound_because: "not_indexed" }, blast_radius: { confidence: "unbound" } },
    };
    const { calls, root } = await mount(model({ findings: { ...REAL.security.findings, findings: [unbound] } }));
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang/ }));
    expect(readerText(widgetTitled(root, "Finding detail"))).toContain(CONTEXT_TEXT.unbound("not_indexed"));
    expect(calls.filter((u) => u.startsWith("/api/v1/impact"))).toEqual([]);
  });

  it("states unreachable only when the payload says so", async () => {
    const sql = finding("rust.lang.security.formatted-sql");
    const unreachable: SecurityFinding = {
      ...sql,
      context: { ...sql.context, reachability: { confidence: "resolved", verdict: "unreachable", route: null, unresolved_because: null } },
    };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [unreachable] } }));
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang/ }));
    const reach = widgetTitled(root, "Finding detail").querySelector("[data-reachability]") as HTMLElement;
    expect(reach.dataset.reachability).toBe("unreachable");
    expect(readerText(reach)).toContain(CONTEXT_TEXT.unreachable);
  });

  it("states a reachable finding's route and hops", async () => {
    const sql = finding("rust.lang.security.formatted-sql");
    const reachable: SecurityFinding = {
      ...sql,
      context: {
        ...sql.context,
        reachability: { confidence: "resolved", verdict: "reachable", route: { symbol: "r", name: "GET /users/:id", hops: 2 }, unresolved_because: null },
      },
    };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [reachable] } }));
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang/ }));
    expect(readerText(widgetTitled(root, "Finding detail"))).toContain("Reachable from GET /users/:id, 2 calls away.");
  });
});

describe("showing the finding detail (HF-1)", () => {
  /** Records every `scrollIntoView` call with the element it was called on. */
  function spyScroll() {
    const calls: { el: Element; arg: ScrollIntoViewOptions }[] = [];
    vi.spyOn(Element.prototype, "scrollIntoView").mockImplementation(function (this: Element, arg?: boolean | ScrollIntoViewOptions) {
      calls.push({ el: this, arg: arg as ScrollIntoViewOptions });
    });
    return calls;
  }
  const show = (rule: string) => userEvent.click(screen.getByRole("button", { name: new RegExp(`Show ${rule.replace(/\./g, "\\.")}`) }));

  it("scrolls the detail into view, smoothly, and moves focus to its heading when Show is pressed", async () => {
    const scrolls = spyScroll();
    const focus = vi.spyOn(HTMLElement.prototype, "focus");
    const { root } = await mount(model());
    expect(scrolls).toEqual([]); // nothing on mount: no finding is shown yet
    await show("rust.lang.security.formatted-sql");
    expect(scrolls).toHaveLength(1);
    const detail = widgetTitled(root, "Finding detail");
    expect(detail.closest(`.${styles.detail}`)).toBe(scrolls[0].el);
    expect(scrolls[0].arg).toEqual({ behavior: "smooth", block: "start" });
    const heading = screen.getByRole("heading", { name: "Finding detail" });
    expect(document.activeElement).not.toBe(heading);
    expect(heading.contains(document.activeElement)).toBe(true);
    expect(detail.contains(document.activeElement)).toBe(true);
    // Moved to, never tabbed to; and its ring is drawn inside the clipped title.
    expect(document.activeElement).toHaveAttribute("tabindex", "-1");
    expect(document.activeElement?.classList).toContain(styles.detailTitle);
    // Focus must not jump the page before the smooth scroll starts.
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
  });

  it("scrolls without animation when the user prefers reduced motion", async () => {
    vi.spyOn(window, "matchMedia").mockImplementation(
      (query: string) => ({ matches: query === "(prefers-reduced-motion: reduce)", media: query, addEventListener() {}, removeEventListener() {} }) as unknown as MediaQueryList,
    );
    const scrolls = spyScroll();
    await mount(model());
    await show("rust.lang.security.formatted-sql");
    expect(scrolls.map((s) => s.arg)).toEqual([{ behavior: "auto", block: "start" }]);
  });

  it("scrolls again for a different finding, and again when Show is pressed on the one already shown", async () => {
    const scrolls = spyScroll();
    await mount(model());
    await show("rust.lang.security.formatted-sql");
    await show("generic.secrets.hardcoded-key");
    await show("generic.secrets.hardcoded-key");
    expect(scrolls).toHaveLength(3);
  });

  it("does not scroll or take focus when the page reloads under a finding already shown", async () => {
    const sql = finding("rust.lang.security.formatted-sql");
    const suppressed = { ...sql, suppression: { state: "suppressed" as const, reason: "r", expires: null, by: { fingerprint: sql.fingerprint } } };
    const written = { file: ".logos/security-ignore.toml", entry: { fingerprint: sql.fingerprint, reason: "r", expires: null }, entries: 0 };
    // Two reads of the page — the second returns fresh finding objects, as a real refetch does.
    stubTriage(
      [model({ findings: { ...REAL.security.findings, findings: [suppressed] } }), model({ findings: { ...REAL.security.findings, findings: [{ ...suppressed, suppression: null }] } })],
      { status: 200, body: written },
    );
    const page = globalThis.fetch;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) =>
        url.startsWith("/api/v1/impact") ? Promise.resolve({ ok: true, json: () => Promise.resolve(IMPACT) } as Response) : page(url, init),
      ),
    );
    const scrolls = spyScroll();
    render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });
    await show("rust.lang.security.formatted-sql");
    expect(scrolls).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "Unsuppress rust.lang.security.formatted-sql at src/lib.rs:3" }));
    await screen.findByRole("button", { name: "Suppress rust.lang.security.formatted-sql at src/lib.rs:3" }); // the reloaded page
    expect(scrolls).toHaveLength(1);
  });

  it("scrolls on a change of `shown` only — a remount under a stale count replays nothing", () => {
    // SecurityContent keeps the count while a reload with no findings unmounts the detail.
    const scrolls = spyScroll();
    const view = render(<FindingDetail finding={null} classification={null} shown={1} />);
    expect(scrolls).toEqual([]);
    view.rerender(<FindingDetail finding={null} classification={null} shown={2} />);
    expect(scrolls).toHaveLength(1);
  });

  it("gives each impact section — its title and table, or its empty sentence — its own spaced section", async () => {
    const both: ImpactResult = {
      ...IMPACT,
      upstream_label: "Breaks if changed",
      downstream_label: "Depends on",
      downstream: [{ symbol: "s:callee", name: "callee", kind: "function", file: "src/util.rs", line: 9, distance: 1 }],
    };
    const { root } = await mount(model(), both);
    await show("rust.lang.security.formatted-sql");
    const detail = widgetTitled(root, "Finding detail");
    await within(detail).findByText("caller");
    const sections = Array.from(detail.querySelectorAll<HTMLElement>(`.${styles.impactSection}`));
    expect(sections).toHaveLength(2);
    expect(sections[0].nextElementSibling).toBe(sections[1]); // the `+` selector that spaces them needs them adjacent
    expect(within(sections[0]).getByText("Breaks if changed")).toBeInTheDocument();
    expect(within(sections[1]).getByText("Depends on")).toBeInTheDocument();
  });

  it("spaces an empty-state sentence from the next section's title", async () => {
    const owner: ImpactResult = {
      ...IMPACT,
      upstream: [],
      downstream_label: "Depends on",
      downstream: [{ symbol: "s:callee", name: "callee", kind: "function", file: "src/util.rs", line: 9, distance: 1 }],
    };
    const { root } = await mount(model(), owner);
    await show("rust.lang.security.formatted-sql");
    const detail = widgetTitled(root, "Finding detail");
    const sentence = await within(detail).findByText("Nothing depends on it.");
    const title = within(detail).getByText("Depends on");
    const section = (el: HTMLElement) => el.closest(`.${styles.impactSection}`);
    expect(section(sentence)).not.toBeNull();
    expect(section(title)).not.toBeNull();
    expect(section(sentence)).not.toBe(section(title));
    expect(section(sentence)?.nextElementSibling).toBe(section(title));
  });

  it("leaves the unresolved-impact message unspaced by the section rule: it is one block, not two sections", async () => {
    const gone: ImpactResult = { ...IMPACT, resolved: null, upstream: [], downstream: [], warnings: ["symbol not found"] };
    const { root } = await mount(model(), gone);
    await show("rust.lang.security.formatted-sql");
    const detail = widgetTitled(root, "Finding detail");
    await within(detail).findByText(CONTEXT_TEXT.impactUnresolved);
    expect(detail.querySelector(`.${styles.impactSection}`)).toBeNull();
  });
});

describe("the gate band (FR-SC-10)", () => {
  it("shows PASS with its checked-rule count when every required scan is present and no rule fails", async () => {
    const passing = { ...REAL.gate_band, passed: true, missing_scans: [], violations: [], notes: [], checked_rules: 1 };
    const { root } = await mount(model({ gate: passing }));
    const band = widgetTitled(root, "Security gate");
    expect(within(band).getByText("PASS")).toBeInTheDocument();
    expect(within(band).queryByText("FAIL")).toBeNull();
    expect(readerText(band)).toContain(SECURITY_GATE_TEXT.pass(1));
  });

  it("shows the verdict, each missing scan and each failing finding when a [security] table is configured", async () => {
    const { root } = await mount(model({ gate: REAL.gate_band }));
    const band = widgetTitled(root, "Security gate");
    expectWidgetCopy(band, securityGate);
    expect(within(band).getByText("FAIL")).toBeInTheDocument();
    expect(readerText(band)).toContain("Failing: 1 missing required scan, 2 findings a rule fails on.");
    expect(readerText(band)).toContain("required scanner bandit has no fresh scan at HEAD");
  });
});

describe("the read after Run scan ends (HF-2)", () => {
  /** The page with Bandit alone in the matrix, in `state`, its consent recorded. */
  function banditPage(state: "stale" | "fresh"): SecurityModel {
    const bandit = REAL.security.status.scanners.find((r) => r.scanner === "bandit")!;
    const stored = { snapshot_head: "a".repeat(40), version: "1.7.10", findings: 0, suppressed: 0, not_fresh: 0, test: 0 };
    const row = { ...bandit, ...stored, state, ...(state === "stale" ? { because: "head_moved" } : {}) } as SecurityScannerStatus;
    const status = { ...REAL.security.status, consent: { consented: ["bandit"], pending: [] }, scanners: [row], other_reports: [] };
    return model({ status });
  }

  /** The State cell's text on Bandit's row of the Scanners table. */
  function banditState(root: HTMLElement): string {
    const matrix = within(widgetTitled(root, "Scanners")).getByRole("table");
    const header = within(matrix).getAllByRole("columnheader").findIndex((h) => h.textContent?.startsWith("State"));
    const row = within(matrix).getByText("bandit", { selector: "td" }).closest("tr") as HTMLElement;
    return row.querySelectorAll("td")[header].textContent ?? "";
  }

  it("never shows the stored state read before the scan once the job ended: the row reads `running` until the reload lands, then `fresh`", async () => {
    const completed = scanJob as unknown as ScanJob;
    const running: ScanJob = { ...completed, state: "running", result: null, events: completed.events.slice(0, 5) };
    const [stale, fresh] = [banditPage("stale"), banditPage("fresh")];
    let ended = false;
    let reads = 0;
    // The reload the job's end asks for is answered only when the test lets it.
    let land: () => void = () => {};
    const landed = new Promise<void>((done) => (land = done));
    const answer = (body: unknown) => ({ ok: true, status: 200, json: () => Promise.resolve(body) }) as Response;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init: RequestInit = {}) => {
        if (url.startsWith("/api/v1/security/install")) return Promise.resolve(answer({ job: null }));
        if (url.startsWith("/api/v1/security/scan")) {
          if (init.method === "POST") return Promise.resolve(answer({ joined: false, job: running }));
          return Promise.resolve(answer({ job: ended ? completed : null }));
        }
        reads += 1;
        return reads === 1 ? Promise.resolve(answer(stale)) : landed.then(() => answer(fresh));
      }),
    );
    const view = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });
    expect(banditState(view.container)).toContain("stale");
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    expect(banditState(view.container)).toBe(LIVE_STATE_TEXT.running);

    ended = true;
    // The job's end is seen and the reload asked for; its answer has not landed.
    await vi.waitFor(() => expect(reads).toBe(2), { timeout: 5000 });
    await act(() => new Promise((tick) => setTimeout(tick, 50)));
    expect(banditState(view.container), "the page read before the scan is never shown as its result").toBe(LIVE_STATE_TEXT.running);
    expect(screen.queryByText(SCAN_JOB_TEXT.completed(0, 1)), "the job reads ended once its result is shown").toBeNull();

    land();
    expect(await screen.findByText(SCAN_JOB_TEXT.completed(0, 1))).toBeInTheDocument();
    expect(banditState(view.container)).toBe("fresh");
    expect(reads, "one reload at the end").toBe(2);
  });
});

describe("one advisory several scanners report is one row naming every reporter (FR-SC-29, S-809)", () => {
  /** The SQL finding as one Semgrep and OSV-Scanner both report, under two ids. */
  function withMerged(): { page: SecurityModel; merged: SecurityFinding } {
    const sql = finding("rust.lang.security.formatted-sql");
    const reporter = (scanner_name: string, fingerprint: string, rule_id: string, severity: SecurityFinding["severity"]) => ({
      scanner_name,
      scanner_version: "2.7.0",
      fingerprint,
      rule_id,
      severity,
    });
    const merged: SecurityFinding = {
      ...sql,
      reporters: [reporter(sql.scanner_name, sql.fingerprint, sql.rule_id, sql.severity), reporter("osv-scanner", "osv-key", "GHSA-aaaa-bbbb-cccc", "unrated")],
    };
    const findings = REAL.security.findings.findings.map((f) => (f.fingerprint === sql.fingerprint ? merged : f));
    return { page: model({ findings: { ...REAL.security.findings, findings } }), merged };
  }

  it("lists it once, its scanner cell naming both scanners", async () => {
    const { page, merged } = withMerged();
    const { root } = await mount(page);
    const rows = [...within(widgetTitled(root, "Findings")).getByRole("table").querySelectorAll("tbody tr")] as HTMLElement[];
    expect(rows).toHaveLength(REAL.security.findings.findings.filter((f) => f.code_scope === "production").length);
    const row = rows.find((r) => r.textContent?.includes(merged.rule_id)) as HTMLElement;
    expect(row.textContent).toContain(`${merged.scanner_name}, osv-scanner`);
  });

  it("names each reporter in the detail with what it reported, and a finding one scanner reported names none", () => {
    stub(model());
    const { merged } = withMerged();
    const classification = { categories: [], unclassified: null };
    const view = render(<FindingDetail finding={merged} classification={classification} shown={0} />);
    const reporters = view.container.querySelector("[data-reporters]") as HTMLElement;
    expect([...reporters.querySelectorAll("li")].map((li) => li.textContent)).toEqual([
      REPORTERS_TEXT.reporter(merged.scanner_name, "2.7.0", merged.rule_id, merged.severity),
      REPORTERS_TEXT.reporter("osv-scanner", "2.7.0", "GHSA-aaaa-bbbb-cccc", "unrated"),
    ]);
    expect(reporters.textContent).toContain(REPORTERS_TEXT.note);
    expect(within(view.container).getByText(REPORTERS_TEXT.label).tagName).toBe("DT");
    cleanup();
    const single = render(<FindingDetail finding={finding("rust.lang.security.formatted-sql")} classification={classification} shown={0} />);
    expect(single.container.querySelector("[data-reporters]")).toBeNull();
  });

  it("states a scanner row's findings counted under another scanner", async () => {
    const status = {
      ...REAL.security.status,
      scanners: REAL.security.status.scanners.map((r) => (r.state === "fresh" ? { ...r, merged: 2 } : r)),
    };
    const { root } = await mount(model({ status }));
    expect(readerText(widgetTitled(root, "Scanners"))).toContain("2 counted under another scanner");
  });
});
