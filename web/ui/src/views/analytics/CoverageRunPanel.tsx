/*
 * The Coverage page's Run coverage job (S-739, FR-CV-15, FR-CV-13; frontend-design
 * §4.10). A run is a detached `coverage-run` job on the server (S-738): the click
 * answers at once, the page follows the job over same-origin Server-Sent Events,
 * and every other view keeps answering meanwhile. `useCoverageRun` holds that
 * state for the Coverage tools widget, which renders two parts of it:
 * `RunControls` in its title row and `RunProgress` — the one polite `aria-live`
 * line — above its table. The run itself is the table's: each tool's State and
 * Elapsed read it (`liveTools`, `useJobClock`) and the cause sits below it (S-803).
 *
 * - Run coverage posts the start. The first run asks for the execution consent:
 *   the server answers `409` with what it lacks, the dialog shows it, and the
 *   accepted start sends exactly the tools and the command text it showed.
 * - While the run goes the button is Cancel. The run is single-flight: a click
 *   from a tab that has not seen it yet joins it, and the line says so.
 * - Cancel kills the tool going with its whole process group; every tool that
 *   did not settle reads *cancelled*.
 * - A run started before the page opened (another tab, a navigation away and
 *   back) is picked up on mount and followed the same way.
 * - A broken stream is not the end of the run: the job is read again and, while
 *   it still goes, followed again — its replay merges by `seq`.
 *
 * When a run this page followed ends, `onFinished` reloads the page once, so the
 * status line and the widgets show what it ingested. The run is shown ended only
 * once that reload has landed (HF-6): the figures the page holds were read before
 * the run, so publishing the end first would say "The figures below are refreshed"
 * over them for as long as the reload takes. Until then every tool keeps its live
 * state, and it goes from `running` straight to what the run ingested.
 *
 * S-750 (FR-CV-17, FR-CV-18): a not-installed tool's Install, or Add dependency,
 * starts the same job with the install or the addition first. Its question comes
 * from the same `409`, with the install lines and the dependency's exact command
 * and files beside the execution consent, in the same dialog; accepting sends
 * back exactly what it showed. The steps it reports before the tools are listed
 * on the tool's own row (`prepareRows`), and Cancel stops whichever is going.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import {
  cancelCoverageRun,
  coverageRequestDetail,
  fetchCoverageRunJob,
  followCoverageRun,
  startCoveragePrepared,
  startCoverageRun,
} from "../../api/coverageClient.ts";
import type {
  CoverageConsentRequired,
  CoverageJob,
  CoveragePrepareAsk,
  CoveragePrepareRequired,
  CoverageToolStatus,
} from "../../api/index.ts";
import { Badge, Button, type BadgeTone } from "../../components/index.ts";
import { COVERAGE_RUN_TEXT } from "../../copy/coverageView.copy.ts";
import { CoverageConsentDialog } from "./CoverageConsentDialog.tsx";
import {
  announcement,
  NO_RUN,
  prepareRows,
  progressRows,
  runnable,
  withJob,
  withSteps,
  type PrepareRow,
  type ProgressRow,
  type RunView,
} from "./coverageRunModel.ts";
import styles from "./AnalyticsView.module.css";

/** How long a broken stream waits before the job is read again. */
export const RUN_RETRY_MS = 1000;

export interface CoverageRunProps {
  /** The coverage status's tool rows (`coverage.runner.tools`). */
  tools: readonly CoverageToolStatus[];
  /** Whether the project's consent named any tool before (`runner.consent.consented`). */
  consentedBefore: boolean;
  /** Called once when a run this page followed ends; the run is shown ended once
   *  a promise it returns settles, whether it resolves or fails. */
  onFinished: () => void | Promise<void>;
  retryMs?: number;
}

/** What a start asks for: the run alone, or a tool readied first (S-750). */
export type RunAction = { kind: "run" } | { kind: "install" } | { kind: "dependency"; tool: string };

/** The followed run and what the page does with it. */
export interface CoverageRunHandle {
  view: RunView;
  rows: ProgressRow[];
  /** What the run readied before its tools (S-750). */
  prepared: PrepareRow[];
  running: boolean;
  /** Whether the last start joined a run already going. */
  joined: boolean;
  /** Whether any tool here can run. */
  canRun: boolean;
  /** A refused or failed request, or a broken stream being read again. */
  alert: string | null;
  run: () => void;
  /** Install the missing managed coverage tools, then run (S-750). */
  install: () => void;
  /** Add `tool`'s missing coverage dev dependency, then run (S-750). */
  addDependency: (tool: string) => void;
  cancel: () => void;
  /** The consent dialog: what it shows, `null` when closed. */
  consent: {
    required: CoverageConsentRequired | CoveragePrepareRequired | null;
    /** The start the dialog answers. */
    action: RunAction;
    willRun: readonly string[];
    consentedBefore: boolean;
    accept: () => void;
    decline: () => void;
  };
}

/** `job` as the latest snapshot, but a run seen ended is never rolled back to
 *  running by an answer that left the server before it ended. */
function latest(view: RunView, job: CoverageJob | null): RunView {
  if (job !== null && view.job?.id === job.id && view.job.state !== "running" && job.state === "running") {
    return withSteps(view, job.events);
  }
  return withJob(view, job);
}

export function useCoverageRun({ tools, consentedBefore, onFinished, retryMs = RUN_RETRY_MS }: CoverageRunProps): CoverageRunHandle {
  const [view, setView] = useState<RunView>(NO_RUN);
  const [joined, setJoined] = useState(false);
  const [required, setRequired] = useState<CoverageConsentRequired | CoveragePrepareRequired | null>(null);
  const [action, setAction] = useState<RunAction>({ kind: "run" });
  const [error, setError] = useState<string | null>(null);
  const [lost, setLost] = useState(false);
  // The run seen going here, whose end reloads the page once.
  const followed = useRef<number | null>(null);
  // The followed run that ended, while the page's reload is on its way.
  const ending = useRef<number | null>(null);
  // The latest `onFinished`.
  const finished = useRef(onFinished);
  useEffect(() => {
    finished.current = onFinished;
  }, [onFinished]);
  // A readied tool runs too: the one whose dependency is added, or each one the
  // install sets up.
  const readied = (t: CoverageToolStatus) =>
    action.kind === "dependency" ? t.tool === action.tool : action.kind === "install" && t.offer === "install";
  const willRun = tools.filter((t) => runnable(t) || readied(t)).map((t) => t.tool);
  const running = view.job?.state === "running";
  const runningId = running && view.job ? view.job.id : null;

  // The run as read, by whichever way it arrives. A followed run's end is shown
  // only once the reload it asks for has settled — failed or not, the run ends —
  // since the figures the page holds were read before the run (HF-6).
  const show = useCallback((next: CoverageJob | null, apply: (shown: RunView, job: CoverageJob) => RunView = latest) => {
    // The run whose end is being shown: a stream, re-read or cancel read of it adds nothing.
    if (next !== null && ending.current === next.id) return;
    if (next === null || next.state === "running" || followed.current !== next.id) {
      if (next !== null) setView((shown) => apply(shown, next));
      return;
    }
    const id = next.id;
    followed.current = null;
    ending.current = id;
    void Promise.resolve()
      .then(() => finished.current())
      .catch(() => undefined)
      .then(() => {
        if (ending.current === id) ending.current = null;
        // A run started meanwhile is the newer one: this end never replaces it.
        // Once the page is left, React drops the update: nothing is published.
        setView((shown) => (shown.job !== null && shown.job.id > id ? shown : latest(shown, next)));
      });
  }, []);

  // A run started elsewhere is picked up; a newer one this page started wins.
  useEffect(() => {
    let live = true;
    fetchCoverageRunJob()
      .then((job) => {
        if (live && job) show(job, (shown, read) => (shown.job !== null && shown.job.id >= read.id ? shown : latest(shown, read)));
      })
      // The run's state is a convenience here: an unreadable one leaves Run
      // working and states nothing it does not know.
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [show]);

  // Follow the run going, until its end — through a broken stream too.
  useEffect(() => {
    if (runningId === null) return;
    followed.current = runningId;
    let live = true;
    let stop = () => undefined as void;
    let retry: ReturnType<typeof setTimeout> | undefined;
    // A run other than the one followed, found ended: the followed run ended
    // before it, and this one ran — and ingested — after, so its end reloads the
    // page in the followed run's place.
    const succeeded = (job: CoverageJob | null) => {
      if (job !== null && job.id !== runningId && job.state !== "running") followed.current = job.id;
    };
    const reread = () => {
      setLost(true);
      retry = setTimeout(() => {
        fetchCoverageRunJob()
          .then((job) => {
            if (!live) return;
            setLost(false);
            succeeded(job);
            show(job);
            if (job?.id === runningId && job.state === "running") open();
          })
          .catch(() => {
            if (live) reread();
          });
      }, retryMs);
    };
    const open = () => {
      stop = followCoverageRun({
        step: (event) => setView((shown) => (shown.job?.id === runningId ? withSteps(shown, [event]) : shown)),
        end: (job) => {
          if (job === null) reread();
          // The stream connected after the followed run ended and another
          // began: its steps carry no job id, so the next run's whole record is
          // read rather than trusting what was credited to the old one.
          else if (job.id !== runningId) {
            stop();
            succeeded(job);
            fetchCoverageRunJob()
              .then((read) => live && show(read?.id === job.id ? read : job))
              .catch(() => live && show(job));
          } else show(job);
        },
        lost: reread,
      });
    };
    open();
    return () => {
      live = false;
      stop();
      clearTimeout(retry);
      // A run that is no longer followed has no broken stream to report.
      setLost(false);
    };
  }, [runningId, retryMs, show]);

  const start = async (next: RunAction, ask?: CoveragePrepareAsk) => {
    setRequired(null);
    setError(null);
    setAction(next);
    try {
      const answer =
        next.kind === "run"
          ? await startCoverageRun(ask)
          : await startCoveragePrepared(next.kind, { consent: [], ...ask, ...(next.kind === "dependency" ? { tool: next.tool } : {}) });
      if (answer.kind === "consent") {
        setRequired(answer.required);
        return;
      }
      setJoined(answer.started.joined);
      show(answer.started.job);
    } catch (e) {
      setError(COVERAGE_RUN_TEXT.requestFailed(coverageRequestDetail(e)));
    }
  };

  const cancel = async () => {
    setError(null);
    try {
      const current = await cancelCoverageRun();
      if (current) show(current);
    } catch (e) {
      setError(COVERAGE_RUN_TEXT.requestFailed(coverageRequestDetail(e)));
    }
  };

  const accept = () => {
    if (required === null) return;
    const { tools: consent, refresh_cmd } = required.consent_required;
    const ask: CoveragePrepareAsk = refresh_cmd !== null ? { consent, refresh_cmd } : { consent };
    if ("install" in required) {
      ask.install = required.install.map((line) => (line.line === "tool" ? `${line.tool} ${line.version}` : line.command));
      ask.dependency = required.dependencies.map((d) => d.command);
    }
    void start(action, ask);
  };

  return {
    view,
    rows: progressRows(view, tools),
    prepared: prepareRows(view),
    running,
    joined,
    canRun: willRun.length > 0,
    alert: error ?? (lost ? COVERAGE_RUN_TEXT.lost : null),
    run: () => void start({ kind: "run" }),
    install: () => void start({ kind: "install" }),
    addDependency: (tool: string) => void start({ kind: "dependency", tool }),
    cancel: () => void cancel(),
    consent: { required, action, willRun, consentedBefore, accept, decline: () => setRequired(null) },
  };
}

/** Run coverage — Cancel while a run goes — with the consent dialog: the Coverage
 *  tools widget's title-row control. */
export function RunControls({ run }: { run: CoverageRunHandle }) {
  return (
    <>
      <div className={styles.controls} role="group" aria-label="Coverage run controls">
        {run.running ? (
          <Button size="sm" onClick={run.cancel}>
            {COVERAGE_RUN_TEXT.cancel}
          </Button>
        ) : (
          <RunButton run={run} />
        )}
      </div>
      <CoverageConsentDialog
        required={run.consent.required}
        action={run.consent.action.kind}
        willRun={run.consent.willRun}
        consentedBefore={run.consent.consentedBefore}
        onAccept={run.consent.accept}
        onDecline={run.consent.decline}
      />
    </>
  );
}

/** The Run coverage button alone — the title row's, and the empty state's beside
 *  the ingest command; disabled while no tool here can run. */
export function RunButton({ run }: { run: CoverageRunHandle }) {
  return (
    <Button
      variant="primary"
      size="sm"
      disabled={!run.canRun}
      title={run.canRun ? undefined : COVERAGE_RUN_TEXT.noToolRunnable}
      onClick={run.run}
    >
      {COVERAGE_RUN_TEXT.run}
    </Button>
  );
}

/** Milliseconds since the run started, now: the last moment the server reported
 *  plus the time since it arrived, never moving back within one run. Ticks once a
 *  second while the run goes — the table's Elapsed cells count with it; the live
 *  line never carries it. */
export function useJobClock(view: RunView): number | null {
  const job = view.job;
  const id = job?.id ?? null;
  const reported = job === null ? null : Math.max(job.elapsed_ms, view.steps.at(-1)?.at_ms ?? 0);
  const [base, setBase] = useState<{ id: number; jobMs: number; at: number } | null>(null);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (id === null || reported === null) {
      setBase(null);
      return;
    }
    const at = Date.now();
    setBase((prior) =>
      prior !== null && prior.id === id && prior.jobMs + (at - prior.at) >= reported ? prior : { id, jobMs: reported, at },
    );
    setNow(at);
  }, [id, reported]);
  const running = job?.state === "running";
  useEffect(() => {
    if (!running) return;
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, [running]);
  return base === null || base.id !== id ? null : base.jobMs + Math.max(0, now - base.at);
}

/** A refused request's alert and the polite live line — what changed last, and
 *  the end of the run. The per-tool part of the run is the table's. */
export function RunProgress({ run }: { run: CoverageRunHandle }) {
  const { job } = run.view;
  return (
    <>
      {run.alert !== null && (
        <p role="alert" className={styles.alert}>
          {run.alert}
        </p>
      )}
      <p aria-live="polite" className={styles.summary}>
        {job && <Badge tone={jobTone(job)}>{job.state}</Badge>} <span>{announcement(run.view, run.rows)}</span>
        {run.joined && job?.state === "running" && <span> {COVERAGE_RUN_TEXT.joined}</span>}
      </p>
    </>
  );
}

function jobTone(job: CoverageJob): BadgeTone {
  if (job.state === "running") return "orange";
  if (job.state === "failed") return "red";
  if (job.state === "completed") return "green";
  return "muted";
}
