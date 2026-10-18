#!/usr/bin/env bash
#
# verify-evidence.sh — decide whether the gate evidence in this worktree proves
# that the tree about to be handed off actually passed its gates, and, when it
# does, write the handoff marker itself.
#
# WHY THIS WRITES THE MARKER
#   The marker is what the sprint coordinator gates its branch merge on. If a
#   session writes the marker and merely *claims* the gates passed, the claim is
#   the thing being trusted — which is the problem. So this script recomputes
#   the tree identity independently, recomputes every gate's verdict from the
#   raw counters rather than trusting the `verdict` field, and only then writes
#   the marker with the tree id embedded. The coordinator re-derives
#   `git rev-parse <branch>^{tree}` before merging and refuses on mismatch, so a
#   hand-written marker carrying a fabricated id does not survive.
#
#   This does not make bypass impossible — a session can edit this script. What
#   it does is make the bypass *visible*: gate.sh records its own content hash in
#   every evidence file, and an edited runner shows up there.
#
# WHAT IT CHECKS, and which failure each check catches
#   - every required gate has an evidence file        (a gate that never ran)
#   - tier is the one the caller asked to certify     (the cheap tier passed off
#                                                     as the whole thing)
#   - every gate verdict is "pass", not "skip"       (a missing tool read as ok)
#   - each unit's verdict RECOMPUTED from counters   (a fabricated verdict field)
#   - finished == expected for every unit            (the 52-of-82 truncation)
#   - started == finished                            (the OOM-kill signature)
#   - all tree_ids equal each other AND the tree      (run full, edit, re-run
#     recomputed right now                            only fast, then commit)
#   - script-suites (S-719): a unit for every          (a suite left out; a red
#     scripts/tests/test-*.sh the tree holds, none      suite; a suite that
#     exited non-zero or counted no passing check       proved nothing)
#
# THE TWO TIERS IT WILL CERTIFY, and why `fast` is not a loophole
#   `--tier full` (the default) is unchanged: every gate in ALL_GATES, each
#   recorded by a full-tier run of gate.sh, all on one tree which is the tree that
#   exists now.
#
#   `--tier fast` certifies what `gate.sh fast` actually runs — clippy, test,
#   arch, public-terms — and NOTHING is relaxed about those four: same
#   recomputation from counters, same tree identity, same refusal of a "skip".
#   (public-terms is required at both tiers, S-716: a term merged after a fast
#   gate is as public as one merged after a full gate. script-suites, S-719, is
#   required at full only: `gate.sh fast` runs it exactly when scripts/ or the
#   plugin tree (plugins/, .claude-plugin/) changed, so fast accepts evidence
#   with or without it and validates it when present.)
#   The fast tier exists because the standing sprint contract runs one full gate
#   on MERGED MAIN at review rather than eight per-branch full gates, and
#   without it a fast-tier session cannot hand off at all. Three properties keep it honest:
#     - the marker records the tier, the gates and the TEST DENOMINATOR, so a
#       merge commit can state what was certified instead of implying "all of it"
#       (gate.sh's fast tier runs tests only over packages touched vs HEAD, which
#       after the session's own commit is the empty set — a pass over 0 binaries);
#     - any gate whose evidence file EXISTS is validated even when this tier does
#       not require it, so "run full, edit, re-run only fast, hand off" still
#       fails on the stale tree id;
#     - it is opt-in per call. Nothing acquires a weaker gate by default.
#
# `set -uo pipefail` without -e: the checks report, they must not abort. bash 3.2.
#
# Usage:
#   bash scripts/verify-evidence.sh --strict
#   bash scripts/verify-evidence.sh --strict --tier fast --write-marker <path>
#   bash scripts/verify-evidence.sh --report            # human-readable, no gate
#
# Exit codes:
#   0  the evidence validates (and the marker was written, if asked)
#   1  the evidence does not validate — nothing was written
#   2  usage error / not a git repository / missing prerequisite

set -uo pipefail

MODE=""
MARKER=""
EXPECT_TIER="full"

while [ $# -gt 0 ]; do
    case "$1" in
        --strict)
            MODE=strict
            shift
            ;;
        --report)
            MODE=report
            shift
            ;;
        --tier)
            EXPECT_TIER="${2:-}"
            shift 2 || shift
            ;;
        --write-marker)
            MARKER="${2:-}"
            shift 2 || shift
            ;;
        *)
            echo "verify-evidence.sh: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

if [ -z "$MODE" ]; then
    echo "usage: bash scripts/verify-evidence.sh --strict [--tier full|fast] [--write-marker <path>]" >&2
    echo "       bash scripts/verify-evidence.sh --report" >&2
    exit 2
fi

case "$EXPECT_TIER" in
    full|fast) ;;
    *)
        echo "verify-evidence.sh: --tier must be \"full\" or \"fast\", got: $EXPECT_TIER" >&2
        exit 2
        ;;
esac

if ! command -v python3 >/dev/null 2>&1; then
    echo "verify-evidence.sh: python3 is required" >&2
    exit 2
fi

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$ROOT" ]; then
    echo "verify-evidence.sh: not inside a git repository" >&2
    exit 2
fi
cd "$ROOT" || exit 2

EVID="$(git rev-parse --path-format=absolute --git-path gate-evidence)"

# Recomputed independently of anything gate.sh recorded. Identical method to
# gate.sh's, deliberately: `git add -A` onto a throwaway index copy yields the
# same tree object that git-commit-and-verify.sh will commit, so the identity is
# invariant across the commit boundary.
idx="$(git rev-parse --path-format=absolute --git-path index)"
tmpidx="$EVID/.verify-index.$$"
mkdir -p "$EVID" || exit 2
if ! cp "$idx" "$tmpidx" 2>/dev/null; then
    echo "verify-evidence.sh: could not copy the index" >&2
    exit 2
fi
GIT_INDEX_FILE="$tmpidx" git add -A >/dev/null 2>&1
TREE_NOW="$(GIT_INDEX_FILE="$tmpidx" git write-tree 2>/dev/null)"
rm -f "$tmpidx"

if [ -z "$TREE_NOW" ]; then
    echo "verify-evidence.sh: could not compute the current tree identity" >&2
    exit 2
fi

VERIFY_OUT="$EVID/.verify-out.$$"
python3 - "$EVID" "$TREE_NOW" "$MODE" "$EXPECT_TIER" >"$VERIFY_OUT" 2>&1 <<'PY'
import glob, json, os, sys

evid, tree_now, mode, expect_tier = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]

# The full tier's contract. A gate absent from disk is a gate that did not run.
ALL_GATES = [
    "clippy", "test", "agents-marker", "deny", "arch", "public-terms",
    "script-suites", "ui-typecheck", "ui-test", "ui-build", "ui-e2e",
]
# What `gate.sh fast` actually runs. Its two UI legs are conditional on web/ui
# having changed (and script-suites, when scripts/ or the plugin tree did), so they are not
# required — but a gate this tier does not require is still VALIDATED when its
# evidence describes THIS tree, so a passing-looking leg cannot be admitted
# unexamined. Evidence describing an
# older tree is history from an earlier run, not a claim about the tree being
# handed off, so it is skipped rather than treated as a failure: the marker will
# name the gates it actually certified.
FAST_GATES = ["clippy", "test", "arch", "public-terms"]

# What a unit counts, per gate, in every message and row that names it: test
# binaries and tests by default. The public-terms gate's one unit counts the
# text files its walk read, and its failures are term hits (gate.sh's
# gate_public_terms). The counters are checked identically; only the words
# differ, and its floor of 1 is the zero-file walk, already reported as such.
DEFAULT_WORDS = {
    "things": "binaries", "short": "bins",
    "killed": "a binary was killed mid-run",
    "failures": "test(s) failed",
    "nothing_ran": "no test binary executed at all",
    "floor": "the tests/*.rs floor",
    "row_field": "passed", "row_extra": "%d passed",
}
UNIT_WORDS = {
    # The agents-marker gate's one unit counts the gated tests scripts/agents-marker.sh
    # names (expected) against those that printed a result line; `passed` is the
    # whole filtered run, for the reader.
    "agents-marker": {
        "things": "gated tests", "short": "gated",
        "killed": "a gated test started and never reported",
        "failures": "test(s) failed",
        "nothing_ran": "the agents-gated test did not execute",
        "floor": None,
        "row_field": "passed", "row_extra": "%d passed",
    },
    # The script-suites gate has one unit per scripts/tests/test-*.sh: a suite
    # that ran to an exit is 1 of 1, and `passed`/`failed` are the checks it
    # counted (gate.sh's gate_script_suites).
    "script-suites": {
        "things": "suites", "short": "suites",
        "killed": "the suite was killed by a signal",
        "failures": "check(s) failed",
        "nothing_ran": "the suite did not run",
        "floor": None,
        "row_field": "passed", "row_extra": "%d checks passed",
    },
    "public-terms": {
        "things": "text files", "short": "files",
        "killed": "the scan was killed mid-walk",
        "failures": "term hit(s)",
        "nothing_ran": "the walk read zero text files",
        "floor": None,
        "row_field": "failed", "row_extra": "%d hits",
    },
}

REQUIRED = FAST_GATES if expect_tier == "fast" else ALL_GATES
OPTIONAL = [g for g in ALL_GATES if g not in REQUIRED]
# A stronger tier than the one asked for is never a problem; a weaker one is.
ALLOWED_TIERS = {"full"} if expect_tier == "full" else {"fast", "full"}

problems = []
seen_trees = {}
rows = []
gates_read = []
tests_finished = 0
tests_expected = 0

for gate in REQUIRED + OPTIONAL:
    path = os.path.join(evid, gate + ".json")
    if not os.path.exists(path):
        if gate in REQUIRED:
            problems.append("%s: no evidence file (the gate did not run)" % gate)
            rows.append((gate, "-", "MISSING", "-", "-"))
        continue
    try:
        with open(path) as fh:
            doc = json.load(fh)
    except (OSError, ValueError) as exc:
        problems.append("%s: evidence file is unreadable (%s)" % (gate, exc))
        rows.append((gate, "-", "UNREADABLE", "-", "-"))
        continue

    if gate not in REQUIRED and doc.get("tree_id") != tree_now:
        continue  # history from an earlier tree; not a claim about this one
    gates_read.append(gate)

    if doc.get("schema") != 1:
        problems.append("%s: unknown evidence schema %r" % (gate, doc.get("schema")))

    tier = doc.get("tier")
    if tier not in ALLOWED_TIERS:
        if expect_tier == "full":
            problems.append(
                "%s: tier is %r, not \"full\" — the fast tier is never sufficient "
                "for a full-tier handoff" % (gate, tier)
            )
        else:
            problems.append(
                "%s: tier is %r, which is neither \"fast\" nor \"full\"" % (gate, tier)
            )

    seen_trees.setdefault(doc.get("tree_id"), []).append(gate)

    verdict = doc.get("verdict")
    if verdict == "skip":
        problems.append(
            "%s: skipped (%s) — a gate that did not run is not a gate that passed"
            % (gate, doc.get("argv"))
        )
    elif verdict != "pass":
        problems.append("%s: verdict %r, exit %r, truncation %r"
                        % (gate, verdict, doc.get("exit_code"), doc.get("truncation")))

    if verdict == "pass" and doc.get("exit_code") not in (0, None):
        problems.append("%s: verdict \"pass\" but exit_code %r"
                        % (gate, doc.get("exit_code")))

    # Recompute every unit's verdict from its own counters. The `verdict` field
    # is the one value an agent must not be able to assert into existence.
    units = doc.get("units") or []
    words = UNIT_WORDS.get(gate, DEFAULT_WORDS)
    for u in units:
        name = "%s/%s" % (gate, u.get("unit"))
        started = u.get("started", 0)
        finished = u.get("finished", 0)
        expected = u.get("expected", 0)
        failed = u.get("failed", 0)
        floor = u.get("floor", 0)

        if started == 0:
            problems.append("%s: %s" % (name, words["nothing_ran"]))
        elif started > finished:
            problems.append(
                "%s: %d %s started, only %d finished — %s"
                % (name, started, words["things"], finished, words["killed"])
            )
        elif finished < expected:
            problems.append(
                "%s: %d of %d %s ran — the run was truncated"
                % (name, finished, expected, words["things"])
            )
        if failed:
            problems.append("%s: %d %s" % (name, failed, words["failures"]))
        if words["floor"] and expected < floor:
            problems.append(
                "%s: expected %d is below %s of %d — the count "
                "is wrong, not the suite" % (name, expected, words["floor"], floor)
            )
        if u.get("verdict") == "pass" and (
            started == 0 or started > finished or finished < expected or failed
        ):
            problems.append("%s: recorded \"pass\" contradicts its own counters" % name)

    if units:
        tot_exp = sum(u.get("expected", 0) for u in units)
        tot_fin = sum(u.get("finished", 0) for u in units)
        tot_extra = sum(u.get(words["row_field"], 0) for u in units)
        rows.append((gate, tier, verdict,
                     "%d/%d %s" % (tot_fin, tot_exp, words["short"]),
                     words["row_extra"] % tot_extra))
    else:
        rows.append((gate, tier, verdict, "exit %s" % doc.get("exit_code"), ""))

    # The script suites (S-719): one unit per suite the tree holds. A unit's exit
    # status is checked on its own — the generic counters above see `failed`,
    # which a suite that exited non-zero without counting a failure would leave
    # at zero.
    if gate == "script-suites" and doc.get("tree_id") == tree_now:
        if verdict == "pass" and not units:
            problems.append("script-suites: passed with no suite unit — nothing was run")
        ran = set()
        for u in units:
            ran.add(u.get("unit"))
            if u.get("exit_code") != 0:
                problems.append("script-suites/%s: the suite exited %r"
                                % (u.get("unit"), u.get("exit_code")))
            elif u.get("verdict") == "pass" and not u.get("passed"):
                problems.append("script-suites/%s: the suite counted no passing check "
                                "— a suite that proved nothing is not a pass" % u.get("unit"))
        for suite in sorted(glob.glob(os.path.join("scripts", "tests", "test-*.sh"))):
            name = os.path.basename(suite)[:-len(".sh")]
            if name not in ran:
                problems.append("script-suites/%s: the tree holds this suite "
                                "but the evidence has no unit for it" % name)

    if gate == "test":
        # The denominator, carried into the marker. A test gate with no units is
        # a pass over ZERO binaries; the marker must say so rather than let a
        # reader infer a suite ran.
        tests_expected += sum(u.get("expected", 0) for u in units)
        tests_finished += sum(u.get("finished", 0) for u in units)

# One tree, and it must be the tree that exists right now. Requiring the gates to
# agree with EACH OTHER is what closes "run full, edit, re-run only the fast
# tier": a stale gate's id no longer matches its siblings.
real_trees = [t for t in seen_trees if t]
if len(real_trees) > 1:
    problems.append(
        "gates disagree about the tree they measured: "
        + "; ".join("%s <- %s" % (t[:12], ",".join(g)) for t, g in seen_trees.items() if t)
    )
for t, gates in seen_trees.items():
    if t and t != tree_now:
        problems.append(
            "stale evidence: %s measured tree %s but the tree is now %s"
            % (",".join(gates), t[:12], tree_now[:12])
        )

width = max([len(r[0]) for r in rows] + [6])
print("evidence in %s" % evid)
print("tree now    %s" % tree_now)
print()
for gate, tier, verdict, detail, extra in rows:
    print("  %-*s  %-5s  %-10s  %-14s %s" % (width, gate, tier, verdict, detail, extra))
print()

if problems:
    print("EVIDENCE REJECTED — %d problem(s):" % len(problems))
    for p in problems:
        print("  - %s" % p)
    sys.exit(1)

print("EVIDENCE VERIFIED for tree %s (tier %s, %d/%d test binaries)"
      % (tree_now, expect_tier, tests_finished, tests_expected))
print("MARKER-META tier=%s gates=%s tests=%d/%d"
      % (expect_tier, ",".join(gates_read), tests_finished, tests_expected))
sys.exit(0)
PY

rc=$?
# Printed, not piped: the exit status above is the verifier's own.
grep -v '^MARKER-META ' "$VERIFY_OUT"
META="$(sed -n 's/^MARKER-META //p' "$VERIFY_OUT" | head -1)"
rm -f "$VERIFY_OUT"

if [ "$MODE" = "report" ]; then
    exit $rc
fi

if [ $rc -ne 0 ]; then
    echo
    echo "verify-evidence.sh: refusing to certify this tree." >&2
    echo "  Run: bash scripts/gate.sh $EXPECT_TIER" >&2
    exit 1
fi

if [ -n "$MARKER" ]; then
    marker_dir="$(dirname "$MARKER")"
    if [ ! -d "$marker_dir" ]; then
        echo "verify-evidence.sh: marker directory does not exist: $marker_dir" >&2
        exit 2
    fi
    {
        echo "REVIEW-COMPLETE"
        echo "EVIDENCE-VERIFIED tree=$TREE_NOW"
        [ -n "$META" ] && echo "EVIDENCE-SCOPE $META"
    } >"$MARKER" || exit 2
    echo "marker written: $MARKER (tree=$TREE_NOW, $META)"
fi

exit 0
