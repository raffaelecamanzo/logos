// The scan job on the Scanners widget over a mocked scan job (S-673, S-792,
// FR-SC-15, FR-SC-07): consent is asked before the first scan and sent with the
// accepted start, a second click while running joins the same job, the job is
// followed to its end and the page reloaded once, cancel posts the cancel, a
// running job is picked up on mount, and a refused request states the server's
// reason. While a job runs, the row of each scanner running reads `running` and
// the row of each tool installing reads `installing`, and one `aria-live` line
// counts what finished. Pinned to the real completed jobs
// `web/tests/security_job.rs` and `web/tests/security_install_job.rs` produced.
import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { InstallJob, ScanJob, SecurityInstallPlan, SecurityScannerStatus, SecurityStatus } from "../../api/types.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import {
  CONSENT_TEXT,
  INSTALL_CONSENT_TEXT,
  INSTALL_JOB_TEXT,
  INSTALL_OUTCOME_TEXT,
  LIVE_STATE_TEXT,
  SCAN_JOB_TEXT,
  scannerMatrix,
} from "../../copy/security.copy.ts";
import { widgetTitled } from "../../test/widgetStack.ts";
import installReal from "./fixtures/install-job.real.json";
import planReal from "./fixtures/install-plan.real.json";
import real from "./fixtures/scan-job.real.json";
import { ScannersWidget } from "./ScannersWidget.tsx";

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

/** Bandit installed on the PATH, never run: the scanner the real jobs run. */
const BANDIT = row("bandit", "never_run", {
  installed: { copy: "path", binary: "/usr/bin/bandit", version: "1.7.10", update_available: null },
  install: null,
});

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

function mount(
  pending: readonly string[] = [],
  onFinished = vi.fn(),
  scanners: SecurityScannerStatus[] = [BANDIT],
  others: SecurityScannerStatus[] = [],
) {
  const status = { head_sha: null, consent: { consented: [], pending }, scanners, other_reports: others, suppressions: [] } as unknown as SecurityStatus;
  const view = render(<ScannersWidget status={status} onFinished={onFinished} pollMs={5} />);
  return { onFinished, root: view.container };
}

/** The State cell of the row of `name` in the Scanners table. */
function stateOf(name: string): string {
  const table = screen.getByRole("table", { name: "Scanners" });
  const row = within(table).getByText(name, { selector: "td" }).closest("tr") as HTMLElement;
  const header = within(table).getAllByRole("columnheader").findIndex((h) => h.textContent?.startsWith("State"));
  return (row.querySelectorAll("td")[header] as HTMLElement).textContent ?? "";
}

/** The one polite summary line. */
const summary = () => document.querySelector("p[aria-live='polite']") as HTMLElement;

describe("Scan controls on the Scanners widget", () => {
  it("states that no scan ran yet, through the widget's catalogue entry", async () => {
    stub(() => ({ body: { job: null } }));
    const { root } = mount();
    expect(await screen.findByText(SCAN_JOB_TEXT.idle)).toBeInTheDocument();
    expectWidgetCopy(widgetTitled(root, "Scanners"), scannerMatrix);
  });

  it("puts Run scan on the widget's title row, with no separate widget", async () => {
    stub(() => ({ body: { job: null } }));
    const { root } = mount();
    const widget = widgetTitled(root, "Scanners");
    const title = widget.querySelector('[data-widget-part="title"]') as HTMLElement;
    expect(within(title).getByRole("button", { name: SCAN_JOB_TEXT.run })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Run a scan" })).toBeNull();
    expect(screen.queryByRole("list", { name: "Scanner progress" }), "the progress list is gone").toBeNull();
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
    await screen.findByText(/Scan running/);
    // The second click, while running, posts again and joins.
    await userEvent.click(screen.getByRole("button", { name: SCAN_JOB_TEXT.run }));
    expect(await screen.findByText(SCAN_JOB_TEXT.joined, { exact: false })).toBeInTheDocument();
    expect(screen.queryByRole("dialog"), "consent is not asked while a scan runs").toBeNull();
    expect(posts(calls).map((c) => JSON.parse(c.body!))).toEqual([{ consent: ["bandit"] }, { consent: [] }]);
  });

  it("shows the scanner running as `running`, counts the finished ones in one live line, and reloads the page once at the end", async () => {
    // The scan stays at Bandit until the test lets it end.
    let [started, ends] = [false, false];
    stub((c) => {
      if (c.method === "POST") {
        started = true;
        return { body: { joined: false, job: RUNNING } };
      }
      return { body: { job: ends ? COMPLETED : started ? RUNNING : null } };
    });
    const { onFinished } = mount([]);
    expect(stateOf("bandit")).toBe("never run" + "Installed but never run; run logos security scan.");
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    // Four scanners had finished and Bandit had started: its row reads running.
    expect(await screen.findByText(SCAN_JOB_TEXT.running(4, ["bandit"]))).toBeInTheDocument();
    expect(stateOf("bandit")).toBe(LIVE_STATE_TEXT.running);
    expect(summary().textContent).toContain(SCAN_JOB_TEXT.running(4, ["bandit"]));
    expect(onFinished, "nothing reloads while it runs").not.toHaveBeenCalled();
    ends = true;

    expect(await screen.findByText(SCAN_JOB_TEXT.completed(0, 1))).toBeInTheDocument();
    expect(stateOf("bandit"), "the job ended: the row reads its stored state").not.toBe(LIVE_STATE_TEXT.running);
    expect(onFinished).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: SCAN_JOB_TEXT.cancel }), "no cancel once it ended").toBeNull();
  });

  it("shows the job ended only once the reload its end asked for has landed (HF-2)", async () => {
    let ends = false;
    stub((c) => (c.method === "POST" ? { body: { joined: false, job: RUNNING } } : { body: { job: ends ? COMPLETED : RUNNING } }));
    let land: () => void = () => {};
    const onFinished = vi.fn(() => new Promise<void>((done) => (land = done)));
    mount([], onFinished);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    expect(await screen.findByText(SCAN_JOB_TEXT.running(4, ["bandit"]))).toBeInTheDocument();
    ends = true;

    await vi.waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
    expect(stateOf("bandit"), "the row's stored state is the page read before the scan").toBe(LIVE_STATE_TEXT.running);
    expect(summary().textContent).toContain(SCAN_JOB_TEXT.running(4, ["bandit"]));

    act(() => land());
    expect(await screen.findByText(SCAN_JOB_TEXT.completed(0, 1))).toBeInTheDocument();
    expect(stateOf("bandit")).not.toBe(LIVE_STATE_TEXT.running);
    expect(onFinished).toHaveBeenCalledTimes(1);
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
    expect(await screen.findByText(SCAN_JOB_TEXT.cancelled([]))).toBeInTheDocument();
    // Bandit started and was killed: it is never shown running once the job ended.
    expect(stateOf("bandit")).not.toBe(LIVE_STATE_TEXT.running);
    const cancel = posts(calls).find((c) => c.url === "/api/v1/security/scan/cancel");
    expect(cancel?.intent).toBe("test-intent-token");
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("names the scanners that finished before a cancel", async () => {
    const afterBandit: ScanJob = { ...COMPLETED, state: "cancelled", result: null, events: COMPLETED.events.slice(0, 6) };
    stub(() => ({ body: { job: afterBandit } }));
    mount([]);
    expect(await screen.findByText(SCAN_JOB_TEXT.cancelled(["bandit"]))).toBeInTheDocument();
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
    expect(await screen.findByText(SCAN_JOB_TEXT.cancelled([]))).toBeInTheDocument();
    await new Promise((done) => setTimeout(done, 200));
    expect(screen.getByText(SCAN_JOB_TEXT.cancelled([]))).toBeInTheDocument();
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  /** An `onFinished` whose reload lands only when the test calls `land`. */
  function heldReload() {
    const held = { land: () => {} };
    const onFinished = vi.fn(() => new Promise<void>((done) => (held.land = done)));
    return { onFinished, land: () => act(() => held.land()) };
  }

  /** Lets `ms` pass in short steps, so each late answer commits as it lands. */
  async function pass(ms: number) {
    for (let t = 0; t < ms; t += 10) await act(() => new Promise((tick) => setTimeout(tick, 10)));
  }

  it("shows an end the cancel is first to see only once the reload landed, and reloads once (HF-2 review)", async () => {
    let cancelled = false;
    stub((c) => {
      if (c.url.endsWith("/cancel")) {
        cancelled = true;
        // The job ended between two polls: the cancel's answer is the first to see it.
        return { body: { job: CANCELLED } };
      }
      if (c.method === "POST") return { body: { joined: false, job: RUNNING } };
      return { body: { job: cancelled ? CANCELLED : RUNNING } };
    });
    const { onFinished, land } = heldReload();
    mount([], onFinished);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.cancel }));
    await vi.waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
    await pass(60);
    expect(summary().textContent, "a poll after the cancel does not show the end first").toContain(SCAN_JOB_TEXT.running(4, ["bandit"]));

    land();
    expect(await screen.findByText(SCAN_JOB_TEXT.cancelled([]))).toBeInTheDocument();
    await pass(60);
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("reloads once when a poll sent before the cancel answers `running` after it (HF-2 review)", async () => {
    let cancelled = false;
    stub((c) => {
      if (c.url.endsWith("/cancel")) {
        cancelled = true;
        return { body: { job: CANCELLED } };
      }
      if (c.method === "POST") return { body: { joined: false, job: RUNNING } };
      // Each read is answered as the job stood when it was sent, and late: the
      // read in flight when the cancel lands answers `running` after the end is shown.
      return { body: { job: cancelled ? CANCELLED : RUNNING }, delayMs: 100 };
    });
    const onFinished = vi.fn(() => Promise.resolve());
    mount([], onFinished);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    await screen.findByRole("button", { name: SCAN_JOB_TEXT.cancel });
    await pass(20);
    await userEvent.click(screen.getByRole("button", { name: SCAN_JOB_TEXT.cancel }));
    await pass(500);
    expect(screen.getByText(SCAN_JOB_TEXT.cancelled([]))).toBeInTheDocument();
    expect(onFinished, "an ended job is never followed again").toHaveBeenCalledTimes(1);
  });

  it("reloads once when the cancel's `running` answer lands while the end's reload is on its way (HF-2 review)", async () => {
    let cancelled = false;
    stub((c) => {
      if (c.url.endsWith("/cancel")) {
        cancelled = true;
        return { body: { job: RUNNING }, delayMs: 150 };
      }
      if (c.method === "POST") return { body: { joined: false, job: RUNNING } };
      return { body: { job: cancelled ? CANCELLED : RUNNING } };
    });
    const { onFinished, land } = heldReload();
    mount([], onFinished);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.cancel }));
    await vi.waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
    await pass(400);
    land();
    expect(await screen.findByText(SCAN_JOB_TEXT.cancelled([]))).toBeInTheDocument();
    await pass(60);
    expect(onFinished, "one reload for one end").toHaveBeenCalledTimes(1);
  });

  it("keeps following a scan started while the previous end's reload is on its way (HF-2 review)", async () => {
    const NEXT: ScanJob = { ...RUNNING, id: RUNNING.id + 1 };
    let [starts, ended] = [0, false];
    stub((c) => {
      if (c.method === "POST") {
        starts += 1;
        return { body: { joined: false, job: starts === 1 ? RUNNING : NEXT } };
      }
      // The newer scan's reads do not answer within the test: only the hook decides what is shown.
      if (starts >= 2) return { body: { job: NEXT }, delayMs: 10_000 };
      return { body: { job: ended ? COMPLETED : RUNNING } };
    });
    const { onFinished, land } = heldReload();
    mount([], onFinished);
    await userEvent.click(await screen.findByRole("button", { name: SCAN_JOB_TEXT.run }));
    await screen.findByText(SCAN_JOB_TEXT.running(4, ["bandit"]));
    ended = true;
    await vi.waitFor(() => expect(onFinished).toHaveBeenCalledTimes(1));
    await userEvent.click(screen.getByRole("button", { name: SCAN_JOB_TEXT.run }));
    await vi.waitFor(() => expect(starts).toBe(2));
    await pass(30);
    land();
    await pass(60);
    expect(summary().textContent, "the earlier end never replaces the newer scan").toContain(SCAN_JOB_TEXT.running(4, ["bandit"]));
    expect(screen.getByRole("button", { name: SCAN_JOB_TEXT.cancel })).toBeInTheDocument();
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

  it("reads only the registered scanner as running, never a report stored under the same name", async () => {
    stub(() => ({ body: { job: RUNNING } }));
    const report = { scanner: "bandit", driver: "bandit", state: "never_run", installed: null, install: null, last_failure: null } as unknown as SecurityScannerStatus;
    mount([], vi.fn(), [BANDIT], [report]);
    await screen.findByText(SCAN_JOB_TEXT.running(4, ["bandit"]));
    const table = screen.getByRole("table", { name: "Scanners" });
    const states = within(table)
      .getAllByText("bandit", { selector: "td" })
      .map((td) => (td.closest("tr")!.querySelectorAll("td")[1] as HTMLElement).textContent);
    expect(states.filter((s) => s === LIVE_STATE_TEXT.running)).toHaveLength(1);
    expect(states.filter((s) => s?.startsWith("never run"))).toHaveLength(1);
  });

  it("re-attaches to a running scan: its scanner reads running at once", async () => {
    stub(() => ({ body: { job: RUNNING } }));
    mount([]);
    expect(await screen.findByText(SCAN_JOB_TEXT.running(4, ["bandit"]))).toBeInTheDocument();
    expect(stateOf("bandit")).toBe(LIVE_STATE_TEXT.running);
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

  it("reads the summary line as one polite live region, which a running scan keeps updating", async () => {
    stub(() => ({ body: { job: RUNNING } }));
    mount([]);
    await screen.findByText(SCAN_JOB_TEXT.running(4, ["bandit"]));
    expect(document.querySelectorAll("[aria-live]")).toHaveLength(1);
    expect(summary().getAttribute("aria-live")).toBe("polite");
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
      // The install stays at Gitleaks' download until the test lets it end.
      let ends = false;
      const calls = stub(
        () => ({ body: { job: null } }),
        (c) => {
          if (c.method === "POST" && !JSON.parse(c.body!).consent) return { status: 409, body: { error: "consent first", plan: PLAN } };
          if (c.method === "POST") {
            started = true;
            return { body: { joined: false, job: INSTALLING } };
          }
          if (!started) return { body: { job: null } };
          return { body: { job: ends ? INSTALLED : INSTALLING } };
        },
      );
      const { onFinished } = mount([], vi.fn(), [row("trivy", "not_installed"), row("gitleaks", "not_installed")]);
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
      // Trivy had landed and Gitleaks was downloading: its row reads installing.
      expect(await screen.findByText(INSTALL_JOB_TEXT.running(1))).toBeInTheDocument();
      expect(stateOf("gitleaks")).toBe(LIVE_STATE_TEXT.installing);
      expect(stateOf("trivy")).not.toBe(LIVE_STATE_TEXT.installing);
      ends = true;
      expect(await screen.findByText(INSTALL_JOB_TEXT.completed(4, { findings: 0, ran: 4 }))).toBeInTheDocument();
      expect(stateOf("gitleaks")).not.toBe(LIVE_STATE_TEXT.installing);
      expect(onFinished).toHaveBeenCalledTimes(1);
    });

    it("reads each scanner of the scan after an install as running in turn, and counts the finished ones", async () => {
      const midScan: InstallJob = { ...INSTALLED, state: "running", result: null, events: INSTALLED.events.slice(0, 12) };
      stub(() => ({ body: { job: null } }), () => ({ body: { job: midScan } }));
      mount([], vi.fn(), [row("trivy", "never_run"), row("gitleaks", "never_run")]);
      // Gitleaks started its scan; the installs, Semgrep's skip and Trivy's run finished.
      expect(await screen.findByText(INSTALL_JOB_TEXT.scanning(2))).toBeInTheDocument();
      expect(stateOf("gitleaks")).toBe(LIVE_STATE_TEXT.running);
      expect(stateOf("trivy")).not.toBe(LIVE_STATE_TEXT.running);
    });

    it("says an install was joined in the install's words, and only while it runs", async () => {
      let ends = false;
      stub(
        () => ({ body: { job: null } }),
        (c) => (c.method === "POST" ? { body: { joined: true, job: INSTALLING } } : { body: { job: ends ? INSTALLED : INSTALLING } }),
      );
      mount([], vi.fn(), [row("trivy", "not_installed")]);
      // The server answers the row's Install with the install already running.
      await userEvent.click(await screen.findByRole("button", { name: "Install trivy" }));
      expect(await screen.findByText(INSTALL_JOB_TEXT.joined, { exact: false })).toBeInTheDocument();
      expect(summary().textContent).not.toContain(SCAN_JOB_TEXT.joined);
      ends = true;
      await screen.findByText(INSTALL_JOB_TEXT.completed(4, { findings: 0, ran: 4 }));
      expect(summary().textContent).not.toContain(INSTALL_JOB_TEXT.joined);
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
      const { onFinished } = mount([], vi.fn(), [row("gitleaks", "not_installed")]);
      expect(await screen.findByText(INSTALL_JOB_TEXT.running(1))).toBeInTheDocument();
      // A second install is not offered while this one runs.
      expect(screen.getByRole("button", { name: INSTALL_JOB_TEXT.installAndScan })).toBeDisabled();
      await userEvent.click(screen.getByRole("button", { name: INSTALL_JOB_TEXT.cancel }));
      expect(await screen.findByText(INSTALL_JOB_TEXT.cancelled(["trivy 0.74.0"]))).toBeInTheDocument();
      // Gitleaks' download was stopped and left nothing: it is no longer installing.
      expect(stateOf("gitleaks")).not.toBe(LIVE_STATE_TEXT.installing);
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

    it("runs a row's Install or Update once, through the same dialog", async () => {
      const calls = stub(
        () => ({ body: { job: null } }),
        (c) => (c.method === "POST" ? { status: 409, body: { error: "consent first", plan: { lines: [PLAN.lines[0]], kept: [], scan_consent: [] } } } : { body: { job: null } }),
      );
      const managed = row("trivy", "never_run", {
        installed: { copy: "managed", binary: "/home/user/.local/share/tools/trivy/0.74.0/trivy", version: "0.74.0", update_available: "0.75.0" },
        install: null,
      });
      mount([], vi.fn(), [managed]);
      await userEvent.click(await screen.findByRole("button", { name: "Update trivy" }));
      const dialog = await screen.findByRole("dialog", { name: INSTALL_CONSENT_TEXT.title });
      expect(within(dialog).queryByRole("list", { name: "Scanners that may reach the network" })).toBeNull();
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
      const events = INSTALLED.events.map((e) =>
        e.event.phase === "install_finished" && e.event.tool === "trivy"
          ? { ...e, event: { phase: "install_finished" as const, tool: "trivy", version: "0.74.0", outcome: "failed" as const, stage: "checksum" as const, reason: "sha256 mismatch" } }
          : e,
      );
      const failed: InstallJob = { ...INSTALLED, events, result: { ...INSTALLED.result!, install: { ...INSTALLED.result!.install, tools } } };
      stub(() => ({ body: { job: null } }), () => ({ body: { job: failed } }));
      mount([]);
      const badge = await screen.findByText("completed");
      expect(badge.className).toMatch(/orange/);
      expect(badge.className).not.toMatch(/green/);
      // The tool left out, and why, stays in the line the list used to hold.
      expect(summary().textContent).toContain(INSTALL_OUTCOME_TEXT.failed("trivy", "checksum", "sha256 mismatch"));
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

    it("states each tool an install left out with its own reason and stage", async () => {
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
      expect(await screen.findByText(INSTALL_JOB_TEXT.cancelled([]), { exact: false })).toBeInTheDocument();
      const line = summary().textContent ?? "";
      expect(line).toContain(INSTALL_OUTCOME_TEXT.unavailable("trivy", "no build for this platform"));
      expect(line).toContain(INSTALL_OUTCOME_TEXT.failed("gitleaks", "checksum", "sha256 mismatch"));
      expect(line, "an install that landed is not listed as left out").not.toContain("gosec");
    });
  });
});
