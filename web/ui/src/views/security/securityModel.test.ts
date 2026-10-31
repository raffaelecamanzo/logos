// The Security page's pure model (S-670, FR-SC-09): filters, edition lookup,
// redaction and the command each absent state names — over the real payload
// `web/tests/api_v1.rs` produced (`fixtures/security.real.json`).
import { describe, expect, it } from "vitest";

import reference from "../../../../../docs/howto/commands.md?raw";
import type {
  InstallJob,
  ScanJob,
  SecurityCategoryCoverage,
  SecurityFinding,
  SecurityInstallStep,
  SecurityModel,
  SecurityScannerStatus,
} from "../../api/types.ts";
import {
  CONTEXT_TEXT,
  FINDINGS_TABLE_TEXT,
  INSTALL_JOB_TEXT,
  INSTALL_OUTCOME_TEXT,
  REDACTED_TEXT,
  SCAN_JOB_TEXT,
  SCOPE_TEXT,
  evidenceNote,
  selectorText,
  suppressionNote,
  suppressionState,
} from "../../copy/security.copy.ts";
import { commandsIn, documentedCommands, isDocumented } from "../../test/commandCheck.ts";
import installReal from "./fixtures/install-job.real.json";
import scanReal from "./fixtures/scan-job.real.json";
import real from "./fixtures/security.real.json";
import {
  NO_FILTERS,
  categoryCount,
  editionOf,
  findingLocation,
  findingMessage,
  findingScanners,
  hiddenByScope,
  ignoredDirsKey,
  inScope,
  jobProgress,
  keeps,
  liveStates,
  markersShown,
  devDependencyText,
  needsReviewText,
  normalisePath,
  notCoveredCommand,
  pathFromSearch,
  reachabilityText,
  reachableWord,
  scannerDrivers,
  scannerStateText,
  scopeFromSearch,
  scopeReasonText,
  selectorKey,
  severityTone,
  suggestedPath,
  summaryText,
  symbolKindWord,
  unclassifiedCount,
  urlWithScope,
} from "./securityModel.ts";

const MODEL = real as unknown as SecurityModel;
const PAGE = MODEL.security;
const [SQL, SECRET] = ["rust.lang.security.formatted-sql", "generic.secrets.hardcoded-key"].map(
  (rule) => PAGE.findings.findings.find((f) => f.rule_id === rule) as SecurityFinding,
);
const documented = documentedCommands(reference);

/** Every `logos …` command in `text` is one docs/howto/commands.md documents. */
function expectDocumentedCommands(text: string) {
  const named = commandsIn("/x.ts", `export const x = ${JSON.stringify(text)};`);
  expect(named.length, `names a command: ${text}`).toBeGreaterThan(0);
  expect(named.filter((c) => !isDocumented(c, documented)).map((c) => c.words.join(" "))).toEqual([]);
}

function row(over: Partial<SecurityScannerStatus> & Pick<SecurityScannerStatus, "state">): SecurityScannerStatus {
  return { scanner: "trivy", driver: "Trivy", installed: null, install: null, last_failure: null, ...over } as SecurityScannerStatus;
}

describe("the real payload is the one the tests are pinned to", () => {
  it("carries both findings, a never-run scanner and a not-covered category", () => {
    expect([SQL, SECRET].every(Boolean)).toBe(true);
    expect(PAGE.findings.never_run).toEqual(["bandit"]);
    expect(editionOf(PAGE, "2025").categories.some((c) => c.state.state === "not_covered")).toBe(true);
  });
});

describe("edition", () => {
  it("classifies the same finding under each edition from the payload alone", () => {
    expect(editionOf(PAGE, "2025").classify(SQL).categories).toEqual(["A05:2025"]);
    expect(editionOf(PAGE, "2021").classify(SQL).categories).toEqual(["A03:2021"]);
    expect(editionOf(PAGE, "2021").categories.map((c) => c.id)[0]).toBe("A01:2021");
  });

  it("keys a finding by scanner AND fingerprint: the same fingerprint from another scanner is not it", () => {
    const other = { ...SQL, scanner_name: "Bandit" };
    expect(editionOf(PAGE, "2025").classify(other)).toEqual({ categories: [], unclassified: null });
  });
});

describe("redaction (FR-SC-03, S-796)", () => {
  const WHY = "Redacted: the scanner's message would contain the secret itself";

  it("explains a secret-class message in words and gives its location, whatever text it holds", () => {
    expect(REDACTED_TEXT.why).toBe(WHY);
    const at = findingLocation(SECRET);
    expect(findingMessage(SECRET)).toBe(`${WHY}, at ${at}`);
    expect(findingMessage({ ...SECRET, message: "password=hunter2" })).toBe(`${WHY}, at ${at}`);
    expect(findingMessage(SQL)).toBe("Formatted SQL query.");
  });

  it("states a redacted finding with no location as having none, not as being somewhere", () => {
    const nowhere = { ...SECRET, path: null, report_path: null };
    expect(findingMessage(nowhere)).toBe(`${WHY}, and the finding has no location`);
  });

  it("reads the report's path for a redacted finding bound to no indexed file", () => {
    expect(findingMessage({ ...SECRET, path: null, report_path: "Cargo.lock", start_line: 7 })).toBe(`${WHY}, at Cargo.lock:7`);
  });
});

describe("code scope (FR-SC-28, S-796)", () => {
  const test = { ...SQL, code_scope: "test" as const, code_scope_reason: "test_symbol" as const };
  const fixture = { ...SQL, code_scope: "test" as const, code_scope_reason: "test_path" as const, fingerprint: "f2" };
  const none = new Map<string, string>();
  const classify = (f: SecurityFinding) => editionOf(PAGE, "2025").classify(f);

  it("lists production unless the scope says otherwise, and the control's three values are the CLI's", () => {
    expect(NO_FILTERS.scope).toBe("production");
    expect(inScope(SQL, "production")).toBe(true);
    expect(inScope(test, "production")).toBe(false);
    expect(inScope(test, "test")).toBe(true);
    expect(inScope(SQL, "test")).toBe(false);
    expect([SQL, test].map((f) => inScope(f, "all"))).toEqual([true, true]);
  });

  it("intersects the scope with every other filter", () => {
    expect(keeps(test, NO_FILTERS, classify(test), none)).toBe(false);
    expect(keeps(test, { ...NO_FILTERS, scope: "all" }, classify(test), none)).toBe(true);
    expect(keeps(test, { ...NO_FILTERS, scope: "test", severity: "critical" }, classify(test), none)).toBe(false);
    expect(keeps(test, { ...NO_FILTERS, scope: "test", severity: "high" }, classify(test), none)).toBe(true);
    expect(keeps(SQL, { ...NO_FILTERS, scope: "test" }, classify(SQL), none)).toBe(false);
  });

  it("counts what the scope alone hides, of what the other filters keep", () => {
    const all = [SQL, test, fixture, SECRET];
    expect(hiddenByScope(all, NO_FILTERS, classify, none)).toBe(2);
    expect(hiddenByScope(all, { ...NO_FILTERS, scope: "all" }, classify, none)).toBe(0);
    expect(hiddenByScope(all, { ...NO_FILTERS, scope: "test" }, classify, none)).toBe(2);
    // A finding another filter already drops is not the scope's doing.
    expect(hiddenByScope(all, { ...NO_FILTERS, severity: "critical" }, classify, none)).toBe(0);
  });

  it("says why a finding reads test, in words, and nothing for production", () => {
    expect(scopeReasonText(test)).toBe(SCOPE_TEXT.reason.test_symbol);
    expect(scopeReasonText(fixture)).toBe(SCOPE_TEXT.reason.test_path);
    expect(scopeReasonText({ ...test, code_scope_reason: null })).toBe(SCOPE_TEXT.reason.unstated);
    expect(scopeReasonText(SQL)).toBeNull();
  });

  it("reads the scope from the query: a known value, case-insensitive, else production", () => {
    expect(scopeFromSearch("")).toBe("production");
    expect(scopeFromSearch("?scope=test")).toBe("test");
    expect(scopeFromSearch("?path=src&scope=ALL")).toBe("all");
    expect(scopeFromSearch("?scope=production")).toBe("production");
    expect(scopeFromSearch("?scope=tests")).toBe("production");
    expect(scopeFromSearch("?rescope=test")).toBe("production");
  });

  it("writes the scope into a URL, production naming none, every other pair kept byte-for-byte", () => {
    expect(urlWithScope("/security", "test")).toBe("/security?scope=test");
    expect(urlWithScope("/security?path=a%20b&repo=web", "all")).toBe("/security?path=a%20b&repo=web&scope=all");
    expect(urlWithScope("/security?scope=all&repo=web#x", "test")).toBe("/security?repo=web&scope=test#x");
    expect(urlWithScope("/security?scope=test", "production")).toBe("/security");
    expect(urlWithScope("/security?path=a&scope=test", "production")).toBe("/security?path=a");
    // A key that merely ends or starts with `scope` is somebody else's.
    expect(urlWithScope("/security?rescope=1", "test")).toBe("/security?rescope=1&scope=test");
    expect(urlWithScope("/security?scopes=1&scope_x=2", "all")).toBe("/security?scopes=1&scope_x=2&scope=all");
    // A percent-encoded `scope` is the same key `scopeFromSearch` reads, and is replaced.
    expect(urlWithScope("/security?%73cope=test", "all")).toBe("/security?scope=all");
    expect(scopeFromSearch("?scope=all")).toBe("all");
  });

  it("takes the last scope of a repeated key, as the member scope does", () => {
    expect(scopeFromSearch("?scope=test&scope=all")).toBe("all");
    expect(scopeFromSearch("?scope=all&scope=bogus")).toBe("production");
    expect(urlWithScope("/security?scope=test&scope=all", "test")).toBe("/security?scope=test");
  });

  it("counts a finding under no category once, and only in the scope it is in", () => {
    const unclassified = { ...SQL, fingerprint: "none", code_scope: "test" as const, code_scope_reason: "test_path" as const };
    const only = (f: SecurityFinding) =>
      f === unclassified ? { categories: [], unclassified: "no_cwe" as const } : { categories: classify(f).categories, unclassified: null };
    const findings = [SQL, unclassified];
    expect(unclassifiedCount(findings, only, "production")).toBe(0);
    expect(unclassifiedCount(findings, only, "test")).toBe(1);
    expect(unclassifiedCount(findings, only, "all")).toBe(1);
  });

  it("words what the scope hid in either direction, and for one or many", () => {
    expect(SCOPE_TEXT.hidden(1, "production")).toBe("No production finding matches these filters; 1 finding does outside this scope (test code, a dev-only dependency, or marked needs review): change the code scope to see it.");
    expect(SCOPE_TEXT.hidden(2, "test-code")).toBe("No test-code finding matches these filters; 2 findings do outside this scope (test code, a dev-only dependency, or marked needs review): change the code scope to see them.");
  });

  it("states each OWASP category's count from the server's two figures, equal to a recount over the listed findings", () => {
    // The edition's classification is keyed by scanner and fingerprint, so the planted twin shares SQL's.
    const sqlInTest = { ...SQL, fingerprint: "twin-in-test", code_scope: "test" as const, code_scope_reason: "test_path" as const };
    const secondInProduction = { ...SQL, fingerprint: "twin-in-production" };
    const twins = [sqlInTest, secondInProduction];
    const findings = [...PAGE.findings.findings, ...twins];
    const byTwin = (f: SecurityFinding) => classify(twins.includes(f) ? SQL : f);
    const planted = classify(SQL).categories;
    expect(planted.length).toBeGreaterThan(0);
    // What `security findings` states with the two planted twins stored: `findings` counts every scope,
    // `production_findings` the production part (the second production twin in both).
    const stated: SecurityCategoryCoverage[] = editionOf(PAGE, "2025").categories.map((c) =>
      c.state.state === "covered" && planted.includes(c.id)
        ? { ...c, state: { state: "covered", findings: c.state.findings + 2 }, production_findings: (c.production_findings ?? 0) + 1 }
        : c,
    );
    for (const id of planted) {
      const category = stated.find((c) => c.id === id) as SecurityCategoryCoverage;
      const recount = (scope: "production" | "test" | "all") =>
        findings.filter((f) => inScope(f, scope) && byTwin(f).categories.includes(id)).length;
      for (const scope of ["production", "test", "all"] as const) {
        expect(categoryCount(category, scope)).toBe(recount(scope));
      }
      // Both scopes are non-empty, so the figures differ and a count of one scope cannot pass for the other.
      expect([categoryCount(category, "production"), categoryCount(category, "test"), categoryCount(category, "all")]).toEqual([
        recount("all") - 1,
        1,
        recount("all"),
      ]);
    }
    expect(categoryCount({ ...stated[0], state: { state: "not_covered" } }, "production")).toBeNull();
  });
});

describe("one advisory several scanners report (FR-SC-29, S-809)", () => {
  const reporter = (scanner_name: string, fingerprint: string) => ({
    scanner_name,
    scanner_version: "1",
    fingerprint,
    rule_id: "CVE-2026-1",
    severity: "high" as const,
  });
  const merged: SecurityFinding = {
    ...SQL,
    scanner_name: "Trivy",
    fingerprint: "t",
    reporters: [reporter("Trivy", "t"), reporter("osv-scanner", "o"), reporter("Trivy", "t2")],
  };
  const classification = editionOf(PAGE, "2025").classify(SQL);
  const drivers = new Map([["trivy", "Trivy"], ["osv-scanner", "osv-scanner"]]);
  const listed = (scanner: string, f: SecurityFinding) => keeps(f, { ...NO_FILTERS, scanner }, classification, drivers);

  it("names every scanner once, its own first, and a finding one scanner reported by that scanner alone", () => {
    expect(findingScanners(merged)).toEqual(["Trivy", "osv-scanner"]);
    expect(findingScanners(SQL)).toEqual([SQL.scanner_name]);
  });

  it("lists a merged finding under each of its reporters, by driver or registry name, and under no other scanner", () => {
    expect(listed("osv-scanner", merged)).toBe(true);
    expect(listed("trivy", merged)).toBe(true);
    expect(listed("semgrep", merged)).toBe(false);
    expect(listed("osv-scanner", SQL)).toBe(false);
  });

  it("states a row's findings counted under another scanner, and nothing when there are none", () => {
    const fresh = (merged?: number) =>
      scannerStateText(row({ state: "fresh", snapshot_head: "h", version: "1", findings: 2, suppressed: 1, merged }));
    expect(fresh(1)).toBe("2 findings, 1 suppressed, 1 counted under another scanner");
    expect(fresh(0)).toBe("2 findings, 1 suppressed");
    expect(fresh(undefined)).toBe("2 findings, 1 suppressed");
  });
});

describe("needs review (FR-SC-31, S-817)", () => {
  const review = { ...SQL, fingerprint: "low", confidence: "low" as const, needs_review: true };
  const medium = { ...SQL, fingerprint: "medium", confidence: "medium" as const, needs_review: false };
  const reviewInTest = { ...review, fingerprint: "low-in-test", code_scope: "test" as const, code_scope_reason: "test_path" as const };
  const none = new Map<string, string>();
  const classify = (f: SecurityFinding) => editionOf(PAGE, "2025").classify(f);

  it("leaves a low-confidence finding out of the default scope and lists it under test or all by its code", () => {
    expect(inScope(review, "production")).toBe(false);
    expect(inScope(medium, "production")).toBe(true);
    expect(inScope(SQL, "production")).toBe(true);
    expect(inScope(review, "all")).toBe(true);
    // Its scope is its code: in production code it is not in `test`.
    expect(inScope(review, "test")).toBe(false);
    expect(inScope(reviewInTest, "test")).toBe(true);
    expect(inScope(reviewInTest, "production")).toBe(false);
  });

  it("counts what the default scope hides, of what the other filters keep", () => {
    const all = [SQL, medium, review, reviewInTest];
    expect(hiddenByScope(all, NO_FILTERS, classify, none)).toBe(2);
    expect(hiddenByScope(all, { ...NO_FILTERS, scope: "all" }, classify, none)).toBe(0);
    expect(hiddenByScope(all, { ...NO_FILTERS, scope: "test" }, classify, none)).toBe(3);
    expect(keeps(review, NO_FILTERS, classify(review), none)).toBe(false);
    expect(keeps(review, { ...NO_FILTERS, scope: "all" }, classify(review), none)).toBe(true);
  });

  it("words why a finding needs review, naming the scanner's confidence, and nothing for another", () => {
    expect(needsReviewText(review)).toBe(SCOPE_TEXT.reviewReason("low"));
    expect(needsReviewText(review)).toContain("low-confidence");
    expect(needsReviewText(review)).toContain("[security] rules");
    expect(needsReviewText(medium)).toBeNull();
    expect(needsReviewText({ ...SQL, confidence: "unstated" as const, needs_review: false })).toBeNull();
  });

  it("states a category's count so production, needs review and test code partition its findings", () => {
    const covered = editionOf(PAGE, "2025").categories.find((c) => c.state.state === "covered" && c.production_findings === 1) as SecurityCategoryCoverage;
    // Three findings stored under it beyond the page's one: one in production, one needing review, one in test code.
    const stated: SecurityCategoryCoverage = {
      ...covered,
      state: { state: "covered", findings: 4 },
      production_findings: 2,
      needs_review_findings: 1,
    };
    expect([categoryCount(stated, "production"), categoryCount(stated, "test"), categoryCount(stated, "all")]).toEqual([2, 1, 4]);
    // A payload from before the count existed reads as none needing review.
    expect(categoryCount({ ...stated, needs_review_findings: undefined }, "test")).toBe(2);
  });
});

describe("dev-only dependencies (FR-SC-30, S-810)", () => {
  const dev = { ...SQL, fingerprint: "dev", code_scope: "dev" as const, code_scope_reason: "dev_dependency" as const };
  const none = new Map<string, string>();
  const classify = (f: SecurityFinding) => editionOf(PAGE, "2025").classify(f);

  it("lists a dev finding under All alone: neither Production nor Test code holds it", () => {
    expect([inScope(dev, "production"), inScope(dev, "test"), inScope(dev, "all")]).toEqual([false, false, true]);
    expect(inScope(SQL, "production")).toBe(true);
  });

  it("counts what the other scopes hide, of what the other filters keep", () => {
    const all = [SQL, dev];
    expect(hiddenByScope(all, NO_FILTERS, classify, none)).toBe(1);
    expect(hiddenByScope(all, { ...NO_FILTERS, scope: "test" }, classify, none)).toBe(2);
    expect(hiddenByScope(all, { ...NO_FILTERS, scope: "all" }, classify, none)).toBe(0);
    expect(keeps(dev, NO_FILTERS, classify(dev), none)).toBe(false);
    expect(keeps(dev, { ...NO_FILTERS, scope: "all" }, classify(dev), none)).toBe(true);
  });

  it("words why a finding reads dev, and nothing for another scope", () => {
    expect(devDependencyText(dev)).toBe(SCOPE_TEXT.reason.dev_dependency);
    expect(devDependencyText(dev)).toContain("[security] rules");
    expect(devDependencyText(SQL)).toBeNull();
    expect(scopeReasonText(dev)).toBeNull();
  });

  it("states a category's count so production, dev, needs review and test code partition its findings", () => {
    const covered = editionOf(PAGE, "2025").categories.find((c) => c.state.state === "covered" && c.production_findings === 1) as SecurityCategoryCoverage;
    const stated: SecurityCategoryCoverage = {
      ...covered,
      state: { state: "covered", findings: 6 },
      production_findings: 2,
      needs_review_findings: 1,
      dev_findings: 2,
    };
    expect([categoryCount(stated, "production"), categoryCount(stated, "test"), categoryCount(stated, "all")]).toEqual([2, 1, 6]);
    // A payload from before the count existed reads as none on a dev-only dependency.
    expect(categoryCount({ ...stated, dev_findings: undefined }, "test")).toBe(3);
  });
});

describe("a finding's location", () => {
  it("is its indexed path and line, else the report's path, else 'no location'", () => {
    expect(findingLocation(SQL)).toBe("src/lib.rs:3");
    const lockfile = { ...SQL, path: null, report_path: "Cargo.lock", start_line: 7 };
    expect(findingLocation(lockfile)).toBe("Cargo.lock:7");
    expect(findingLocation({ ...lockfile, start_line: null })).toBe("Cargo.lock");
    expect(findingLocation({ ...lockfile, report_path: null })).toBe("no location");
  });
});

describe("the Findings table's Symbol and Reachable words (S-793)", () => {
  it("reads a symbol kind in words, from its wire token", () => {
    expect(symbolKindWord("function")).toBe("function");
    expect(symbolKindWord("type_alias")).toBe("type alias");
  });

  it("reads reachable / unreachable / unknown as yes / no / unknown — unknown is never no", () => {
    expect(reachableWord("reachable")).toBe("yes");
    expect(reachableWord("unreachable")).toBe("no");
    expect(reachableWord("unknown")).toBe("unknown");
    expect(reachableWord("unknown")).not.toBe(FINDINGS_TABLE_TEXT.reachable.unreachable);
  });

  it("states the sentence behind each word: the route and hops, no route reaches it, or why it is unknown", () => {
    const route = { symbol: "r", name: "GET /users/:id", hops: 2 };
    expect(reachabilityText({ confidence: "resolved", verdict: "reachable", route, unresolved_because: null })).toBe(
      CONTEXT_TEXT.reachable("GET /users/:id", 2),
    );
    expect(reachabilityText({ confidence: "resolved", verdict: "unreachable", route: null, unresolved_because: null })).toBe(
      CONTEXT_TEXT.unreachable,
    );
    expect(reachabilityText({ confidence: "unresolved", verdict: "unknown", route: null, unresolved_because: "language" })).toBe(
      CONTEXT_TEXT.unknown("language"),
    );
    expect(reachabilityText({ confidence: "unresolved", verdict: "unknown", route: null, unresolved_because: null })).toBe(
      CONTEXT_TEXT.unknown(null),
    );
  });
});

describe("filters mirror `security findings`", () => {
  const drivers = scannerDrivers(PAGE.status);
  const c2025 = editionOf(PAGE, "2025").classify;
  const kept = (filters: Partial<typeof NO_FILTERS>) =>
    PAGE.findings.findings.filter((f) => keeps(f, { ...NO_FILTERS, ...filters }, c2025(f), drivers)).map((f) => f.rule_id);

  it("keeps everything with no filter set", () => {
    expect(kept({})).toHaveLength(2);
  });

  it("matches a scanner by registry name or driver name, and not by a near miss", () => {
    expect(kept({ scanner: "semgrep" })).toHaveLength(2);
    expect(kept({ scanner: "Semgrep OSS" })).toHaveLength(2);
    expect(kept({ scanner: "semgre" })).toHaveLength(0);
  });

  it("matches a category by full id or number, under the edition in force", () => {
    expect(kept({ category: "A05:2025" })).toEqual([SQL.rule_id]);
    expect(kept({ category: "A05" })).toEqual([SQL.rule_id]);
    expect(kept({ category: "A03:2021" })).toEqual([]);
  });

  it("keeps a file, or a directory holding it — never a sibling sharing its prefix", () => {
    expect(kept({ path: "src/lib.rs" })).toHaveLength(2);
    expect(kept({ path: "src" })).toHaveLength(2);
    expect(kept({ path: "./src/" })).toHaveLength(2);
    expect(kept({ path: "sr" })).toHaveLength(0);
    expect(kept({ path: "src/lib" })).toHaveLength(0);
  });

  it("keeps active or suppressed findings — an expired suppression is active", () => {
    const suppressed = { ...SECRET, suppression: { state: "suppressed" as const, reason: "test key", expires: null, by: { fingerprint: SECRET.fingerprint } } };
    const expired = { ...SQL, suppression: { state: "expired" as const, reason: "old", expires: "2026-01-01", by: { fingerprint: SQL.fingerprint } } };
    const keep = (f: SecurityFinding, suppression: "active" | "suppressed") =>
      keeps(f, { ...NO_FILTERS, suppression }, c2025(f), drivers);
    expect([keep(suppressed, "suppressed"), keep(suppressed, "active")]).toEqual([true, false]);
    expect([keep(expired, "suppressed"), keep(expired, "active")]).toEqual([false, true]);
    // An out-of-date verdict suppresses nothing: the finding is active (FR-SC-32).
    const stale = { ...SQL, suppression: { state: "out_of_date" as const, reason: "r", expires: null, by: { fingerprint: SQL.fingerprint }, changed: [] } };
    expect([keep(stale, "suppressed"), keep(stale, "active")]).toEqual([false, true]);
  });

  it("words an out-of-date verdict and its evidence (FR-SC-32)", () => {
    const by = { fingerprint: "f" };
    const change = (kind: "changed" | "gone" | "unverifiable", detail?: string) => ({ symbol: "s", name: "DefaultCookieConfig", change: kind, ...(detail ? { detail } : {}) });
    const stale = (...changed: ReturnType<typeof change>[]) => ({ state: "out_of_date" as const, reason: "r", expires: null, by, changed });
    expect(suppressionNote(stale(change("changed")))).toBe("r");
    expect(evidenceNote(stale(change("changed")))).toBe("`DefaultCookieConfig` changed");
    expect(suppressionNote(stale(change("gone")))).toBe("r");
    expect(evidenceNote(stale(change("gone")))).toBe("`DefaultCookieConfig` no longer exists");
    expect(evidenceNote(stale(change("unverifiable", "the index is behind a.go: run `logos sync a.go`")))).toBe(
      "`DefaultCookieConfig` could not be checked: the index is behind a.go: run `logos sync a.go`",
    );
    expect(suppressionState(stale())).toBe("out of date");
    expect(suppressionNote({ state: "out_of_date", reason: "r", expires: null, by: { path: "src" }, changed: [change("changed")] })).toBe(
      "r — covers everything under src",
    );
    expect(evidenceNote({ state: "suppressed", reason: "r", expires: null, by })).toBe("No evidence recorded");
    expect(evidenceNote({ state: "expired", reason: "r", expires: null, by })).toBeNull();
  });

  it("matches an unbound finding's path as the report named it, and a pasted path with spaces", () => {
    const lockfile = { ...SQL, path: null, report_path: "Cargo.lock" };
    expect(keeps(lockfile, { ...NO_FILTERS, path: "Cargo.lock" }, c2025(lockfile), drivers)).toBe(true);
    expect(normalisePath("  ./src/  ")).toBe("src");
  });

  it("keeps one reachability verdict, unknown apart from unreachable", () => {
    // Both real findings are `unknown` (Rust here binds no call across files).
    expect(kept({ reachability: "unknown" })).toHaveLength(2);
    expect(kept({ reachability: "unreachable" })).toHaveLength(0);
    expect(kept({ reachability: "reachable" })).toHaveLength(0);
  });

  it("keeps the least severity and above", () => {
    expect(kept({ severity: "high" })).toHaveLength(2);
    expect(kept({ severity: "critical" })).toHaveLength(0);
  });

  it("lists unrated findings alone, and gates them as medium under a rated least severity (S-768)", () => {
    const unrated = { ...SQL, severity: "unrated" as const, severity_reason: "the scanner gave none" };
    const medium = { ...SQL, severity: "medium" as const };
    const keep = (f: SecurityFinding, severity: (typeof NO_FILTERS)["severity"]) =>
      keeps(f, { ...NO_FILTERS, severity }, c2025(f), drivers);
    expect([keep(unrated, "unrated"), keep(medium, "unrated"), keep(SQL, "unrated")]).toEqual([true, false, false]);
    expect([keep(unrated, "low"), keep(unrated, "medium"), keep(unrated, "high")]).toEqual([true, true, false]);
    expect(kept({ severity: "unrated" })).toHaveLength(0);
  });

  it("keeps the registry's driver when a report is stored under a registry name spelled as its driver", () => {
    const lowercase = row({ scanner: "semgrep", driver: "semgrep", state: "fresh", snapshot_head: "a", version: "1", findings: 0, suppressed: 0 });
    const status = { ...PAGE.status, other_reports: [lowercase] };
    expect(scannerDrivers(status).get("semgrep")).toBe("Semgrep OSS");
  });

  it("reads `?path=` from a deep link, normalised", () => {
    expect(pathFromSearch("?path=./src/analysis/")).toBe("src/analysis");
    expect(pathFromSearch("?seed=x")).toBe("");
    expect(normalisePath("src\\db.rs")).toBe("src/db.rs");
  });
});

describe("each absent state names a documented command that fixes it", () => {
  it("not installed: the read-model's install command, else ingesting a CI report", () => {
    const trivy = PAGE.status.scanners.find((r) => r.scanner === "trivy") as SecurityScannerStatus;
    expect(scannerStateText(trivy)).toContain("logos security install trivy");
    expectDocumentedCommands(scannerStateText(trivy));
    const codeql = PAGE.status.scanners.find((r) => r.scanner === "codeql") as SecurityScannerStatus;
    expect(codeql.install).toBeNull();
    expect(scannerStateText(codeql)).toContain("logos security ingest <report>");
    expectDocumentedCommands(scannerStateText(codeql));
  });

  it("stale with no copy here (a CI report, an unregistered tool): an ingest, never a scan that cannot run", () => {
    const stored = { snapshot_head: "a", version: "1", findings: 0, suppressed: 0 };
    const ciOnly = row({ scanner: "codeql", state: "stale", because: "head_moved", ...stored });
    expect(scannerStateText(ciOnly)).toContain("logos security ingest <report>");
    expect(scannerStateText(ciOnly)).not.toContain("logos security scan");
    const other = row({ scanner: "Snyk", driver: "Snyk", state: "stale", because: "files_changed", ...stored });
    expect(scannerStateText(other)).toBe("A file it scanned changed; run logos security ingest <report>.");
    expectDocumentedCommands(scannerStateText(other));
  });

  it("disabled names the config file, and fresh states its count with any suppressed", () => {
    expect(scannerStateText(row({ state: "disabled" }))).toBe("Disabled in .logos/config.toml; enable it there to scan with it.");
    const fresh = row({ state: "fresh", snapshot_head: "a", version: "1", findings: 3, suppressed: 1 });
    expect(scannerStateText(fresh)).toBe("3 findings, 1 suppressed");
    expect(scannerStateText(row({ state: "fresh", snapshot_head: "a", version: "1", findings: 1, suppressed: 0 }))).toBe("1 finding");
  });

  it("never run and stale: a scan — with --consent while the project's consent does not name the scanner", () => {
    const bandit = PAGE.status.scanners.find((r) => r.scanner === "bandit") as SecurityScannerStatus;
    // The real payload's consent awaits bandit.
    expect(PAGE.status.consent.pending).toEqual(["bandit"]);
    expect(scannerStateText(bandit, PAGE.status.consent.pending)).toBe("Installed but never run; run logos security scan --consent.");
    expect(scannerStateText(bandit, [])).toBe("Installed but never run; run logos security scan.");
    expectDocumentedCommands(scannerStateText(bandit, PAGE.status.consent.pending));
    const installed = { copy: "path" as const, binary: "/stub/bin/trivy", version: "0.50.0", update_available: null };
    const stale = row({ state: "stale", because: "head_moved", snapshot_head: "a", version: "1", findings: 0, suppressed: 0, installed });
    expect(scannerStateText(stale)).toBe("HEAD moved since its scan; run logos security scan.");
    expectDocumentedCommands(scannerStateText(stale));
    // Only a scoped re-scan stored (FR-SC-17): stale, and the fix is a full scan.
    const scoped = row({ state: "stale", because: "scoped_only", snapshot_head: "a", version: "1", findings: 2, suppressed: 0, installed });
    expect(scannerStateText(scoped)).toBe("Only a scoped re-scan of some paths is stored; run logos security scan.");
    expectDocumentedCommands(scannerStateText(scoped));
  });

  it("not covered: a scan when a capable scanner is installed, else install-and-scan, else an ingest", () => {
    const category: SecurityCategoryCoverage = {
      id: "A03:2025",
      name: "Software Supply Chain Failures",
      scanners: ["trivy", "osv-scanner", "dependency-check"],
      enabled_scanners: [],
      state: { state: "not_covered" },
    };
    // trivy is installable on the real payload; an install alone would produce no
    // report, so the command installs and scans.
    expect(notCoveredCommand(category, PAGE.status)).toBe("logos security scan --install --consent");
    // Installed, never run → a scan; with --consent while consent awaits it.
    const ran = { ...PAGE.status, scanners: [row({ state: "never_run" })] };
    expect(notCoveredCommand(category, ran)).toBe("logos security scan");
    const awaiting = { ...ran, consent: { consented: [], pending: ["trivy"] } };
    expect(notCoveredCommand(category, awaiting)).toBe("logos security scan --consent");
    // Nothing installable → an ingest; an opt-in scanner, which `scan --install`
    // never installs, does not count as installable (S-746).
    const none = { ...PAGE.status, scanners: [row({ scanner: "dependency-check", state: "not_installed" })] };
    expect(notCoveredCommand(category, none)).toBe("logos security ingest <report>");
    const optIn = row({ scanner: "dependency-check", state: "not_installed", install: "logos security install dependency-check", opt_in: true });
    expect(notCoveredCommand(category, { ...PAGE.status, scanners: [optIn] })).toBe("logos security ingest <report>");
    // Stale and installed here → a scan refreshes it; stale with no copy is not idle.
    const stored = { snapshot_head: "a", version: "1", findings: 0, suppressed: 0 };
    const installed = { copy: "path" as const, binary: "/stub/bin/trivy", version: "0.50.0", update_available: null };
    const staleHere = { ...PAGE.status, scanners: [row({ state: "stale", because: "head_moved", ...stored, installed })] };
    expect(notCoveredCommand(category, staleHere)).toBe("logos security scan");
    const staleCi = { ...PAGE.status, scanners: [row({ state: "stale", because: "head_moved", ...stored })] };
    expect(notCoveredCommand(category, staleCi)).toBe("logos security ingest <report>");
    for (const status of [PAGE.status, ran, awaiting, none]) expectDocumentedCommands(notCoveredCommand(category, status));
  });
});

describe("severityTone", () => {
  it("is red for critical and high, orange for medium, muted below — text always rides with it", () => {
    expect(["critical", "high"].map((s) => severityTone(s as never))).toEqual(["red", "red"]);
    expect(severityTone("medium")).toBe("orange");
    expect(["low", "info"].map((s) => severityTone(s as never))).toEqual(["muted", "muted"]);
  });
});

describe("path suppressions (S-770)", () => {
  const at = (path: string | null, report_path: string | null = path): SecurityFinding => ({ ...SQL, path, report_path });

  it("offers the finding's directory, the file itself at the root, and a ** glob for an absolute report path", () => {
    expect(suggestedPath(at("src/db/query.rs"))).toBe("src/db");
    expect(suggestedPath(at("tests/fixtures/keys.txt"))).toBe("tests/fixtures");
    expect(suggestedPath(at("Cargo.lock"))).toBe("Cargo.lock");
    expect(suggestedPath(at(null, "./web/ui/package-lock.json"))).toBe("web/ui");
    expect(suggestedPath(at(null, "/home/me/logos/Cargo.lock"))).toBe("**/Cargo.lock");
    expect(suggestedPath(at(null, "C:\\work\\logos\\Cargo.lock"))).toBe("**/Cargo.lock");
    expect(suggestedPath(at(null, null))).toBe("");
  });

  it("keys a path entry by its whole selector, so a narrowing is a different entry", () => {
    const keys = [{ path: "src" }, { path: "src", rule: "r" }, { path: "src", scanner: "r" }, { fingerprint: "src" }].map(selectorKey);
    expect(new Set(keys).size).toBe(4);
    expect(selectorKey({ path: "src", rule: "r" })).toBe(selectorKey({ rule: "r", path: "src" }));
  });

  it("names which entry covers a path-suppressed finding, and leaves a fingerprint's note as it was", () => {
    expect(selectorText({ path: "tests/fixtures", rule: "k", scanner: "gitleaks" })).toBe("everything under tests/fixtures, rule k, scanner gitleaks");
    expect(selectorText({ fingerprint: "0123456789abcdef".repeat(4) })).toBe("fingerprint 0123456789ab…");
    expect(suppressionNote({ state: "suppressed", reason: "r", expires: "2027-01-01", by: { path: "src" } })).toBe("r (until 2027-01-01) — covers everything under src");
    expect(suppressionNote({ state: "expired", reason: "r", expires: "2026-01-01", by: { path: "src" } })).toBe("r (expired after 2026-01-01) — covers everything under src");
    expect(suppressionNote({ state: "suppressed", reason: "r", expires: null, by: { fingerprint: "f" } })).toBe("r");
  });

  it("names a rule-wide entry as the one that suppressed a finding, with its reason and expiry (S-814)", () => {
    const by = { rule: "CKV_K8S_21", scanner: "checkov" };
    expect(selectorText(by)).toBe("rule CKV_K8S_21 of checkov, in every file");
    expect(suppressionNote({ state: "suppressed", reason: "deploy tool sets it", expires: null, by })).toBe(
      "deploy tool sets it — suppressed by a rule-wide entry (rule CKV_K8S_21 of checkov, in every file)",
    );
    expect(suppressionNote({ state: "suppressed", reason: "r", expires: "2027-01-01", by })).toBe(
      "r (until 2027-01-01) — suppressed by a rule-wide entry (rule CKV_K8S_21 of checkov, in every file)",
    );
    expect(suppressionNote({ state: "expired", reason: "r", expires: "2026-01-01", by })).toBe(
      "r (expired after 2026-01-01) — rule-wide entry (rule CKV_K8S_21 of checkov, in every file)",
    );
  });

  it("keys a rule-wide entry apart from a path entry that narrows to the same rule and scanner", () => {
    const keys = [{ rule: "r", scanner: "s" }, { path: "src", rule: "r", scanner: "s" }, { rule: "s", scanner: "r" }, { fingerprint: "r" }].map(selectorKey);
    expect(new Set(keys).size).toBe(4);
  });
});

const SCAN = scanReal as unknown as ScanJob;
const INSTALL = installReal as unknown as InstallJob;
const ending = (job: ScanJob | InstallJob, count: number, state: ScanJob["state"] = "running") =>
  ({ ...job, state, result: null, events: job.events.slice(0, count) }) as ScanJob | InstallJob;

describe("a job's progress and the live state of each row (S-792)", () => {
  it("reads which scanner runs and how many finished from the events the page already polls", () => {
    // Four not-installed scanners finished; Bandit started.
    const progress = jobProgress(ending(SCAN, 5));
    expect(progress.running).toEqual(["bandit"]);
    expect(progress.finished.map((f) => f.scanner)).toEqual(["semgrep", "trivy", "gitleaks", "osv-scanner"]);
    expect(progress.finished.some((f) => f.ran)).toBe(false);
    // Once Bandit finished, nothing runs and it ran.
    const after = jobProgress(ending(SCAN, 6));
    expect(after.running).toEqual([]);
    expect(after.finished.find((f) => f.scanner === "bandit")?.ran).toBe(true);
    expect(jobProgress(null)).toEqual({ finished: [], running: [], installing: [], toolsDone: 0, scanning: false });
  });

  it("reads which tool installs, and when the scan after the install began", () => {
    // Trivy landed; Gitleaks downloads.
    const downloading = jobProgress(ending(INSTALL, 3));
    expect(downloading).toMatchObject({ installing: ["gitleaks"], toolsDone: 1, scanning: false });
    // Every install finished and Semgrep's skip was reported: the scan began.
    expect(jobProgress(ending(INSTALL, 9))).toMatchObject({ installing: [], toolsDone: 4, scanning: true, running: [] });
    expect(jobProgress(ending(INSTALL, 10)).running).toEqual(["trivy"]);
  });

  it("gives a row a live state only while its job runs", () => {
    expect([...liveStates(ending(SCAN, 5))]).toEqual([["bandit", "running"]]);
    expect([...liveStates(ending(INSTALL, 3))]).toEqual([["gitleaks", "installing"]]);
    for (const state of ["completed", "cancelled", "failed"] as const) {
      expect(liveStates(ending(SCAN, 5, state)).size, state).toBe(0);
      expect(liveStates(ending(INSTALL, 3, state)).size, state).toBe(0);
    }
    expect(liveStates(null).size).toBe(0);
  });

  it("states the progress of a running job without a clock, and its outcome once it ended", () => {
    expect(summaryText(ending(SCAN, 5))).toBe(SCAN_JOB_TEXT.running(4, ["bandit"]));
    expect(summaryText(ending(INSTALL, 3))).toBe(INSTALL_JOB_TEXT.running(1));
    expect(summaryText(ending(INSTALL, 12))).toBe(INSTALL_JOB_TEXT.scanning(2));
    expect(summaryText({ ...SCAN })).toBe(SCAN_JOB_TEXT.completed(0, 1));
    expect(summaryText({ ...SCAN, state: "failed", result: null, error: "boom" })).toBe(SCAN_JOB_TEXT.failed("boom"));
    expect(summaryText(null)).toBe(SCAN_JOB_TEXT.idle);
    expect(summaryText(ending(SCAN, 5))).not.toMatch(/\d+ s\b/);
  });

  it("names the findings a completed job found, not a fixed zero", () => {
    expect(summaryText({ ...SCAN, result: { ...SCAN.result!, findings: 3 } })).toBe(SCAN_JOB_TEXT.completed(3, 1));
    const withScan = { ...INSTALL, result: { ...INSTALL.result!, scan: { ...INSTALL.result!.scan!, findings: 5 } } };
    expect(summaryText(withScan)).toBe(INSTALL_JOB_TEXT.completed(4, { findings: 5, ran: 4 }));
  });

  it("counts a tool already present as not installed by this job, and the scan as begun by its first scanner alone", () => {
    const steps: SecurityInstallStep[] = [
      { phase: "install_started", tool: "trivy", version: "0.74.0" },
      { phase: "install_finished", tool: "trivy", version: "0.74.0", outcome: "already_present", binary: "/bin/trivy" },
      { phase: "install_started", tool: "gosec", version: "2.21.4" },
      { phase: "install_finished", tool: "gosec", version: "2.21.4", outcome: "installed", binary: "/bin/gosec" },
      { phase: "started", scanner: "bandit" },
    ];
    const job = { ...INSTALL, state: "completed", events: steps.map((event, seq) => ({ seq, at_ms: seq, event })) } as InstallJob;
    expect(jobProgress(job)).toMatchObject({ scanning: true, running: ["bandit"], toolsDone: 2 });
    expect(summaryText({ ...job, result: { ...INSTALL.result!, install: { ...INSTALL.result!.install, tools: [
      { tool: "trivy", version: "0.74.0", outcome: "already_present", binary: "/bin/trivy" },
      { tool: "gosec", version: "2.21.4", outcome: "installed", binary: "/bin/gosec" },
    ] }, scan: null } })).toBe(INSTALL_JOB_TEXT.completed(1, null));
    expect(summaryText({ ...job, state: "running", result: null })).toBe(INSTALL_JOB_TEXT.scanning(0));
  });

  it("lists a tool an install left out from its events, whatever the job's own state", () => {
    const left: SecurityInstallStep[] = [
      { phase: "install_started", tool: "trivy", version: "0.74.0" },
      { phase: "install_finished", tool: "trivy", version: "0.74.0", outcome: "unavailable", reason: "no build" },
      { phase: "install_started", tool: "gitleaks", version: "8.18.4" },
      { phase: "install_finished", tool: "gitleaks", version: "8.18.4", outcome: "failed", stage: "checksum", reason: "mismatch" },
    ];
    const job = { ...INSTALL, state: "failed", result: null, error: "x", events: left.map((event, seq) => ({ seq, at_ms: seq, event })) } as InstallJob;
    expect(summaryText(job)).toBe(
      `${INSTALL_JOB_TEXT.failed("x")}${INSTALL_JOB_TEXT.notInstalled([INSTALL_OUTCOME_TEXT.unavailable("trivy", "no build"), INSTALL_OUTCOME_TEXT.failed("gitleaks", "checksum", "mismatch")])}`,
    );
  });
});

describe("the Ignored dirs marker comes from the kind, never the note (S-792)", () => {
  it("maps each state and kind to its marker", () => {
    expect(ignoredDirsKey({})).toBe("none");
    expect(ignoredDirsKey({ ignored_paths: "excluded" })).toBe("skipped");
    for (const kind of ["unproven", "history", "whole_project", "partial"] as const) {
      expect(ignoredDirsKey({ ignored_paths: "scanned", coverage_kind: kind, coverage_note: "any words at all" })).toBe(kind);
    }
    // A note alone — a server that sent no kind — is never parsed for one.
    expect(ignoredDirsKey({ ignored_paths: "scanned", coverage_note: "exclusion flag not proven by the harness" })).toBe("scanned");
  });

  it("lists each marker shown once, in a fixed order, and none that is not shown", () => {
    const rows = [
      { ignored_paths: "scanned", coverage_kind: "history" },
      { ignored_paths: "excluded" },
      { ignored_paths: "scanned", coverage_kind: "history" },
      {},
    ] as const;
    expect(markersShown(rows).map((m) => m.key)).toEqual(["skipped", "history", "none"]);
    expect(markersShown([])).toEqual([]);
  });
});
