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
 *   finding's graph context already carries. A `?path=` deep link (Files & Risk, S-671) seeds the path.
 * - Absent states: each scanner state and each not-covered category resolves to
 *   the command that fixes it (`copy/security.copy.ts`).
 * - Redaction: a secret-class message is never rendered, whatever text it holds.
 */

import type {
  OwaspEditionYear,
  SecurityCategoryCoverage,
  SecurityEditionClassification,
  SecurityFinding,
  SecurityFindingContext,
  SecurityPage,
  SecurityScannerStatus,
  SecuritySeverity,
  SecurityStatus,
  SecurityUnclassified,
} from "../../api/types.ts";
import type { BadgeTone } from "../../components/index.ts";
import { REDACTED, SCANNER_STATE_TEXT, SECURITY_COMMANDS, notCoveredText } from "../../copy/security.copy.ts";

/** Severities from most to least severe. */
export const SEVERITIES: readonly SecuritySeverity[] = ["critical", "high", "medium", "low", "info"];

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

/** The message a finding renders: a secret-class one reads `[redacted]` (FR-SC-03). */
export function findingMessage(f: SecurityFinding): string {
  return f.redacted ? REDACTED : f.message;
}

/** `path:line`, the report's path when the finding bound to no indexed file. */
export function findingLocation(f: SecurityFinding): string {
  const path = f.path ?? f.report_path;
  if (path === null) return "no location";
  return f.start_line !== null ? `${path}:${f.start_line}` : path;
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
}

export const NO_FILTERS: FindingFilters = {
  severity: "",
  scanner: "",
  category: "",
  path: "",
  suppression: "",
  reachability: "",
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

/** A severity's rank, higher for more severe: the one order the filter and the
 *  table's sort share. */
export function severityRank(s: SecuritySeverity): number {
  return SEVERITIES.length - SEVERITIES.indexOf(s);
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
  const severity = filters.severity === "" || severityRank(f.severity) >= severityRank(filters.severity);
  const want = filters.scanner.toLowerCase();
  const scanner =
    want === "" || f.scanner_name.toLowerCase() === want || scanners.get(want) === f.scanner_name;
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
  return severity && scanner && category && path && suppression && reachability;
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
    case "stale":
      // A report ingested from CI, or from a tool that is not a registered
      // scanner, has no copy here to run: only a newer report refreshes it.
      return SCANNER_STATE_TEXT.stale(row.because, row.installed !== null ? SECURITY_COMMANDS.scan : SECURITY_COMMANDS.ingest);
    case "fresh":
      return SCANNER_STATE_TEXT.fresh(row.findings, row.suppressed);
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
