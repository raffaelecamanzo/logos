#!/usr/bin/env python3
"""The deterministic half of the logos-sec-review skill (FR-SC-19).

Every judgement the skill makes is the model's; everything with a binary outcome
is here: reading the security section of the branch delta, the pre-classification
of each finding by class and reachability confidence, the proof of a fix by a
targeted re-scan, the OWASP coverage the report must state, the suppression draft
and the report check. The shell scripts one directory up are thin wrappers that
select a subcommand.

The plugin's logos-review skill owns the helpers every review shares (loading and
writing JSON, printing to stderr, one logos call that answers JSON). They are
loaded from that skill's own `scripts/lib/delta.py`, not copied.

Standard library only. Exit codes: 0 good answer, 1 the other answer, 2 usage,
3 environment or unreadable input. Each subcommand ends with one `denominator:`
line on stderr.
"""

import argparse
import importlib.util
import os
import re
import sys
from datetime import date

HERE = os.path.dirname(os.path.abspath(__file__))
SIBLING = os.path.join(HERE, "..", "..", "..", "logos-review", "scripts", "lib", "delta.py")


def load_sibling():
    sys.dont_write_bytecode = True  # importing must leave no __pycache__ in the sibling skill
    spec = importlib.util.spec_from_file_location("logos_review_delta", SIBLING)
    if spec is None or spec.loader is None or not os.path.isfile(SIBLING):
        print(f"logos-sec-review: the logos-review skill's helper is missing: {os.path.normpath(SIBLING)}", file=sys.stderr)
        sys.exit(3)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


rv = load_sibling()
try:
    eprint, load, dump, logos_json = rv.eprint, rv.load, rv.dump, rv.logos_json
except AttributeError as err:
    print(f"logos-sec-review: the logos-review skill's helper no longer offers what this skill reads: {err}", file=sys.stderr)
    sys.exit(3)

VERDICTS = ("fix", "escalate-secret", "escalate-dependency", "false-positive", "untrusted")
# What the delta calls a finding the branch brought or reached; every other status
# is counted and never judged.
JUDGED = ("introduced", "touched")
SEVERITY = ("info", "low", "medium", "high", "critical")
# Scanners whose every finding is a dependency finding, by normalised driver name.
DEPENDENCY_SCANNERS = ("osv-scanner", "dependency-check")
# Trivy reports dependencies, secrets and misconfigurations: only an advisory id
# makes its finding a dependency finding.
ADVISORY = re.compile(
    r"^(CVE-\d{4}-\d{4,}|GHSA(-[0-9a-z]{4}){3}|GO-\d{4}-\d{4,}|PYSEC-\d{4}-\d+|RUSTSEC-\d{4}-\d{4}|OSV-\d{4}-\d+)$",
    re.IGNORECASE,
)
FINGERPRINT = re.compile(r"[0-9a-f]{64}")
ROTATE_NOTE = "rotate the credential; the value remains in git history"


def plural(n, one, many):
    return f"{n} {one if n == 1 else many}"


def scanner_key(name):
    return (name or "").strip().lower()


def severity_rank(name):
    try:
        return SEVERITY.index((name or "").lower())
    except ValueError:
        return len(SEVERITY)  # a label this helper has never seen outranks the rest


# ── the delta read ───────────────────────────────────────────────────────────


def cmd_extract(args):
    """The security section of a `logos review --json` document, kept apart."""
    doc = load(args.review)
    section = (doc.get("sections") or {}).get("security")
    if not isinstance(section, dict):
        why = doc.get("not_measured_because") or "the review carries no security section"
        section = {"measured": False, "not_measured_because": why, "findings": [], "suppressed": [], "counts": {}}
    out = {
        "base": doc.get("base"),
        "merge_base": doc.get("merge_base"),
        "security": section,
    }
    dump(out, args.out)
    if not section.get("measured"):
        print(f"not measured: {section.get('not_measured_because') or 'no reason given'}")
        eprint("denominator: 0 findings classified (the security delta measured nothing; an empty list is not a clean one)")
        return 1
    counts = section.get("counts") or {}
    listed = len(section.get("findings") or [])
    judged = sum(int(counts.get(k, 0)) for k in JUDGED)
    methods = ", ".join(f"{s.get('scanner')}={s.get('method')}" for s in section.get("scanners") or []) or "none"
    print(
        f"measured: base {doc.get('base')} merge-base {doc.get('merge_base')}; {plural(judged, 'finding', 'findings')} to judge "
        f"(introduced {counts.get('introduced', 0)}, touched {counts.get('touched', 0)}); "
        f"resolved {counts.get('resolved', 0)}, stale {counts.get('stale', 0)}, pre-existing {counts.get('pre_existing', 0)}, "
        f"suppressed {counts.get('suppressed', 0)}; compared by {methods}"
    )
    for note in section.get("notes") or []:
        print(f"note: {note}")
    eprint(
        f"denominator: {judged} to judge of {listed} unsuppressed finding(s) listed, "
        f"{counts.get('suppressed', 0)} suppressed (not judged), {len(section.get('scanners') or [])} scanner(s) compared"
    )
    return 0


# ── the triage ───────────────────────────────────────────────────────────────


def where(e):
    path = e.get("path")
    if not path:
        return "-"
    return f"{path}:{e['start_line']}" if e.get("start_line") is not None else path


def subject_of(e):
    """The subject a report line names a finding by. The fingerprint prefix keeps
    two findings of one rule on one line apart."""
    return f"{where(e)} {e.get('rule_id') or '-'} [{(e.get('fingerprint') or '')[:12]}]"


def is_dependency(e):
    scanner = scanner_key(e.get("scanner"))
    if scanner in DEPENDENCY_SCANNERS:
        return True
    return scanner == "trivy" and bool(ADVISORY.match(e.get("rule_id") or ""))


def propose(e):
    """(class, proposed verdict, final?, reason). `final` means the model may not
    move it; a proposed `fix` is the one verdict it may downgrade, and only to
    `false-positive`."""
    if e.get("redacted"):
        return "secret", "escalate-secret", True, f"secret-class: {ROTATE_NOTE}; never closed by deleting the value"
    if is_dependency(e):
        return "dependency", "escalate-dependency", True, "dependency finding: a human approves the version change"
    if not e.get("path"):
        return "code", "untrusted", True, "the finding binds to no file, so there is nothing to edit"
    confidence = e.get("confidence")
    if confidence != "resolved":
        return "code", "untrusted", True, f"reachability confidence is {confidence or 'missing'}, not resolved"
    if e.get("freshness") == "stale":
        return "code", "untrusted", True, "stale: the file changed since the scan; re-scan it, then review again"
    return "code", "fix", False, "a trusted finding in code the branch changed: confirm it is a true positive, else false-positive"


def cmd_triage(args):
    doc = load(args.delta)
    section = doc.get("security") or {}
    if not section.get("measured"):
        print(f"not measured: {section.get('not_measured_because') or 'no reason given'}")
        eprint("denominator: 0 findings triaged (the security delta measured nothing)")
        return 1
    entries = []
    for n, f in enumerate([f for f in section.get("findings") or [] if f.get("status") in JUDGED], 1):
        klass, verdict, final, reason = propose(f)
        entries.append(
            {
                "id": f"s{n}",
                "subject": subject_of(f),
                "status": f.get("status"),
                "class": klass,
                "proposed": verdict,
                "final": final,
                "reason": reason,
                "scanner": f.get("scanner"),
                "fingerprint": f.get("fingerprint"),
                "rule_id": f.get("rule_id"),
                "severity": f.get("severity"),
                "owasp": f.get("owasp") or [],
                "path": f.get("path"),
                "line": f.get("start_line"),
                "confidence": f.get("confidence"),
                "freshness": f.get("freshness"),
                "message": f.get("message"),
            }
        )
    counts = section.get("counts") or {}
    out = {
        "base": doc.get("base"),
        "merge_base": doc.get("merge_base"),
        "entries": entries,
        "not_judged": {k: counts.get(k, 0) for k in ("resolved", "stale", "pre_existing", "suppressed")},
    }
    dump(out, args.out)
    for e in entries:
        mark = f"{e['proposed']}?" if not e["final"] else e["proposed"]
        print(f"{e['id']} | {mark} | {e['subject']} | {e['status']} {e['severity']} {e['scanner']} | {e['reason']}")
    if not entries:
        print("NO ENTRIES")
    eprint(
        f"denominator: {len(entries)} triaged of {len(section.get('findings') or [])} unsuppressed finding(s) listed "
        f"({counts.get('suppressed', 0)} suppressed, {counts.get('pre_existing', 0)} pre-existing, "
        f"{counts.get('resolved', 0)} resolved, {counts.get('stale', 0)} stale: not judged)"
    )
    return 0


def pick(path, ids):
    """The triage entries named by `ids`; an id the triage does not hold ends the run (exit 2)."""
    by_id = {e["id"]: e for e in load(path).get("entries") or []}
    missing = [i for i in ids if i not in by_id]
    if missing:
        eprint(f"logos-sec-review: an --entry is not in {path}: {', '.join(missing)}")
        sys.exit(2)
    return [by_id[i] for i in ids]


# What the line- and tab-separated `files` and `plan` outputs cannot carry: split
# back in bash, a tab or line break in a repository file name would become
# arguments of its own to `logos security scan` (`--consent`, `--install`).
UNSPLITTABLE = ("\t", "\n", "\r", "\0")


def pick_carried(path, ids):
    """`pick`, ending the run (exit 2) when an entry's file name holds a tab, a
    line break or a NUL: that file is escalated, never re-scanned from here."""
    chosen = pick(path, ids)
    bad = [e["id"] for e in chosen if any(c in (e.get("path") or "") for c in UNSPLITTABLE)]
    if bad:
        eprint(
            f"logos-sec-review: {', '.join(bad)} names a file whose name holds a tab or a line break; "
            "it is not re-scanned by this script: escalate it"
        )
        sys.exit(2)
    return chosen


def cmd_files(args):
    """The files of the named entries, one per line, without repeats."""
    chosen = pick_carried(args.triage, args.entry)
    seen = []
    for e in chosen:
        if e.get("path") and e["path"] not in seen:
            seen.append(e["path"])
    for p in seen:
        print(p)
    eprint(f"denominator: {plural(len(chosen), 'entry', 'entries')}, {plural(len(seen), 'file', 'files')}")
    return 0


# ── the targeted re-scan ─────────────────────────────────────────────────────


def groups_of(chosen):
    """(scanner, [paths]) in first-seen order: one scan per scanner."""
    groups = {}
    for e in chosen:
        paths = groups.setdefault(e["scanner"], [])
        if e["path"] not in paths:
            paths.append(e["path"])
    return list(groups.items())


def cmd_plan(args):
    """The `logos security scan` argument lists the re-scan runs, one per line."""
    chosen = pick_carried(args.triage, args.entry)
    refused = [e["id"] for e in chosen if e["proposed"] != "fix"]
    if refused:
        eprint(f"logos-sec-review: only a fix is re-scanned; {', '.join(refused)} is not a fix entry")
        return 2
    groups = groups_of(chosen)
    for scanner, paths in groups:
        argv = ["--scanner", scanner]
        for p in paths:
            argv += ["--path", p]
        print("\t".join(argv))
    eprint(f"denominator: {plural(len(chosen), 'entry', 'entries')}, {plural(len(groups), 'scan', 'scans')}")
    return 0


def cmd_rescan_record(args):
    """Hold each scan's answer (`--scan FILE`, one per scan) against what was
    asked, and keep the record the proof reads."""
    scans = [load(p) for p in args.scan]
    chosen = pick(args.triage, args.entry)
    ran = {}
    for scan in scans:
        for run in scan.get("scanners") or []:
            if run.get("outcome") == "ran":
                ran[scanner_key(run.get("driver"))] = run  # the triage names a scanner by its driver
    rows, problems = [], []
    groups = groups_of(chosen)
    for scanner, paths in groups:
        run = ran.get(scanner_key(scanner))
        if run is None:
            problems.append(f"{scanner}: no `ran` outcome in the re-scan (installed, enabled and consented?)")
            continue
        scope = run.get("scope") or {}
        if scope.get("kind") == "paths":
            done = scope.get("paths") or []
            absent = [p for p in paths if p not in done]
            if absent:
                problems.append(f"{scanner}: the re-scan did not cover {', '.join(absent)}")
                continue
            how = "paths"
        else:
            how = "whole project"  # a scanner that cannot be narrowed ran on everything: a superset
        rows.append({"scanner": scanner, "driver": run.get("driver"), "version": run.get("version"), "paths": paths, "scope": how})
    dump({"rescan": rows, "problems": problems, "entries": [e["id"] for e in chosen]}, args.out)
    for r in rows:
        print(f"rescanned: {r['driver']} {r['version'] or 'unknown'} on {', '.join(r['paths'])} ({r['scope']})")
    for p in problems:
        print(f"NOT RESCANNED: {p}")
    eprint(f"denominator: {len(rows)} scanner run(s) held to {len(groups)} asked, {len(problems)} problem(s)")
    return 1 if problems else 0


# ── the proof of a fix ───────────────────────────────────────────────────────


def before_of(delta):
    """Every fingerprint the delta knew, by (scanner, path): unsuppressed and
    suppressed, whatever the status."""
    seen = {}
    section = delta.get("security") or {}
    for f in (section.get("findings") or []) + (section.get("suppressed") or []):
        seen.setdefault((scanner_key(f.get("scanner")), f.get("path")), set()).add(f.get("fingerprint"))
    return seen


def newer_than(project, path, record):
    """True when the file was modified after the re-scan record was written."""
    try:
        return os.stat(os.path.join(project, path)).st_mtime_ns > os.stat(record).st_mtime_ns
    except OSError:
        return True  # a file that cannot be read cannot be shown scanned as it stands


def cmd_proof(args):
    delta = load(args.delta)
    record = load(args.rescan)
    chosen = pick(args.triage, args.entry)
    scanned = {scanner_key(r["scanner"]): r for r in record.get("rescan") or []}
    before = before_of(delta)
    answers = {}
    bad = 0
    for e in chosen:
        key = scanner_key(e["scanner"])
        why = None
        run = scanned.get(key)
        if e["proposed"] != "fix":
            why = "not a fix entry"
        elif run is None:
            why = "no targeted re-scan of this scanner is recorded"
        elif e["path"] not in run["paths"]:
            why = f"the re-scan did not cover {e['path']}"
        elif newer_than(args.project, e["path"], args.rescan):
            why = f"{e['path']} changed after the re-scan; scan it again"
        else:
            if key not in answers:
                answers[key] = {}
            if e["path"] not in answers[key]:
                answers[key][e["path"]] = logos_json(
                    args.logos, args.project, ["security", "findings", "--json", "--scanner", e["scanner"], "--path", e["path"]]
                )
            answer = answers[key][e["path"]]
            if answer is None:
                why = "`logos security findings --json` did not answer"
            else:
                why = judge(e, answer, before.get((key, e["path"]), set()))
        if why is None:
            print(
                f"PROVEN {e['id']} | {e['subject']} | rescan: {run['driver']} {run['version'] or 'unknown'} "
                f"paths={','.join(run['paths'])} fingerprint absent"
            )
        else:
            bad += 1
            print(f"NOT PROVEN {e['id']} | {e['subject']} | {why}")
    eprint(f"denominator: {plural(len(chosen), 'fix entry', 'fix entries')} checked, {len(chosen) - bad} proven, {bad} not proven")
    return 1 if bad else 0


def judge(e, answer, known):
    """None when the stored findings prove the fix, else why they do not."""
    key = scanner_key(e["scanner"])
    if not any(scanner_key(r.get("driver")) == key for r in answer.get("scanned") or []):
        return "no stored report of this scanner after the re-scan"
    # The fingerprint is looked for in every stored finding the answer holds, wherever it was bound.
    if any(f.get("fingerprint") == e["fingerprint"] for f in answer.get("findings") or []):
        return "the fingerprint is still listed after the re-scan"
    # A severity this helper does not know is the strictest floor: nothing new may appear.
    floor = SEVERITY.index((e.get("severity") or "").lower()) if (e.get("severity") or "").lower() in SEVERITY else 0
    for f in answer.get("findings") or []:
        if f.get("path") != e["path"] and f.get("report_path") != e["path"]:
            continue
        if f.get("freshness") == "stale":
            return f"a finding of {e['path']} reads stale: the file changed after the re-scan"
        if f.get("fingerprint") not in known and severity_rank(f.get("severity")) >= floor:
            return (
                f"a new finding at or above {e.get('severity')} appeared in {e['path']}: "
                f"{f.get('rule_id')} {str(f.get('fingerprint'))[:12]}"
            )
    return None


# ── what the report must state ───────────────────────────────────────────────


def state_of(category):
    st = category.get("state")
    return (st.get("state") if isinstance(st, dict) else st) or ""


def cmd_coverage(args):
    answer = load(args.findings)
    uncovered = [
        {"id": c.get("id"), "name": c.get("name")}
        for c in answer.get("categories") or []
        if state_of(c) == "not_covered"
    ]
    scanned = [
        {"driver": r.get("driver"), "version": r.get("version") or "unknown", "head_sha": r.get("head_sha"), "findings": r.get("findings")}
        for r in answer.get("scanned") or []
    ]
    cats = answer.get("categories") or []
    dump({"edition": answer.get("edition"), "scanned": scanned, "uncovered": uncovered, "categories": len(cats)}, args.out)
    paths = args.paths or "whole project"
    for s in scanned:
        print(f"SCANNED | {s['driver']} {s['version']} | paths: {paths}")
    if not scanned:
        print("SCANNED | no scanner has a stored report | paths: none")
    ids = ", ".join(u["id"] for u in uncovered) or "none"
    print(f"COVERAGE | uncovered: {ids}")
    for u in uncovered:
        print(f"  {u['id']} {u['name']}")
    eprint(
        f"denominator: {len(scanned)} stored report(s), {plural(len(cats), 'OWASP category', 'OWASP categories')} "
        f"(edition {answer.get('edition')}), {len(uncovered)} uncovered"
    )
    return 0 if cats else 1


# ── the suppression draft ────────────────────────────────────────────────────


def cmd_draft(args):
    reason = " ".join(args.reason.split())
    if not FINGERPRINT.fullmatch(args.fingerprint):
        eprint("logos-sec-review: --fingerprint must be the finding's 64-character lowercase hex fingerprint")
        return 2
    if not reason:
        eprint("logos-sec-review: --reason is required: an entry without one rejects the whole ignore file")
        return 2
    if '"' in reason or "\\" in reason or re.search(r"[\x00-\x1f\x7f]", reason):
        eprint("logos-sec-review: --reason must not hold a quote, a backslash or a control character")
        return 2
    lines = ["[[ignore]]", f'fingerprint = "{args.fingerprint}"', f'reason = "{reason}"']
    if args.expires:
        try:
            if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", args.expires):
                raise ValueError(args.expires)
            date.fromisoformat(args.expires)
        except ValueError:
            eprint("logos-sec-review: --expires must be a calendar date, YYYY-MM-DD")
            return 2
        lines.append(f"expires = {args.expires}")
    print("\n".join(lines))
    eprint("denominator: 1 draft entry (printed, not written: the write is held by the guardrail, which asks the human)")
    return 0


# ── the report check ─────────────────────────────────────────────────────────

CLAIM_WORDS = re.compile(
    r"\bsecure[ds]?\b|\bsafe(ly)?\b|vulnerability[- ]free|\ball clear\b"
    r"|\bno (remaining |further |known )?(vulnerabilit(y|ies)|security (issues?|problems?)|findings?)\b"
    r"|\bzero (remaining |known )?(vulnerabilit(y|ies)|security (issues?|problems?)|findings?)\b"
    r"|\bfree of (known )?vulnerabilit(y|ies)\b",
    re.IGNORECASE,
)
# The report lines that name a triage subject as their first field, and the one verdict each belongs to.
OUTCOME_VERDICT = {
    "FIXED": "fix",
    "UNPROVEN": "fix",
    "ROTATE": "escalate-secret",
    "DEPENDENCY": "escalate-dependency",
    "SUPPRESS": "false-positive",
}
KEYWORDS = ("NO ENTRIES", "NOT MEASURED", "ENTRY", "FIXED", "UNPROVEN", "ROTATE", "DEPENDENCY", "SUPPRESS", "SCANNED", "COVERAGE")
TESTS = re.compile(r"^(\d+) passed, (\d+) failed \(of (\d+)\)$")
RESCAN = re.compile(r"^rescan: .+ \S+ paths=(.+) fingerprint absent$")
SUPPRESS_STATES = ("held at ask", "approved at ask", "declined at ask")


class Report:
    def __init__(self):
        self.problems = []
        self.entries = {}  # subject -> verdict
        self.lines = {k: {} for k in OUTCOME_VERDICT}  # keyword -> subject -> fields
        self.singles = {k: [] for k in ("NO ENTRIES", "NOT MEASURED", "COVERAGE")}
        self.scanned = []

    def bad(self, n, why):
        self.problems.append(f"line {n}: {why}")

    def line(self, n, raw):
        keyword = next((k for k in KEYWORDS if raw == k or raw.startswith(k + " |")), None)
        if keyword is None:
            prose = CLAIM_WORDS.search(raw)
            if prose:
                self.bad(n, f"a claim of safety or security in prose: '{prose.group(0)}'")
            return
        fields = [f.strip() for f in raw.split(" | ")]
        # The subject names a file and a rule: it is never scanned for claims. -1: the line has none.
        subject_at = 2 if keyword == "ENTRY" else 1 if keyword in OUTCOME_VERDICT else -1
        scanned_text = " | ".join(f for i, f in enumerate(fields) if i != subject_at)
        hit = CLAIM_WORDS.search(scanned_text)
        if hit:
            self.bad(n, f"a claim of safety or security: '{hit.group(0)}'")
        if keyword in self.singles:
            self.singles[keyword].append((n, fields))
            return
        if keyword == "SCANNED":
            self.scanned.append((n, fields))
            return
        if keyword == "ENTRY":
            if len(fields) != 4 or not fields[3]:
                self.bad(n, "ENTRY needs a verdict, a subject and a reason: ENTRY | verdict | subject | reason")
                return
            verdict, subject = fields[1], fields[2]
            if verdict not in VERDICTS:
                self.bad(n, f"'{verdict}' is not a verdict ({', '.join(VERDICTS)})")
                return
            if subject in self.entries:
                self.bad(n, f"two ENTRY lines name {subject}")
                return
            self.entries[subject] = (verdict, n)
            return
        if len(fields) < 3 or not all(fields[1:]):
            self.bad(n, f"{keyword} needs a subject and its evidence fields, none empty")
            return
        if fields[1] in self.lines[keyword]:
            self.bad(n, f"two {keyword} lines name {fields[1]}")
            return
        self.lines[keyword][fields[1]] = (n, fields)

    # ── the rules over the lines ────────────────────────────────────────────

    def judge(self):
        for subject, (verdict, n) in self.entries.items():
            self.entry_needs(subject, verdict, n)
        self.outcomes_name_their_verdict()
        self.fixed_lines()
        self.suppress_lines()
        self.measured_or_judged()
        self.coverage_and_scanned()

    def entry_needs(self, subject, verdict, n):
        """What each verdict obliges the report to carry."""
        fixed, unproven = subject in self.lines["FIXED"], subject in self.lines["UNPROVEN"]
        if verdict == "fix" and fixed == unproven:
            self.bad(n, f"{subject} is a fix and needs exactly one of FIXED or UNPROVEN")
        if verdict != "fix" and (fixed or unproven):
            self.bad(n, f"{subject} is {verdict}: it is never edited, so it has no FIXED or UNPROVEN line")
        if verdict == "escalate-secret":
            row = self.lines["ROTATE"].get(subject)
            text = (" | ".join(row[1][2:]) if row else "").lower()
            if row is None or "rotate" not in text or "git history" not in text:
                self.bad(n, f"{subject} is a secret: it needs a ROTATE line that says to rotate it and that the value remains in git history")
        if verdict == "escalate-dependency":
            row = self.lines["DEPENDENCY"].get(subject)
            labels = ("proposal:", "changelog risk:", "blast radius:")
            got = row[1][2:5] if row else []
            if len(got) != 3 or not all(f.startswith(l) and f[len(l):].strip() for f, l in zip(got, labels)):
                self.bad(n, f"{subject} is a dependency finding: it needs DEPENDENCY | subject | proposal: … | changelog risk: … | blast radius: …")
        if verdict == "false-positive" and subject not in self.lines["SUPPRESS"]:
            self.bad(n, f"{subject} is a false positive: it needs a SUPPRESS line with its draft, held at the guardrail")

    def outcomes_name_their_verdict(self):
        for keyword, allowed in OUTCOME_VERDICT.items():
            for subject, (n, _) in self.lines[keyword].items():
                got = self.entries.get(subject, (None,))[0]
                if got != allowed:
                    self.bad(n, f"{keyword} names {subject}, which is {'not judged' if got is None else got}, not {allowed}")

    def fixed_lines(self):
        for subject, (n, fields) in self.lines["FIXED"].items():
            if len(fields) != 5:
                self.bad(n, "FIXED needs: FIXED | subject | rescan: … | tests: … | P passed, F failed (of T)")
                continue
            if not RESCAN.match(fields[2]):
                self.bad(n, "FIXED must cite the targeted re-scan: 'rescan: <scanner> <version> paths=<a,b> fingerprint absent'")
            if not fields[3].startswith("tests:") or not fields[3][6:].strip():
                self.bad(n, "FIXED must name the test command: 'tests: <command>'")
            m = TESTS.match(fields[4])
            if not m:
                self.bad(n, "FIXED needs the test count: '<P> passed, <F> failed (of <T>)'")
            else:
                p, f, t = (int(x) for x in m.groups())
                if f != 0 or t < 1 or p + f != t:
                    self.bad(n, f"FIXED claims a test run of {fields[4]}: a fix is claimed only for a passing run of at least one test")

    def suppress_lines(self):
        for subject, (n, fields) in self.lines["SUPPRESS"].items():
            if len(fields) != 5 or not fields[2].startswith("fingerprint ") or not fields[3].startswith("state: ") or not fields[4].startswith("reason: "):
                self.bad(n, "SUPPRESS needs: SUPPRESS | subject | fingerprint <64 hex> | state: <held|approved|declined> at ask | reason: …")
                continue
            if not FINGERPRINT.fullmatch(fields[2][len("fingerprint "):]):
                self.bad(n, "SUPPRESS fingerprint is not a 64-character lowercase hex fingerprint")
            if fields[3][len("state: "):] not in SUPPRESS_STATES:
                self.bad(n, f"SUPPRESS state must be one of: {', '.join(SUPPRESS_STATES)}")

    def measured_or_judged(self):
        if self.singles["NOT MEASURED"]:
            if self.entries or self.singles["NO ENTRIES"]:
                self.bad(self.singles["NOT MEASURED"][0][0], "the report says NOT MEASURED and also lists a judgement")
        elif bool(self.singles["NO ENTRIES"]) == bool(self.entries):
            n = self.singles["NO ENTRIES"][0][0] if self.singles["NO ENTRIES"] else 1
            self.bad(n, "the report must be NO ENTRIES, or list at least one ENTRY, and not both")

    def coverage_and_scanned(self):
        if len(self.singles["COVERAGE"]) != 1:
            self.bad(1, "the report needs exactly one COVERAGE line naming the uncovered OWASP categories (or 'none')")
        elif not self.singles["COVERAGE"][0][1][1:] or not self.singles["COVERAGE"][0][1][1].startswith("uncovered: "):
            self.bad(self.singles["COVERAGE"][0][0], "COVERAGE needs: COVERAGE | uncovered: <ids, or none>")
        if not self.scanned and not self.singles["NOT MEASURED"]:
            self.bad(1, "the report needs a SCANNED line for each scanner it relies on: SCANNED | <scanner> <version> | paths: …")
        for n, fields in self.scanned:
            if len(fields) != 3 or not fields[1] or not fields[2].startswith("paths: ") or not fields[2][7:].strip():
                self.bad(n, "SCANNED needs: SCANNED | <scanner> <version> | paths: <paths>")

    def against(self, rows, coverage):
        by_subject = {r["subject"]: r for r in rows}
        self.entries_against(by_subject)
        self.citations_against(by_subject)
        if coverage is not None:
            self.coverage_against(coverage)

    def entries_against(self, by_subject):
        for subject, (verdict, n) in self.entries.items():
            row = by_subject.get(subject)
            if row is None:
                self.bad(n, f"{subject} is not a row of the triage")
            elif row["final"] and verdict != row["proposed"]:
                self.bad(n, f"{subject}: the gate set {row['proposed']} and nothing may change it, the report says {verdict}")
            elif not row["final"] and verdict not in (row["proposed"], "false-positive"):
                self.bad(n, f"{subject}: a proposed fix may become false-positive and nothing else, the report says {verdict}")
        for subject in by_subject:
            if subject not in self.entries:
                self.bad(1, f"{subject} is a row of the triage and is not judged")

    def citations_against(self, by_subject):
        for subject, (n, fields) in self.lines["FIXED"].items():
            row = by_subject.get(subject)
            m = RESCAN.match(fields[2]) if len(fields) > 2 else None
            if row and m and row["path"] not in m.group(1).split(","):
                self.bad(n, f"FIXED cites a re-scan that does not list {row['path']}")
        for subject, (n, fields) in self.lines["SUPPRESS"].items():
            row = by_subject.get(subject)
            if row and len(fields) > 2 and fields[2] != f"fingerprint {row['fingerprint']}":
                self.bad(n, "SUPPRESS names a fingerprint other than the finding's")

    def coverage_against(self, coverage):
        known = {f"{s['driver']} {s['version']}" for s in coverage.get("scanned") or []}
        for n, fields in self.scanned:
            if len(fields) > 1 and fields[1] not in known:
                self.bad(n, f"SCANNED names '{fields[1]}', which no stored report carries")
        want = sorted(u["id"] for u in coverage.get("uncovered") or [])
        if self.singles["COVERAGE"]:
            n, fields = self.singles["COVERAGE"][0]
            said = fields[1][len("uncovered: "):] if len(fields) > 1 and fields[1].startswith("uncovered: ") else ""
            got = [] if said.strip() == "none" else sorted(x.strip() for x in said.split(",") if x.strip())
            if got != want:
                self.bad(n, f"COVERAGE lists {', '.join(got) or 'none'}; the stored coverage leaves {', '.join(want) or 'none'} uncovered")


def cmd_check_report(args):
    try:
        with open(args.report, encoding="utf-8") as handle:
            lines = handle.read().splitlines()
    except (OSError, UnicodeDecodeError) as err:
        eprint(f"logos-sec-review: cannot read {args.report}: {err}")
        return 3
    report = Report()
    for n, raw in enumerate(lines, 1):
        report.line(n, raw.strip())
    report.judge()
    if args.triage:
        report.against(load(args.triage).get("entries") or [], load(args.coverage) if args.coverage else None)
        held = "held to the triage; " + (
            "held to the coverage" if args.coverage else "no coverage given, so the scanners and categories are not compared"
        )
    else:
        held = "no triage given, so the verdicts are not compared with the gate"
    for p in report.problems:
        print(p)
    eprint(
        f"denominator: {len(lines)} report line(s), {len(report.entries)} ENTRY, {len(report.lines['FIXED'])} FIXED, "
        f"{len(report.lines['UNPROVEN'])} UNPROVEN; {len(report.problems)} problem(s); {held}"
    )
    return 1 if report.problems else 0


# ── the command line ─────────────────────────────────────────────────────────


def build():
    p = argparse.ArgumentParser(prog="secrev.py")
    sub = p.add_subparsers(dest="cmd", required=True)

    def add(name, fn, **flags):
        s = sub.add_parser(name)
        for flag, kw in flags.items():
            s.add_argument("--" + flag.replace("_", "-"), **kw)
        s.set_defaults(fn=fn)

    req = {"required": True}
    add("extract", cmd_extract, review=req, out=req)
    add("triage", cmd_triage, delta=req, out=req)
    add("files", cmd_files, triage=req, entry={"action": "append", "required": True})
    add("plan", cmd_plan, triage=req, entry={"action": "append", "required": True})
    add(
        "rescan-record",
        cmd_rescan_record,
        triage=req,
        out=req,
        scan={"action": "append", "required": True},
        entry={"action": "append", "required": True},
    )
    add(
        "proof",
        cmd_proof,
        triage=req,
        delta=req,
        rescan=req,
        entry={"action": "append", "required": True},
        logos=req,
        project=req,
    )
    add("coverage", cmd_coverage, findings=req, out=req, paths={"default": ""})
    add("draft", cmd_draft, fingerprint=req, reason=req, expires={"default": ""})
    add("check-report", cmd_check_report, report=req, triage={"default": ""}, coverage={"default": ""})
    return p


def main():
    try:
        args = build().parse_args()
    except SystemExit as err:
        sys.exit(2 if err.code not in (0, None) else 0)
    sys.exit(args.fn(args))


if __name__ == "__main__":
    main()
