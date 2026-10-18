/*
 * The Security page's copy catalogue (S-670, FR-SC-09, FR-UI-39). Later security
 * UI stories (Files & Risk links, scan controls, triage, the workspace roll-up)
 * extend this module rather than starting a second one.
 *
 * Every absent state names the command that fixes it (CR-206, NFR-CC-04): no
 * scan at all, a scanner that is not installed or never ran, an OWASP category
 * nothing that ran can detect. Each command is a documented one —
 * `catalogueCommands.test.ts` holds this module to `docs/howto/commands.md`.
 * A not-covered category is never a count of zero (BR-66), and a reachability
 * Logos could not decide reads unknown, never unreachable (S-669).
 */

import type { SecurityFindingContext, SecurityStaleBecause, SecuritySuppression } from "../api/types.ts";

import { plural, type CopyEntry } from "./types.ts";

export const scannerMatrix: CopyEntry = {
  what: "Each security scanner's state here: not installed, disabled, installed but never run, stale since HEAD moved, or fresh with its finding count.",
  why: "A scanner that has not run on this commit has looked at nothing, so its silence is not a clean result.",
};

export const securityFindings: CopyEntry = {
  what: "Every stored finding with its severity, scanner, location and the symbol that encloses it, narrowed by the filters above the table.",
  why: "The enclosing symbol and whether a route reaches it say which findings an attacker can get to first.",
};

export const owaspView: CopyEntry = {
  what: "The findings grouped by OWASP Top 10 category for the chosen edition, with the categories no scanner that ran can detect marked not covered.",
  why: "A category nothing looked at is a blind spot, which a count of zero would hide.",
};

export const findingDetail: CopyEntry = {
  what: "One finding in the code graph: its enclosing symbol, whether a route reaches it, how much depends on it, and how far each claim can be trusted.",
  why: "A finding a route reaches, inside code many symbols depend on, is the one to fix first.",
};

export const securityGate: CopyEntry = {
  what: "The verdict the quality check reaches on the [security] table in .logos/rules.toml: the required scanners and the findings its rules fail on.",
  why: "The check fails on these, and exits 5 when a required scan is missing, so this is what blocks a merge.",
};

/** The commands an absent state names when the read-model names none itself. */
export const SECURITY_COMMANDS = {
  scan: "logos security scan",
  /** The first scan of a scanner the project's consent does not name yet. */
  scanWithConsent: "logos security scan --consent",
  /** Installs what Logos can install and is missing, then scans — an install
   *  alone produces no report, so it covers nothing. */
  installAndScan: "logos security scan --install --consent",
  ingest: "logos security ingest <report>",
} as const;

/** The text a secret-class finding's message renders as (FR-SC-03). */
export const REDACTED = "[redacted]";

/** The named absences, each with the command that fixes it. */
export const SECURITY_ABSENCE = {
  /** Nothing ever scanned or ingested: not a clean result. */
  noScan:
    "No security scan has run yet, so nothing was looked at; run logos security scan (with --consent the first time), or logos security ingest <report> for a report produced in CI.",
  /** Scanned, but the filters keep nothing. */
  noMatch: "No finding matches these filters.",
  /** Scanned, and nothing was found. */
  clean: "The scanners that ran found nothing.",
  /** A filter on a path that holds no finding. */
  noFindingAt: (path: string) => `No finding in ${path}.`,
  /** No finding selected for the detail. */
  noneSelected: "Choose a finding in the table to see it in the code graph.",
} as const;

/** The fixing sentence for each scanner state that is an absence. */
/** Why a stored report is stale, each naming what the next scan refreshes. */
const STALE_BECAUSE: Record<SecurityStaleBecause, string> = {
  head_moved: "HEAD moved since its scan",
  files_changed: "A file it scanned changed",
  scoped_only: "Only a scoped re-scan of some paths is stored",
};

export const SCANNER_STATE_TEXT = {
  /** Installable here: the read-model names the exact command. */
  notInstalled: (install: string | null) =>
    install !== null
      ? `Not installed; run ${install}.`
      : "Not installed here; ingest its report from CI with logos security ingest <report>.",
  /** `command` is the scan, with `--consent` while the project's consent does
   *  not name the scanner yet. */
  neverRun: (command: string) => `Installed but never run; run ${command}.`,
  /** `command` refreshes it: a scan when it is installed here, else an ingest of
   *  a newer report (one produced in CI, or by a tool Logos does not run). */
  stale: (because: SecurityStaleBecause, command: string) => `${STALE_BECAUSE[because]}; run ${command}.`,
  disabled: "Disabled in .logos/config.toml; enable it there to scan with it.",
  fresh: (findings: number, suppressed: number) =>
    `${findings} ${plural(findings, "finding", "findings")}${suppressed > 0 ? `, ${suppressed} suppressed` : ""}`,
} as const;

/** A not-covered category's sentence, naming the command that covers it. */
export function notCoveredText(command: string): string {
  return `Not covered: no scanner that ran can detect this category; run ${command}.`;
}

/** The OWASP view's figure-row sentences. */
export const OWASP_TEXT = {
  edition: (year: string) => `OWASP Top 10:${year}`,
  covered: (findings: number) => `${findings} ${plural(findings, "finding", "findings")}`,
  unclassified: (n: number) =>
    `${n} ${plural(n, "finding has", "findings have")} no OWASP category: no CWE, or one the edition does not list.`,
} as const;

/** Why a finding has no OWASP category under the edition shown (FR-SC-04). */
export const UNCLASSIFIED_TEXT = {
  no_cwe: "Unclassified: the rule carries no CWE.",
  unmapped: "Unclassified: its CWE is not in this edition.",
} as const;

/** The gate band's sentences. */
export const SECURITY_GATE_TEXT = {
  pass: (checked: number) => `Passing: ${checked} security ${plural(checked, "rule", "rules")} checked, every required scan present.`,
  fail: (missing: number, violations: number) =>
    `Failing: ${missing} missing required ${plural(missing, "scan", "scans")}, ${violations} ${plural(violations, "finding", "findings")} a rule fails on.`,
} as const;

/** Why a finding has no enclosing symbol. */
const UNBOUND_BECAUSE: Record<NonNullable<SecurityFindingContext["enclosing"]["unbound_because"]>, string> = {
  no_location: "the report names no file or line",
  not_indexed: "its file is not in the index (a lockfile, say)",
  stale: "its file changed since the scan, so its line may point elsewhere",
  no_symbol: "its line lies inside no symbol",
};

/** Why the graph cannot rule out a route reaching a symbol (S-669). */
const UNRESOLVED_BECAUSE: Record<NonNullable<SecurityFindingContext["reachability"]["unresolved_because"]>, string> = {
  language: "the file's language binds no call across files",
  no_routes: "the graph holds no route to walk from",
  unproven_route: "a route names a handler the graph could not prove",
  dispatch: "a call can reach it through a supertype's method",
  value_reference: "it is passed as a value, and only calls are followed",
  not_in_graph: "it is not in the graph yet",
};

/** The detail's sentences over one finding's graph context. */
export const CONTEXT_TEXT = {
  unbound: (because: keyof typeof UNBOUND_BECAUSE) => `No enclosing symbol: ${UNBOUND_BECAUSE[because]}.`,
  reachable: (route: string, hops: number) => `Reachable from ${route}, ${hops} ${plural(hops, "call", "calls")} away.`,
  unreachable: "Unreachable: no route reaches it, and the graph sees every way in.",
  /** Unknown is never unreachable: the reason is always stated when there is one. */
  unknown: (because: keyof typeof UNRESOLVED_BECAUSE | null) =>
    because === null ? "Unknown: no route was found, and none could be ruled out." : `Unknown: ${UNRESOLVED_BECAUSE[because]}.`,
  blast: (upstream: number, downstream: number) =>
    `${upstream} ${plural(upstream, "symbol depends", "symbols depend")} on it; it depends on ${downstream}.`,
  confidence: (label: string) => `confidence: ${label}`,
  /** `impact` no longer resolves the enclosing symbol: its lists are unknown,
   *  not empty. */
  impactUnresolved:
    "The enclosing symbol is no longer in the graph, so what depends on it cannot be listed; the sizes above are from the last read.",
} as const;

// ── Run scan (S-673, FR-SC-15, FR-SC-07) ─────────────────────────────────────

export const scanJob: CopyEntry = {
  what: "Runs every installed, enabled scanner on this project and ingests what each reports, showing each scanner as it starts and finishes. While a default scanner is missing, Install & scan installs the missing ones first, then scans.",
  why: "The scan runs in the background, so the rest of the UI keeps answering, and a cancel stops every scanner process at once.",
};

/** The scan panel's sentences. */
export const SCAN_JOB_TEXT = {
  run: "Run scan",
  cancel: "Cancel scan",
  idle: "No scan has run from the UI since serve started.",
  running: (seconds: number) => `Scanning — ${seconds} s so far`,
  joined: "A scan was already running, so this click joined it.",
  completed: (findings: number, ran: number) =>
    `Scan finished: ${findings} ${plural(findings, "finding", "findings")} from ${ran} ${plural(ran, "scanner", "scanners")} that ran.`,
  cancelled: "Scan cancelled: every scanner process was stopped, and the previous results are kept.",
  failed: (error: string) => `The scan failed: ${error}`,
  requestFailed: (detail: string) => `The scan could not be started or cancelled: ${detail}`,
  scannerRunning: "running",
  /** A scanner that started and never settled because the scan ended first:
   *  cancelled, or failed. */
  scannerUnfinished: "did not finish",
} as const;

/** One scanner's settled outcome in the panel. */
export const SCAN_OUTCOME_TEXT = {
  ran: (findings: number) => `ran: ${findings} ${plural(findings, "finding", "findings")}`,
  not_installed: "not installed",
  disabled: "disabled",
  not_selected: "not selected",
  requires_build: "not run: every language present needs a build",
  not_applicable: (reason: string) => `not run: ${reason}`,
  failed: (exitCode: number | null, log: string | null) =>
    `failed${exitCode !== null ? ` (exit ${exitCode})` : ""}${log !== null ? `; its output is in ${log}` : ""}`,
} as const;

/** The first-use consent dialog (FR-SC-07): it names the scanners, and says
 *  where the answer is kept. */
export const CONSENT_TEXT = {
  title: "Let these scanners reach the network?",
  lead: "Each scanner below may download rules or vulnerability data when it runs:",
  where:
    "Your answer is recorded once for this project in your user-level Logos config, never in the repository, and later scans do not ask again. A scanner installed later is asked about on its own.",
  accept: "Allow and scan",
  decline: "Not now",
} as const;

// ── Install and Install & scan (S-747, FR-SC-24) ─────────────────────────────

/** A download size in decimal megabytes, as the CLI's consent states it. */
export function megabytes(bytes: number): string {
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

/** The install job's sentences in the scan panel, and the matrix's actions. */
export const INSTALL_JOB_TEXT = {
  installAndScan: "Install & scan",
  install: "Install",
  update: "Update",
  cancel: "Cancel install",
  running: (seconds: number) => `Installing — ${seconds} s so far`,
  scanning: (seconds: number) => `Installed; scanning — ${seconds} s so far`,
  joined: "An install was already running, so this click joined it.",
  completed: (installed: number, scan: { findings: number; ran: number } | null) =>
    `Installed ${installed} ${plural(installed, "tool", "tools")}` +
    (scan === null
      ? "."
      : `; scan finished: ${scan.findings} ${plural(scan.findings, "finding", "findings")} from ${scan.ran} ${plural(scan.ran, "scanner", "scanners")} that ran.`),
  /** `installed` names each `<tool> <version>` that landed before the cancel. */
  cancelled: (installed: readonly string[]) =>
    `Install cancelled: the download was stopped and no partial tool was left. ${
      installed.length > 0 ? `Installed before the cancel: ${installed.join(", ")}.` : "No tool was installed."
    }`,
  failed: (error: string) => `The install failed: ${error}`,
  requestFailed: (detail: string) => `The install could not be started or cancelled: ${detail}`,
} as const;

/** One tool's state in the install job, as the panel lists it. */
export const INSTALL_OUTCOME_TEXT = {
  installing: "installing",
  installed: "installed",
  already_present: "already installed",
  unavailable: (reason: string) => `not installed: ${reason}`,
  failed: (stage: string, reason: string) => `failed at ${stage}: ${reason}`,
  cancelled: "cancelled: nothing of it was left",
} as const;

/** The matrix's copy cell: which copy runs, and a newer pin when there is one. */
export const COPY_TEXT = {
  managed: "managed",
  path: "path",
  override: "override",
  updateAvailable: (version: string) => `update available: ${version}`,
} as const;

/** The install consent dialog (FR-SC-22, FR-SC-23, FR-SC-24): it extends the
 *  scan's consent dialog with what each install downloads. */
export const INSTALL_CONSENT_TEXT = {
  title: "Install these scanners?",
  lead: "Logos downloads each tool below from the source named, checks it against its pinned SHA-256 and installs it in your user-level tools directory, never in the repository:",
  tools: "Tools to install",
  /** `source` is the download host; `recorded` when this version's consent is already recorded. */
  line: (tool: string, version: string, source: string, size: number, recorded: boolean) =>
    `${tool} ${version} — ${source}, ${megabytes(size)}${recorded ? " (consent already recorded)" : ""}`,
  /** A support install a tool needs: `uv` and the managed Python, or the Java runtime. */
  support: (tool: string, version: string, source: string, size: number, forTools: readonly string[]) =>
    `${tool} ${version} — ${source}, ${megabytes(size)}, needed by ${forTools.join(", ")}`,
  optIn: "Opt-in scanners — installed only when ticked:",
  javaRuntime: (version: string, size: number) => `Brings the Java runtime it needs: java ${version}, ${megabytes(size)}.`,
  licence: (tool: string) => `I accept the ${tool} licence terms above`,
  where:
    "Your install consent is recorded once per tool version in your user-level Logos config and covers every project; a newer pinned version is asked about again.",
  accept: "Install",
  acceptAndScan: "Install and scan",
  decline: "Not now",
} as const;

// ── Triage (S-672, FR-SC-12) ─────────────────────────────────────────────────

/** The committed file a suppression is written to (mirrors `IGNORE_FILE`). */
export const IGNORE_FILE = ".logos/security-ignore.toml";

/** A suppressed or expired finding's label beside its state badge: the reason
 *  it was accepted for, and until when. */
export function suppressionNote(suppression: SecuritySuppression): string {
  const { reason, expires } = suppression;
  if (suppression.state === "expired") return `${reason} (expired after ${expires ?? "its date"})`;
  return expires === null ? reason : `${reason} (until ${expires})`;
}

/** The suppression dialog and the findings table's triage actions. */
export const SUPPRESS_TEXT = {
  title: "Suppress this finding?",
  lead: (rule: string, location: string) => `Accept the risk of ${rule} at ${location}. It stays listed, labelled with your reason.`,
  reason: "Reason",
  reasonHint: "Required: why the risk is accepted. Reviewers read it in the file.",
  reasonRequired: "A suppression needs a reason: say why the risk is accepted.",
  expires: "Expires (optional)",
  expiresHint: "The last day it suppresses; leave it empty to keep it until it is removed.",
  where: `Written to ${IGNORE_FILE}, which is committed with the repository, so the suppression is reviewed like code.`,
  confirm: "Suppress",
  cancel: "Cancel",
  suppress: "Suppress…",
  unsuppress: "Unsuppress",
  saveFailed: (detail: string) => `The suppression was not written: ${detail}`,
  removeFailed: (detail: string) => `The suppression was not removed: ${detail}`,
} as const;
