/*
 * The Security page's pure model (S-670, FR-SC-09): the decisions the view makes
 * over one `/api/v1/security` read, kept out of the components so each is tested
 * on its own.
 *
 * - Edition: the payload carries both OWASP editions (`editions`), so switching
 *   re-classifies from memory — nothing here fetches.
 * - Filters mirror `security findings` (`keeps` in
 *   `logos-core/src/security/surface.rs`): least severity, scanner by registry or
 *   driver name, category by full id or number, a file or a directory holding it,
 *   and suppression — plus route reachability, which FR-SC-08 lists and each
 *   finding's graph context already carries. The code scope is the one clause
 *   `keeps` here folds in that the server applies in a pass of its own (so it can
 *   count `hidden_by_scope`; `hiddenByScope` below counts the same way). A `?path=` deep link (Files & Risk, S-671) seeds the path.
 * - Absent states: each scanner state and each not-covered category resolves to
 *   the command that fixes it (`copy/security.copy.ts`).
 * - Redaction: a secret-class message is never rendered, whatever text it holds;
 *   the page says why, and where the finding is (S-796).
 * - Code scope (FR-SC-28, S-796): the payload lists every finding with its
 *   `code_scope`; the Production · Test code · All control narrows it from
 *   memory, is kept in the URL's `?scope=`, and the OWASP counts follow it.
 * - Triage: "everything under this path" starts from the finding's directory
 *   (S-770), and an entry's selector keys the rows an unsuppress is in flight for.
 * - Live state (S-792, CR-245): the job the page already polls says which
 *   scanner runs and which tool installs — `jobProgress` reads its events, and
 *   `liveStates` is what each Scanners row reads while the job runs. Nothing is
 *   added to the job's wire.
 * - Ignored dirs: a row's marker is picked from its `coverage_kind`, never from
 *   the note's text; `markersShown` is what the legend lists.
 */

import type {
  InstallJob,
  OwaspEditionYear,
  ScanJob,
  SecurityCoverage,
  SecurityCategoryCoverage,
  SecurityCodeScope,
  SecurityEditionClassification,
  SecurityFinding,
  SecurityFindingContext,
  SecurityPage,
  SecurityInstallStep,
  SecurityScannerStatus,
  SecuritySelector,
  SecuritySeverity,
  SecurityStatus,
  SecurityUnclassified,
} from "../../api/types.ts";
import type { BadgeTone } from "../../components/index.ts";
import { pairKey } from "../../workspace/scope.ts";
import {
  CONTEXT_TEXT,
  FINDINGS_TABLE_TEXT,
  INSTALL_JOB_TEXT,
  INSTALL_OUTCOME_TEXT,
  REDACTED_TEXT,
  SCAN_JOB_TEXT,
  SCOPE_TEXT,
  SCANNER_STATE_TEXT,
  SECURITY_COMMANDS,
  notCoveredText,
  IGNORED_DIRS_TEXT,
  type IgnoredDirsMarker,
} from "../../copy/security.copy.ts";

/** Severities from most to least severe, `unrated` (no rating, S-768) where a
 *  threshold gates it: with `medium`, listed just after it. */
export const SEVERITIES: readonly SecuritySeverity[] = ["critical", "high", "medium", "unrated", "low", "info"];

/** A count for each severity: what `SeverityRow` (S-789) is given. The server's
 *  `SeverityCounts` (with its `total`) is one; typed here, beside `SEVERITIES`,
 *  so the row names no further importer of the wire-type hub. */
export type SeverityCounts = Record<SecuritySeverity, number>;

/** The editions the page offers, the default first. */
export const EDITIONS: readonly OwaspEditionYear[] = ["2025", "2021"];

/** A finding's identity: its scanner and its fingerprint (unique within it). */
export function findingKey(f: { scanner_name: string; fingerprint: string }): string {
  return `${f.scanner_name}\u0000${f.fingerprint}`;
}

/** One finding's classification under one edition. */
export interface Classification {
  categories: string[];
  unclassified: SecurityUnclassified | null;
}

/** The edition's classification of every stored finding, by {@link findingKey},
 *  and its category coverage — read from the payload, never refetched. */
export function editionOf(page: SecurityPage, year: OwaspEditionYear): {
  categories: SecurityCategoryCoverage[];
  classify: (f: SecurityFinding) => Classification;
} {
  const edition: SecurityEditionClassification | undefined = page.editions.find((e) => e.edition === year);
  const byKey = new Map((edition?.findings ?? []).map((c) => [findingKey(c), c]));
  return {
    categories: edition?.categories ?? [],
    classify: (f) => {
      const c = byKey.get(findingKey(f));
      return { categories: c?.categories ?? [], unclassified: c?.unclassified ?? null };
    },
  };
}

/** The message a finding renders: a secret-class one reads why it is withheld
 *  and where the finding is, whatever text it holds (FR-SC-03, S-796). */
export function findingMessage(f: SecurityFinding): string {
  if (!f.redacted) return f.message;
  const located = (f.path ?? f.report_path) !== null;
  return REDACTED_TEXT.why + (located ? REDACTED_TEXT.at(findingLocation(f)) : REDACTED_TEXT.nowhere);
}

/** `path:line`, the report's path when the finding bound to no indexed file. */
export function findingLocation(f: SecurityFinding): string {
  const path = f.path ?? f.report_path;
  if (path === null) return "no location";
  return f.start_line !== null ? `${path}:${f.start_line}` : path;
}

/** The word the Reachable column reads for a verdict: yes / no / unknown.
 *  `unknown` stays its own word — never `no` (NFR-CC-04). */
export function reachableWord(verdict: SecurityFindingContext["reachability"]["verdict"]): string {
  return FINDINGS_TABLE_TEXT.reachable[verdict];
}

/** The sentence behind a finding's Reachable word: the Reachable cell's tooltip
 *  and the detail's line. */
export function reachabilityText(r: SecurityFindingContext["reachability"]): string {
  if (r.verdict === "reachable" && r.route !== null) return CONTEXT_TEXT.reachable(r.route.name, r.route.hops);
  if (r.verdict === "unreachable") return CONTEXT_TEXT.unreachable;
  return CONTEXT_TEXT.unknown(r.unresolved_because);
}

/** A symbol's kind as the Symbol column reads it: the wire token's words
 *  (`type_alias` → "type alias"). */
export function symbolKindWord(kind: string): string {
  return kind.replaceAll("_", " ");
}

/** The path "everything under this path" offers for `f` (S-770): the directory
 *  holding its file, or the file itself at the project root. An absolute report
 *  path — a lockfile a scanner names where it found it — has no project-relative
 *  spelling on the page, so its file name is offered under a leading `**`
 *  segment, for the user to narrow.
 *  `""` when the finding has no location. */
export function suggestedPath(f: SecurityFinding): string {
  const path = (f.path ?? f.report_path ?? "").replaceAll("\\", "/").replace(/^(\.\/)+/, "");
  const cut = path.lastIndexOf("/");
  if (path.startsWith("/") || /^[A-Za-z]:\//.test(path)) return `**/${path.slice(cut + 1)}`;
  return cut === -1 ? path : path.slice(0, cut);
}

/** One entry's identity: what an unsuppress is keyed by, so every row one path
 *  entry covers waits on the same removal. */
export function selectorKey(s: SecuritySelector): string {
  if ("fingerprint" in s) return `fingerprint\u0000${s.fingerprint}`;
  if (!("path" in s)) return `rule\u0000${s.rule}\u0000${s.scanner}`;
  return `path\u0000${s.path}\u0000${s.rule ?? ""}\u0000${s.scanner ?? ""}`;
}

/** The page's filters; an empty string is "any". */
export interface FindingFilters {
  severity: SecuritySeverity | "";
  scanner: string;
  category: string;
  path: string;
  suppression: "" | "active" | "suppressed";
  /** The route-reachability verdict (FR-SC-08); `unknown` is its own value,
   *  never folded into `unreachable`. */
  reachability: "" | SecurityFindingContext["reachability"]["verdict"];
  /** Which code the table lists (FR-SC-28): production by default, never "any". */
  scope: ScopeFilter;
}

/** The code scope control's values, as `security findings --scope` spells them. */
export type ScopeFilter = Exclude<SecurityCodeScope, "dev"> | "all";

/** The scopes the control offers, the default first. */
export const SCOPES: readonly ScopeFilter[] = ["production", "test", "all"];

/** The scope a query string names; absent or unrecognised is production. The last
 *  `scope` wins, as the last `repo` does (`memberFromSearch`). */
export function scopeFromSearch(search: string): ScopeFilter {
  const want = new URLSearchParams(search).getAll("scope").at(-1)?.toLowerCase();
  return SCOPES.find((s) => s === want) ?? "production";
}

/** `url` with its `?scope=` set to `scope`. Production is the default, so it
 *  carries none; every other pair — `?repo=`, `?path=` — is kept byte-for-byte,
 *  as `urlWithMember` keeps them. A pair is `scope` by its decoded key
 *  (`pairKey`), as `scopeFromSearch` reads it. */
export function urlWithScope(url: string, scope: ScopeFilter): string {
  const hashAt = url.indexOf("#");
  const hash = hashAt === -1 ? "" : url.slice(hashAt);
  const bare = hashAt === -1 ? url : url.slice(0, hashAt);
  const queryAt = bare.indexOf("?");
  const path = queryAt === -1 ? bare : bare.slice(0, queryAt);
  const kept = (queryAt === -1 ? "" : bare.slice(queryAt + 1)).split("&").filter((pair) => pair !== "" && pairKey(pair) !== "scope");
  if (scope !== "production") kept.push(`scope=${scope}`);
  return `${path}${kept.length > 0 ? `?${kept.join("&")}` : ""}${hash}`;
}

/** Why a finding reads `test`, in words — the `TEST` tag's hover and the detail's
 *  line; `null` for a production finding. */
export function scopeReasonText(f: SecurityFinding): string | null {
  if (f.code_scope !== "test") return null;
  return f.code_scope_reason === null ? SCOPE_TEXT.reason.unstated : SCOPE_TEXT.reason[f.code_scope_reason];
}

/** Why a finding reads `dev`, in words — the `DEV` tag's hover and the detail's
 *  line; `null` for a finding that is not on a dev-only dependency (FR-SC-30). */
export function devDependencyText(f: SecurityFinding): string | null {
  return f.code_scope === "dev" ? SCOPE_TEXT.reason.dev_dependency : null;
}

/** Why a finding reads "needs review", in words — the tag's hover and the
 *  detail's line; `null` for a finding its scanner does not mark low-confidence. */
export function needsReviewText(f: SecurityFinding): string | null {
  return f.needs_review ? SCOPE_TEXT.reviewReason(f.confidence) : null;
}

/** Does the scope list a finding? */
export function inScope(f: SecurityFinding, scope: ScopeFilter): boolean {
  if (scope === "all") return true;
  // The default scope also leaves out a finding its scanner marks low-confidence
  // (FR-SC-31, S-817): "needs review", listed under `all`, counted apart. A
  // finding on a dev-only dependency (`dev`, FR-SC-30, S-810) is in neither
  // production nor test code, so only `all` lists it.
  return scope === "production" ? f.code_scope === "production" && !f.needs_review : f.code_scope === "test";
}

/** How many findings the scope alone leaves out of the table: those every
 *  other filter keeps (the CLI's `hidden_by_scope`, `hidden_needs_review` and
 *  `hidden_dev` together, counted the same way). */
export function hiddenByScope(
  findings: readonly SecurityFinding[],
  filters: FindingFilters,
  classify: (f: SecurityFinding) => Classification,
  scanners: ReadonlyMap<string, string>,
): number {
  const rest = { ...filters, scope: "all" as const };
  return findings.filter((f) => !inScope(f, filters.scope) && keeps(f, rest, classify(f), scanners)).length;
}

/** How many findings in `scope` fall under no OWASP category. */
export function unclassifiedCount(
  findings: readonly SecurityFinding[],
  classify: (f: SecurityFinding) => Classification,
  scope: ScopeFilter,
): number {
  return findings.filter((f) => inScope(f, scope) && classify(f).unclassified !== null).length;
}

/** A covered category's count of the findings in `scope`, as the server states
 *  it: `state.findings` counts every code scope and `production_findings` the
 *  production part of it, suppressed findings in both (`categories[]` of
 *  `security findings`). Test code is what is left of the first after the
 *  second, so the three scopes partition one figure and the page holds no count
 *  of its own. `null` for a category nothing could detect. */
export function categoryCount(c: SecurityCategoryCoverage, scope: ScopeFilter): number | null {
  if (c.state.state !== "covered") return null;
  const production = c.production_findings ?? 0;
  // Production, dev-only, needs review and test code partition `findings`
  // (FR-SC-30, FR-SC-31).
  const review = c.needs_review_findings ?? 0;
  const dev = c.dev_findings ?? 0;
  return scope === "all" ? c.state.findings : scope === "production" ? production : c.state.findings - production - review - dev;
}

export const NO_FILTERS: FindingFilters = {
  severity: "",
  scanner: "",
  category: "",
  path: "",
  suppression: "",
  reachability: "",
  scope: "production",
};

/** A path as `security findings --path` reads it: no `./`, no trailing `/`, `/`
 *  separators. */
export function normalisePath(path: string): string {
  return path.trim().replace(/\\/g, "/").replace(/^(\.\/)+/, "").replace(/\/+$/, "");
}

/** The `?path=` a deep link carries (S-671's Files & Risk links), or `""`. */
export function pathFromSearch(search: string): string {
  return normalisePath(new URLSearchParams(search).get("path") ?? "");
}

/** A severity's rank, higher for more severe: the order the table sorts by. */
export function severityRank(s: SecuritySeverity): number {
  return SEVERITIES.length - SEVERITIES.indexOf(s);
}

/** The severity a threshold compares, as the server's `Severity::gated_as`
 *  does: `unrated` is gated as `medium`. */
export function gatedAs(s: SecuritySeverity): SecuritySeverity {
  return s === "unrated" ? "medium" : s;
}

/** Severity badge tone: critical/high red, medium orange, the rest muted. Text
 *  always rides with the colour (a11y). The Files & Risk findings cell shares it. */
export function severityTone(severity: SecuritySeverity): BadgeTone {
  if (severity === "critical" || severity === "high") return "red";
  if (severity === "medium") return "orange";
  return "muted";
}

/** Does the finding pass every filter that is set? `scanners` maps a registry
 *  name to its driver, so `semgrep` matches `Semgrep OSS`. */
export function keeps(
  f: SecurityFinding,
  filters: FindingFilters,
  classification: Classification,
  scanners: ReadonlyMap<string, string>,
): boolean {
  // `unrated` lists the unrated findings alone; a rated least severity compares
  // an unrated finding as a gate does, as `medium` (the CLI's `--severity`).
  const severity =
    filters.severity === "" ||
    (filters.severity === "unrated"
      ? f.severity === "unrated"
      : severityRank(gatedAs(f.severity)) >= severityRank(filters.severity));
  // A merged finding is any of its reporters' (FR-SC-29).
  const want = filters.scanner.toLowerCase();
  const scanner =
    want === "" || findingScanners(f).some((name) => name.toLowerCase() === want || scanners.get(want) === name);
  const category =
    filters.category === "" ||
    classification.categories.some((id) => {
      const c = filters.category.toLowerCase();
      return id.toLowerCase() === c || id.split(":")[0].toLowerCase() === c;
    });
  const at = normalisePath(filters.path);
  const where = f.path ?? f.report_path;
  const path = at === "" || (where !== null && (where === at || where.startsWith(`${at}/`)));
  const suppressed = f.suppression?.state === "suppressed";
  const suppression = filters.suppression === "" || suppressed === (filters.suppression === "suppressed");
  const reachability = filters.reachability === "" || f.context.reachability.verdict === filters.reachability;
  return severity && scanner && category && path && suppression && reachability && inScope(f, filters.scope);
}

/** The scanners that reported `f`, each once, its own first: every reporter of
 *  one advisory several scanners report (FR-SC-29, S-809), else its scanner. */
export function findingScanners(f: Pick<SecurityFinding, "scanner_name" | "reporters">): string[] {
  const names = (f.reporters ?? []).map((r) => r.scanner_name);
  return [...new Set([f.scanner_name, ...names])];
}

/** Registry name → driver name, over the registered scanners. An unregistered
 *  report's name IS its driver, so it needs no entry — and one spelled like a
 *  registry name (`semgrep`) must not replace the registry's (`Semgrep OSS`). */
export function scannerDrivers(status: SecurityStatus): Map<string, string> {
  return new Map(status.scanners.map((row) => [row.scanner.toLowerCase(), row.driver]));
}

/** The sentence a scanner row states: its absence and the command that fixes
 *  it, or its finding count when fresh. `pending` is `status.consent.pending`:
 *  the scanners whose first scan needs `--consent`. */
export function scannerStateText(row: SecurityScannerStatus, pending: readonly string[] = []): string {
  switch (row.state) {
    case "not_installed":
      return SCANNER_STATE_TEXT.notInstalled(row.install);
    case "never_run":
      return SCANNER_STATE_TEXT.neverRun(scanFor([row], pending));
    case "disabled":
      return SCANNER_STATE_TEXT.disabled;
    case "not_applicable":
      return SCANNER_STATE_TEXT.notApplicable(row.reason);
    case "stale":
      // A report ingested from CI, or from a tool that is not a registered
      // scanner, has no copy here to run: only a newer report refreshes it.
      return SCANNER_STATE_TEXT.stale(row.because, row.installed !== null ? SECURITY_COMMANDS.scan : SECURITY_COMMANDS.ingest);
    case "fresh":
      return SCANNER_STATE_TEXT.fresh(row.findings, row.suppressed, row.merged);
  }
}

/** The scan that runs `rows`: with `--consent` when one of them awaits it. */
function scanFor(rows: readonly SecurityScannerStatus[], pending: readonly string[]): string {
  return rows.some((row) => pending.includes(row.scanner)) ? SECURITY_COMMANDS.scanWithConsent : SECURITY_COMMANDS.scan;
}

/**
 * The command that would cover a not-covered category: a scan when one of the
 * scanners that can detect it is installed but has not produced a report, else
 * installing and scanning when Logos can install one of them here (an install
 * alone produces no report) — never counting an opt-in scanner, which `scan
 * --install` does not install — else ingesting a report produced elsewhere,
 * which always works.
 */
export function notCoveredCommand(category: SecurityCategoryCoverage, status: SecurityStatus): string {
  const rows = category.scanners.flatMap((name) => status.scanners.filter((row) => row.scanner === name));
  const idle = rows.filter((row) => row.state === "never_run" || (row.state === "stale" && row.installed !== null));
  if (idle.length > 0) return scanFor(idle, status.consent.pending);
  const installable = rows.some((row) => row.state === "not_installed" && row.install !== null && row.opt_in !== true);
  return installable ? SECURITY_COMMANDS.installAndScan : SECURITY_COMMANDS.ingest;
}

/** The not-covered sentence for a category, with its command. */
export function notCoveredSentence(category: SecurityCategoryCoverage, status: SecurityStatus): string {
  return notCoveredText(notCoveredCommand(category, status));
}

/** The status types the Scanners widget reads, re-exported so the widget takes them
 *  from the model: a new importer of `api/types.ts` would push its fan-in past the
 *  architecture rule's cap. */
export type { SecurityScannerStatus, SecurityStatus } from "../../api/types.ts";

/** The scan or install job the page follows. */
export type Job = ScanJob | InstallJob;

/** What a job's replayed events say is happening, by registry name. */
export interface JobProgress {
  /** Scanners that finished, in the order they did, each with whether it ran. */
  finished: { scanner: string; ran: boolean }[];
  /** Scanners started and not finished. */
  running: string[];
  /** Tools whose install started and has not finished. */
  installing: string[];
  /** Tools whose install finished, whatever its outcome. */
  toolsDone: number;
  /** Whether the scan after an install has begun. */
  scanning: boolean;
}

/** The progress a job's events state; an absent job states none. */
export function jobProgress(job: Job | null): JobProgress {
  const steps: readonly SecurityInstallStep[] = job?.events.map((e) => e.event) ?? [];
  const running = new Set<string>();
  const installing = new Set<string>();
  const finished: JobProgress["finished"] = [];
  let toolsDone = 0;
  let scanning = false;
  for (const step of steps) {
    switch (step.phase) {
      case "started":
        running.add(step.scanner);
        scanning = true;
        break;
      case "finished":
        running.delete(step.scanner);
        finished.push({ scanner: step.scanner, ran: step.outcome === "ran" });
        scanning = true;
        break;
      case "install_started":
        installing.add(step.tool);
        break;
      case "install_finished":
        installing.delete(step.tool);
        toolsDone += 1;
        break;
      case "cancelled":
        break;
    }
  }
  return { finished, running: [...running], installing: [...installing], toolsDone, scanning };
}

/** A row's live state while its job runs. */
export type LiveState = "running" | "installing";

/** Each scanner running, and each tool installing, in a job that has not ended;
 *  the Scanners rows read these in place of their stored state. Once the job
 *  ends, none: the job is shown ended only after the page's reload landed, so
 *  every row then shows what the job stored (HF-2). */
export function liveStates(job: Job | null): ReadonlyMap<string, LiveState> {
  if (job?.state !== "running") return new Map();
  const { running, installing } = jobProgress(job);
  return new Map<string, LiveState>([...installing.map((t) => [t, "installing"] as const), ...running.map((n) => [n, "running"] as const)]);
}

/** The tools an install left out, each with why, from its events: the result
 *  exists only once it completed, and a cancelled install states them too. */
function toolsLeftOut(job: InstallJob): string[] {
  return job.events.flatMap(({ event }) => {
    if (event.phase !== "install_finished") return [];
    if (event.outcome === "unavailable") return [INSTALL_OUTCOME_TEXT.unavailable(event.tool, event.reason)];
    if (event.outcome === "failed") return [INSTALL_OUTCOME_TEXT.failed(event.tool, event.stage, event.reason)];
    return [];
  });
}

/** An ended install's own sentence: what it installed and what its scan found. */
function installOutcome(job: InstallJob): string {
  switch (job.state) {
    case "completed": {
      const installed = job.result?.install.tools.filter((t) => t.outcome === "installed").length ?? 0;
      const scan = job.result?.scan ?? null;
      return INSTALL_JOB_TEXT.completed(installed, scan && { findings: scan.findings, ran: scan.scanners.filter((s) => s.outcome === "ran").length });
    }
    case "cancelled": {
      const report = job.events.map(({ event }) => event).find((e) => e.phase === "cancelled");
      return INSTALL_JOB_TEXT.cancelled(report?.phase === "cancelled" ? report.installed : []);
    }
    case "failed":
      return INSTALL_JOB_TEXT.failed(job.error ?? "");
    case "running":
      return "";
  }
}

/** The one summary line's sentence for `job`: its progress while it runs, its
 *  outcome once it ended. */
export function summaryText(job: Job | null): string {
  if (job === null) return SCAN_JOB_TEXT.idle;
  const progress = jobProgress(job);
  if (job.kind === "tool-install") {
    if (job.state === "running") {
      return progress.scanning ? INSTALL_JOB_TEXT.scanning(progress.finished.length) : INSTALL_JOB_TEXT.running(progress.toolsDone);
    }
    const left = toolsLeftOut(job);
    return installOutcome(job) + (left.length > 0 ? INSTALL_JOB_TEXT.notInstalled(left) : "");
  }
  switch (job.state) {
    case "running":
      return SCAN_JOB_TEXT.running(progress.finished.length, progress.running);
    case "completed": {
      const ran = job.result?.scanners.filter((s) => s.outcome === "ran").length ?? 0;
      return SCAN_JOB_TEXT.completed(job.result?.findings ?? 0, ran);
    }
    case "cancelled":
      return SCAN_JOB_TEXT.cancelled(progress.finished.filter((f) => f.ran).map((f) => f.scanner));
    case "failed":
      return SCAN_JOB_TEXT.failed(job.error ?? "");
  }
}

/** Which Ignored dirs marker a row shows. */
export type IgnoredDirsKey = keyof typeof IGNORED_DIRS_TEXT;

/** The markers in the order the legend lists them. */
const MARKER_ORDER: readonly IgnoredDirsKey[] = ["skipped", "unproven", "history", "whole_project", "partial", "scanned", "none"];

/** A row's marker, from its `ignored_paths` and `coverage_kind` — never from its
 *  note. A row with no whole-project run to state shows `none`. */
export function ignoredDirsKey(row: SecurityCoverage): IgnoredDirsKey {
  if (row.ignored_paths === undefined) return "none";
  if (row.ignored_paths === "excluded") return "skipped";
  return row.coverage_kind ?? "scanned";
}

/** The legend: each marker some row shows, once, in a fixed order. */
export function markersShown(rows: readonly SecurityCoverage[]): { key: IgnoredDirsKey; text: IgnoredDirsMarker }[] {
  const shown = new Set(rows.map(ignoredDirsKey));
  return MARKER_ORDER.filter((key) => shown.has(key)).map((key) => ({ key, text: IGNORED_DIRS_TEXT[key] }));
}
