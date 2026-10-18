/*
 * SecurityView → "Security" (S-670, FR-SC-09, FR-SC-04, FR-SC-13) — the
 * member-scoped Security page over `/api/v1/security`, in navigation group B
 * beside Rule findings and Coverage. Verdict-first: the findings count leads,
 * then the `[security]` gate band (only with the table), the scanner matrix, the
 * findings table with its filters, the OWASP Top 10 view and the finding detail.
 *
 * One read serves the whole page: the filters and the 2025/2021 edition switch
 * work over it, so neither refetches (the payload carries both editions). The
 * detail asks `impact` for the blast-radius lists, since the payload carries only
 * their sizes (S-669).
 *
 * Absent states are stated with the command that fixes them, never as zero (no
 * scan, not installed, never run, not covered — `copy/security.copy.ts`). No
 * source snippet is rendered anywhere: the payload carries none, and a
 * secret-class message reads `[redacted]` whatever text it holds. Every read is a
 * GET (ADR-28). The actions are intent-guarded POSTs, each followed by a
 * reload: Run scan (`ScanJobPanel`, S-673), a detached scan followed by its
 * replayed progress; Install & scan, and a matrix row's Install or Update
 * (S-747, FR-SC-24), a `tool-install` job the same panel follows after one
 * consent dialog; and triage (S-672, FR-SC-12), where Suppress… opens
 * `SuppressDialog` and Unsuppress removes the entry from the committed ignore
 * file. A suppressed finding stays listed, labelled with its reason.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { fetchSecurity } from "../../api/client.ts";
import { requestDetail, suppressFinding, unsuppressFinding } from "../../api/securityClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import type {
  OwaspEditionYear,
  SecurityCategoryCoverage,
  SecurityFinding,
  SecurityGateBand,
  SecurityModel,
  SecurityPage,
  SecurityScannerStatus,
} from "../../api/types.ts";
import {
  Badge,
  Button,
  Callout,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  FigureNote,
  SelectField,
  TextField,
  Widget,
  WidgetStack,
  type BadgeTone,
  type Column,
} from "../../components/index.ts";
import {
  COPY_TEXT,
  INSTALL_JOB_TEXT,
  OWASP_TEXT,
  SECURITY_ABSENCE,
  SECURITY_GATE_TEXT,
  SUPPRESS_TEXT,
  owaspView,
  scannerMatrix,
  securityFindings,
  securityGate,
  suppressionNote,
} from "../../copy/security.copy.ts";
import { plural } from "../../copy/types.ts";
import { FindingDetail } from "./FindingDetail.tsx";
import { ScanJobPanel, type InstallRequest } from "./ScanJobPanel.tsx";
import { SuppressDialog } from "./SuppressDialog.tsx";
import {
  EDITIONS,
  NO_FILTERS,
  SEVERITIES,
  editionOf,
  findingKey,
  findingLocation,
  findingMessage,
  keeps,
  notCoveredSentence,
  pathFromSearch,
  scannerDrivers,
  scannerStateText,
  severityRank,
  severityTone,
  type Classification,
  type FindingFilters,
} from "./securityModel.ts";
import { EditionSwitch } from "./EditionSwitch.tsx";
import styles from "./SecurityView.module.css";

export function SecurityView() {
  const model = useApiResource<SecurityModel>(() => fetchSecurity(), []);
  return (
    <AsyncResource
      resource={model}
      loadingLabel="Loading the security findings…"
      isEmpty={(m) => !m.status.indexed}
      empty={<EmptyState message="No index yet — run" command="logos index" />}
    >
      {(m) => <SecurityContent page={m.security} reload={model.reload} />}
    </AsyncResource>
  );
}

function SecurityContent({ page, reload }: { page: SecurityPage; reload: () => void }) {
  const [edition, setEdition] = useState<OwaspEditionYear>(EDITIONS[0]);
  // A `?path=` deep link (Files & Risk, S-671) seeds the path filter once.
  const [filters, setFilters] = useState<FindingFilters>(() => ({
    ...NO_FILTERS,
    path: pathFromSearch(window.location.search),
  }));
  const [selected, setSelected] = useState<string | null>(null);
  // The finding the suppression dialog is open for, and a refused unsuppress.
  const [suppressing, setSuppressing] = useState<SecurityFinding | null>(null);
  const [triageError, setTriageError] = useState<string | null>(null);
  // A matrix row's Install or Update, until the scan panel takes it (S-747).
  const [request, setRequest] = useState<InstallRequest | null>(null);
  const requested = useCallback(() => setRequest(null), []);
  // The fingerprints with an unsuppress in flight, or done and awaiting the
  // reloaded page: the row's button stays disabled until the page shows the
  // removal, so a double click cannot answer "no entry" for a removal that
  // succeeded. The ref guards within one tick; the state disables.
  const unsuppressing = useRef(new Set<string>());
  const [busy, setBusy] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    unsuppressing.current.clear();
    setBusy(new Set());
  }, [page]);
  // A category id names its edition (`A05:2025`), so a category filter cannot
  // carry across a switch: it is cleared, never left applied behind a select
  // that no longer offers it.
  const switchEdition = (year: OwaspEditionYear) => {
    setEdition(year);
    setFilters((f) => ({ ...f, category: "" }));
  };

  const { categories, classify } = useMemo(() => editionOf(page, edition), [page, edition]);
  const drivers = useMemo(() => scannerDrivers(page.status), [page.status]);
  const scanned = page.findings.scanned.length > 0;
  const kept = page.findings.findings.filter((f) => keeps(f, filters, classify(f), drivers));
  const detail = page.findings.findings.find((f) => findingKey(f) === selected) ?? null;
  const total = page.findings.total;

  const suppress = async (finding: SecurityFinding, reason: string, expires: string | null) => {
    await suppressFinding(finding.fingerprint, reason, expires);
    setSuppressing(null);
    setTriageError(null);
    reload();
  };
  const unsuppress = async (finding: SecurityFinding) => {
    const inFlight = unsuppressing.current;
    if (inFlight.has(finding.fingerprint)) return;
    inFlight.add(finding.fingerprint);
    setBusy(new Set(inFlight));
    setTriageError(null);
    try {
      await unsuppressFinding(finding.fingerprint);
      reload();
    } catch (e) {
      inFlight.delete(finding.fingerprint);
      setBusy(new Set(inFlight));
      setTriageError(SUPPRESS_TEXT.removeFailed(requestDetail(e)));
    }
  };

  return (
    <WidgetStack>
      <Callout label="SECURITY" tone={!scanned || total === 0 ? "muted" : "signal"}>
        <span>
          {scanned
            ? `${total} ${plural(total, "finding", "findings")} from ${page.findings.scanned.length} ${plural(page.findings.scanned.length, "report", "reports")}`
            : "no security scan yet"}
        </span>
      </Callout>
      {page.gate && <GateBand band={page.gate} />}
      <ScanJobPanel
        pending={page.status.consent.pending}
        onFinished={reload}
        scanners={page.status.scanners}
        request={request}
        onRequested={requested}
      />
      <ScannerMatrix page={page} onInstall={(tool, update) => setRequest({ tool, update })} />
      <FindingsWidget
        page={page}
        scanned={scanned}
        kept={kept}
        filters={filters}
        setFilters={setFilters}
        categories={categories}
        classify={classify}
        onSelect={setSelected}
        onSuppress={setSuppressing}
        onUnsuppress={(f) => void unsuppress(f)}
        unsuppressing={busy}
        triageError={triageError}
      />
      <OwaspWidget page={page} edition={edition} setEdition={switchEdition} categories={categories} classify={classify} />
      {scanned && total > 0 && <FindingDetail finding={detail} classification={detail ? classify(detail) : null} />}
      {suppressing && (
        <SuppressDialog
          key={findingKey(suppressing)}
          finding={suppressing}
          onSubmit={(reason, expires) => suppress(suppressing, reason, expires)}
          onCancel={() => setSuppressing(null)}
        />
      )}
    </WidgetStack>
  );
}

/** The `[security]` gate band (FR-SC-10): the verdict `check` reaches on the table. */
function GateBand({ band }: { band: SecurityGateBand }) {
  return (
    <Widget
      title="Security gate"
      badge={band.passed ? <Badge tone="green">PASS</Badge> : <Badge tone="red">FAIL</Badge>}
      copy={securityGate}
      figure={
        <span>
          {band.passed
            ? SECURITY_GATE_TEXT.pass(band.checked_rules)
            : SECURITY_GATE_TEXT.fail(band.missing_scans.length, band.violations.length)}
        </span>
      }
    >
      {(band.missing_scans.length > 0 || band.violations.length > 0 || band.notes.length > 0) && (
        <ul className={styles.list}>
          {band.missing_scans.map((m) => (
            <li key={`missing:${m.scanner}`}>{m.message}</li>
          ))}
          {band.violations.map((v, i) => (
            <li key={`violation:${i}`}>
              <span className="mono">{v.rule}</span> — {v.message}
            </li>
          ))}
          {band.notes.map((n) => (
            <li key={`note:${n}`} className="muted">
              {n}
            </li>
          ))}
        </ul>
      )}
    </Widget>
  );
}

function stateTone(row: SecurityScannerStatus): BadgeTone {
  if (row.state === "fresh") return "green";
  if (row.state === "stale") return "orange";
  return "muted";
}

/** The copy that runs — `managed`, `path` or `override` — and a newer pin. */
function CopyCell({ row }: { row: SecurityScannerStatus }) {
  if (row.installed === null) return <>—</>;
  const update = row.installed.update_available;
  return (
    <>
      {COPY_TEXT[row.installed.copy]}
      {update !== null && (
        <>
          {" "}
          <Badge tone="orange">{COPY_TEXT.updateAvailable(update)}</Badge>
        </>
      )}
    </>
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

/** The matrix columns; `pending` names the scanners whose first scan needs consent. */
function scannerColumns(
  pending: readonly string[],
  onInstall: (tool: string, update: boolean) => void,
): Column<SecurityScannerStatus>[] {
  return [
    { key: "scanner", header: "Scanner", mono: true, sortValue: (r) => r.scanner, cell: (r) => r.scanner },
    {
      key: "state",
      header: "State",
      sortValue: (r) => r.state,
      cell: (r) => <Badge tone={stateTone(r)}>{r.state.replace("_", " ")}</Badge>,
    },
    {
      key: "version",
      header: "Version",
      mono: true,
      cell: (r) => (r.state === "fresh" || r.state === "stale" ? r.version : (r.installed?.version ?? "—")),
    },
    { key: "copy", header: "Copy", cell: (r) => <CopyCell row={r} /> },
    { key: "detail", header: "What it means", cell: (r) => scannerStateText(r, pending) },
    { key: "action", header: "Action", cell: (r) => <ActionCell row={r} onInstall={onInstall} /> },
  ];
}

/** A matrix row's key. A report stored under a registry name spelled as its
 *  driver (`semgrep`) is an `other_reports` row beside the registry's `semgrep`
 *  (driver `Semgrep OSS`), so the name alone is not unique; with the driver it is
 *  — `other_reports` never holds a registry driver. */
function matrixKey(row: SecurityScannerStatus): string {
  return `${row.scanner}\u0000${row.driver}`;
}

function ScannerMatrix({ page, onInstall }: { page: SecurityPage; onInstall: (tool: string, update: boolean) => void }) {
  const rows = [...page.status.scanners, ...page.status.other_reports];
  const fresh = rows.filter((r) => r.state === "fresh").length;
  return (
    <Widget
      title="Scanners"
      copy={scannerMatrix}
      figure={
        <span>
          {fresh} <FigureNote>of {rows.length} {plural(rows.length, "scanner", "scanners")} fresh at HEAD</FigureNote>
        </span>
      }
    >
      <DataTable caption="Scanners" columns={scannerColumns(page.status.consent.pending, onInstall)} rows={rows} rowKey={matrixKey} />
    </Widget>
  );
}

interface FindingsWidgetProps {
  page: SecurityPage;
  scanned: boolean;
  kept: SecurityFinding[];
  filters: FindingFilters;
  setFilters: (f: FindingFilters) => void;
  categories: SecurityCategoryCoverage[];
  classify: (f: SecurityFinding) => Classification;
  onSelect: (key: string) => void;
  /** Opens the suppression dialog for an unsuppressed finding. */
  onSuppress: (f: SecurityFinding) => void;
  /** Removes the entry naming a suppressed or expired finding. */
  onUnsuppress: (f: SecurityFinding) => void;
  /** The fingerprints whose unsuppress is in flight or awaiting the reload:
   *  their button is disabled. */
  unsuppressing: ReadonlySet<string>;
  /** Why the last unsuppress was refused, if it was. */
  triageError: string | null;
}

function FindingsWidget({
  page,
  scanned,
  kept,
  filters,
  setFilters,
  categories,
  classify,
  onSelect,
  onSuppress,
  onUnsuppress,
  unsuppressing,
  triageError,
}: FindingsWidgetProps) {
  if (!scanned) {
    return <Widget title="Findings" copy={securityFindings} absence={SECURITY_ABSENCE.noScan} />;
  }
  if (page.findings.total === 0) {
    return <Widget title="Findings" copy={securityFindings} absence={SECURITY_ABSENCE.clean} />;
  }
  const columns: Column<SecurityFinding>[] = [
    {
      key: "sev",
      header: "Severity",
      sortValue: (f) => severityRank(f.severity),
      cell: (f) => <Badge tone={severityTone(f.severity)}>{f.severity}</Badge>,
    },
    { key: "rule", header: "Rule", mono: true, sortValue: (f) => f.rule_id, cell: (f) => f.rule_id },
    { key: "loc", header: "Location", mono: true, sortValue: (f) => findingLocation(f), cell: (f) => findingLocation(f) },
    {
      key: "msg",
      header: "Message",
      cell: (f) => (
        <>
          {findingMessage(f)}
          {f.suppression && (
            <>
              {" "}
              <Badge tone="muted">{f.suppression.state}</Badge>{" "}
              <span className="muted" data-suppression-reason>
                {suppressionNote(f.suppression)}
              </span>
            </>
          )}
        </>
      ),
    },
    { key: "owasp", header: "OWASP", mono: true, cell: (f) => classify(f).categories.join(", ") || "—" },
    {
      key: "symbol",
      header: "Enclosing symbol",
      mono: true,
      cell: (f) => f.context.enclosing.symbol?.name ?? "—",
    },
    { key: "reach", header: "Route reach", cell: (f) => f.context.reachability.verdict },
    {
      key: "open",
      header: "Detail",
      cell: (f) => (
        <Button size="sm" onClick={() => onSelect(findingKey(f))} aria-label={`Show ${f.rule_id} at ${findingLocation(f)}`}>
          Show
        </Button>
      ),
    },
    {
      key: "triage",
      header: "Triage",
      cell: (f) =>
        f.suppression ? (
          <Button
            size="sm"
            onClick={() => onUnsuppress(f)}
            disabled={unsuppressing.has(f.fingerprint)}
            aria-label={`Unsuppress ${f.rule_id} at ${findingLocation(f)}`}
          >
            {SUPPRESS_TEXT.unsuppress}
          </Button>
        ) : (
          <Button size="sm" onClick={() => onSuppress(f)} aria-label={`Suppress ${f.rule_id} at ${findingLocation(f)}`}>
            {SUPPRESS_TEXT.suppress}
          </Button>
        ),
    },
  ];
  const set = (patch: Partial<FindingFilters>) => setFilters({ ...filters, ...patch });
  const scanners = page.findings.scanned.map((s) => s.driver);
  return (
    <Widget
      title="Findings"
      copy={securityFindings}
      figure={
        <span>
          {kept.length} <FigureNote>of {page.findings.total} {plural(page.findings.total, "finding", "findings")} shown</FigureNote>
        </span>
      }
    >
      <div className={styles.filters} role="group" aria-label="Filter the findings">
        <SelectField
          label="Least severity"
          value={filters.severity}
          onChange={(e) => set({ severity: e.target.value as FindingFilters["severity"] })}
        >
          <option value="">Any</option>
          {SEVERITIES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </SelectField>
        <SelectField label="Scanner" value={filters.scanner} onChange={(e) => set({ scanner: e.target.value })}>
          <option value="">Any</option>
          {scanners.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </SelectField>
        <SelectField label="OWASP category" value={filters.category} onChange={(e) => set({ category: e.target.value })}>
          <option value="">Any</option>
          {categories.map((c) => (
            <option key={c.id} value={c.id}>
              {c.id} {c.name}
            </option>
          ))}
        </SelectField>
        <TextField label="Path" value={filters.path} placeholder="src/" onChange={(e) => set({ path: e.target.value })} />
        <SelectField
          label="Suppression"
          value={filters.suppression}
          onChange={(e) => set({ suppression: e.target.value as FindingFilters["suppression"] })}
        >
          <option value="">Any</option>
          <option value="active">Active</option>
          <option value="suppressed">Suppressed</option>
        </SelectField>
        <SelectField
          label="Route reach"
          value={filters.reachability}
          onChange={(e) => set({ reachability: e.target.value as FindingFilters["reachability"] })}
        >
          <option value="">Any</option>
          <option value="reachable">Reachable</option>
          <option value="unreachable">Unreachable</option>
          <option value="unknown">Unknown</option>
        </SelectField>
      </div>
      {triageError !== null && (
        <p role="alert" className={styles.alert}>
          {triageError}
        </p>
      )}
      <DataTable
        caption="Security findings"
        columns={columns}
        rows={kept}
        rowKey={(f) => findingKey(f)}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
        empty={filters.path !== "" && kept.length === 0 ? SECURITY_ABSENCE.noFindingAt(filters.path) : SECURITY_ABSENCE.noMatch}
      />
    </Widget>
  );
}

interface OwaspWidgetProps {
  page: SecurityPage;
  edition: OwaspEditionYear;
  setEdition: (e: OwaspEditionYear) => void;
  categories: SecurityCategoryCoverage[];
  classify: (f: SecurityFinding) => Classification;
}

/** The OWASP Top 10 view with the 2025/2021 switch (FR-SC-04): switching reads the
 *  other edition from the same payload. */
function OwaspWidget({ page, edition, setEdition, categories, classify }: OwaspWidgetProps) {
  const unclassified = page.findings.findings.filter((f) => classify(f).unclassified !== null).length;
  const columns: Column<SecurityCategoryCoverage>[] = [
    { key: "id", header: "Category", mono: true, cell: (c) => c.id },
    { key: "name", header: "Name", cell: (c) => c.name },
    {
      key: "state",
      header: "Findings",
      cell: (c) =>
        c.state.state === "covered" ? OWASP_TEXT.covered(c.state.findings) : notCoveredSentence(c, page.status),
    },
  ];
  return (
    <Widget
      title="OWASP Top 10"
      copy={owaspView}
      figure={
        <span>
          {OWASP_TEXT.edition(edition)}
          {unclassified > 0 && <FigureNote> · {OWASP_TEXT.unclassified(unclassified)}</FigureNote>}
        </span>
      }
    >
      <EditionSwitch edition={edition} onChange={setEdition} />
      <DataTable caption={OWASP_TEXT.edition(edition)} columns={columns} rows={categories} rowKey={(c) => c.id} />
    </Widget>
  );
}
