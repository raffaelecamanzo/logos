/*
 * SecurityView → "Security" (S-670, FR-SC-09, FR-SC-04, FR-SC-13) — the
 * member-scoped Security page over `/api/v1/security`, in navigation group A
 * (assess) between Health and Coverage. Verdict-first: the findings count leads,
 * then the `[security]` gate band (only with the table), the scanner matrix, the
 * findings table with its filters, the OWASP Top 10 view and the finding detail.
 *
 * The Scanners widget (`ScannersWidget`) also runs the scan: its title row holds
 * Run scan, Install & scan and Cancel, and its rows show each scanner's live
 * state while the job runs (S-792, CR-245).
 *
 * One read serves the whole page: the filters and the 2025/2021 edition switch
 * work over it, so neither refetches (the payload carries both editions). The
 * detail asks `impact` for the blast-radius lists, since the payload carries only
 * their sizes (S-669).
 *
 * Absent states are stated with the command that fixes them, never as zero (no
 * scan, not installed, never run, not covered — `copy/security.copy.ts`). No
 * source snippet is rendered anywhere: the payload carries none, and a
 * secret-class message reads why it is withheld, with the finding's location,
 * whatever text it holds. Every read is a
 * GET (ADR-28). The actions are intent-guarded POSTs, each followed by a
 * reload: Run scan (`useScanJob`, S-673), a detached scan followed by its
 * replayed progress; Install & scan, and a matrix row's Install or Update
 * (S-747, FR-SC-24), a `tool-install` job the same hook follows after one
 * consent dialog; and triage (S-672, FR-SC-12), where Suppress… opens
 * `SuppressDialog` and Unsuppress removes the entry from the committed ignore
 * file. A suppressed finding stays listed, its row tagged with its state; the
 * reason and expiry are in the detail, as is the message (no row carries one,
 * frontend-design §4.23). The Suppressions widget lists every entry of that file with the findings it
 * suppresses counted and an orphaned one flagged (S-770); a path entry covers
 * every finding under it, and Unsuppress on any of them removes that entry.
 *
 * A suppression that records evidence (FR-SC-32, S-818) stops suppressing when a
 * symbol its reason rests on changes: the finding is listed again, its badge
 * reads "out of date", and Re-confirm beside Unsuppress re-records the evidence
 * and keeps the reason.
 *
 * Code scope (FR-SC-28, S-796): the payload lists every finding with its scope;
 * the Production · Test code · All control narrows the table from memory,
 * production by default, and is kept in the URL's `?scope=`. A test-code row
 * carries a muted `TEST` tag whose hover says why, and the figure rows state
 * "· T in test code" — the findings the counts leave out. The OWASP counts
 * follow the scope too.
 */

import { useEffect, useMemo, useRef, useState } from "react";

import { fetchSecurity } from "../../api/client.ts";
import { reconfirmFinding, requestDetail, suppressFinding, unsuppressFinding } from "../../api/securityClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import type {
  OwaspEditionYear,
  SecurityCategoryCoverage,
  SecurityEntryStatus,
  SecurityFinding,
  SecurityGateBand,
  SecurityModel,
  SecurityPage,
  SecuritySelector,
} from "../../api/types.ts";
import {
  abbreviatePaths,
  Badge,
  Button,
  Callout,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  FigureNote,
  PathCell,
  SelectField,
  TextField,
  Widget,
  WidgetStack,
  type Column,
} from "../../components/index.ts";
import {
  ANCHOR_TEXT,
  CONTEXT_TEXT,
  EVIDENCE_TEXT,
  FINDING_COUNT_TEXT,
  FINDINGS_TABLE_TEXT,
  OWASP_TEXT,
  SCOPE_TEXT,
  SECURITY_ABSENCE,
  SECURITY_GATE_TEXT,
  SUPPRESS_TEXT,
  SUPPRESSIONS_TEXT,
  owaspView,
  securityFindings,
  securityGate,
  selectorText,
  suppressionEntries,
  suppressionState,
} from "../../copy/security.copy.ts";
import { plural } from "../../copy/types.ts";
import { currentUrl, replaceUrl } from "../../router.tsx";
import { FindingDetail } from "./FindingDetail.tsx";
import { ScannersWidget } from "./ScannersWidget.tsx";
import { SuppressDialog } from "./SuppressDialog.tsx";
import {
  EDITIONS,
  NO_FILTERS,
  SCOPES,
  SEVERITIES,
  categoryCount,
  devDependencyText,
  editionOf,
  findingKey,
  findingLocation,
  findingScanners,
  hiddenByScope,
  keeps,
  needsReviewText,
  notCoveredSentence,
  pathFromSearch,
  reachabilityText,
  reachableWord,
  scannerDrivers,
  scopeFromSearch,
  scopeReasonText,
  selectorKey,
  severityRank,
  severityTone,
  symbolKindWord,
  unclassifiedCount,
  urlWithScope,
  type Classification,
  type FindingFilters,
  type ScopeFilter,
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

function SecurityContent({ page, reload }: { page: SecurityPage; reload: () => Promise<void> }) {
  const [edition, setEdition] = useState<OwaspEditionYear>(EDITIONS[0]);
  // A `?path=` deep link (Files & Risk, S-671) seeds the path filter once, and
  // `?scope=` the code scope: production unless the URL names another.
  const [filters, setFilters] = useState<FindingFilters>(() => ({
    ...NO_FILTERS,
    path: pathFromSearch(window.location.search),
    scope: scopeFromSearch(window.location.search),
  }));
  // Back, Forward and a click on the Security link itself change the URL under
  // a mounted page (`navigate` dispatches `popstate`); the scope follows it, so
  // the control and the address never disagree.
  useEffect(() => {
    const follow = () => {
      const scope = scopeFromSearch(window.location.search);
      setFilters((f) => (f.scope === scope ? f : { ...f, scope }));
    };
    window.addEventListener("popstate", follow);
    return () => window.removeEventListener("popstate", follow);
  }, []);
  // The scope is the one filter the URL keeps: a change rewrites `?scope=` in
  // place, adding no history entry.
  const applyFilters = (next: FindingFilters) => {
    if (next.scope !== filters.scope) replaceUrl(urlWithScope(currentUrl(), next.scope));
    setFilters(next);
  };
  const [selected, setSelected] = useState<string | null>(null);
  // Counts every Show, so pressing it on the finding already shown brings the detail back into view.
  const [shown, setShown] = useState(0);
  const select = (key: string) => {
    setSelected(key);
    setShown((n) => n + 1);
  };
  // The finding the suppression dialog is open for, and a refused unsuppress or re-confirm.
  const [suppressing, setSuppressing] = useState<SecurityFinding | null>(null);
  const [triageError, setTriageError] = useState<string | null>(null);
  // The entries (by `selectorKey`) with an unsuppress or a re-confirm in flight, or done and
  // awaiting the reloaded page: every button rewriting one stays disabled until
  // the page shows the change, so a double click cannot answer "no entry" for
  // a removal that succeeded. The ref guards within one tick; the state disables.
  const rewriting = useRef(new Set<string>());
  const [busy, setBusy] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    rewriting.current.clear();
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
  // What the scope alone hides: said where the table would otherwise read empty.
  const hidden = hiddenByScope(page.findings.findings, filters, classify, drivers);
  const detail = page.findings.findings.find((f) => findingKey(f) === selected) ?? null;
  const total = page.findings.total;

  const suppress = async (selector: SecuritySelector, reason: string, expires: string | null, evidence: readonly string[] = []) => {
    await suppressFinding(selector, reason, expires, evidence);
    setSuppressing(null);
    setTriageError(null);
    reload();
  };
  // Removes the entry `selector` names: a finding's `suppression.by`, or a
  // Suppressions row's own entry.
  const unsuppress = (selector: SecuritySelector) =>
    rewrite(selector, () => unsuppressFinding(selector), SUPPRESS_TEXT.removeFailed);
  // Re-records the evidence of the entry that suppresses the finding with this
  // fingerprint, keeping its reason (FR-SC-32): the entry `selector` is busy
  // until the reloaded page shows the verdict again.
  const reconfirm = (fingerprint: string, selector: SecuritySelector) =>
    rewrite(selector, () => reconfirmFinding(fingerprint), EVIDENCE_TEXT.reconfirmFailed);
  // One write to an entry, and its refusal stated: both writes change the
  // entry, so each disables every button on it until the page reloads.
  const rewrite = async (selector: SecuritySelector, write: () => Promise<unknown>, failed: (detail: string) => string) => {
    const key = selectorKey(selector);
    const inFlight = rewriting.current;
    if (inFlight.has(key)) return;
    inFlight.add(key);
    setBusy(new Set(inFlight));
    setTriageError(null);
    try {
      await write();
      reload();
    } catch (e) {
      inFlight.delete(key);
      setBusy(new Set(inFlight));
      setTriageError(failed(requestDetail(e)));
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
      <ScannersWidget status={page.status} onFinished={reload} />
      <FindingsWidget
        page={page}
        scanned={scanned}
        kept={kept}
        hidden={hidden}
        filters={filters}
        setFilters={applyFilters}
        categories={categories}
        classify={classify}
        onSelect={select}
        onSuppress={setSuppressing}
        onUnsuppress={(selector) => void unsuppress(selector)}
        onReconfirm={(fingerprint, selector) => void reconfirm(fingerprint, selector)}
        rewriting={busy}
        triageError={triageError}
      />
      <SuppressionsWidget
        entries={page.findings.suppressions}
        onUnsuppress={(selector) => void unsuppress(selector)}
        onReconfirm={(fingerprint, selector) => void reconfirm(fingerprint, selector)}
        rewriting={busy}
      />
      <OwaspWidget
        page={page}
        edition={edition}
        setEdition={switchEdition}
        categories={categories}
        classify={classify}
        scope={filters.scope}
      />
      {scanned && total > 0 && <FindingDetail finding={detail} classification={detail ? classify(detail) : null} shown={shown} />}
      {suppressing && (
        <SuppressDialog
          key={findingKey(suppressing)}
          finding={suppressing}
          onSubmit={(selector, reason, expires, evidence) => suppress(selector, reason, expires, evidence)}
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

interface FindingsWidgetProps {
  page: SecurityPage;
  scanned: boolean;
  kept: SecurityFinding[];
  /** The findings the other filters keep and the code scope hides. */
  hidden: number;
  filters: FindingFilters;
  setFilters: (f: FindingFilters) => void;
  categories: SecurityCategoryCoverage[];
  classify: (f: SecurityFinding) => Classification;
  onSelect: (key: string) => void;
  /** Opens the suppression dialog for an unsuppressed finding. */
  onSuppress: (f: SecurityFinding) => void;
  /** Removes the entry a suppressed, out-of-date or expired finding's label names. */
  onUnsuppress: (selector: SecuritySelector) => void;
  /** Re-confirms an out-of-date verdict: the finding's fingerprint, and the
   *  selector of the entry it re-records. */
  onReconfirm: (fingerprint: string, selector: SecuritySelector) => void;
  /** The entries (by `selectorKey`) whose unsuppress or re-confirm is in flight or awaiting
   *  the reload: every button rewriting one is disabled. */
  rewriting: ReadonlySet<string>;
  /** Why the last unsuppress or re-confirm was refused, if it was. */
  triageError: string | null;
}

/** A finding's location — its path through `PathCell` (FR-UI-44), with `:line`
 *  — and, for one found in history (S-767), its `historical` label and short
 *  commit: the payload's own spelling, as the CLI and MCP print it. `label` is
 *  the abbreviation the table gave the path. */
function LocationCell({ finding, label }: { finding: SecurityFinding; label: string | undefined }) {
  const { anchor } = finding;
  const path = finding.path ?? finding.report_path;
  return (
    <>
      {path === null ? findingLocation(finding) : <PathCell path={path} label={label} />}
      {path !== null && finding.start_line !== null && (
        <>
          <wbr />
          <span>:{finding.start_line}</span>
        </>
      )}
      <TestTag finding={finding} />
      <ReasonTag why={devDependencyText(finding)} label={SCOPE_TEXT.devTag} data={{ "data-code-scope": "dev" }} />
      <ReasonTag why={needsReviewText(finding)} label={SCOPE_TEXT.reviewTag} data={{ "data-needs-review": "true" }} />
      {finding.binding === "historical" && (
        <>
          {" "}
          <Badge tone="muted">{ANCHOR_TEXT.historical}</Badge>
        </>
      )}
      {anchor && (
        <>
          {" "}
          <span className="muted" title={ANCHOR_TEXT.commit(anchor)} data-commit>
            {anchor.short_commit}
          </span>
        </>
      )}
    </>
  );
}

/** The muted `TEST` tag of a finding in test code, its reason on hover and, for
 *  a reader without one, after it; nothing for a production finding (FR-SC-28). */
function TestTag({ finding }: { finding: SecurityFinding }) {
  const why = scopeReasonText(finding);
  if (why === null) return null;
  return (
    <>
      {" "}
      <span title={why} data-code-scope="test">
        <Badge tone="muted">{SCOPE_TEXT.tag}</Badge>
        <span className="sr-only"> {why}</span>
      </span>
    </>
  );
}

/** The muted tag of a finding left out of the default scope for a reason other
 *  than test code — `DEV` for a dev-only dependency (FR-SC-30), `REVIEW` for a
 *  finding its scanner marks low-confidence (FR-SC-31) — its reason on hover and,
 *  for a reader without one, after it, on a line of its own under the location.
 *  Nothing when `why` is `null`. `data` is the attribute the specs find it by. */
function ReasonTag({ why, label, data }: { why: string | null; label: string; data: Record<`data-${string}`, string> }) {
  if (why === null) return null;
  return (
    <span className={styles.reviewTag} title={why} {...data}>
      <Badge tone="muted">{label}</Badge>
      <span className="sr-only"> {why}</span>
    </span>
  );
}

/** The code scope control: which code the table lists. */
function ScopeField({ value, onChange }: { value: ScopeFilter; onChange: (scope: ScopeFilter) => void }) {
  return (
    <SelectField label={SCOPE_TEXT.label} value={value} onChange={(e) => onChange(e.target.value as ScopeFilter)}>
      {SCOPES.map((s) => (
        <option key={s} value={s}>
          {SCOPE_TEXT.options[s]}
        </option>
      ))}
    </SelectField>
  );
}

/** A rule id, truncated to the column with its full value as tooltip. The full
 *  id stays the cell's text, so it is also the accessible name. */
function RuleCell({ ruleId }: { ruleId: string }) {
  return (
    <span className={styles.rule} title={ruleId}>
      {ruleId}
    </span>
  );
}

/** The enclosing symbol's kind and short name — "function `render`" — with the
 *  full symbol as tooltip; `—` and the reason when no symbol encloses it. */
function SymbolCell({ finding }: { finding: SecurityFinding }) {
  const { symbol, unbound_because } = finding.context.enclosing;
  if (symbol === null) {
    const why = CONTEXT_TEXT.unbound(unbound_because ?? "no_location");
    return (
      <span title={why}>
        —<span className="sr-only"> {why}</span>
      </span>
    );
  }
  return (
    <span title={symbol.symbol}>
      {symbolKindWord(symbol.kind)} <span className={`mono ${styles.symbolName}`}>{symbol.name}</span>
    </span>
  );
}

/** The sentence an empty table reads: the scope's hidden findings first, since
 *  they are the ones the reader can still reach, then a path with no finding. */
function emptyTable(filters: FindingFilters, hidden: number): string {
  if (hidden > 0) {
    return SCOPE_TEXT.hidden(hidden, filters.scope === "production" ? "production" : "test-code");
  }
  return filters.path !== "" ? SECURITY_ABSENCE.noFindingAt(filters.path) : SECURITY_ABSENCE.noMatch;
}

function FindingsWidget({
  page,
  scanned,
  kept,
  hidden,
  filters,
  setFilters,
  categories,
  classify,
  onSelect,
  onSuppress,
  onUnsuppress,
  onReconfirm,
  rewriting,
  triageError,
}: FindingsWidgetProps) {
  // One abbreviation per distinct path over the whole table, so no two rows share a label.
  const labels = useMemo(() => abbreviatePaths(kept.flatMap((f) => (f.path ?? f.report_path) ?? [])), [kept]);
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
      cell: (f) => (
        <span title={f.severity_reason}>
          <Badge tone={severityTone(f.severity)}>{f.severity}</Badge>
          {f.suppression && (
            <>
              {" "}
              <Badge tone={f.suppression.state === "out_of_date" ? "orange" : "muted"}>{suppressionState(f.suppression)}</Badge>
            </>
          )}
        </span>
      ),
    },
    { key: "rule", header: "Rule", mono: true, sortValue: (f) => f.rule_id, cell: (f) => <RuleCell ruleId={f.rule_id} /> },
    {
      key: "loc",
      header: "Location",
      mono: true,
      sortValue: (f) => findingLocation(f),
      cell: (f) => <LocationCell finding={f} label={labels.get(f.path ?? f.report_path ?? "")} />,
    },
    { key: "symbol", header: "Symbol", cell: (f) => <SymbolCell finding={f} /> },
    {
      key: "reach",
      header: <span title={FINDINGS_TABLE_TEXT.reachableTip}>Reachable</span>,
      cell: (f) => (
        <span title={reachabilityText(f.context.reachability)} data-reachable={f.context.reachability.verdict}>
          {reachableWord(f.context.reachability.verdict)}
        </span>
      ),
    },
    { key: "scanner", header: "Scanner", cell: (f) => findingScanners(f).join(", ") },
    { key: "owasp", header: "OWASP", mono: true, cell: (f) => classify(f).categories.join(", ") || "—" },
    {
      key: "open",
      header: "Show",
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
          <>
            {f.suppression.state === "out_of_date" && (
              <>
                <Button
                  size="sm"
                  title={EVIDENCE_TEXT.reconfirmTip}
                  onClick={() => f.suppression && onReconfirm(f.fingerprint, f.suppression.by)}
                  disabled={rewriting.has(selectorKey(f.suppression.by))}
                  aria-label={`Re-confirm ${f.rule_id} at ${findingLocation(f)}`}
                >
                  {EVIDENCE_TEXT.reconfirm}
                </Button>{" "}
              </>
            )}
            <Button
              size="sm"
              onClick={() => f.suppression && onUnsuppress(f.suppression.by)}
              disabled={rewriting.has(selectorKey(f.suppression.by))}
              aria-label={`Unsuppress ${f.rule_id} at ${findingLocation(f)}`}
            >
              {SUPPRESS_TEXT.unsuppress}
            </Button>
          </>
        ) : (
          <Button size="sm" onClick={() => onSuppress(f)} aria-label={`Suppress ${f.rule_id} at ${findingLocation(f)}`}>
            {SUPPRESS_TEXT.suppress}
          </Button>
        ),
    },
  ];
  const set = (patch: Partial<FindingFilters>) => setFilters({ ...filters, ...patch });
  const scanners = page.findings.scanned.map((s) => s.driver);
  const { counted, excluded } = page.counts;
  return (
    <Widget
      title="Findings"
      copy={securityFindings}
      figure={
        <div>
          <span>
            {kept.length} <FigureNote>of {page.findings.total} {plural(page.findings.total, "finding", "findings")} shown</FigureNote>
          </span>
          <FigureNote block>
            {FINDING_COUNT_TEXT.totals(counted.critical, counted.high, counted.medium, counted.low, counted.info, counted.unrated)}{" "}
            {FINDING_COUNT_TEXT.testCode(excluded.test)} {FINDING_COUNT_TEXT.devOnly(excluded.dev)}{" "}
            {FINDING_COUNT_TEXT.needsReview(excluded.needs_review)}
          </FigureNote>
          <FigureNote block>{FINDING_COUNT_TEXT.excluded(excluded.not_fresh, excluded.suppressed)}</FigureNote>
        </div>
      }
    >
      <div className={styles.filters} role="group" aria-label="Filter the findings">
        <ScopeField value={filters.scope} onChange={(scope) => set({ scope })} />
        <SelectField
          label="Least severity"
          value={filters.severity}
          onChange={(e) => set({ severity: e.target.value as FindingFilters["severity"] })}
        >
          <option value="">Any</option>
          {SEVERITIES.map((s) => (
            <option key={s} value={s}>
              {s === "unrated" ? "unrated only" : s}
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
          label="Reachable"
          title={FINDINGS_TABLE_TEXT.reachableTip}
          value={filters.reachability}
          onChange={(e) => set({ reachability: e.target.value as FindingFilters["reachability"] })}
        >
          <option value="">Any</option>
          <option value="reachable">{reachableWord("reachable")}</option>
          <option value="unreachable">{reachableWord("unreachable")}</option>
          <option value="unknown">{reachableWord("unknown")}</option>
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
        className={styles.findings}
        empty={emptyTable(filters, hidden)}
      />
    </Widget>
  );
}

/** Every entry of the ignore file with what it suppresses now (S-770, FR-SC-11):
 *  the count of findings it suppresses, and an orphaned or expired entry
 *  flagged. Shown once there is an entry; removing one is the same Unsuppress
 *  the findings table sends. */
function SuppressionsWidget({
  entries,
  onUnsuppress,
  onReconfirm,
  rewriting,
}: {
  entries: SecurityEntryStatus[];
  onUnsuppress: (selector: SecuritySelector) => void;
  onReconfirm: (fingerprint: string, selector: SecuritySelector) => void;
  rewriting: ReadonlySet<string>;
}) {
  if (entries.length === 0) return null;
  const selectorOf = (e: SecurityEntryStatus): SecuritySelector => {
    if ("fingerprint" in e.entry) return { fingerprint: e.entry.fingerprint };
    if (!("path" in e.entry)) return { rule: e.entry.rule, scanner: e.entry.scanner };
    return { path: e.entry.path, ...(e.entry.rule ? { rule: e.entry.rule } : {}), ...(e.entry.scanner ? { scanner: e.entry.scanner } : {}) };
  };
  const suppressed = entries.reduce((n, e) => n + e.suppressed, 0);
  const columns: Column<SecurityEntryStatus>[] = [
    { key: "what", header: "Suppresses", mono: true, cell: (e) => selectorText(selectorOf(e)) },
    {
      key: "reason",
      header: "Reason",
      cell: (e) => (
        <>
          {e.entry.reason}
          {e.out_of_date.length > 0 && (
            <>
              <br />
              <span data-entry-out-of-date>{EVIDENCE_TEXT.entryOutOfDate(e.out_of_date)}</span>
            </>
          )}
        </>
      ),
    },
    { key: "expires", header: "Expires", mono: true, cell: (e) => e.entry.expires ?? "—" },
    { key: "count", header: "Findings", sortValue: (e) => e.suppressed, cell: (e) => SUPPRESSIONS_TEXT.count(e.suppressed) },
    {
      key: "state",
      header: "State",
      cell: (e) => (
        <>
          <Badge tone={e.orphaned || e.expired || e.out_of_date.length > 0 ? "orange" : "muted"}>{SUPPRESSIONS_TEXT.state(e)}</Badge>
          {e.should_name && (
            <>
              {" "}
              <span className="muted">{SUPPRESSIONS_TEXT.shouldName(e.should_name)}</span>
            </>
          )}
        </>
      ),
    },
    {
      key: "remove",
      header: "Triage",
      cell: (e) => (
        <>
          {e.out_of_date.length > 0 && "fingerprint" in e.entry && (
            <>
              <Button
                size="sm"
                title={EVIDENCE_TEXT.reconfirmTip}
                onClick={() => "fingerprint" in e.entry && onReconfirm(e.entry.fingerprint, selectorOf(e))}
                disabled={rewriting.has(selectorKey(selectorOf(e)))}
                aria-label={SUPPRESSIONS_TEXT.reconfirm(selectorOf(e))}
              >
                {EVIDENCE_TEXT.reconfirm}
              </Button>{" "}
            </>
          )}
          <Button
            size="sm"
            onClick={() => onUnsuppress(selectorOf(e))}
            disabled={rewriting.has(selectorKey(selectorOf(e)))}
            aria-label={SUPPRESSIONS_TEXT.remove(selectorOf(e))}
          >
            {SUPPRESS_TEXT.unsuppress}
          </Button>
        </>
      ),
    },
  ];
  return (
    <Widget title={SUPPRESSIONS_TEXT.title} copy={suppressionEntries} figure={<span>{SUPPRESSIONS_TEXT.figure(entries.length, suppressed)}</span>}>
      <DataTable caption={SUPPRESSIONS_TEXT.caption} columns={columns} rows={entries} rowKey={(e) => selectorKey(selectorOf(e))} />
    </Widget>
  );
}

interface OwaspWidgetProps {
  page: SecurityPage;
  edition: OwaspEditionYear;
  setEdition: (e: OwaspEditionYear) => void;
  categories: SecurityCategoryCoverage[];
  classify: (f: SecurityFinding) => Classification;
  /** The page's code scope: the counts below are of the findings it lists. */
  scope: ScopeFilter;
}

/** The OWASP Top 10 view with the 2025/2021 switch (FR-SC-04): switching reads the
 *  other edition from the same payload. Each count is of the findings in the
 *  code scope — production by default — read off the category's own
 *  `state.findings` and `production_findings`, the figures `security findings`
 *  states, never counted again here; the figure states the
 *  findings in test code the page counts apart: the one number the Findings
 *  figure and `security status` state (FR-SC-28). */
function OwaspWidget({ page, edition, setEdition, categories, classify, scope }: OwaspWidgetProps) {
  const unclassified = unclassifiedCount(page.findings.findings, classify, scope);
  const columns: Column<SecurityCategoryCoverage>[] = [
    { key: "id", header: "Category", mono: true, cell: (c) => c.id },
    { key: "name", header: "Name", cell: (c) => c.name },
    {
      key: "state",
      header: "Findings",
      cell: (c) =>
        c.state.state === "covered" ? OWASP_TEXT.covered(categoryCount(c, scope) ?? 0) : notCoveredSentence(c, page.status),
    },
  ];
  return (
    <Widget
      title="OWASP Top 10"
      copy={owaspView}
      figure={
        <span>
          {OWASP_TEXT.edition(edition)}{" "}
          <FigureNote>
            {FINDING_COUNT_TEXT.testCode(page.counts.excluded.test)} {FINDING_COUNT_TEXT.devOnly(page.counts.excluded.dev)}{" "}
            {FINDING_COUNT_TEXT.needsReview(page.counts.excluded.needs_review)}
          </FigureNote>
          {unclassified > 0 && <FigureNote> · {OWASP_TEXT.unclassified(unclassified)}</FigureNote>}
        </span>
      }
    >
      <EditionSwitch edition={edition} onChange={setEdition} />
      <DataTable caption={OWASP_TEXT.edition(edition)} columns={columns} rows={categories} rowKey={(c) => c.id} />
    </Widget>
  );
}
