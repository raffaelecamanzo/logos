/*
 * ScannersWidget (S-670, S-792, FR-SC-09, FR-SC-15; CR-245) — the Security page's
 * lead widget: one row per registered scanner, plus one per report ingested from a
 * tool that is not a registered scanner. It also runs the scan: Run scan (or
 * Install & scan) and Cancel sit on its title row, with the consent and install
 * dialogs (`ScanControls`), and the one polite summary line above the table counts
 * the job's progress (`ScanSummary`). While a job runs, the row of the scanner
 * running reads `running` and the row of a tool installing reads `installing`
 * (`liveStates`); when it ends the page reloads, and once the reload has landed
 * every row shows its stored state — never the one read before the job. The
 * job's wire is the one the page already polled — nothing is added.
 *
 * Columns: Scanner · State · Version · Findings · Ignored dirs · Action.
 * - Scanner: the name; on Dependency-Check's row, whether it has an NVD API key
 *   (`NVD key: set / not set`, S-794) — never the key.
 * - State: the stored state, or the live one; under an absent state the command
 *   that fixes it (NFR-CC-04: never a bare zero).
 * - Version: the version, then the copy that runs and a newer pin.
 * - Findings: the count — `never run` is its own word — and, for a scanner whose
 *   last scan failed, a red `LAST SCAN FAILED` that expands to the cause and the
 *   log path.
 * - Ignored dirs: a marker chosen from the row's `coverage_kind`, never from the
 *   note's text; the full note is the marker's tooltip and accessible name. A
 *   legend under the table says each marker shown in plain words.
 */

import { useCallback, useId, useMemo, useState } from "react";

import { Badge, Button, DataTable, FigureNote, Widget, type BadgeTone, type Column } from "../../components/index.ts";
import {
  COPY_TEXT,
  FINDINGS_CELL_TEXT,
  IGNORED_DIRS_LEGEND,
  IGNORED_DIRS_TEXT,
  INSTALL_JOB_TEXT,
  LIVE_STATE_TEXT,
  NVD_KEY_TEXT,
  SCANNER_STATE_TEXT,
  scannerMatrix,
} from "../../copy/security.copy.ts";
import { plural } from "../../copy/types.ts";
import { ScanControls, ScanSummary, useScanJob, type InstallRequest } from "./ScanJobPanel.tsx";
import {
  ignoredDirsKey,
  liveStates,
  markersShown,
  scannerStateText,
  type LiveState,
  type SecurityScannerStatus,
  type SecurityStatus,
} from "./securityModel.ts";
import styles from "./SecurityView.module.css";

function stateTone(row: SecurityScannerStatus): BadgeTone {
  if (row.state === "fresh") return "green";
  if (row.state === "stale") return "orange";
  return "muted";
}

/** The scanner's name; on Dependency-Check's row, whether it has an NVD API key,
 *  with where a key comes from as the not-set line's tooltip (S-794). */
function ScannerCell({ row }: { row: SecurityScannerStatus }) {
  if (row.nvd_api_key === undefined) return <>{row.scanner}</>;
  return (
    <>
      {row.scanner}
      <p className={`muted ${styles.fix}`} title={row.nvd_api_key === "not_set" ? NVD_KEY_TEXT.hint : undefined}>
        {NVD_KEY_TEXT[row.nvd_api_key]}
      </p>
    </>
  );
}

/** The state a row reads: the live one while its job runs, else the stored one;
 *  under an absent stored state, the command that fixes it. */
function StateCell({ row, live, pending }: { row: SecurityScannerStatus; live: LiveState | undefined; pending: readonly string[] }) {
  if (live !== undefined) return <Badge tone="orange">{LIVE_STATE_TEXT[live]}</Badge>;
  return (
    <>
      <Badge tone={stateTone(row)}>{row.state.replace("_", " ")}</Badge>
      {row.state !== "fresh" && <p className={`muted ${styles.fix}`}>{scannerStateText(row, pending)}</p>}
    </>
  );
}

/** The version, then the copy that runs — `managed`, `path` or `override` — and a newer pin. */
function VersionCell({ row }: { row: SecurityScannerStatus }) {
  const version = row.state === "fresh" || row.state === "stale" ? row.version : (row.installed?.version ?? "—");
  if (row.installed === null) return <>{version}</>;
  const update = row.installed.update_available;
  return (
    <>
      {version} <span className="muted">· {COPY_TEXT[row.installed.copy]}</span>
      {update !== null && (
        <>
          {" "}
          <Badge tone="orange">{COPY_TEXT.updateAvailable(update)}</Badge>
        </>
      )}
    </>
  );
}

/** `LAST SCAN FAILED`: a disclosure that opens to the one masked line the server
 *  read from the failure log (S-769) and where the log is. */
function FailedScan({ row }: { row: SecurityScannerStatus }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  if (row.last_failure === null) return null;
  return (
    <>
      <button type="button" className={styles.disclosure} aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}>
        <Badge tone="red">{SCANNER_STATE_TEXT.lastFailed}</Badge>
      </button>
      {open && (
        <p id={id} className={`muted ${styles.fix}`}>
          {SCANNER_STATE_TEXT.failureCause(row.last_failure_cause, row.last_failure)}
        </p>
      )}
    </>
  );
}

function FindingsCell({ row }: { row: SecurityScannerStatus }) {
  const count =
    row.state === "fresh" || row.state === "stale"
      ? SCANNER_STATE_TEXT.fresh(row.findings, row.suppressed)
      : row.state === "never_run"
        ? FINDINGS_CELL_TEXT.neverRun
        : FINDINGS_CELL_TEXT.none;
  return (
    <>
      {count}
      {row.last_failure !== null && (
        <div>
          <FailedScan row={row} />
        </div>
      )}
    </>
  );
}

/** A row's Ignored dirs marker. A `scanned` one carries the server's full note as
 *  its tooltip and, with the marker's words, its accessible name. */
function IgnoredDirsCell({ row }: { row: SecurityScannerStatus }) {
  const key = ignoredDirsKey(row);
  const { marker } = IGNORED_DIRS_TEXT[key];
  if (key === "none" || key === "skipped") return <>{marker}</>;
  return (
    <span role="note" title={row.coverage_note} aria-label={row.coverage_note !== undefined ? `${marker}: ${row.coverage_note}` : marker}>
      <Badge tone="orange">{marker}</Badge>
    </span>
  );
}

/** A row's action: Install a scanner Logos can install here and is missing,
 *  Update a managed copy older than the pin. */
function ActionCell({ row, onInstall }: { row: SecurityScannerStatus; onInstall: (tool: string, update: boolean) => void }) {
  if (row.state === "not_installed" && row.install !== null) {
    return (
      <Button size="sm" aria-label={`${INSTALL_JOB_TEXT.install} ${row.scanner}`} onClick={() => onInstall(row.scanner, false)}>
        {INSTALL_JOB_TEXT.install}
      </Button>
    );
  }
  if (row.installed?.copy === "managed" && row.installed.update_available !== null) {
    return (
      <Button size="sm" aria-label={`${INSTALL_JOB_TEXT.update} ${row.scanner}`} onClick={() => onInstall(row.scanner, true)}>
        {INSTALL_JOB_TEXT.update}
      </Button>
    );
  }
  return null;
}

/** A matrix row's key. A report stored under a registry name spelled as its
 *  driver (`semgrep`) is an `other_reports` row beside the registry's `semgrep`
 *  (driver `Semgrep OSS`), so the name alone is not unique; with the driver it is
 *  — `other_reports` never holds a registry driver. */
function matrixKey(row: SecurityScannerStatus): string {
  return `${row.scanner}\u0000${row.driver}`;
}

export interface ScannersWidgetProps {
  status: SecurityStatus;
  /** Called once when a job this widget followed ends: the page reloads, and
   *  the job reads ended once the reload settles. */
  onFinished: () => void | Promise<void>;
  pollMs?: number;
}

export function ScannersWidget({ status, onFinished, pollMs }: ScannersWidgetProps) {
  // A row's Install or Update, until the job takes it (S-747).
  const [request, setRequest] = useState<InstallRequest | null>(null);
  const requested = useCallback(() => setRequest(null), []);
  const scan = useScanJob({ pending: status.consent.pending, onFinished, pollMs, scanners: status.scanners, request, onRequested: requested });
  const live = useMemo(() => liveStates(scan.job), [scan.job]);
  const rows = [...status.scanners, ...status.other_reports];
  const fresh = rows.filter((r) => r.state === "fresh").length;
  const onInstall = (tool: string, update: boolean) => setRequest({ tool, update });
  // A live state names a registered scanner: a report from another tool that
  // happens to share the name is not the one running.
  const liveOf = (row: SecurityScannerStatus) => (status.scanners.includes(row) ? live.get(row.scanner) : undefined);

  const columns: Column<SecurityScannerStatus>[] = [
    { key: "scanner", header: "Scanner", mono: true, sortValue: (r) => r.scanner, cell: (r) => <ScannerCell row={r} /> },
    {
      key: "state",
      header: "State",
      sortValue: (r) => r.state,
      cell: (r) => <StateCell row={r} live={liveOf(r)} pending={status.consent.pending} />,
    },
    { key: "version", header: "Version", mono: true, cell: (r) => <VersionCell row={r} /> },
    { key: "findings", header: "Findings", cell: (r) => <FindingsCell row={r} /> },
    { key: "ignored", header: "Ignored dirs", cell: (r) => <IgnoredDirsCell row={r} /> },
    { key: "action", header: "Action", cell: (r) => <ActionCell row={r} onInstall={onInstall} /> },
  ];

  return (
    <Widget
      title="Scanners"
      copy={scannerMatrix}
      badge={<ScanControls scan={scan} />}
      figure={
        <span>
          {fresh} <FigureNote>of {rows.length} {plural(rows.length, "scanner", "scanners")} fresh at HEAD</FigureNote>
        </span>
      }
    >
      <ScanSummary scan={scan} />
      <DataTable caption="Scanners" columns={columns} rows={rows} rowKey={matrixKey} />
      <ul className={styles.legend} aria-label={IGNORED_DIRS_LEGEND}>
        {markersShown(rows).map(({ key, text }) => (
          <li key={key}>
            <span className="mono">{text.marker}</span>: {text.means}
          </li>
        ))}
      </ul>
    </Widget>
  );
}
