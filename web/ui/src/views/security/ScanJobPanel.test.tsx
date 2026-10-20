// ScanJobPanel over a mocked scan job (S-673, FR-SC-15, FR-SC-07): consent is
// asked before the first scan and sent with the accepted start, a second click
// while running joins the same job, the job is followed to its end and the page
// reloaded once, cancel posts the cancel, a running job is picked up on mount,
// and a refused request states the server's reason. Pinned to the real completed
// job `web/tests/security_job.rs` produced.
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { InstallJob, ScanJob, SecurityInstallPlan, SecurityScannerStatus } from "../../api/types.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import {
  CONSENT_TEXT,
  INSTALL_CONSENT_TEXT,
  INSTALL_JOB_TEXT,
  INSTALL_OUTCOME_TEXT,
  SCAN_JOB_TEXT,
  SCAN_OUTCOME_TEXT,
  scanJob,
} from "../../copy/security.copy.ts";
import { widgetTitled } from "../../test/widgetStack.ts";
import installReal from "./fixtures/install-job.real.json";
import planReal from "./fixtures/install-plan.real.json";
import real from "./fixtures/scan-job.real.json";
import { ScanJobPanel, type InstallRequest } from "./ScanJobPanel.tsx";

const COMPLETED = real as unknown as ScanJob;

/** The completed job as it stood while Bandit was still running. */
const RUNNING: ScanJob = {
  ...COMPLETED,
  state: "running",
  result: null,
  events: COMPLETED.events.filter((e) => !(e.event.phase === "finished" && e.event.scanner === "bandit")).slice(0, 5),
};

const CANCELLED: ScanJob = { ...RUNNING, state: "cancelled" };

/** The real Install & scan job `web/tests/security_install_job.rs` produced
 *  (its temporary tools directory renamed), and the plan its first start got. */
const INSTALLED: InstallJob = { ...(installReal as unknown as InstallJob), id: 7 };
const PLAN = planReal as unknown as SecurityInstallPlan;

/** The install as it stood while Gitleaks downloaded, newer than any scan here. */
const INSTALLING: InstallJob = { ...INSTALLED, state: "running", result: null, events: INSTALLED.events.slice(0, 3) };

/** Cancelled during Gitleaks' download: Trivy had landed. */
const INSTALL_CANCELLED: InstallJob = {
  ...INSTALLING,
  state: "cancelled",
  events: [
    ...INSTALLING.events,
    { seq: 3, at_ms: 900, event: { phase: "install_finished", tool: "gitleaks", version: "8.18.4", outcome: "cancelled" } },
    { seq: 4, at_ms: 900, event: { phase: "cancelled", installed: ["trivy 0.74.0"] } },
  ],
};

/** A matrix row in `state`, installable here unless `install` is null. */
function row(scanner: string, state: "not_installed" | "never_run", extra: Partial<SecurityScannerStatus> = {}): SecurityScannerStatus {
  return { scanner, driver: scanner, state, installed: null, install: `logos security install ${scanner}`, last_failure: null, ...extra } as SecurityScannerStatus;
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
  /** Hold the reply back this long. */
  delayMs?: number;
}

/** Stubs fetch with `route` for the scan's endpoints and `install` for the
 *  install's (S-747; by default, no install ever ran), recording every call. */
function stub(route: (call: Call) => Reply, install: (call: Call) => Reply = () => ({ body: { job: null } })): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init: RequestInit = {}) => {
      const call: Call = {
        method: init.method ?? "GET",
        url,
        body: typeof init.body === "string" ? init.body : null,
        intent: new Headers(init.headers).get("x-logos-intent"),
      };
      calls.push(call);
      const reply = call.url.startsWith("/api/v1/security/install") ? install(call) : route(call);
      const status = reply.status ?? 200;
      const answer = (value: Response) =>
        reply.delayMs ? new Promise<Response>((done) => setTimeout(() => done(value), reply.delayMs)) : Promise.resolve(value);
      return answer({
        ok: status >= 200 && status < 300,
        status,
        // A fresh object per read, as a real response body is.
        json: () => Promise.resolve(structuredClone(reply.body)),
        text: () => Promise.resolve(typeof reply.body === "string" ? reply.body : JSON.stringify(reply.body)),
      } as Response);
    }),
  );
  return calls;
}

const posts = (calls: Call[]) => calls.filter((c) => c.method === "POST");

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function mount(pending: readonly string[] = [], onFinished = vi.fn(), scanners: SecurityScannerStatus[] = []) {
  const view = render(<ScanJobPanel pending={pending} onFinished={onFinished} pollMs={5} scanners={scanners} />);
  return { onFinished, root: view.container };
}

describe("ScanJobPanel", () => {
  it("states that no scan ran yet, through its catalogue entry", async () => {
    stub(() => ({ body: { job: null } }));
    const { root } = mount();
    expect(await screen.findByText(SCAN_JOB_TEXT.idle)).toBeInTheDocument();
    expectWidgetCopy(widgetTitled(root, "Run a scan"), scanJob);
  });

  it("asks consent naming the scanners before the first scan, and sends it with the accepted start", async () => {
    let started = false;
    const calls = stub((c) => {
      if (c.method === "POST") {
        started = true;
        return { body: { joined: false, job: RUNNING } };
      }
      return { body: { job: started ? COMPLETED : null } };
    });
    mount(["bandit", "semgrep"]);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));

    const dialog = await screen.findByRole("dialog", { name: CONSENT_TEXT.title });
    const listed = within(within(dialog).getByRole("list")).getAllByRole("listitem").map((li) => li.textContent);
    expect(listed).toEqual(["bandit", "semgrep"]);
    expect(posts(calls), "nothing starts before the answer").toEqual([]);

    await userEvent.click(within(dialog).getByRole("button", { name: CONSENT_TEXT.accept }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(posts(calls)).toHaveLength(1);
    expect(posts(calls)[0]).toMatchObject({ url: "/api/v1/security/scan", intent: "test-intent-token" });
    expect(JSON.parse(posts(calls)[0].body!)).toEqual({ consent: ["bandit", "semgrep"] });
  });

  it("starts nothing when the consent is declined", async () => {
    const calls = stub(() => ({ body: { job: null } }));
    mount(["bandit"]);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    await userEvent.click(await screen.findByRole("button", { name: CONSENT_TEXT.decline }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(posts(calls)).toEqual([]);
  });

  it("opens the dialog with the server's scanners when the start answers 409, and accepts exactly those", async () => {
    const calls = stub((c) =>
      c.method === "POST" && JSON.parse(c.body!).consent.length === 0
        ? { status: 409, body: { error: "a scan needs consent first", consent_required: ["trivy", "bandit"] } }
        : c.method === "POST"
          ? { body: { joined: false, job: RUNNING } }
          : { body: { job: null } },
    );
    mount([]);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("trivy")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: CONSENT_TEXT.accept }));
    expect(posts(calls).map((c) => JSON.parse(c.body!))).toEqual([{ consent: [] }, { consent: ["trivy", "bandit"] }]);
  });

  it("joins the running job on a second click, without asking consent again", async () => {
    let starts = 0;
    const calls = stub((c) => {
      if (c.method === "POST") {
        starts += 1;
        return { body: { joined: starts > 1, job: RUNNING } };
      }
      return { body: { job: starts > 0 ? RUNNING : null } };
    });
    mount(["bandit"]);
    // The first click asks; the accepted start runs.
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    await userEvent.click(await screen.findByRole("button", { name: CONSENT_TEXT.accept }));
    await screen.findByText(/Scanning/);
    // The second click, while running, posts again and joins.
    await userEvent.click(screen.getByRole("button", { name: SCAN_JOB_TEXT.run }));
    expect(await screen.findByText(SCAN_JOB_TEXT.joined)).toBeInTheDocument();
    expect(screen.queryByRole("dialog"), "consent is not asked while a scan runs").toBeNull();
    expect(posts(calls).map((c) => JSON.parse(c.body!))).toEqual([{ consent: ["bandit"] }, { consent: [] }]);
  });

  it("follows the job to its end, shows each scanner's outcome and reloads the page once", async () => {
    let polls = 0;
    stub((c) => {
      if (c.method === "POST") return { body: { joined: false, job: RUNNING } };
      polls += 1;
      return { body: { job: polls > 2 ? COMPLETED : polls > 1 ? RUNNING : null } };
    });
    const { onFinished } = mount([]);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    const progress = await screen.findByRole("list", { name: "Scanner progress" });
    expect(within(progress).getByText(/bandit/).parentElement!.textContent).toContain(SCAN_JOB_TEXT.scannerRunning);

    expect(await screen.findByText(SCAN_JOB_TEXT.completed(0, 1))).toBeInTheDocument();
    const done = screen.getByRole("list", { name: "Scanner progress" });
    expect(done.textContent).toContain(`bandit — ${SCAN_OUTCOME_TEXT.ran(0)}`);
    // Only what started is listed: the scanners not installed here are not rows.
    expect(within(done).getAllByRole("listitem")).toHaveLength(1);
    expect(onFinished).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: SCAN_JOB_TEXT.cancel }), "no cancel once it ended").toBeNull();
  });

  it("keeps following the job after a failed read, and reloads once when it ends", async () => {
    let polls = 0;
    stub((c) => {
      if (c.method === "POST") return { body: { joined: false, job: RUNNING } };
      polls += 1;
      if (polls === 1) return { body: { job: null } };
      if (polls === 2) return { status: 503, body: "busy" };
      return { body: { job: COMPLETED } };
    });
    const { onFinished } = mount([]);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    expect(await screen.findByText(SCAN_JOB_TEXT.completed(0, 1))).toBeInTheDocument();
    expect(onFinished).toHaveBeenCalledTimes(1);
    expect(polls).toBeGreaterThanOrEqual(3);
    expect(screen.queryByRole("alert"), "a read that succeeds again clears the failure").toBeNull();
  });

  it("cancels the running job and states that the previous results are kept", async () => {
    let cancelled = false;
    const calls = stub((c) => {
      if (c.url.endsWith("/cancel")) {
        cancelled = true;
        return { body: { job: RUNNING } };
      }
      if (c.method === "POST") return { body: { joined: false, job: RUNNING } };
      return { body: { job: cancelled ? CANCELLED : RUNNING } };
    });
    const { onFinished } = mount([]);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.cancel }));
    expect(await screen.findByText(SCAN_JOB_TEXT.cancelled)).toBeInTheDocument();
    // Bandit started and was killed: it did not finish, and is never shown running.
    const progress = screen.getByRole("list", { name: "Scanner progress" });
    expect(progress.textContent).toContain(`bandit — ${SCAN_JOB_TEXT.scannerUnfinished}`);
    expect(progress.textContent).not.toContain(SCAN_JOB_TEXT.scannerRunning);
    const cancel = posts(calls).find((c) => c.url === "/api/v1/security/scan/cancel");
    expect(cancel?.intent).toBe("test-intent-token");
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("never rolls an ended job back to running when the cancel's answer arrives late", async () => {
    let cancelled = false;
    stub((c) => {
      if (c.url.endsWith("/cancel")) {
        cancelled = true;
        // Taken as the cancel landed, answered after the polls saw the end.
        return { body: { job: RUNNING }, delayMs: 80 };
      }
      if (c.method === "POST") return { body: { joined: false, job: RUNNING } };
      return { body: { job: cancelled ? CANCELLED : RUNNING } };
    });
    const { onFinished } = mount([]);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.cancel }));
    expect(await screen.findByText(SCAN_JOB_TEXT.cancelled)).toBeInTheDocument();
    await new Promise((done) => setTimeout(done, 200));
    expect(screen.getByText(SCAN_JOB_TEXT.cancelled)).toBeInTheDocument();
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("picks up a scan that was already running when the page opened", async () => {
    let polls = 0;
    stub(() => {
      polls += 1;
      return { body: { job: polls > 1 ? COMPLETED : RUNNING } };
    });
    const { onFinished } = mount([]);
    expect(await screen.findByText(SCAN_JOB_TEXT.completed(0, 1))).toBeInTheDocument();
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("does not reload for a scan that had already ended before the page opened", async () => {
    stub(() => ({ body: { job: COMPLETED } }));
    const { onFinished } = mount([]);
    expect(await screen.findByText(SCAN_JOB_TEXT.completed(0, 1))).toBeInTheDocument();
    expect(onFinished).not.toHaveBeenCalled();
  });

  it("states a failed scan's reason, and a refused start's", async () => {
    stub((c) => (c.method === "POST" ? { status: 500, body: { error: "invalid .logos/config.toml" } } : { body: { job: null } }));
    mount([]);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    expect((await screen.findByRole("alert")).textContent).toContain("invalid .logos/config.toml");

    cleanup();
    stub(() => ({ body: { job: { ...COMPLETED, state: "failed", result: null, error: "the job panicked" } } }));
    mount([]);
    expect(await screen.findByText(SCAN_JOB_TEXT.failed("the job panicked"))).toBeInTheDocument();
  });

  it("shows a failed scanner's one-line cause in its progress row (S-769)", async () => {
    const at = COMPLETED.events.length;
    const failed: ScanJob = {
      ...COMPLETED,
      events: [
        ...COMPLETED.events,
        { seq: at, at_ms: 500, event: { phase: "started", scanner: "dependency-check" } },
        {
          seq: at + 1,
          at_ms: 900,
          event: {
            phase: "finished",
            scanner: "dependency-check",
            driver: "dependency-check",
            version: "13.0.0",
            outcome: "failed",
            exit_code: 1,
            log: ".logos/security/dependency-check.failed.log",
            stderr: "[ERROR] Unable to continue dependency-check analysis.",
            cause: "[ERROR] Error updating the NVD Data",
          },
        },
      ],
    } as ScanJob;
    stub(() => ({ body: { job: failed } }));
    mount([]);
    const progress = await screen.findByRole("list", { name: "Scanner progress" });
    expect(within(progress).getByText("dependency-check", { selector: "span" }).parentElement!.textContent).toContain(
      "failed (exit 1): [ERROR] Error updating the NVD Data; its output is in .logos/security/dependency-check.failed.log",
    );
  });

  it("states a failed scanner with its exit code, its cause and its log", () => {
    expect(SCAN_OUTCOME_TEXT.failed(3, ".logos/security/bandit.failed.log", "ERROR: no rules")).toBe(
      "failed (exit 3): ERROR: no rules; its output is in .logos/security/bandit.failed.log",
    );
    expect(SCAN_OUTCOME_TEXT.failed(null, null, "could not start bandit")).toBe("failed: could not start bandit");
  });

  describe("Install & scan (S-747)", () => {
    it("replaces Run scan while a default scanner is missing, never for an opt-in one alone", async () => {
      stub(() => ({ body: { job: null } }));
      mount([], vi.fn(), [row("trivy", "not_installed"), row("codeql", "not_installed", { opt_in: true })]);
      expect(await screen.findByRole("button", { name: INSTALL_JOB_TEXT.installAndScan })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: SCAN_JOB_TEXT.run })).toBeNull();

      cleanup();
      stub(() => ({ body: { job: null } }));
      mount([], vi.fn(), [row("trivy", "never_run"), row("codeql", "not_installed", { opt_in: true }), row("semgrep", "not_installed", { install: null })]);
      expect(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: INSTALL_JOB_TEXT.installAndScan })).toBeNull();
    });

    it("asks once with the server's plan, sends the dialog's answers, follows the install and its scan, and reloads once", async () => {
      let started = false;
      let polls = 0;
      const calls = stub(
        () => ({ body: { job: null } }),
        (c) => {
          if (c.method === "POST" && !JSON.parse(c.body!).consent) return { status: 409, body: { error: "consent first", plan: PLAN } };
          if (c.method === "POST") {
            started = true;
            return { body: { joined: false, job: INSTALLING } };
          }
          if (!started) return { body: { job: null } };
          polls += 1;
          return { body: { job: polls > 1 ? INSTALLED : INSTALLING } };
        },
      );
      const { onFinished } = mount([], vi.fn(), [row("trivy", "not_installed")]);
      await userEvent.click(await screen.findByRole("button", { name: INSTALL_JOB_TEXT.installAndScan }));
      const dialog = await screen.findByRole("dialog", { name: INSTALL_CONSENT_TEXT.title });
      expect(within(dialog).getByRole("checkbox", { name: /^codeql/ })).not.toBeChecked();
      await userEvent.click(within(dialog).getByRole("button", { name: INSTALL_CONSENT_TEXT.acceptAndScan }));

      expect(posts(calls).map((c) => [c.url, JSON.parse(c.body!)])).toEqual([
        ["/api/v1/security/install", { scan: true }],
        [
          "/api/v1/security/install",
          {
            scan: true,
            consent: ["trivy 0.74.0", "gitleaks 8.18.4", "osv-scanner 1.9.2", "gosec 2.21.4"],
            opt_in: [],
            accept_licences: [],
            scan_consent: ["trivy", "gitleaks", "osv-scanner", "gosec"],
          },
        ],
      ]);
      expect(posts(calls).every((c) => c.intent === "test-intent-token")).toBe(true);
      expect(await screen.findByText(INSTALL_JOB_TEXT.completed(4, { findings: 0, ran: 4 }))).toBeInTheDocument();
      const progress = screen.getByRole("list", { name: "Scanner progress" });
      expect(progress.textContent).toContain(`gosec — ${INSTALL_OUTCOME_TEXT.installed}`);
      expect(progress.textContent).toContain(`gosec — ${SCAN_OUTCOME_TEXT.ran(0)}`);
      expect(onFinished).toHaveBeenCalledTimes(1);
    });

    it("cancels the running install and names the tools it installed", async () => {
      let cancelled = false;
      const calls = stub(
        () => ({ body: { job: null } }),
        (c) => {
          if (c.url.endsWith("/cancel")) {
            cancelled = true;
            return { body: { job: INSTALLING } };
          }
          return { body: { job: cancelled ? INSTALL_CANCELLED : INSTALLING } };
        },
      );
      const { onFinished } = mount();
      expect(await screen.findByText(INSTALL_JOB_TEXT.running(Math.floor(INSTALLING.elapsed_ms / 1000)))).toBeInTheDocument();
      // A second install is not offered while this one runs.
      expect(screen.getByRole("button", { name: INSTALL_JOB_TEXT.installAndScan })).toBeDisabled();
      await userEvent.click(screen.getByRole("button", { name: INSTALL_JOB_TEXT.cancel }));
      expect(await screen.findByText(INSTALL_JOB_TEXT.cancelled(["trivy 0.74.0"]))).toBeInTheDocument();
      const progress = screen.getByRole("list", { name: "Scanner progress" });
      expect(progress.textContent).toContain(`gitleaks — ${INSTALL_OUTCOME_TEXT.cancelled}`);
      expect(posts(calls).map((c) => [c.url, c.intent])).toEqual([["/api/v1/security/install/cancel", "test-intent-token"]]);
      expect(onFinished).toHaveBeenCalledTimes(1);
    });

    it("follows the newer of the member's scan and install on mount, whichever answers first", async () => {
      // An older completed scan answers at once; the running install later.
      stub(
        () => ({ body: { job: { ...COMPLETED, id: 3 } } }),
        () => ({ body: { job: INSTALLING }, delayMs: 40 }),
      );
      mount();
      expect(await screen.findByRole("button", { name: INSTALL_JOB_TEXT.cancel })).toBeInTheDocument();

      cleanup();
      // A newer scan beats an older install that answers first.
      stub(
        () => ({ body: { job: { ...COMPLETED, id: 9 } }, delayMs: 40 }),
        () => ({ body: { job: { ...INSTALLED, id: 2 } } }),
      );
      mount();
      expect(await screen.findByText(SCAN_JOB_TEXT.completed(0, 1))).toBeInTheDocument();
    });

    it("runs a row's Install or Update request once, through the same dialog", async () => {
      const calls = stub(
        () => ({ body: { job: null } }),
        (c) => (c.method === "POST" ? { status: 409, body: { error: "consent first", plan: { lines: [PLAN.lines[0]], kept: [], scan_consent: [] } } } : { body: { job: null } }),
      );
      const onRequested = vi.fn();
      const request: InstallRequest = { tool: "trivy", update: true };
      render(<ScanJobPanel pending={[]} onFinished={vi.fn()} pollMs={5} request={request} onRequested={onRequested} />);
      const dialog = await screen.findByRole("dialog", { name: INSTALL_CONSENT_TEXT.title });
      expect(within(dialog).queryByRole("list", { name: "Scanners that may reach the network" })).toBeNull();
      expect(onRequested).toHaveBeenCalledTimes(1);
      await userEvent.click(within(dialog).getByRole("button", { name: INSTALL_CONSENT_TEXT.accept }));
      expect(posts(calls).map((c) => JSON.parse(c.body!))).toEqual([
        { tools: ["trivy"], update: true },
        { tools: ["trivy"], update: true, consent: ["trivy 0.74.0"], opt_in: [], accept_licences: [], scan_consent: [] },
      ]);
    });

    it("badges a completed install green only when no tool failed or was unavailable", async () => {
      stub(() => ({ body: { job: null } }), () => ({ body: { job: INSTALLED } }));
      mount([]);
      expect((await screen.findByText("completed")).className).toMatch(/green/);

      cleanup();
      const tools = INSTALLED.result!.install.tools.map((t, i) =>
        i === 0 ? { tool: t.tool, version: t.version, outcome: "failed" as const, stage: "checksum" as const, reason: "sha256 mismatch" } : t,
      );
      const failed: InstallJob = { ...INSTALLED, result: { ...INSTALLED.result!, install: { ...INSTALLED.result!.install, tools } } };
      stub(() => ({ body: { job: null } }), () => ({ body: { job: failed } }));
      mount([]);
      const badge = await screen.findByText("completed");
      expect(badge.className).toMatch(/orange/);
      expect(badge.className).not.toMatch(/green/);
    });

    it("states a refused install, such as one while a scan runs", async () => {
      stub(
        () => ({ body: { job: null } }),
        (c) => (c.method === "POST" ? { status: 409, body: { error: "a scan is running on this project" } } : { body: { job: null } }),
      );
      mount([], vi.fn(), [row("trivy", "not_installed")]);
      await userEvent.click(await screen.findByRole("button", { name: INSTALL_JOB_TEXT.installAndScan }));
      expect((await screen.findByRole("alert")).textContent).toBe(INSTALL_JOB_TEXT.requestFailed("a scan is running on this project"));
    });

    it("states each settled tool's outcome with its own reason and stage", async () => {
      const settled: InstallJob = {
        ...INSTALL_CANCELLED,
        events: [
          { seq: 0, at_ms: 10, event: { phase: "install_started", tool: "trivy", version: "0.74.0" } },
          { seq: 1, at_ms: 20, event: { phase: "install_finished", tool: "trivy", version: "0.74.0", outcome: "unavailable", reason: "no build for this platform" } },
          { seq: 2, at_ms: 30, event: { phase: "install_started", tool: "gitleaks", version: "8.18.4" } },
          { seq: 3, at_ms: 40, event: { phase: "install_finished", tool: "gitleaks", version: "8.18.4", outcome: "failed", stage: "checksum", reason: "sha256 mismatch" } },
          { seq: 4, at_ms: 50, event: { phase: "install_started", tool: "gosec", version: "2.21.4" } },
          { seq: 5, at_ms: 60, event: { phase: "install_finished", tool: "gosec", version: "2.21.4", outcome: "already_present", binary: "/usr/local/bin/gosec" } },
          { seq: 6, at_ms: 70, event: { phase: "cancelled", installed: [] } },
        ],
      };
      stub(
        () => ({ body: { job: null } }),
        () => ({ body: { job: settled } }),
      );
      mount([]);
      expect(await screen.findByText(INSTALL_JOB_TEXT.cancelled([]))).toBeInTheDocument();
      const progress = screen.getByRole("list", { name: "Scanner progress" });
      expect(progress.textContent).toContain(`trivy — ${INSTALL_OUTCOME_TEXT.unavailable("no build for this platform")}`);
      expect(progress.textContent).toContain(`gitleaks — ${INSTALL_OUTCOME_TEXT.failed("checksum", "sha256 mismatch")}`);
      expect(progress.textContent).toContain(`gosec — ${INSTALL_OUTCOME_TEXT.already_present}`);
    });
  });
});
