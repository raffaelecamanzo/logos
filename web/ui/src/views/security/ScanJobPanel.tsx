/*
 * The Security page's scan job (S-673, FR-SC-15; S-747, FR-SC-24; S-792, CR-245).
 * A scan, or an install, is a detached job on the server: the click answers at
 * once, the page follows the job by reading its replayed events every `pollMs`,
 * and every other view keeps answering meanwhile. `useScanJob` holds that state
 * for the Scanners widget, which renders three parts of it: `ScanControls` in
 * its title row, `ScanSummary` — the one polite `aria-live` line — above the
 * table, and each row's live state (`liveStates`, `securityModel.ts`).
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
 *   same job, and the summary line says it joined. The first scan asks for consent
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
 * When a job this page followed ends, `onFinished` reloads the page, so the
 * matrix and the findings show what it stored. The job is shown ended only
 * once that reload has landed: the stored states the page held were read
 * before the job ran, and a row that just scanned never reads its old `stale`
 * as the scan's result (HF-2).
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
  SecurityInstallAsk,
  SecurityInstallPlan,
  SecurityScannerStatus,
} from "../../api/types.ts";
import { Badge, Button, type BadgeTone } from "../../components/index.ts";
import { INSTALL_JOB_TEXT, SCAN_JOB_TEXT } from "../../copy/security.copy.ts";
import { ConsentDialog } from "./ConsentDialog.tsx";
import { InstallDialog, type InstallAnswers } from "./InstallDialog.tsx";
import { summaryText, type Job } from "./securityModel.ts";
import styles from "./SecurityView.module.css";

/** How often a running job is read again. */
export const SCAN_POLL_MS = 750;

/** A row's Install or Update, handed to the job by the matrix. */
export interface InstallRequest {
  tool: string;
  update: boolean;
}

export interface ScanJobProps {
  /** The scanners the project's consent does not name yet (`status.consent.pending`). */
  pending: readonly string[];
  /** Called once when a job this page followed ends: the page's reload. The
   *  job is shown ended once what it returns settles. */
  onFinished: () => void | Promise<void>;
  pollMs?: number;
  /** The matrix rows: a missing default scanner turns Run scan into Install & scan. */
  scanners?: readonly SecurityScannerStatus[];
  /** A row's Install or Update to run; `onRequested` is called once it is taken. */
  request?: InstallRequest | null;
  onRequested?: () => void;
}

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

/** The followed job and what the page does with it. */
export interface ScanJobHandle {
  job: Job | null;
  /** Whether the last start joined a job already running. */
  joined: boolean;
  running: boolean;
  installing: boolean;
  /** Install & scan is the action: while a default scanner is missing, and
   *  while its own job runs (disabled). */
  installAndScan: boolean;
  /** A refused or failed request, or a failed read of the running job. */
  alert: string | null;
  /** Run scan, or Install & scan; asks for consent first when it is needed. */
  run: () => void;
  cancel: () => void;
  /** The consent dialog: the scanners it names, empty when closed. */
  consent: { scanners: readonly string[]; accept: () => void; decline: () => void };
  /** The install dialog: the plan it shows, `null` when closed. */
  install: { plan: SecurityInstallPlan | null; scan: boolean; accept: (answers: InstallAnswers) => void; decline: () => void };
}

export function useScanJob({
  pending,
  onFinished,
  pollMs = SCAN_POLL_MS,
  scanners = [],
  request = null,
  onRequested,
}: ScanJobProps): ScanJobHandle {
  const [job, setJob] = useState<Job | null>(null);
  const [joined, setJoined] = useState(false);
  const [asking, setAsking] = useState<readonly string[]>([]);
  // The install the dialog is open for, and the plan it shows.
  const [installAsk, setInstallAsk] = useState<{ ask: SecurityInstallAsk; plan: SecurityInstallPlan } | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A failed read of a running job: stated, and read again, so one bad poll
  // never leaves the page showing a job that already ended.
  const [pollError, setPollError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  // The job seen running here, whose end reloads the page once.
  const followed = useRef<number | null>(null);
  // The followed job that ended, while the page's reload is on its way.
  const ending = useRef<number | null>(null);
  const running = job?.state === "running";
  const installing = running && job.kind === "tool-install";
  const missing = scanners.some(missingDefault);
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

  // The job as read. A followed job's end is shown only once the reload it
  // asks for has landed: until then each row keeps its live state, since the
  // stored state the page holds was read before the job ran (HF-2).
  const show = (next: Job | null) => {
    // The job whose end is being shown: a poll or cancel read of it adds nothing.
    if (next !== null && ending.current === next.id) return;
    if (next === null || next.state === "running" || followed.current !== next.id) {
      // A read sent before the job ended is older than its end: never rolled back to running.
      setJob((shown) => (next !== null && shown?.id === next.id && shown.state !== "running" ? shown : next));
      return;
    }
    followed.current = null;
    ending.current = next.id;
    void Promise.resolve(onFinished()).finally(() => {
      ending.current = null;
      // A job started meanwhile is the newer one: this end never replaces it.
      setJob((shown) => (shown !== null && shown.id !== next.id ? newer(shown, next) : next));
    });
  };

  useEffect(() => {
    if (job?.state !== "running") return;
    followed.current = job.id;
    const read: () => Promise<Job | null> = job.kind === "tool-install" ? fetchInstallJob : fetchScanJob;
    const timer = setTimeout(() => {
      read()
        .then((next) => {
          setPollError(null);
          show(next);
        })
        .catch((e: unknown) => {
          setPollError(SCAN_JOB_TEXT.requestFailed(e instanceof Error ? e.message : String(e)));
          setRetry((n) => n + 1);
        });
    }, pollMs);
    return () => clearTimeout(timer);
    // The timer calls the `show` of the render that armed it, and so that render's
    // `onFinished` — the page's `reload`, which never changes; a new one is no reason to read again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [job, retry, pollMs]);

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

  const cancel = async () => {
    setError(null);
    try {
      const current = job?.kind === "tool-install" ? await cancelInstall() : await cancelScan();
      // The answer is the job as it stood when the cancel landed, shown as a
      // poll's read is: never rolled back to running, and an end the cancel is
      // first to see waits for the reload like any other.
      if (current) show(current);
    } catch (e) {
      setError((job?.kind === "tool-install" ? INSTALL_JOB_TEXT : SCAN_JOB_TEXT).requestFailed(requestDetail(e)));
    }
  };

  return {
    job,
    joined,
    running,
    installing,
    installAndScan,
    alert: error ?? pollError,
    run,
    cancel: () => void cancel(),
    consent: { scanners: asking, accept: () => void start(asking), decline: () => setAsking([]) },
    install: {
      plan: installAsk?.plan ?? null,
      scan: installAsk?.ask.scan === true,
      accept: (answers) => {
        if (installAsk) void install({ ...installAsk.ask, ...answers });
      },
      decline: () => setInstallAsk(null),
    },
  };
}

/** Run scan (or Install & scan) and Cancel, with the consent and install dialogs:
 *  the Scanners widget's title-row controls. */
export function ScanControls({ scan }: { scan: ScanJobHandle }) {
  return (
    <>
      <div className={styles.controls} role="group" aria-label="Scan controls">
        <Button variant="primary" size="sm" disabled={scan.installing} onClick={scan.run}>
          {scan.installAndScan ? INSTALL_JOB_TEXT.installAndScan : SCAN_JOB_TEXT.run}
        </Button>
        {scan.running && (
          <Button size="sm" onClick={scan.cancel}>
            {scan.installing ? INSTALL_JOB_TEXT.cancel : SCAN_JOB_TEXT.cancel}
          </Button>
        )}
      </div>
      <ConsentDialog scanners={scan.consent.scanners} onAccept={scan.consent.accept} onDecline={scan.consent.decline} />
      <InstallDialog plan={scan.install.plan} scan={scan.install.scan} onAccept={scan.install.accept} onDecline={scan.install.decline} />
    </>
  );
}

/** The one summary line — a polite live region — and a refused request's alert.
 *  The line counts the scanners and tools finished while a job runs, and states
 *  its outcome once it ended; every row's own state is in the table. */
export function ScanSummary({ scan }: { scan: ScanJobHandle }) {
  const { job, joined } = scan;
  return (
    <>
      {scan.alert !== null && (
        <p role="alert" className={styles.alert}>
          {scan.alert}
        </p>
      )}
      <p aria-live="polite" className={styles.summary}>
        {job && <Badge tone={stateTone(job)}>{job.state}</Badge>} <span>{summaryText(job)}</span>
        {joined && job?.state === "running" && <span> {job.kind === "tool-install" ? INSTALL_JOB_TEXT.joined : SCAN_JOB_TEXT.joined}</span>}
      </p>
    </>
  );
}

function stateTone(job: Job): BadgeTone {
  if (job.state === "completed") return installLeftAToolOut(job) ? "orange" : "green";
  if (job.state === "failed") return "red";
  if (job.state === "running") return "orange";
  return "muted";
}

/** A completed install that left a tool failed or unavailable did not finish
 *  clean, whatever its scan did: its badge says so, not only its line. */
function installLeftAToolOut(job: Job): boolean {
  return (
    job.kind === "tool-install" &&
    (job.result?.install.tools.some((t) => t.outcome === "failed" || t.outcome === "unavailable") ?? false)
  );
}
