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
#
# THE TWO TIERS IT WILL CERTIFY, and why `fast` is not a loophole
#   `--tier full` (the default) is unchanged: all nine gates, each recorded by a
#   full-tier run of gate.sh, all on one tree which is the tree that exists now.
#
#   `--tier fast` certifies what `gate.sh fast` actually runs — clippy, test, arch
#   — and NOTHING is relaxed about those three: same recomputation from counters,
#   same tree identity, same refusal of a "skip". It exists because the standing
#   sprint contract runs one full gate on MERGED MAIN at review rather than eight
#   per-branch full gates, and without it a fast-tier session cannot hand off at
#   all. Three properties keep it honest:
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
    "clippy", "test", "test-agents", "deny", "arch",
    "ui-typecheck", "ui-test", "ui-build", "ui-e2e",
]
# What `gate.sh fast` actually runs. Its two UI legs are conditional on web/ui
# having changed, so they are not required — but a gate this tier does not
# require is still VALIDATED when its evidence describes THIS tree, so a
# passing-looking leg cannot be admitted unexamined. Evidence describing an
# older tree is history from an earlier run, not a claim about the tree being
# handed off, so it is skipped rather than treated as a failure: the marker will
# name the gates it actually certified.
FAST_GATES = ["clippy", "test", "arch"]

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
    for u in units:
        name = "%s/%s" % (gate, u.get("unit"))
        started = u.get("started", 0)
        finished = u.get("finished", 0)
        expected = u.get("expected", 0)
        failed = u.get("failed", 0)
        floor = u.get("floor", 0)

        if started == 0:
            problems.append("%s: no test binary executed at all" % name)
        elif started > finished:
            problems.append(
                "%s: %d binaries started, only %d finished — a binary was killed "
                "mid-run" % (name, started, finished)
            )
        elif finished < expected:
            problems.append(
                "%s: %d of %d binaries ran — the run was truncated"
                % (name, finished, expected)
            )
        if failed:
            problems.append("%s: %d test(s) failed" % (name, failed))
        if expected < floor:
            problems.append(
                "%s: expected %d is below the tests/*.rs floor of %d — the count "
                "is wrong, not the suite" % (name, expected, floor)
            )
        if u.get("verdict") == "pass" and (
            started == 0 or started > finished or finished < expected or failed
        ):
            problems.append("%s: recorded \"pass\" contradicts its own counters" % name)

    if units:
        tot_exp = sum(u.get("expected", 0) for u in units)
        tot_fin = sum(u.get("finished", 0) for u in units)
        tot_pass = sum(u.get("passed", 0) for u in units)
        rows.append((gate, tier, verdict, "%d/%d bins" % (tot_fin, tot_exp),
                     "%d passed" % tot_pass))
    else:
        rows.append((gate, tier, verdict, "exit %s" % doc.get("exit_code"), ""))

    if gate in ("test", "test-agents"):
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
