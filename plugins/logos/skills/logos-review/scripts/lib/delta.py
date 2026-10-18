#!/usr/bin/env python3
"""The deterministic half of the logos-review skill (FR-IN-15).

Every judgement the skill makes is the model's; everything with a binary outcome
is here: flattening the branch delta into entries, the coverage lookup, the trust
gate, the re-measurement, the worktree's dirty set and the report check. The shell
scripts one directory up are thin wrappers that select a subcommand.

Entries come from `logos review --json` (branch mode) or from `logos improvements
--json` (debt mode, `debt` below). Both are flattened into one shape, so the gate
and the re-measurement do not care which mode produced them:

    id, key, kind, dimension, rule, status, symbol, name, file, line, measure,
    before, after, threshold, detail, trust

Standard library only. Exit codes: 0 good answer, 1 the other answer, 2 usage,
3 environment or unreadable input. Each subcommand ends with one `denominator:`
line on stderr.
"""

import argparse
import json
import os
import re
import subprocess
import sys
from collections import Counter

VERDICTS = ("fix", "test-first", "no-action-warranted", "untrusted")
# The trust label of an entry the review may act on. Every other label is
# untrusted: `stale`, `same-file-only`, `no-resolved-edges`, and a label this
# helper has never seen.
TRUSTED = "resolved"
# What `remeasure` accepts as evidence that an entry no longer offends.
PROVEN = ("resolved", "improved")
# Dimensions whose offender names a cycle of modules or a chain of directories,
# not one symbol: there is nothing to edit as a unit.
STRUCTURAL = ("acyclicity", "depth")
# `.logos/` is the store, written by every logos call, except the files that are
# the quality contract: an edit to one of those is never invisible.
CONTRACT_FILES = ("rules.toml", "rules-baseline.json", "security-ignore.toml")


def eprint(*parts):
    print(*parts, file=sys.stderr)


def plural(n):
    return f"{n} {'entry' if n == 1 else 'entries'}"


def load(path):
    try:
        with open(path, encoding="utf-8") as handle:
            doc = json.load(handle)
    except (OSError, ValueError) as err:
        eprint(f"logos-review: cannot read {path}: {err}")
        sys.exit(3)
    if not isinstance(doc, dict):
        eprint(f"logos-review: {path} is not a JSON object")
        sys.exit(3)
    return doc


def dump(doc, out):
    """Write JSON beside its target and rename it over, so a reader never sees a
    half-written file and a failure leaves the previous one in place."""
    tmp = f"{out}.tmp.{os.getpid()}"
    try:
        with open(tmp, "w", encoding="utf-8") as handle:
            handle.write(json.dumps(doc, indent=2, ensure_ascii=False) + "\n")
        os.replace(tmp, out)
    finally:
        if os.path.exists(tmp):
            os.remove(tmp)


def where(e):
    """`file:line`, or the file alone, or `-`: how a report names an entry."""
    if e["file"] and e["line"] is not None:
        return f"{e['file']}:{e['line']}"
    return e["file"] or "-"


def subject_of(e):
    """The subject a report line names an entry by."""
    return f"{where(e)} {e['name']}"


# ── entries ──────────────────────────────────────────────────────────────────


def make_entry(kind, **fields):
    entry = dict(
        kind=kind,
        dimension=None,
        rule=None,
        status=None,
        symbol=None,
        name="",
        file="",
        line=None,
        measure=None,
        before=None,
        after=None,
        threshold=None,
        detail="",
        trust="",
    )
    entry.update(fields)
    return entry


def entry_key(e):
    """The identity an entry keeps across two reviews: what it is, what it
    counts, and the symbol (or the name at its file, where there is none)."""
    who = e["symbol"] or f"{e['name']}@{e['file']}"
    return "|".join([e["kind"], e["dimension"] or e["rule"] or "", e["measure"] or "", who])


def offender_entry(o):
    dimension = o.get("dimension")
    return make_entry(
        "offender",
        dimension=dimension,
        status=o.get("status"),
        symbol=None if dimension in STRUCTURAL else o.get("node_id"),
        name=o.get("name", ""),
        file=o.get("file") or "",
        line=o.get("line"),
        measure=o.get("measure"),
        before=o.get("before"),
        after=o.get("after"),
        threshold=o.get("threshold"),
        detail=o.get("detail", ""),
        trust=o.get("trust", ""),
    )


def violation_entry(v):
    subject = v.get("subject") or ""
    # A symbol id carries its file module in backticks. A file, a module key or an
    # empty subject names no single symbol.
    return make_entry(
        "violation",
        rule=v.get("rule"),
        status=v.get("status"),
        symbol=subject if "`" in subject else None,
        name=subject or v.get("rule", ""),
        file=v.get("file") or "",
        before=v.get("before"),
        after=v.get("after"),
        threshold=v.get("threshold"),
        detail=v.get("message", ""),
        trust=v.get("trust", ""),
    )


def clone_entries(group):
    """The members a branch made duplicates, and how many were duplicates before."""
    entries, inherited = [], 0
    for m in group.get("members") or []:
        if not m.get("newly_duplicated"):
            inherited += 1
            continue
        entries.append(
            make_entry(
                "clone",
                dimension="uniqueness",
                status="new",
                symbol=m.get("node_id"),
                name=m.get("name", ""),
                file=m.get("file") or "",
                line=m.get("line"),
                detail=f"member of an introduced near-clone group of {group.get('size')}",
                trust=m.get("trust", ""),
            )
        )
    return entries, inherited


def flatten_review(doc):
    """The listed entries of a `logos review --json` document, and the number of
    clone-group members that were already duplicates before the branch."""
    entries = [offender_entry(o) for o in doc.get("offenders") or []]
    entries += [violation_entry(v) for v in doc.get("violations") or []]
    inherited = 0
    for group in doc.get("introduced_clone_groups") or []:
        members, skipped = clone_entries(group)
        entries += members
        inherited += skipped
    return entries, inherited


def entries_of(doc):
    """Entries with their ids, from a review document or a debt document."""
    if doc.get("mode") == "debt":
        entries = [make_entry(**e) for e in doc.get("entries") or []]
    else:
        entries, _ = flatten_review(doc)
    for n, e in enumerate(entries, 1):
        e["key"] = entry_key(e)
        e["id"] = f"e{n}"
    return entries


def counted(doc):
    """(entries counted, of them unchanged) from the review's own counts."""
    total = unchanged = 0
    for field in ("offender_counts", "violation_counts"):
        c = doc.get(field) or {}
        total += sum(int(c.get(k, 0)) for k in ("new", "worsened", "improved", "resolved", "unchanged"))
        unchanged += int(c.get("unchanged", 0))
    return total, unchanged


def cmd_summary(args):
    doc = load(args.delta)
    if not doc.get("measured"):
        print(f"not measured: {doc.get('not_measured_because') or 'no reason given'}")
        eprint("denominator: 0 entries classified (the review measured nothing)")
        return 1
    entries, inherited = flatten_review(doc)
    total, unchanged = counted(doc)
    by_kind = Counter(e["kind"] for e in entries)
    stale = len(doc.get("stale_files") or [])
    print(
        f"measured: base {doc.get('base')} merge-base {doc.get('merge_base')} "
        f"before-state {doc.get('before_state')}; {plural(len(entries))} to judge "
        f"(offenders {by_kind['offender']}, violations {by_kind['violation']}, "
        f"clone members {by_kind['clone']}); stale files {stale}"
    )
    eprint(
        f"denominator: {len(entries)} listed of {total} offender and violation entries counted "
        f"({unchanged} unchanged, not listed); {inherited} clone-group member(s) already cloned before the branch, not listed"
    )
    return 0


# ── coverage lookup ──────────────────────────────────────────────────────────


def logos_json(binary, project, argv):
    """One logos call that answers JSON, or None. stderr is discarded: the
    store's own notices are not this script's to relay."""
    proc = subprocess.run(
        [binary, *argv], cwd=project, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False
    )
    try:
        return json.loads(proc.stdout.decode("utf-8", "replace"))
    except ValueError:
        return None


def classify_reach(e, reach, imp):
    """(coverage, note, tests) for an entry whose file has test-marked dependents
    `reach`, given the `impact` answer for its symbol."""
    resolved = (imp or {}).get("resolved")
    if not resolved:
        return "unknown", "the symbol does not resolve in the graph", []
    if resolved.get("file") != e["file"]:
        # A bare name met a different node: never credit its tests.
        return "unknown", f"the name resolves to {resolved.get('file')}, not {e['file']}", []
    upstream = {u["file"] for u in imp.get("upstream") or [] if u.get("file")}
    hit = sorted(upstream & reach)
    if hit:
        return "covered", f"reached from {len(hit)} test file(s)", hit[:5]
    return "file-only", f"{len(reach)} test file(s) depend on the file, none reaches this symbol", []


def bare_name(symbol):
    """The identifier a symbol id ends in: `deep` for ``src/`lib.rs`/deep().``,
    `bar` for `pkg/Foo#bar().`, `new` for the second `new` of a file,
    ``src/`lib.rs`/new(1).`` (the `(N)` disambiguator is not part of the name).
    None when it ends in none."""
    m = re.search(r"([A-Za-z_$][\w$]*)(?:\(\d*\))?[^\w$]*$", symbol or "")
    return m.group(1) if m else None


def own_node(imp, symbol):
    """The node of an `impact` answer — the resolved one or an alternative — that
    is the entry's own symbol: a full symbol ends in its coordinate-less id, after
    the space that closes the coordinate. None when no node is."""
    imp = imp or {}
    for node in [imp.get("resolved")] + list(imp.get("alternatives") or []):
        if node and (node.get("symbol") or "").endswith(" " + symbol):
            return node
    return None


def lookup_coverage(e, tests_for, impact_of):
    """(coverage, note, tests) of one entry; the two graph reads are passed in.

    The entry's symbol is the id without its repository coordinate (`node_id`),
    which `logos impact` does not resolve. So the id is asked first, and when it
    does not resolve, the bare name it ends in. A name meets the first symbol of
    that name, which can be a sibling in the same file (`A::new` for `B::new`), so
    its answer is credited only for the entry's own node: the resolved one, or an
    alternative asked again by its full symbol. Any other answer reads `unknown`."""
    if not e["symbol"]:
        return "no-symbol", "the entry names no single symbol", []
    if not e["file"]:
        return "unknown", "the entry names no file", []
    reach = tests_for(e["file"])
    if reach is None:
        return "unknown", "the graph does not know the file", []
    if not reach:
        return "uncovered", "no test file depends on the file", []
    imp = impact_of(e["symbol"])
    name = bare_name(e["symbol"])
    if not (imp or {}).get("resolved") and name and name != e["symbol"]:
        imp = impact_of(name)
        if (imp or {}).get("resolved"):
            own = own_node(imp, e["symbol"])
            if own is None:
                return "unknown", f"the name '{name}' resolves to another symbol, never this one", []
            if own is not imp["resolved"]:
                imp = impact_of(own["symbol"])
    return classify_reach(e, reach, imp)


def cmd_coverage(args):
    entries = entries_of(load(args.delta))
    tests_of_file = {}  # file -> set of test files that reach it, or None when the graph does not know it
    calls = 0

    def tests_for(file):
        nonlocal calls
        if file not in tests_of_file:
            calls += 1
            answer = logos_json(args.logos, args.project, ["affected", "--tests-only", "--json", file])
            if answer is None or file in (answer.get("unknown") or []):
                tests_of_file[file] = None
            else:
                tests_of_file[file] = {a["file"] for a in answer.get("affected") or [] if a.get("file")}
        return tests_of_file[file]

    def impact_of(symbol):
        nonlocal calls
        calls += 1
        return logos_json(args.logos, args.project, ["impact", symbol, "--depth", str(args.depth), "--json"])

    result = []
    tally = Counter()
    for e in entries:
        cov, note, tests = lookup_coverage(e, tests_for, impact_of)
        tally[cov] += 1
        result.append(dict(id=e["id"], key=e["key"], coverage=cov, tests=tests, note=note))
    dump({"coverage": result}, args.out)
    parts = ", ".join(f"{k} {v}" for k, v in sorted(tally.items())) or "none"
    eprint(
        f"denominator: {len(entries)} entries, {sum(1 for e in entries if e['symbol'])} with a symbol, "
        f"{calls} logos call(s); coverage: {parts}. Reach is by test-marked FILES: a unit test inside "
        f"the offender's own file is not seen."
    )
    return 0


# ── trust gate ───────────────────────────────────────────────────────────────


def gate(e, cov):
    """(proposed verdict, final?, reason). `final` is False only for a proposed
    `fix`: the model may downgrade it to no-action-warranted, never upgrade
    anything else."""
    trust = e.get("trust") or ""
    if trust != TRUSTED:
        return "untrusted", True, f"trust label '{trust or 'missing'}': reported, never edited"
    if e["status"] in PROVEN:
        return "no-action-warranted", True, f"the branch already {e['status']} it"
    if not e["symbol"]:
        return (
            "no-action-warranted",
            True,
            "names no single symbol (a chain, a cycle or a file-level rule): nothing to edit as one unit",
        )
    if cov != "covered":
        return "test-first", True, f"no test reach ({cov}): a characterisation test comes before any edit"
    return "fix", False, "trusted and covered by tests; confirm it is worth the change"


def coverage_of(e, cov_by_id):
    """The coverage answer for an entry, only when it was computed for this very
    entry: a coverage file from an earlier run must not land on another symbol."""
    answer = cov_by_id.get(e["id"])
    return answer["coverage"] if answer and answer.get("key") == e["key"] else "unknown"


def cmd_triage(args):
    entries = entries_of(load(args.delta))
    cov_by_id = {c["id"]: c for c in load(args.coverage).get("coverage") or []}
    out = []
    tally = Counter()
    for e in entries:
        cov = coverage_of(e, cov_by_id)
        proposed, final, reason = gate(e, cov)
        tally[proposed] += 1
        out.append(dict(e, coverage=cov, proposed=proposed, final=final, reason=reason))
    dump({"entries": out}, args.out)
    for e in out:
        print(
            f"{e['id']}\t{e['proposed']}{'' if e['final'] else '?'}\t{e['status']}\t"
            f"{e['dimension'] or e['rule'] or '-'}\t{e['name']}\t{where(e)}\t{e['coverage']}\t{e['trust'] or '-'}\t{e['reason']}"
        )
    parts = ", ".join(f"{v} {k}" for k, v in sorted(tally.items())) or "none"
    eprint(f"denominator: {len(out)} entries gated: {parts}")
    return 0


# ── re-measurement ───────────────────────────────────────────────────────────


def shown(value):
    """A value as a report prints it: a missing one is gone, never 0 or null."""
    return "gone" if value is None else value


def same_unit(a, b):
    """Whether two entries are one offender: by key, or, for a debt unit (never
    keyed like a review entry), by name, file and dimension against an offender."""
    if "debt" not in (a["kind"], b["kind"]):
        return a["key"] == b["key"]
    other = b if a["kind"] == "debt" else a
    return (
        other["kind"] == "offender"
        and a["name"] == b["name"]
        and a["file"] == b["file"]
        and (a["dimension"] is None or b["dimension"] is None or a["dimension"] == b["dimension"])
    )


def find_after(b, after):
    """The entry of the re-run that is `b`, or None."""
    return next((e for e in after if e["kind"] != "debt" and same_unit(b, e)), None)


def moved_elsewhere(b, after):
    """Entries of the re-run on the same symbol and dimension as `b` that are
    still offending (a Focus container that now crosses on its span reads as one
    entry gone and another new)."""
    return [
        a
        for a in after
        if a["symbol"] and a["symbol"] == b["symbol"] and a["dimension"] == b["dimension"] and a["status"] in ("new", "worsened")
    ]


def remeasured(b, a, stale):
    """(proven?, line) for the entry `b` of the triage, given its match `a` in the
    re-run (or None) and the re-run's stale files."""
    label = f"{b['id']} {b['name']} ({where(b)})"
    if a is not None and a["trust"] != TRUSTED:
        return False, f"not proven: {label}: the re-run labels it '{a['trust']}', so it was not measured as edited; sync the file and re-run"
    if a is not None and a["status"] in PROVEN:
        return True, (
            f"proven: {label}: {a['status']}, {b['measure'] or 'value'} {shown(b['after'])} -> {shown(a['after'])} "
            f"(merge-base {shown(a['before'])})"
        )
    if a is not None:
        return False, f"not proven: {label}: still {a['status']}, {b['measure'] or 'value'} {shown(b['after'])} -> {shown(a['after'])}"
    if b["file"] in stale:
        return False, f"not proven: {label}: its file is still stale; sync it and re-run"
    if b["status"] == "new":
        # An entry the branch added and then fixed is in neither state of the
        # delta: the merge-base never had it, so the delta cannot say `resolved`.
        return True, f"cleared: {label}: no longer listed; it was new on this branch, so the merge-base had no entry to resolve"
    return False, f"not proven: {label}: no longer listed but not resolved, so it is back at its merge-base value (unchanged)"


def judged(b, after, stale):
    """(proven?, line) for one asked entry, including the check that a cleared
    entry did not simply move to another measure."""
    ok, line = remeasured(b, find_after(b, after), stale)
    again = moved_elsewhere(b, after) if line.startswith("cleared") else []
    if again:
        return False, f"not proven: {b['id']} {b['name']}: the symbol is still listed under another measure ({again[0]['measure']})"
    return ok, line


def unexplained(before, after, asked):
    """Entries the re-run lists as new or worsened that the triage never had and
    that are not another face of an entry that was asked about: what the edit
    added."""
    return [
        a
        for a in after
        if a["status"] in ("new", "worsened")
        and not any(same_unit(b, a) for b in before)
        and not any(moved_elsewhere(b, [a]) for b in asked)
    ]


def cmd_remeasure(args):
    before = load(args.triage).get("entries") or []
    by_id = {e["id"]: e for e in before}
    after_doc = load(args.after)
    if not after_doc.get("measured"):
        print(f"not proven: the re-run measured nothing ({after_doc.get('not_measured_because') or 'no reason given'})")
        eprint(f"denominator: 0 of {len(args.entry)} entries re-measured")
        return 1
    after = entries_of(after_doc)
    stale = set(after_doc.get("stale_files") or [])
    asked = [by_id[i] for i in args.entry if i in by_id]
    if len(asked) != len(args.entry):
        eprint(f"logos-review: an --entry is not in {args.triage}")
        return 2
    bad = 0
    for b in asked:
        ok, line = judged(b, after, stale)
        print(line)
        bad += 0 if ok else 1
    # A debt triage is the where-to-start list, not the delta: it cannot say what
    # else the branch added, so the check applies to a branch triage only.
    extra = [] if any(b["kind"] == "debt" for b in before) else unexplained(before, after, asked)
    for a in extra:
        print(f"not proven: the re-run lists an offender the triage did not have: {subject_of(a)} ({a['status']}); the edit may have moved the offence")
    eprint(f"denominator: {plural(len(asked))} re-measured against {len(after)} listed in the re-run; {len(extra)} unexplained")
    return 1 if bad or extra else 0


def cmd_files(args):
    """The files of the named triage entries, one per line, without repeats."""
    by_id = {e["id"]: e for e in load(args.triage).get("entries") or []}
    if any(i not in by_id for i in args.entry):
        eprint(f"logos-review: an --entry is not in {args.triage}")
        return 2
    files = []
    for i in args.entry:
        f = by_id[i]["file"]
        if f and f not in files:
            files.append(f)
    for f in files:
        print(f)
    eprint(f"denominator: {plural(len(args.entry))}, {len(files)} file(s)")
    return 0


# ── the worktree's dirty set ─────────────────────────────────────────────────


def git_out(project, *argv):
    proc = subprocess.run(["git", "-C", project, *argv], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False)
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def kept(path):
    """Whether a dirty path counts: the store is excluded, except the quality
    contract inside it."""
    return not path.startswith(".logos/") or path.rsplit("/", 1)[-1] in CONTRACT_FILES


def dirty_paths(project):
    """Project-relative paths that are changed, added, deleted or untracked, with
    the store excluded (but never the quality contract inside it); None when the
    project is not in a git repository. A rename is a deletion and an addition."""
    rc, prefix = git_out(project, "rev-parse", "--show-prefix")
    if rc != 0:
        return None
    prefix = prefix.strip()
    rc, text = git_out(
        project, "-c", "core.quotePath=false", "status", "--porcelain", "-z", "--no-renames", "--untracked-files=all", "--", "."
    )
    paths = set()
    for rec in text.split("\0") if rc == 0 else []:
        path = rec[3:]
        if prefix and path.startswith(prefix):
            path = path[len(prefix):]
        if len(rec) > 3 and kept(path):
            paths.add(path)
    return sorted(paths)


def content_hash(project, path):
    if not os.path.isfile(os.path.join(project, path)):
        return "deleted"
    rc, out = git_out(project, "hash-object", "--", path)
    return out.strip() if rc == 0 else "unhashable"


def cmd_dirty(args):
    paths = dirty_paths(args.project)
    if paths is None:
        eprint(f"logos-review: {args.project} is not a git repository")
        return 3
    for p in paths:
        print(f"{p}\t{content_hash(args.project, p)}" if args.hash else p)
    eprint(f"denominator: {len(paths)} dirty path(s) of the worktree under {args.project}")
    return 0


# ── report check ─────────────────────────────────────────────────────────────

# A claim about the aggregate quality number, in words or as its scale. The
# subject of a line (a file and a name) is not scanned: a function called `score`
# is not a claim about the signal.
SIGNAL_CLAIM = re.compile(r"\b(signals?|aggregates?|scores?)\b|\b10[,.\s]?000\b", re.IGNORECASE)
RE_ENTRY = re.compile(r"^ENTRY \| (?P<verdict>[a-z-]+) \| (?P<subject>.+?) \| (?P<reason>.*\S)\s*$")
RE_FIXED = re.compile(
    r"^FIXED \| (?P<subject>.+?) \| remeasured: (?P<how>resolved|improved|cleared) (?P<before>\S+) -> (?P<after>\S+) \| "
    r"tests: (?P<cmd>.+?) \| (?P<p>\d+) passed, (?P<f>\d+) failed \(of (?P<t>\d+)\)\s*$"
)
RE_UNPROVEN = re.compile(r"^UNPROVEN \| (?P<subject>.+?) \| (?P<why>.*\S)\s*$")
RE_NO_ENTRIES = re.compile(r"^NO ENTRIES(\s|$)")
GRAMMAR = {
    "ENTRY": ("ENTRY | <verdict> | <file>:<line> <name> | <reason>", RE_ENTRY),
    "FIXED": (
        "FIXED | <subject> | remeasured: resolved|improved|cleared <before> -> <after> | tests: <command> | P passed, F failed (of T)",
        RE_FIXED,
    ),
    "UNPROVEN": ("UNPROVEN | <subject> | <why>", RE_UNPROVEN),
}


class Report:
    """The verdict and outcome lines of a report, and what is wrong with them."""

    def __init__(self):
        self.problems = []
        self.entries = []  # (subject, verdict)
        self.outcomes = {}  # subject -> ["FIXED" | "UNPROVEN", ...]
        self.no_entries = False

    def note(self, n, text):
        self.problems.append(f"line {n}: {text}")

    def line(self, n, line):
        word = line.split(" ", 1)[0].split("|", 1)[0]
        if RE_NO_ENTRIES.match(line):
            self.no_entries = True
            return
        if word not in GRAMMAR:
            self.prose(n, line)
            return
        usage, regex = GRAMMAR[word]
        m = regex.match(line)
        if not m:
            self.note(n, f"malformed {word} line ({usage})")
            self.prose(n, line)
            return
        self.prose(n, line.replace(m["subject"], "", 1))
        getattr(self, f"on_{word.lower()}")(n, m)

    def prose(self, n, text):
        claim = SIGNAL_CLAIM.search(text)
        if claim:
            self.note(n, f"an aggregate-signal claim ('{claim.group(0)}'); a fix is proven by its entry, never by the signal")

    def on_entry(self, n, m):
        if m["verdict"] not in VERDICTS:
            self.note(n, f"'{m['verdict']}' is not a verdict ({', '.join(VERDICTS)})")
            return
        subject = m["subject"].strip()
        if self.verdict_of(subject) is not None:
            self.note(n, f"'{subject}' has two ENTRY lines; an entry gets exactly one verdict")
        self.entries.append((subject, m["verdict"]))

    def on_fixed(self, n, m):
        p, f, t = int(m["p"]), int(m["f"]), int(m["t"])
        if f != 0 or p < 1 or t < 1 or p + f > t:
            self.note(n, f"the test run is not a pass ({p} passed, {f} failed, of {t})")
        self.outcomes.setdefault(m["subject"].strip(), []).append("FIXED")

    def on_unproven(self, n, m):
        self.outcomes.setdefault(m["subject"].strip(), []).append("UNPROVEN")

    def verdict_of(self, subject):
        return next((v for s, v in self.entries if s == subject), None)

    def judge(self):
        if self.no_entries and (self.entries or self.outcomes):
            self.problems.append("the report says NO ENTRIES and lists an entry or claims an edit")
        if not self.no_entries and not self.entries:
            self.problems.append("the report has neither ENTRY lines nor NO ENTRIES")
        for subject, kinds in self.outcomes.items():
            if self.verdict_of(subject) != "fix":
                self.problems.append(f"'{subject}' has an outcome line but its verdict is '{self.verdict_of(subject) or 'absent'}': only a fix is edited")
            if len(kinds) != 1:
                self.problems.append(f"'{subject}' has {len(kinds)} outcome lines; it needs exactly one")
        for subject, verdict in self.entries:
            if verdict == "fix" and subject not in self.outcomes:
                self.problems.append(f"'{subject}' is a fix with no FIXED or UNPROVEN line")

    def against(self, triage):
        """Hold the report to the gate's rows: every row judged, and no verdict the
        gate did not allow (a proposed fix may be downgraded; nothing else moves)."""
        allowed = {}
        for e in triage:
            allowed.setdefault(subject_of(e), set()).update({e["proposed"]} if e["final"] else {"fix", "no-action-warranted"})
        for subject, verdict in self.entries:
            if subject not in allowed:
                self.problems.append(f"'{subject}' is not a row of the triage")
            elif verdict not in allowed[subject]:
                self.problems.append(f"'{subject}' is judged '{verdict}' but the gate allows {sorted(allowed[subject])}")
        judged = {s for s, _ in self.entries}
        for subject in allowed:
            if not self.no_entries and subject not in judged:
                self.problems.append(f"the triage row '{subject}' has no ENTRY line")


def cmd_check_report(args):
    try:
        with open(args.report, encoding="utf-8") as handle:
            lines = handle.read().splitlines()
    except (OSError, UnicodeDecodeError) as err:
        eprint(f"logos-review: cannot read {args.report}: {err}")
        return 3
    report = Report()
    for n, raw in enumerate(lines, 1):
        report.line(n, raw.strip())
    report.judge()
    if args.triage:
        report.against(load(args.triage).get("entries") or [])
    for p in report.problems:
        print(p)
    fixed = sum(1 for k in report.outcomes.values() if "FIXED" in k)
    eprint(
        f"denominator: {len(lines)} report line(s), {len(report.entries)} ENTRY, {fixed} FIXED, "
        f"{sum(1 for k in report.outcomes.values() if 'UNPROVEN' in k)} UNPROVEN; {len(report.problems)} problem(s)"
        + ("; held to the triage" if args.triage else "; no triage given, so the verdicts are not compared with the gate")
    )
    return 1 if report.problems else 0


# ── debt mode ────────────────────────────────────────────────────────────────

THIN = ("same-file-only", "no-resolved-edges")


def language_maps(binary, project):
    """(extension -> language, language -> thin cause) from the binary's own
    answers; the cause is set only where the graph binds calls within files only."""
    ext_lang = {}
    for lang in (logos_json(binary, project, ["languages", "--json"]) or {}).get("languages") or []:
        for ext in lang.get("extensions") or []:
            ext_lang[ext.lower()] = lang.get("name")
    thin = {}
    for row in (logos_json(binary, project, ["status", "--json"]) or {}).get("resolution_by_language") or []:
        cause = ((row.get("calls") or {}).get("cross_file_absence") or {}).get("cause")
        if cause in THIN:
            thin[row.get("language")] = cause
    return ext_lang, thin


def debt_trust(file, dirty, ext_lang, thin):
    """A unit has no trust label of its own. A dirty file may not be in the graph
    yet and reads `stale`, the cautious label review uses; a language the graph
    binds within files only reads its cause."""
    if file in dirty:
        return "stale"
    ext = file.rsplit(".", 1)[-1].lower() if "." in file else ""
    return thin.get(ext_lang.get(ext), TRUSTED)


def cmd_debt(args):
    answer = logos_json(args.logos, args.project, ["improvements", "--json"])
    if answer is None:
        eprint("logos-review: `logos improvements --json` did not answer JSON (an older logos has no `improvements`)")
        return 3
    if not answer.get("recorded"):
        print("not recorded: no snapshot carries a where-to-start list; run `logos scan` first")
        eprint("denominator: 0 units (nothing recorded; an empty list here is not 'clean')")
        return 1
    ext_lang, thin = language_maps(args.logos, args.project)
    dirty = set(dirty_paths(args.project) or [])
    entries = []
    for u in answer.get("units") or []:
        file = u.get("file") or ""
        dims = u.get("dimensions") or []
        entries.append(
            make_entry(
                "debt",
                dimension=dims[0] if dims else None,
                status="debt",
                symbol=u.get("name"),
                name=u.get("name", ""),
                file=file,
                line=u.get("line"),
                measure=u.get("kind"),
                detail=f"{u.get('detail', '')}; est. {u.get('points')} point(s)",
                trust=debt_trust(file, dirty, ext_lang, thin),
            )
        )
    dump({"mode": "debt", "basis": answer.get("basis"), "entries": entries}, args.out)
    print(f"debt list: {len(entries)} unit(s), ranked by estimated points recovered")
    eprint(
        f"denominator: {len(entries)} unit(s) of the recorded where-to-start list (at most 10); "
        f"{sum(1 for e in entries if e['trust'] != TRUSTED)} untrusted. Debt entries are named by bare name; "
        f"coverage rejects a name that resolves to another file."
    )
    return 0


# ── main ─────────────────────────────────────────────────────────────────────


def build_parser():
    p = argparse.ArgumentParser(prog="delta.py", description=__doc__.split("\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)

    def add(name, fn, **options):
        s = sub.add_parser(name)
        for flag, kw in options.items():
            s.add_argument("--" + flag.replace("_", "-"), **kw)
        s.set_defaults(fn=fn)

    req = dict(required=True)
    add("summary", cmd_summary, delta=req)
    add("coverage", cmd_coverage, delta=req, logos=req, project=req, out=req, depth=dict(type=int, required=True))
    add("triage", cmd_triage, delta=req, coverage=req, out=req)
    add("remeasure", cmd_remeasure, triage=req, after=req, entry=dict(action="append", required=True))
    add("files", cmd_files, triage=req, entry=dict(action="append", required=True))
    add("dirty", cmd_dirty, project=req, hash=dict(action="store_true"))
    add("check-report", cmd_check_report, report=req, triage=dict())
    add("debt", cmd_debt, logos=req, project=req, out=req)
    return p


def main(argv):
    args = build_parser().parse_args(argv)
    try:
        return args.fn(args)
    except Exception as err:  # an unreadable or unexpected input is exit 3, never the "other answer"
        eprint(f"logos-review: {type(err).__name__}: {err}")
        return 3


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
