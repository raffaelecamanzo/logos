// SecurityView over a mocked `/api/v1/security` (S-670, FR-SC-09): the absent
// states name their fixing commands, no snippet is rendered and a secret-class
// message reads [redacted], the edition switch re-classifies without a refetch,
// `?path=` narrows the table, and the detail states reachability honestly.
// Pinned to the real payload `web/tests/api_v1.rs` produced.
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ImpactResult, SecurityEntryStatus, SecurityFinding, SecurityGateBand, SecurityModel, SecurityPage } from "../../api/types.ts";
import { expectWidgetCopy, readerText } from "../../copy/expectWidgetCopy.ts";
import {
  ANCHOR_TEXT,
  CONTEXT_TEXT,
  COPY_TEXT,
  IGNORED_PATHS_TEXT,
  FINDING_COUNT_TEXT,
  INSTALL_JOB_TEXT,
  OWASP_TEXT,
  REDACTED,
  SECURITY_ABSENCE,
  SECURITY_GATE_TEXT,
  SUPPRESS_TEXT,
  SUPPRESSIONS_TEXT,
  UNCLASSIFIED_TEXT,
  findingDetail,
  owaspView,
  scanJob,
  scannerMatrix,
  securityFindings,
  securityGate,
} from "../../copy/security.copy.ts";
import { SCOPED_ONLY_COUNTED, SCOPED_ONLY_EXCLUDED } from "../../test/scopedOnlyProject.ts";
import { expectOneWidgetStack, widgetTitle, widgetTitled } from "../../test/widgetStack.ts";
import real from "./fixtures/security.real.json";
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
      // The scan panel's own reads (S-673, S-747) are not the page's reads.
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

describe("SecurityView over the real payload", () => {
  it("renders one stack of catalogued widgets, the gate band left out without a table", async () => {
    const { root } = await mount(model());
    const widgets = expectOneWidgetStack(root);
    expect(widgets.map(widgetTitle)).toEqual(["Run a scan", "Scanners", "Findings", "OWASP Top 10", "Finding detail"]);
    expectWidgetCopy(widgetTitled(root, "Run a scan"), scanJob);
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
    expect(readerText(rowOf("gitleaks"))).toContain(`${COPY_TEXT.managed} ${COPY_TEXT.updateAvailable("8.19.0")}`);
    expect(readerText(rowOf("osv-scanner"))).toContain(COPY_TEXT.override);
    expect(readerText(rowOf("bandit"))).toContain(COPY_TEXT.path);
    expect(within(rowOf("trivy")).getByRole("button", { name: "Install trivy" })).toBeInTheDocument();
    expect(within(rowOf("gitleaks")).getByRole("button", { name: "Update gitleaks" })).toBeInTheDocument();
    // A copy that is not Logos's, or at its pin, has nothing to install or update;
    // a scanner Logos cannot install here offers no Install.
    for (const name of ["osv-scanner", "bandit", "semgrep", "codeql"]) {
      expect(within(rowOf(name)).queryByRole("button"), name).toBeNull();
    }
    // While a default scanner is missing, the panel's action is Install & scan.
    expect(within(widgetTitled(root, "Run a scan")).getByRole("button", { name: INSTALL_JOB_TEXT.installAndScan })).toBeInTheDocument();
  });

  it("states each scanner's coverage of the ignored directories, with the note when it read them (FR-SC-26)", async () => {
    const [semgrep, trivy, gitleaks, ...rest] = REAL.security.status.scanners;
    const note = "it reads the committed history, which holds any file committed inside an ignored directory";
    const scanners = [
      { ...semgrep, ignored_paths: "excluded" },
      trivy,
      { ...gitleaks, ignored_paths: "scanned", coverage_note: note },
      ...rest,
    ];
    const { root } = await mount(model({ status: { ...REAL.security.status, scanners } as SecurityPage["status"] }));
    const matrix = widgetTitled(root, "Scanners");
    expect(within(matrix).getByRole("columnheader", { name: "Ignored paths" })).toBeInTheDocument();
    const cellOf = (name: string) => {
      const row = within(matrix).getByText(name, { selector: "td" }).closest("tr") as HTMLElement;
      const header = within(matrix).getAllByRole("columnheader").findIndex((h) => h.textContent === "Ignored paths");
      return readerText(row.querySelectorAll("td")[header] as HTMLElement);
    };
    expect(cellOf("semgrep")).toBe(IGNORED_PATHS_TEXT.excluded);
    expect(cellOf("gitleaks")).toBe(`${IGNORED_PATHS_TEXT.scanned} ${note}`);
    // Not installed here: no whole-project run to state, never a guess.
    expect(cellOf("trivy")).toBe(IGNORED_PATHS_TEXT.absent);
  });

  it("reads a scanner with nothing to scan as not applicable, and names a failed scan's cause (S-769)", async () => {
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
      return readerText(row.querySelectorAll("td")[header] as HTMLElement);
    };
    expect(cellOf("gosec", "State")).toBe("not applicable");
    expect(cellOf("gosec", "What it means")).toBe("Nothing to scan: no Go source file (.go) among the files the project does not ignore.");
    expect(cellOf("dependency-check", "What it means")).toContain(
      "last scan failed [ERROR] Error updating the NVD Data — the output is in .logos/security/dependency-check.failed.log.",
    );
    // A row without a failure log says nothing of one.
    expect(cellOf("semgrep", "What it means")).not.toContain("last scan failed");
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

describe("no snippet, and secrets redacted (FR-SC-03, ADR-79 §2)", () => {
  it("renders a secret-class message as [redacted] in the table and the detail, and never a snippet", async () => {
    const secret = { ...finding("generic.secrets.hardcoded-key"), message: `key=${PLANTED}` };
    // A snippet the server never sends: proof the page renders no such field.
    const withSnippet = { ...finding("rust.lang.security.formatted-sql"), snippet: PLANTED } as SecurityFinding;
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [withSnippet, secret] } }));
    expect(root.textContent).not.toContain(PLANTED);
    const table = within(widgetTitled(root, "Findings")).getByRole("table");
    expect(within(table).getAllByText(REDACTED)).toHaveLength(1);

    await userEvent.click(screen.getByRole("button", { name: /Show generic\.secrets\.hardcoded-key/ }));
    expect(within(widgetTitled(root, "Finding detail")).getByText(REDACTED)).toBeInTheDocument();
    expect(root.textContent).not.toContain(PLANTED);
    expect(root.querySelector("pre, code[class*='snippet']")).toBeNull();
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
  it("badges a suppressed finding's row, and only that row", async () => {
    const secret = { ...finding("generic.secrets.hardcoded-key"), suppression: { state: "suppressed" as const, reason: "test key", expires: null, by: { fingerprint: finding("generic.secrets.hardcoded-key").fingerprint } } };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [finding("rust.lang.security.formatted-sql"), secret] } }));
    const table = within(widgetTitled(root, "Findings")).getByRole("table");
    const rowOf = (loc: string) => within(table).getByText(loc).closest("tr") as HTMLElement;
    expect(within(rowOf("src/lib.rs:6")).getByText("suppressed")).toBeInTheDocument();
    expect(within(rowOf("src/lib.rs:3")).queryByText("suppressed")).toBeNull();
  });

  it("labels a suppressed finding with its reason, and an expired one with when it lapsed (S-672)", async () => {
    const secret = { ...finding("generic.secrets.hardcoded-key"), suppression: { state: "suppressed" as const, reason: "rotated test key", expires: "2027-01-31", by: { fingerprint: finding("generic.secrets.hardcoded-key").fingerprint } } };
    const sql = { ...finding("rust.lang.security.formatted-sql"), suppression: { state: "expired" as const, reason: "legacy path", expires: "2026-01-31", by: { fingerprint: finding("rust.lang.security.formatted-sql").fingerprint } } };
    const { root } = await mount(model({ findings: { ...REAL.security.findings, findings: [sql, secret] } }));
    const table = within(widgetTitled(root, "Findings")).getByRole("table");
    const rowOf = (loc: string) => within(table).getByText(loc).closest("tr") as HTMLElement;
    expect(rowOf("src/lib.rs:6").querySelector("[data-suppression-reason]")?.textContent).toBe("rotated test key (until 2027-01-31)");
    expect(rowOf("src/lib.rs:3").querySelector("[data-suppression-reason]")?.textContent).toBe("legacy path (expired after 2026-01-31)");
    // Either way the entry exists, so the action removes it.
    expect(within(rowOf("src/lib.rs:6")).getByRole("button", { name: /^Unsuppress / })).toBeInTheDocument();
    expect(within(rowOf("src/lib.rs:3")).getByRole("button", { name: /^Unsuppress / })).toBeInTheDocument();
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
      const isTriage = url.startsWith("/api/v1/security/suppress") || url.startsWith("/api/v1/security/unsuppress");
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

describe("triage (S-672, FR-SC-12)", () => {
  const sqlLoc = "src/lib.rs:3";

  it("suppresses from the dialog with an intent-guarded POST, then reloads and shows the reason", async () => {
    const sql = finding("rust.lang.security.formatted-sql");
    const after = model({
      findings: { ...REAL.security.findings, findings: [{ ...sql, suppression: { state: "suppressed", reason: "constant query", expires: null, by: { fingerprint: sql.fingerprint } } }] },
    });
    const written = { file: ".logos/security-ignore.toml", entry: { fingerprint: sql.fingerprint, reason: "constant query", expires: null }, entries: 1 };
    const sent = stubTriage([model({ findings: { ...REAL.security.findings, findings: [sql] } }), after], { status: 200, body: written });
    const view = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Suppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    const dialog = screen.getByRole("dialog", { name: SUPPRESS_TEXT.title });
    expect(dialog.textContent).toContain(".logos/security-ignore.toml");
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "constant query");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));

    const reason = await screen.findByText("constant query");
    expect(reason).toHaveAttribute("data-suppression-reason");
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

describe("path suppressions (S-770)", () => {
  const sqlLoc = "src/lib.rs:3";
  const under = { path: "src" };
  /** Both findings, each suppressed by the one `src` path entry when `entry` holds it. */
  function page(entry: SecurityEntryStatus | null, extra: SecurityEntryStatus[] = []): SecurityModel {
    const by = entry ? { state: "suppressed" as const, reason: "generated", expires: null, by: under } : null;
    const findings = [finding("rust.lang.security.formatted-sql"), finding("generic.secrets.hardcoded-key")].map((f) => ({ ...f, suppression: by }));
    return model({ findings: { ...REAL.security.findings, findings, suppressions: [...(entry ? [entry] : []), ...extra] } });
  }
  const entry: SecurityEntryStatus = { entry: { path: "src", reason: "generated", expires: null }, expired: false, orphaned: false, suppressed: 2, should_name: null };
  const orphan: SecurityEntryStatus = { entry: { path: "tests/fixtures", scanner: "gitleaks", reason: "planted", expires: null }, expired: false, orphaned: true, suppressed: 0, should_name: null };

  it("suppresses everything under a path from the dialog, and Unsuppress sends back the same selector", async () => {
    const written = { file: ".logos/security-ignore.toml", entry: entry.entry, entries: 1 };
    const sent = stubTriage([page(null), page(entry), page(null)], { status: 200, body: written });
    const { container: root } = render(<SecurityView />);
    await screen.findByRole("heading", { name: "Scanners" });

    await userEvent.click(screen.getByRole("button", { name: `Suppress rust.lang.security.formatted-sql at ${sqlLoc}` }));
    const dialog = screen.getByRole("dialog", { name: SUPPRESS_TEXT.title });
    await userEvent.click(within(dialog).getByRole("radio", { name: SUPPRESS_TEXT.scopePath }));
    await userEvent.type(within(dialog).getByRole("textbox", { name: SUPPRESS_TEXT.reason }), "generated");
    await userEvent.click(within(dialog).getByRole("button", { name: SUPPRESS_TEXT.confirm }));

    // Both rows the entry covers say so, and the entry is listed with its count.
    const table = await within(widgetTitled(root, "Findings")).findByRole("table");
    await vi.waitFor(() => expect(table.querySelectorAll("[data-suppression-reason]").length).toBe(2));
    for (const note of table.querySelectorAll("[data-suppression-reason]")) {
      expect(note.textContent).toBe("generated — covers everything under src");
    }
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
    const lapsed: SecurityEntryStatus = { entry: { fingerprint: fp, reason: "rotated", expires: "2026-01-31" }, expired: true, orphaned: false, suppressed: 0, should_name: null };
    const renamed: SecurityEntryStatus = { entry: { fingerprint: now.replace("f", "e"), reason: "old key", expires: null }, expired: false, orphaned: true, suppressed: 0, should_name: now };
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
    const { root } = await mount(model({ counts: { counted, excluded: { not_fresh: 7, suppressed: 9 } } }));
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
    expect(within(widgetTitled(root, "Findings")).getAllByText("src/lib.rs:3")).toHaveLength(1);
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

  it("narrows by route reach", async () => {
    const { root } = await mount(model());
    await userEvent.selectOptions(screen.getByLabelText("Route reach"), "unknown");
    expect(readerText(widgetTitled(root, "Findings"))).toContain("2 of 2 findings shown");
    await userEvent.selectOptions(screen.getByLabelText("Route reach"), "reachable");
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

describe("the finding detail (FR-SC-13)", () => {
  it("states unknown reachability as unknown with its reason, never as unreachable, and fetches the impact lists", async () => {
    const { calls, root } = await mount(model());
    await userEvent.click(screen.getByRole("button", { name: /Show rust\.lang\.security\.formatted-sql/ }));
    const detail = widgetTitled(root, "Finding detail");
    const reach = detail.querySelector("[data-reachability]") as HTMLElement;
    expect(reach.dataset.reachability).toBe("unknown");
    expect(readerText(reach)).toContain("Unknown: the file's language binds no call across files.");
    expect(readerText(reach)).not.toMatch(/unreachable/i);
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
