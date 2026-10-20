// The Security page's pure model (S-670, FR-SC-09): filters, edition lookup,
// redaction and the command each absent state names — over the real payload
// `web/tests/api_v1.rs` produced (`fixtures/security.real.json`).
import { describe, expect, it } from "vitest";

import reference from "../../../../../docs/howto/commands.md?raw";
import type { SecurityCategoryCoverage, SecurityFinding, SecurityModel, SecurityScannerStatus } from "../../api/types.ts";
import { REDACTED, selectorText, suppressionNote } from "../../copy/security.copy.ts";
import { commandsIn, documentedCommands, isDocumented } from "../../test/commandCheck.ts";
import real from "./fixtures/security.real.json";
import {
  NO_FILTERS,
  editionOf,
  findingLocation,
  findingMessage,
  keeps,
  normalisePath,
  notCoveredCommand,
  pathFromSearch,
  scannerDrivers,
  scannerStateText,
  selectorKey,
  severityTone,
  suggestedPath,
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

describe("redaction (FR-SC-03)", () => {
  it("renders a secret-class message as [redacted], whatever text it holds", () => {
    expect(findingMessage(SECRET)).toBe(REDACTED);
    expect(findingMessage({ ...SECRET, message: "password=hunter2" })).toBe(REDACTED);
    expect(findingMessage(SQL)).toBe("Formatted SQL query.");
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
});
