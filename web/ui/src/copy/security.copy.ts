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

import type {
  SecurityCommitAnchor,
  SecurityCoverageKind,
  SecurityEntryStatus,
  SecurityFindingContext,
  SecuritySelector,
  SecurityStaleBecause,
  SecurityEvidenceChange,
  SecuritySuppression,
} from "../api/types.ts";

import { plural, type CopyEntry } from "./types.ts";

export const scannerMatrix: CopyEntry = {
  what: "Each security scanner's state here, live while a scan or install runs: not installed, disabled, installed but never run, not applicable when the project has none of the files it reads, stale since HEAD moved, or fresh with its finding count — with its version and copy, why its last scan failed if it did, and whether a whole-project scan keeps it out of the directories the project ignores. Run scan, Install & scan and Cancel sit on the title row.",
  why: "A scanner that has not run on this commit has looked at nothing, so its silence is not a clean result.",
};

export const securityFindings: CopyEntry = {
  what: "Every stored finding with its severity, scanner, location and the symbol that encloses it, narrowed by the filters above the table; production code by default, a finding in test code tagged TEST.",
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

/** What a secret-class finding's message reads as (FR-SC-03, S-796): the
 *  reason the page shows no message, then where the finding is. The stored text
 *  (`redact.rs`) is what the CLI and MCP print; the page explains it instead. */
export const REDACTED_TEXT = {
  why: "Redacted: the scanner's message would contain the secret itself",
  at: (location: string) => `, at ${location}`,
  /** A finding bound to no file: its location is not withheld, it has none. */
  nowhere: ", and the finding has no location",
} as const;

/** The code scope control and the `TEST` tag (FR-SC-28, S-796). A test-code
 *  row says why it reads `test`: its enclosing symbol, or its path. */
export const SCOPE_TEXT = {
  label: "Code scope",
  options: { production: "Production", test: "Test code", all: "All" },
  tag: "TEST",
  /** The tag of a finding its scanner marks low-confidence (FR-SC-31, S-817):
   *  one word, since the Findings table holds no wider cell at 1024 px; the hover
   *  and the reader's text say "Needs review". */
  reviewTag: "REVIEW",
  /** Why it reads that: the scanner's own word, and what it changes. */
  reviewReason: (confidence: string) =>
    `Needs review: the scanner marks it ${confidence}-confidence. It is left out of the default list and counts, and still counts for the [security] rules, bless and the branch delta.`,
  reason: {
    /** A test-code finding the payload gave no reason for. */
    unstated: "In test code.",
    test_symbol: "In test code: the symbol that encloses it is a test.",
    test_path: "In test code: its path is a test, fixtures or testdata path.",
    /** A finding on a package its lockfile marks dev-only (FR-SC-30, S-810). */
    dev_dependency:
      "Dev-only dependency: its lockfile marks the package dev. It is left out of the default list and counts, and still counts for the [security] rules, bless and the branch delta.",
  },
  /** The tag of a finding on a dev-only dependency: one word, as the others. */
  devTag: "DEV",
  /** A table that lists nothing because the scope hid what the other filters kept. */
  hidden: (n: number, scopeWord: string) =>
    `No ${scopeWord} finding matches these filters; ${n} ${plural(n, "finding does", "findings do")} outside this scope (test code, a dev-only dependency, or marked needs review): change the code scope to see ${plural(n, "it", "them")}.`,
} as const;

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
  /** Nothing it reads is in the project (S-769): no scan is missing. */
  notApplicable: (reason: string) => `Nothing to scan: ${reason}.`,
  /** Its last scan failed: a red label that expands to the one-line cause from
   *  its log and where that log is. */
  lastFailed: "LAST SCAN FAILED",
  failureCause: (cause: string | undefined, log: string) => `${cause !== undefined ? `${cause} — ` : ""}the output is in ${log}.`,
  /** `merged`: its findings another scanner reports too, counted on that
   *  scanner's row once (FR-SC-29); unstated at 0. */
  fresh: (findings: number, suppressed: number, merged = 0) =>
    `${findings} ${plural(findings, "finding", "findings")}${suppressed > 0 ? `, ${suppressed} suppressed` : ""}${
      merged > 0 ? `, ${merged} counted under another scanner` : ""
    }`,
} as const;

/** A finding several scanners report (FR-SC-29, S-809): one advisory on one
 *  lockfile, package and version, named by every reporter. */
export const REPORTERS_TEXT = {
  label: "Reported by",
  /** One reporter: its scanner and version, and what it reported it as. */
  reporter: (scanner: string, version: string, rule: string, severity: string) =>
    `${scanner} ${version}: ${rule} (${severity})`,
  note: "One advisory on one package, counted once at the highest rating any scanner gave. Suppressing it by any reporter's fingerprint suppresses it.",
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

/** The sentences of a finding count — the Security page's and the workspace
 *  roll-up's, one spelling (S-760): a finding counts when no current entry
 *  suppresses it and its own file is unchanged since the scan. */
export const FINDING_COUNT_TEXT = {
  /** The counted findings in words, with what they are. */
  totals: (critical: number, high: number, medium: number, low: number, info: number, unrated: number) =>
    `${critical} critical · ${high} high · ${medium} medium · ${low} low · ${info} info · ${unrated} unrated — fresh, unsuppressed findings only`,
  /** What the count leaves out, and why. */
  excluded: (notFresh: number, suppressed: number) =>
    `Left out: ${notFresh} ${plural(notFresh, "finding", "findings")} in files changed since the scan, and ${suppressed} suppressed.`,
  /** The findings in test code, which no count includes (FR-SC-28): stated
   *  beside the count, `security status`'s "N more in test code". */
  testCode: (n: number) => `· ${n} in test code`,
  /** The findings their scanner marks low-confidence, which no count includes
   *  (FR-SC-31, S-817): `security status`'s "N more need review". */
  needsReview: (n: number) => `· ${n} more ${plural(n, "needs", "need")} review`,
  /** The findings on a dev-only dependency, which no count includes (FR-SC-30,
   *  S-810): `security status`'s "N more in dev-only dependencies". */
  devOnly: (n: number) => `· ${n} more in dev-only dependencies`,
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
  historical: "the secret is only in its commit's history — its file is gone, or its line holds other code now",
};

/** A secret found only in history (S-767): the label beside its location and
 *  the tooltip on its short commit. The label is the `historical` binding token
 *  and the short commit is the payload's own, so the page reads both exactly
 *  as the CLI and MCP print them. */
export const ANCHOR_TEXT = {
  historical: "historical",
  commit: (anchor: SecurityCommitAnchor) => `Found in commit ${anchor.commit}, at ${anchor.path}`,
} as const;

/** Why the graph cannot rule out a route reaching a symbol (S-669). */
const UNRESOLVED_BECAUSE: Record<NonNullable<SecurityFindingContext["reachability"]["unresolved_because"]>, string> = {
  language: "the file's language binds no call across files",
  no_routes: "the graph holds no route to walk from",
  unproven_route: "a route names a handler the graph could not prove",
  dispatch: "a call can reach it through a supertype's method",
  value_reference: "it is passed as a value, and only calls are followed",
  not_in_graph: "it is not in the graph yet",
};

/** The Findings table's own words (frontend-design §4.23): the Reachable column
 *  reads yes / no / unknown — `unknown` is never `no` (NFR-CC-04) — and the full
 *  sentence behind each is the cell's tooltip. */
export const FINDINGS_TABLE_TEXT = {
  /** The Reachable header's tooltip, and the filter's name (CR-245). */
  reachableTip: "an HTTP route handler can call this code",
  reachable: { reachable: "yes", unreachable: "no", unknown: "unknown" },
  /** The detail's line for a suppressed or expired finding's state, reason and expiry. */
  suppression: "Suppression",
} as const;

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

/** The scan controls' sentences, and the summary line's (CR-245). */
export const SCAN_JOB_TEXT = {
  run: "Run scan",
  cancel: "Cancel scan",
  idle: "No scan has run from the UI since serve started.",
  /** The summary line while it runs: the scanners finished so far, and the ones
   *  running now. No time is counted: a live region repeats what changes. */
  running: (done: number, running: readonly string[]) =>
    `Scan running — ${done} ${plural(done, "scanner", "scanners")} done${running.length > 0 ? `; ${running.join(", ")} running` : ""}`,
  joined: "A scan was already running, so this click joined it.",
  completed: (findings: number, ran: number) =>
    `Scan finished: ${findings} ${plural(findings, "finding", "findings")} from ${ran} ${plural(ran, "scanner", "scanners")} that ran.`,
  /** `finished` names the scanners that had finished before the cancel. */
  cancelled: (finished: readonly string[]) =>
    `Scan cancelled: every scanner process was stopped, and the previous results are kept.${
      finished.length > 0 ? ` Finished before the cancel: ${finished.join(", ")}.` : ""
    }`,
  failed: (error: string) => `The scan failed: ${error}`,
  requestFailed: (detail: string) => `The scan could not be started or cancelled: ${detail}`,
} as const;

/** A row's live state while a job runs (it replaces the stored state's badge). */
export const LIVE_STATE_TEXT = {
  running: "running",
  installing: "installing",
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

/** The install job's sentences in the scan summary line, and the matrix's actions. */
export const INSTALL_JOB_TEXT = {
  installAndScan: "Install & scan",
  install: "Install",
  update: "Update",
  cancel: "Cancel install",
  /** The summary line while the install runs, then while the scan after it does. */
  running: (done: number) => `Installing — ${done} ${plural(done, "tool", "tools")} done`,
  scanning: (done: number) => `Installed; scanning — ${done} ${plural(done, "scanner", "scanners")} done`,
  joined: "An install was already running, so this click joined it.",
  completed: (installed: number, scan: { findings: number; ran: number } | null) =>
    `Installed ${installed} ${plural(installed, "tool", "tools")}` +
    (scan === null
      ? "."
      : `; scan finished: ${scan.findings} ${plural(scan.findings, "finding", "findings")} from ${scan.ran} ${plural(scan.ran, "scanner", "scanners")} that ran.`),
  /** What an ended install left out, each tool with why, after its outcome. */
  notInstalled: (left: readonly string[]) => ` Not installed: ${left.join("; ")}.`,
  /** `installed` names each `<tool> <version>` that landed before the cancel. */
  cancelled: (installed: readonly string[]) =>
    `Install cancelled: the download was stopped and no partial tool was left. ${
      installed.length > 0 ? `Installed before the cancel: ${installed.join(", ")}.` : "No tool was installed."
    }`,
  failed: (error: string) => `The install failed: ${error}`,
  requestFailed: (detail: string) => `The install could not be started or cancelled: ${detail}`,
} as const;

/** A tool the install left out, with why — in the summary line once the job ended. */
export const INSTALL_OUTCOME_TEXT = {
  unavailable: (tool: string, reason: string) => `${tool} (${reason})`,
  failed: (tool: string, stage: string, reason: string) => `${tool} (failed at ${stage}: ${reason})`,
} as const;

/** One marker of the Ignored dirs column and what it means in plain words. */
export interface IgnoredDirsMarker {
  readonly marker: string;
  readonly means: string;
}

/**
 * The Ignored dirs column's markers (FR-SC-26, CR-245): a short marker per way a
 * whole-project scan treats the directories the project ignores, each with one
 * legend line. The page picks a marker from the row's `coverage_kind`, never from
 * the note's text; the full note is the marker's tooltip and accessible name.
 * `none` marks a row with no whole-project run to state (not installed, disabled).
 */
export const IGNORED_DIRS_TEXT: Readonly<Record<"none" | "skipped" | "scanned" | SecurityCoverageKind, IgnoredDirsMarker>> = {
  none: {
    marker: "—",
    means:
      "No whole-project scan to describe: the scanner is not installed, disabled, has nothing to scan here or was refused by the repository's config, or this is a report ingested from another tool.",
  },
  skipped: {
    marker: "skipped",
    means: "This scanner kept out of the directories your .gitignore excludes (vendor/, target/…), or your project ignores none.",
  },
  unproven: {
    marker: "not proven †",
    means:
      "This scanner may have looked inside directories your .gitignore excludes (vendor/, target/…), so findings there can appear; Logos could not prove it skipped them.",
  },
  history: {
    marker: "reads history ‡",
    means:
      "This scanner reads the committed history, which holds any file once committed inside a directory you now ignore, so findings there can appear.",
  },
  whole_project: {
    marker: "whole project §",
    means:
      "This scanner was given the whole project, because the list of files outside your ignored directories could not be passed to it, so findings in ignored directories can appear.",
  },
  partial: {
    marker: "partly skipped ¶",
    means:
      "This scanner was told to skip your ignored directories except some it could not take (a name it could misread, or a list too long), so findings in those can appear.",
  },
  scanned: {
    marker: "scanned",
    means: "This scanner read directories your .gitignore excludes; the server gave no further detail.",
  },
};

/** The legend's accessible name. */
export const IGNORED_DIRS_LEGEND = "Ignored dirs markers";

/** The Findings cell's words where a scanner has no count to show: `never run` and
 *  `0 findings` are different words and never share a cell (NFR-CC-04). */
export const FINDINGS_CELL_TEXT = {
  neverRun: "never run",
  none: "—",
} as const;

/** Dependency-Check's row (FR-SC-27, S-794): whether it has an NVD API key,
 *  never what the key is. `hint` is the not-set line's tooltip: where a key
 *  comes from. */
export const NVD_KEY_TEXT = {
  set: "NVD key: set",
  not_set: "NVD key: not set",
  hint: "Dependency-Check 13 needs an NVD API key: export NVD_API_KEY before starting logos serve, or set your own extra_args in the user config.",
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

/** A suppressed, out-of-date or expired finding's label beside its state badge: the reason
 *  it was accepted for, and until when — and, for a path entry (S-770) or a
 *  rule-wide one (S-814), which entry covers it, since removing that entry
 *  unsuppresses everything it covers. */
export function suppressionNote(suppression: SecuritySuppression): string {
  const { reason, expires } = suppression;
  const covers = coverNote(suppression);
  if (suppression.state === "expired") return `${reason} (expired after ${expires ?? "its date"})${covers}`;
  // An out-of-date verdict keeps the reason it was given for; what changed is the Evidence row's (`evidenceNote`).
  return (expires === null ? reason : `${reason} (until ${expires})`) + covers;
}

/** The note's tail naming the entry that covers a finding: nothing for a fingerprint, the glob
 *  for a path, and the rule-wide entry — which an expired one no longer "suppresses by". */
function coverNote({ state, by }: SecuritySuppression): string {
  if ("fingerprint" in by) return "";
  if ("path" in by) return ` — covers ${selectorText(by)}`;
  return ` — ${state === "suppressed" ? "suppressed by a rule-wide entry" : "rule-wide entry"} (${selectorText(by)})`;
}

/** The state badge's word: `out_of_date` reads "out of date". */
export function suppressionState(suppression: SecuritySuppression): string {
  return suppression.state === "out_of_date" ? "out of date" : suppression.state;
}

/** One changed evidence symbol, in the sentence "`DefaultCookieConfig` changed". */
export function changeText(change: SecurityEvidenceChange): string {
  switch (change.change) {
    case "changed":
      return `\`${change.name}\` changed`;
    case "gone":
      return `\`${change.name}\` no longer exists`;
    case "unverifiable":
      return `\`${change.name}\` could not be checked${change.detail ? `: ${change.detail}` : ""}`;
  }
}

/** The changed evidence symbols as one sentence: "`A` changed; `B` no longer exists". */
export function changesText(changes: readonly SecurityEvidenceChange[]): string {
  return changes.map(changeText).join("; ");
}

/** A suppression's evidence line: what its reason rests on, "no evidence
 *  recorded" when it records none (FR-SC-32), or — for an out-of-date verdict —
 *  the symbols that changed. `null` for an expired entry, which is not checked. */
export function evidenceNote(suppression: SecuritySuppression): string | null {
  switch (suppression.state) {
    case "suppressed": {
      const evidence = suppression.evidence ?? [];
      return evidence.length === 0 ? EVIDENCE_TEXT.none : EVIDENCE_TEXT.resting(evidence.map((e) => e.name));
    }
    case "out_of_date":
      return changesText(suppression.changed);
    case "expired":
      return null;
  }
}

/** The evidence a suppression records and the verdict that re-opens when it
 *  changes (FR-SC-32, S-818). */
export const EVIDENCE_TEXT = {
  outOfDate: "Verdict out of date: re-review",
  none: "No evidence recorded",
  resting: (names: readonly string[]) => `Reason rests on ${names.map((n) => `\`${n}\``).join(", ")}, unchanged`,
  label: "Evidence",
  reconfirm: "Re-confirm",
  reconfirmTip: "Re-record the evidence as it is now, keeping the reason: the finding is suppressed again.",
  reconfirmFailed: (detail: string) => `The verdict was not re-confirmed: ${detail}`,
  recordEvidence: "Record the enclosing symbol as evidence",
  recordEvidenceHint: (name: string) =>
    `The suppression then holds only while \`${name}\` is unchanged: if its source changes the finding comes back as "verdict out of date".`,
  entryOutOfDate: (changes: readonly SecurityEvidenceChange[]) => `${EVIDENCE_TEXT.outOfDate} — ${changesText(changes)}`,
} as const;

/** How an entry's selector reads: a fingerprint's first twelve digits, the
 *  path glob with its narrowing, or a rule of a scanner in every file. */
export function selectorText(selector: SecuritySelector): string {
  if ("fingerprint" in selector) return `fingerprint ${selector.fingerprint.slice(0, 12)}…`;
  if (!("path" in selector)) return `rule ${selector.rule} of ${selector.scanner}, in every file`;
  const narrowing = [selector.rule && `rule ${selector.rule}`, selector.scanner && `scanner ${selector.scanner}`].filter(Boolean);
  return [`everything under ${selector.path}`, ...narrowing].join(", ");
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
  // "Everything under this path" (S-770).
  scope: "What to suppress",
  scopeFinding: "Only this finding",
  scopePath: "Everything under this path",
  path: "Path",
  pathHint: "A glob relative to the project root; a directory covers everything under it. The whole project (*, **, .) is refused.",
  onlyRule: (rule: string) => `Only rule ${rule}`,
  onlyScanner: (scanner: string) => `Only ${scanner} findings`,
  pathRequired: "Name the path the suppression covers.",
} as const;

export const suppressionEntries: CopyEntry = {
  what: `Every entry of ${IGNORE_FILE}: what it suppresses, why, until when, and how many stored findings it suppresses now. An entry that names no finding is orphaned.`,
  why: "An orphaned entry is a fixed finding or a typo, and a path entry can cover more than one finding: the count shows how much risk each entry accepts.",
};

/** The Suppressions widget (S-770). */
export const SUPPRESSIONS_TEXT = {
  title: "Suppressions",
  figure: (entries: number, findings: number) =>
    `${entries} ${plural(entries, "entry", "entries")} suppressing ${findings} ${plural(findings, "finding", "findings")}`,
  caption: `The entries of ${IGNORE_FILE}`,
  count: (n: number) => `${n} suppressed`,
  /** The one state an entry is in, orphaned first: it is the one to act on. */
  state: (e: SecurityEntryStatus) =>
    e.orphaned ? "orphaned" : e.expired ? "expired" : e.out_of_date.length > 0 ? "out of date" : "active",
  shouldName: (fingerprint: string) => `should name fingerprint ${fingerprint.slice(0, 12)}…`,
  remove: (selector: SecuritySelector) => `Remove the entry for ${selectorText(selector)}`,
  reconfirm: (selector: SecuritySelector) => `Re-confirm the entry for ${selectorText(selector)}`,
} as const;
