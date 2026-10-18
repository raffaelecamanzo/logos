/*
 * ScanJobPanel (S-673, FR-SC-15; S-747, FR-SC-24) — the Security page's Run scan
 * and Install & scan control. A scan, or an install, is a detached job on the
 * server: the click answers at once, the panel follows the job by reading its
 * replayed events every `pollMs`, and every other view keeps answering meanwhile.
 *
 * - While any default scanner is missing (a *not installed* row Logos can
 *   install, not an opt-in one), **Install & scan** replaces Run scan: one
 *   `tool-install` job installs the missing scanners, then runs the full scan.
 * - A row's Install or Update (`request`) runs the same job for that tool alone.
 * - An install asks first in one dialog (`InstallDialog`): the server answers the
 *   start with the plan while a line is unanswered, and the accepted start sends
 *   exactly what the dialog showed and the user ticked. While an install runs, a
 *   second one is not offered (the server refuses it rather than joining: its
 *   request may name other tools), and a row's Install or Update states that
 *   refusal.
 * - A click while a scan runs posts again and joins it: the server answers the
 *   same job, and the panel says it joined. The first scan asks for consent
 *   first: the dialog names the scanners the project's consent lacks (from the
 *   page, or from the server's `409`), and the accepted start sends exactly
 *   those names. A scanner the dialog did not show is asked for again.
 * - Cancel stops every scanner process, or the download running; the previous
 *   results stay, no partial tool is left, and a cancelled install names the
 *   tools it installed.
 * - A job started before the page opened (another tab, a navigation away and
 *   back) is picked up on mount and followed the same way: the newer of the
 *   member's scan and install.
 *
 * When a job this panel followed ends, `onFinished` reloads the page, so the
 * matrix and the findings show what it stored.
 */

import { useEffect, useRef, useState } from "react";

import {
  cancelInstall,
  cancelScan,
  fetchInstallJob,
  fetchScanJob,
  requestDetail,
  startInstall,
  startScan,
} from "../../api/securityClient.ts";
import type {
  InstallJob,
  ScanJob,
  SecurityInstallAsk,
  SecurityInstallPlan,
  SecurityInstallStep,
  SecurityScannerRun,
  SecurityScannerStatus,
  SecurityToolInstall,
} from "../../api/types.ts";
import { Badge, Button, Widget, type BadgeTone } from "../../components/index.ts";
import {
  INSTALL_JOB_TEXT,
  INSTALL_OUTCOME_TEXT,
  SCAN_JOB_TEXT,
  SCAN_OUTCOME_TEXT,
  scanJob,
} from "../../copy/security.copy.ts";
import { ConsentDialog } from "./ConsentDialog.tsx";
import { InstallDialog, type InstallAnswers } from "./InstallDialog.tsx";
import styles from "./SecurityView.module.css";

/** How often a running job is read again. */
export const SCAN_POLL_MS = 750;

/** A row's Install or Update, handed to the panel by the matrix. */
export interface InstallRequest {
  tool: string;
  update: boolean;
}

export interface ScanJobPanelProps {
  /** The scanners the project's consent does not name yet (`status.consent.pending`). */
  pending: readonly string[];
  /** Called once when a job this panel followed ends. */
  onFinished: () => void;
  pollMs?: number;
  /** The matrix rows: a missing default scanner turns Run scan into Install & scan. */
  scanners?: readonly SecurityScannerStatus[];
  /** A row's Install or Update to run; `onRequested` is called once it is taken. */
  request?: InstallRequest | null;
  onRequested?: () => void;
}

type Job = ScanJob | InstallJob;

/** The newer of two jobs: each serve numbers its jobs in order. */
function newer(a: Job | null, b: Job | null): Job | null {
  if (a === null) return b;
  if (b === null) return a;
  return b.id > a.id ? b : a;
}

/** Whether `row` is a default scanner Logos can install here and is missing. */
export function missingDefault(row: SecurityScannerStatus): boolean {
  return row.state === "not_installed" && row.install !== null && !row.opt_in;
}

export function ScanJobPanel({
  pending,
  onFinished,
  pollMs = SCAN_POLL_MS,
  scanners = [],
  request = null,
  onRequested,
}: ScanJobPanelProps) {
  const [job, setJob] = useState<Job | null>(null);
  const [joined, setJoined] = useState(false);
  const [asking, setAsking] = useState<readonly string[]>([]);
  // The install the dialog is open for, and the plan it shows.
  const [installAsk, setInstallAsk] = useState<{ ask: SecurityInstallAsk; plan: SecurityInstallPlan } | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A failed read of a running job: stated, and read again, so one bad poll
  // never leaves the panel showing a job that already ended.
  const [pollError, setPollError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  // The job seen running here, whose end reloads the page once.
  const followed = useRef<number | null>(null);
  const running = job?.state === "running";
  const installing = running && job.kind === "tool-install";
  const missing = scanners.some(missingDefault);
  // Install & scan stays the action while its own job runs, disabled.
  const installAndScan = installing || (!running && missing);

  useEffect(() => {
    let live = true;
    // The state is a convenience on this page: an unreadable one leaves the
    // buttons working and states nothing it does not know.
    for (const read of [fetchScanJob, fetchInstallJob]) {
      read()
        .then((current) => {
          if (live) setJob((shown) => newer(shown, current));
        })
        .catch(() => undefined);
    }
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    if (!job) return;
    if (job.state === "running") {
      followed.current = job.id;
      const read: () => Promise<Job | null> = job.kind === "tool-install" ? fetchInstallJob : fetchScanJob;
      const timer = setTimeout(() => {
        read()
          .then((next) => {
            setPollError(null);
            setJob(next);
          })
          .catch((e: unknown) => {
            setPollError(SCAN_JOB_TEXT.requestFailed(e instanceof Error ? e.message : String(e)));
            setRetry((n) => n + 1);
          });
      }, pollMs);
      return () => clearTimeout(timer);
    }
    if (followed.current === job.id) {
      followed.current = null;
      onFinished();
    }
    return undefined;
  }, [job, retry, pollMs, onFinished]);

  const start = async (consent: readonly string[]) => {
    setAsking([]);
    setError(null);
    try {
      const answer = await startScan(consent);
      if (answer.kind === "consent") {
        setAsking(answer.scanners);
        return;
      }
      setJoined(answer.started.joined);
      setJob(answer.started.job);
    } catch (e) {
      setError(SCAN_JOB_TEXT.requestFailed(requestDetail(e)));
    }
  };

  const install = async (ask: SecurityInstallAsk) => {
    setInstallAsk(null);
    setError(null);
    try {
      const answer = await startInstall(ask);
      if (answer.kind === "consent") {
        setInstallAsk({ ask, plan: answer.plan });
        return;
      }
      setJoined(answer.started.joined);
      setJob(answer.started.job);
    } catch (e) {
      setError(INSTALL_JOB_TEXT.requestFailed(requestDetail(e)));
    }
  };

  // A row's Install or Update: taken once, then run like any install.
  useEffect(() => {
    if (!request) return;
    onRequested?.();
    void install({ tools: [request.tool], update: request.update });
  }, [request, onRequested]);

  const run = () => {
    if (installAndScan) {
      void install({ scan: true });
      return;
    }
    if (!running && pending.length > 0) {
      setAsking([...pending]);
      return;
    }
    void start([]);
  };

  const accepted = (answers: InstallAnswers) => {
    if (installAsk) void install({ ...installAsk.ask, ...answers });
  };

  const cancel = async () => {
    setError(null);
    try {
      const current = job?.kind === "tool-install" ? await cancelInstall() : await cancelScan();
      // The answer is the job as it stood when the cancel landed; a poll that
      // already saw it end is newer, and is never rolled back to running.
      if (current) setJob((shown) => (shown?.id === current.id && shown.state !== "running" ? shown : current));
    } catch (e) {
      setError((job?.kind === "tool-install" ? INSTALL_JOB_TEXT : SCAN_JOB_TEXT).requestFailed(requestDetail(e)));
    }
  };

  const rows = job?.kind === "tool-install" ? installRows(job) : scannerRows(job);
  return (
    <Widget title="Run a scan" copy={scanJob} badge={job && <Badge tone={stateTone(job)}>{job.state}</Badge>} figure={<span>{figure(job, joined)}</span>}>
      <div className={styles.switch} role="group" aria-label="Scan controls">
        <Button variant="primary" size="sm" disabled={installing} onClick={run}>
          {installAndScan ? INSTALL_JOB_TEXT.installAndScan : SCAN_JOB_TEXT.run}
        </Button>
        {running && (
          <Button size="sm" onClick={() => void cancel()}>
            {installing ? INSTALL_JOB_TEXT.cancel : SCAN_JOB_TEXT.cancel}
          </Button>
        )}
      </div>
      {(error ?? pollError) !== null && (
        <p role="alert" className={styles.alert}>
          {error ?? pollError}
        </p>
      )}
      {rows.length > 0 && (
        <ul className={styles.list} aria-label="Scanner progress">
          {rows.map(([key, name, text]) => (
            <li key={key}>
              <span className="mono">{name}</span> — {text}
            </li>
          ))}
        </ul>
      )}
      <ConsentDialog scanners={asking} onAccept={() => void start(asking)} onDecline={() => setAsking([])} />
      <InstallDialog
        plan={installAsk?.plan ?? null}
        scan={installAsk?.ask.scan === true}
        onAccept={accepted}
        onDecline={() => setInstallAsk(null)}
      />
    </Widget>
  );
}

function figure(job: Job | null, joined: boolean): string {
  if (!job) return SCAN_JOB_TEXT.idle;
  return job.kind === "tool-install" ? installFigure(job, joined) : scanFigure(job, joined);
}

function scanFigure(job: ScanJob, joined: boolean): string {
  switch (job.state) {
    case "running":
      return joined ? SCAN_JOB_TEXT.joined : SCAN_JOB_TEXT.running(Math.floor(job.elapsed_ms / 1000));
    case "completed": {
      const ran = job.result?.scanners.filter((s) => s.outcome === "ran").length ?? 0;
      return SCAN_JOB_TEXT.completed(job.result?.findings ?? 0, ran);
    }
    case "cancelled":
      return SCAN_JOB_TEXT.cancelled;
    case "failed":
      return SCAN_JOB_TEXT.failed(job.error ?? "");
  }
}

function installFigure(job: InstallJob, joined: boolean): string {
  const seconds = Math.floor(job.elapsed_ms / 1000);
  switch (job.state) {
    case "running":
      if (joined) return INSTALL_JOB_TEXT.joined;
      return job.events.some(({ event }) => isScanStep(event)) ? INSTALL_JOB_TEXT.scanning(seconds) : INSTALL_JOB_TEXT.running(seconds);
    case "completed": {
      const installed = job.result?.install.tools.filter((t) => t.outcome === "installed").length ?? 0;
      const scan = job.result?.scan ?? null;
      return INSTALL_JOB_TEXT.completed(
        installed,
        scan && { findings: scan.findings, ran: scan.scanners.filter((s) => s.outcome === "ran").length },
      );
    }
    case "cancelled": {
      const report = job.events.map(({ event }) => event).find((e) => e.phase === "cancelled");
      return INSTALL_JOB_TEXT.cancelled(report?.phase === "cancelled" ? report.installed : []);
    }
    case "failed":
      return INSTALL_JOB_TEXT.failed(job.error ?? "");
  }
}

function stateTone(job: Job): BadgeTone {
  if (job.state === "completed") return installLeftAToolOut(job) ? "orange" : "green";
  if (job.state === "failed") return "red";
  if (job.state === "running") return "orange";
  return "muted";
}

/** A completed install that left a tool failed or unavailable did not finish
 *  clean, whatever its scan did: its badge says so, not only its rows. */
function installLeftAToolOut(job: Job): boolean {
  return (
    job.kind === "tool-install" &&
    (job.result?.install.tools.some((t) => t.outcome === "failed" || t.outcome === "unavailable") ?? false)
  );
}

function isScanStep(step: SecurityInstallStep): step is Extract<SecurityInstallStep, { scanner: string }> {
  return step.phase === "started" || step.phase === "finished";
}

/** Each scanner that started, with its latest state, in the order it started.
 *  Once the job has ended, one that never settled did not finish. */
function scannerRows(job: ScanJob | null): Row[] {
  return scanRows(job?.events.map((e) => e.event) ?? [], job !== null && job.state !== "running");
}

/** A progress row: its key, the tool or scanner it names, and its state. */
type Row = [string, string, string];

/** The scan's per-scanner rows from its steps. */
function scanRows(steps: readonly SecurityInstallStep[], ended: boolean): Row[] {
  const rows = new Map<string, string>();
  for (const event of steps) {
    if (!isScanStep(event)) continue;
    if (event.phase === "started") rows.set(event.scanner, SCAN_JOB_TEXT.scannerRunning);
    else if (rows.has(event.scanner)) rows.set(event.scanner, outcomeText(event));
  }
  return [...rows].map(([scanner, text]): Row => [
    `scan:${scanner}`,
    scanner,
    ended && text === SCAN_JOB_TEXT.scannerRunning ? SCAN_JOB_TEXT.scannerUnfinished : text,
  ]);
}

/** Each tool the install started, with its latest state, then the scan's
 *  scanners as they ran. */
function installRows(job: InstallJob): Row[] {
  const tools = new Map<string, string>();
  for (const { event } of job.events) {
    if (event.phase === "install_started") tools.set(event.tool, INSTALL_OUTCOME_TEXT.installing);
    else if (event.phase === "install_finished") tools.set(event.tool, installText(event));
  }
  const installs = [...tools].map(([tool, text]): Row => [`install:${tool}`, tool, text]);
  return [...installs, ...scanRows(job.events.map((e) => e.event), job.state !== "running")];
}

/** A settled tool's line: the outcomes that carry a reason are formatted, the
 *  others are their own copy key. */
function installText(tool: SecurityToolInstall): string {
  if (tool.outcome === "unavailable") return INSTALL_OUTCOME_TEXT.unavailable(tool.reason);
  if (tool.outcome === "failed") return INSTALL_OUTCOME_TEXT.failed(tool.stage, tool.reason);
  return INSTALL_OUTCOME_TEXT[tool.outcome];
}

function outcomeText(run: SecurityScannerRun): string {
  switch (run.outcome) {
    case "ran":
      return SCAN_OUTCOME_TEXT.ran(run.findings);
    case "failed":
      return SCAN_OUTCOME_TEXT.failed(run.exit_code, run.log);
    case "requires_build":
      return SCAN_OUTCOME_TEXT.requires_build;
    case "not_applicable":
      return SCAN_OUTCOME_TEXT.not_applicable(run.reason);
    case "not_installed":
      return SCAN_OUTCOME_TEXT.not_installed;
    case "disabled":
      return SCAN_OUTCOME_TEXT.disabled;
    case "not_selected":
      return SCAN_OUTCOME_TEXT.not_selected;
  }
}
