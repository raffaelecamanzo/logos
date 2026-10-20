#!/usr/bin/env bash
#
# verify-sync-history.sh — replay pinned commit ranges of a repository, one
# `logos sync` per commit from a seeded linked worktree, and check that the
# incrementally maintained store equals a fresh index of the tip (S-753,
# CR-236, UAT-SY-08).
#
# ON-DEMAND REVIEW CHECK. A real-history replay takes minutes, so no gate.sh
# tier and no workflow calls this script (the default tier carries the minimal
# sync fixtures, FR-SY-10); scripts/tests/test-verify-sync-history.sh asserts
# that. Run it at review when a change touches sync, extraction or binding.
#
# WHAT IT DOES, per range (UAT-SY-08's steps)
#   1. primary   a `--shared` clone of the repository at the base; `logos index`.
#   2. worktree  a linked worktree of the primary at the base, opened with
#                `logos status --json`: its store must not exist before that
#                open, and the open must log the engine's seed line (FR-WT-03).
#                A failed seed falls back to a full index, which also leaves a
#                store behind, so a store alone proves nothing.
#   3. replay    each commit of base..tip in order: check it out in the
#                worktree, then `logos sync` the paths it changed (`git
#                diff-tree -z --no-renames`, so a rename syncs both paths and a
#                deletion syncs the deleted path) — the paths the post-commit
#                hook means to sync in a real session. The hook itself reads
#                `diff-tree` without -z (logos-core/src/hooks/mod.rs), so a
#                path git quotes reaches its `sync` quoted; the replay passes
#                every path as written.
#   4. verify    `logos verify --json` in the worktree: `ok`, zero node, edge
#                and file deltas, empty lost- and extra-edge lists (S-752).
#   5. cold      a second `--shared` clone at the tip, `logos index`, and the
#                two stores' edge sets compared by (kind, source symbol, target
#                symbol) — the key S-752's `verify` uses.
#   A range is EQUIVALENT only when every step ran and both comparisons are
#   empty; otherwise DRIFT, with the drifted edges listed, or FAILED when a step
#   could not run (which is never a pass).
#
# THE ORIGINAL CHECKOUT IS ONLY READ. Every clone is `--shared` (its objects
# come through alternates), every logos command runs inside a clone under the
# work directory, and the work directory may not sit inside any repository. The
# project's `.logos/config.toml` and `rules.toml` are copied from the
# repository's checkout into each clone, unless the clone tracks them (then the
# committed copy rides along with each checkout); its `.logos/logos.db` is
# never opened.
# Git hooks are off in the clones (`core.hooksPath` is an empty directory), so
# no checkout syncs behind the replay's back.
#
# BOUNDED IN TIME. The whole run has one wall-clock budget (--timeout): every
# logos command, from the version probe on, runs under scripts/run-bounded.pl
# with what is left of it, and a command that reaches it is killed and the run
# exits 4 with no verdict. The local git steps between them (clone, checkout,
# diff-tree) and the two readers are not bounded one by one; the budget is
# checked again before the next logos command.
#
# The binary is copied into the work directory first, so a rebuild during the
# run cannot change what is measured.
#
# `set -uo pipefail` without -e: a failing step is a verdict, not an abort.
# bash 3.2.
#
# Usage:
#   bash scripts/verify-sync-history.sh [--logos PATH] [--repo DIR]
#                                       [--range LABEL=BASE..TIP]… [--work DIR]
#                                       [--timeout SECONDS] [--list]
#     --logos    the binary under test (default `logos` on PATH)
#     --repo     the repository whose history is replayed (default the one
#                holding this script); only read
#     --range    a range to replay, repeatable (default the three Sprint 106
#                ranges, see --list); BASE must be an ancestor of TIP and the
#                range must hold no merge commit
#     --work     where the clones and raw outputs go, under ranges/<label>/
#                (default a fresh directory
#                under $TMPDIR, removed afterwards; a given one is kept); must
#                not be inside a git repository
#     --timeout  wall-clock budget for the whole run (default 3600)
#     --list     print the pinned default ranges and exit
#
# Exit codes:
#   0  every range is EQUIVALENT
#   1  at least one range DRIFTED (and none FAILED)
#   2  usage error / missing prerequisite / a range that does not resolve,
#      is not ancestral, is empty or holds a merge / a work directory inside a
#      repository
#   3  a step of at least one range FAILED (a clone, the worktree, index, the
#      seed, a checkout, sync, verify, or an unreadable store): that range has
#      no verdict
#   4  the --timeout budget ran out: no verdict
#   130/143  interrupted (INT/TERM). The logos command then running keeps its
#      own process group (run-bounded.pl) and finishes alone

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BOUNDED="$HERE/scripts/run-bounded.pl"

# The three Sprint 106 session replays of the C1 experiment
# (logos-docs/notes/incremental-sync-edge-loss/): before S-751 they lost 1, 32
# and 171 edges (S-681's one lost edge cancelled by one extra).
PINNED_RANGES="S-681=ff0bc3bd5164082f6d54c4c846496d0e49599bc8..ae7b86226a58af9da90b6fac33d971c531d0080f
S-685=ff0bc3bd5164082f6d54c4c846496d0e49599bc8..5e9a4e469e03c769dcadd773391e61436dffcdd2
S-747=27dc0b5af9447e4a14b78a4dee7d658ea309b005..71252f73f061bc9732f69a30fb520034ef5f8d7a"

# How many drifted edges each comparison prints; the totals are always exact.
SHOW=20

# The first open must log this (logos-core/src/engine.rs, at `info`), which the
# default `warn` filter hides.
SEED_LOG=logos_core::engine=info
SEED_LINE="seeded the worktree store from the primary checkout"

LOGOS_ARG=logos
REPO="$HERE"
RANGES=""
WORK=""
TIMEOUT=3600

usage() {
    sed -n '/^# Usage:/,/^# Exit codes:/p' "$0" | sed 's/^# \{0,1\}//' | sed '$d' >&2
    exit 2
}
die2() { echo "verify-sync-history: $*" >&2; exit 2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --logos) [ $# -ge 2 ] || usage; LOGOS_ARG="$2"; shift 2 ;;
        --repo) [ $# -ge 2 ] || usage; REPO="$2"; shift 2 ;;
        --range) [ $# -ge 2 ] || usage; RANGES="$RANGES$2
"; shift 2 ;;
        --work) [ $# -ge 2 ] || usage; WORK="$2"; shift 2 ;;
        --timeout) [ $# -ge 2 ] || usage; TIMEOUT="$2"; shift 2 ;;
        --list) printf '%s\n' "$PINNED_RANGES"; exit 0 ;;
        -h | --help) usage ;;
        *) echo "verify-sync-history: unknown argument '$1'" >&2; usage ;;
    esac
done
[ -n "$RANGES" ] || RANGES="$PINNED_RANGES"

case "$TIMEOUT" in
    '' | *[!0-9]* | 0*) die2 "--timeout wants a positive number of seconds" ;;
esac
for tool in git python3 perl; do
    command -v "$tool" >/dev/null 2>&1 || die2 "$tool is required"
done
case "$LOGOS_ARG" in
    */*) LOGOS_SRC="$LOGOS_ARG" ;;
    *) LOGOS_SRC="$(command -v "$LOGOS_ARG" 2>/dev/null)" || LOGOS_SRC="" ;;
esac
[ -n "$LOGOS_SRC" ] && [ -f "$LOGOS_SRC" ] && [ -x "$LOGOS_SRC" ] \
    || die2 "no executable logos binary at '$LOGOS_ARG'"

G=(git -c maintenance.auto=false -c gc.auto=0 -c advice.detachedHead=false)
REPO="$(cd "$REPO" 2>/dev/null && pwd -P)" || die2 "--repo is not a directory"
"${G[@]}" -C "$REPO" rev-parse --git-dir >/dev/null 2>&1 || die2 "$REPO is not a git repository"

# ------------------------------------------------------------------ ranges

# Every range is resolved and checked before any work starts, so a typo costs
# nothing. LABELS / BASES / TIPS are parallel arrays.
LABELS=()
BASES=()
TIPS=()
commit_of() { "${G[@]}" -C "$REPO" rev-parse --verify -q "$1^{commit}" 2>/dev/null; }
while IFS= read -r spec; do
    [ -n "$spec" ] || continue
    label="${spec%%=*}"
    span="${spec#*=}"
    case "$spec" in *=*..*) ;; *) die2 "--range wants LABEL=BASE..TIP, got '$spec'" ;; esac
    # A label names a directory under $WORK/ranges: it starts with a letter or
    # digit, so `.` and `..` can never step outside it.
    case "$label" in
        [A-Za-z0-9]*) case "$label" in *[!A-Za-z0-9._-]*) label_bad=1 ;; *) label_bad="" ;; esac ;;
        *) label_bad=1 ;;
    esac
    [ -z "$label_bad" ] || die2 "range label '$label' wants a letter or digit, then letters, digits, '.', '_' or '-'"
    for seen in ${LABELS[@]+"${LABELS[@]}"}; do
        [ "$seen" != "$label" ] || die2 "range label '$label' is given twice"
    done
    base="$(commit_of "${span%%..*}")" || die2 "$label: base '${span%%..*}' is not a commit in $REPO"
    tip="$(commit_of "${span#*..}")" || die2 "$label: tip '${span#*..}' is not a commit in $REPO"
    "${G[@]}" -C "$REPO" merge-base --is-ancestor "$base" "$tip" 2>/dev/null \
        || die2 "$label: base ${base:0:8} is not an ancestor of tip ${tip:0:8}"
    [ "$base" != "$tip" ] || die2 "$label: the range is empty"
    merges="$("${G[@]}" -C "$REPO" rev-list --merges --count "$base..$tip")"
    [ "$merges" = 0 ] || die2 "$label: the range holds $merges merge commit(s); replay needs a linear range"
    LABELS+=("$label")
    BASES+=("$base")
    TIPS+=("$tip")
done <<EOF
$RANGES
EOF

# ------------------------------------------------------------------ work dir

trap 'exit 130' INT
trap 'exit 143' TERM
if [ -z "$WORK" ]; then
    WORK="${TMPDIR:-/tmp}/logos-sync-history.$$"
    trap 'rm -rf "$WORK"' EXIT
fi
mkdir -p "$WORK" || die2 "cannot create $WORK"
WORK="$(cd "$WORK" && pwd -P)"
# Inside a repository, logos would find that repository's root (and store)
# from a clone that failed to materialise; outside one, it cannot.
if "${G[@]}" -C "$WORK" rev-parse --git-dir >/dev/null 2>&1; then
    die2 "the work directory $WORK is inside a git repository"
fi
export GIT_CEILING_DIRECTORIES="$WORK"
mkdir -p "$WORK/bin" "$WORK/no-hooks" || die2 "cannot populate $WORK"
L="$WORK/bin/logos"
/bin/cp "$LOGOS_SRC" "$L" && chmod +x "$L" || die2 "cannot copy $LOGOS_SRC into $WORK"

# The two readers run under `python3 -I` from files, not inline: bash 3.2
# brace-expands an inline script inside "$(…)".
cat >"$WORK/bin/read_verify.py" <<'PY'
"""Print the verdict-bearing fields of a `logos verify --json` report as
`key value` lines, then up to SHOW lost and extra edges. Exit 3 when the
report is not readable."""
import json, sys

path, show = sys.argv[1], int(sys.argv[2])
try:
    with open(path) as fh:
        r = json.load(fh)
    fields = ("ok", "node_delta", "edge_delta", "file_delta",
              "lost_edges_total", "extra_edges_total")
    vals = dict((k, r[k]) for k in fields)
except (OSError, ValueError, KeyError, TypeError) as exc:
    print("unreadable %s" % exc)
    sys.exit(3)
for k in fields:
    v = vals[k]
    print("%s %s" % (k, str(v).lower() if isinstance(v, bool) else v))
for side in ("lost", "extra"):
    for e in (r.get(side + "_edges") or [])[:show]:
        print("edge %s %s %s -> %s | %s -> %s" % (
            side, e.get("kind"), e.get("source_file"), e.get("target_file"),
            e.get("source_symbol"), e.get("target_symbol")))
PY
cat >"$WORK/bin/edge_diff.py" <<'PY'
"""Compare the edge sets of two logos stores, read-only, keyed by (kind,
source symbol, target symbol) — the query GraphStore::symbol_edges runs.
Prints `cold N`, `incremental N`, `lost N` (cold only), `extra N`
(incremental only), then up to SHOW edges of each. `kind` is the stored
EdgeKind discriminant. Exit 3 when a store cannot be read."""
import os, sqlite3, sys, urllib.request

cold, incr, show = sys.argv[1], sys.argv[2], int(sys.argv[3])
Q = ("SELECT e.kind, ss.symbol, ts.symbol, sf.path, tf.path FROM edges e "
     "JOIN nodes sn ON sn.id = e.source JOIN symbols ss ON ss.id = sn.symbol_id "
     "LEFT JOIN files sf ON sf.id = sn.file_id "
     "JOIN nodes tn ON tn.id = e.target JOIN symbols ts ON ts.id = tn.symbol_id "
     "LEFT JOIN files tf ON tf.id = tn.file_id")

def load(path):
    # Escaped: an unescaped `#` or `?` in the path would end the file name, so
    # both stores would open one file, read-write.
    url = urllib.request.pathname2url(os.path.abspath(path))
    con = sqlite3.connect("file:%s?mode=ro" % url, uri=True)
    try:
        return dict(((k, s, t), (sf, tf)) for k, s, t, sf, tf in con.execute(Q))
    finally:
        con.close()

try:
    c, i = load(cold), load(incr)
except sqlite3.Error as exc:
    print("unreadable %s" % exc)
    sys.exit(3)
lost = sorted(k for k in c if k not in i)
extra = sorted(k for k in i if k not in c)
print("cold %d" % len(c))
print("incremental %d" % len(i))
print("lost %d" % len(lost))
print("extra %d" % len(extra))
for side, keys, files in (("lost", lost, c), ("extra", extra, i)):
    for k in keys[:show]:
        sf, tf = files[k]
        print("edge %s kind=%s %s -> %s | %s -> %s" % (side, k[0], sf, tf, k[1], k[2]))
PY

# ------------------------------------------------------------------ steps

DEADLINE=$((SECONDS + TIMEOUT))

# bounded <log> <cmd…>: run under what is left of the budget, stdout+stderr to
# <log>; sets RC. Out of budget: report and exit 4 at once.
bounded() {
    local log="$1" left=$((DEADLINE - SECONDS))
    shift
    [ "$left" -gt 0 ] || out_of_time
    perl "$BOUNDED" "$left" "$@" >"$log" 2>&1
    RC=$?
    [ "$RC" -ne 142 ] || out_of_time
}
# logos_in <log> <dir> <args…>: the copied binary, run inside <dir>, bounded
# (run-bounded.pl has no working-directory option).
logos_in() {
    local log="$1" dir="$2"
    shift 2
    bounded "$log" sh -c 'cd "$1" && shift && exec "$@"' sh "$dir" "$L" "$@"
}
out_of_time() {
    echo
    echo "TIMEOUT: the ${TIMEOUT}s budget ran out${CUR:+ during range $CUR}; no verdict"
    exit 4
}

# seed_cfg <clone>: the project's config and rules, never its store. A file the
# clone tracks is left alone: overwriting it would dirty the checkout and stop
# the next one.
seed_cfg() {
    local f
    mkdir -p "$1/.logos" || return 1
    for f in config.toml rules.toml; do
        [ -f "$REPO/.logos/$f" ] || continue
        "${G[@]}" -C "$1" ls-files --error-unmatch ".logos/$f" >/dev/null 2>&1 && continue
        /bin/cp "$REPO/.logos/$f" "$1/.logos/$f" || return 1
    done
}

# clone_at <dir> <commit>: a hook-free `--shared` clone checked out at commit.
clone_at() {
    rm -rf -- "$1"
    "${G[@]}" clone -q --shared --no-checkout "$REPO" "$1" >/dev/null 2>&1 \
        && "${G[@]}" -C "$1" config core.hooksPath "$WORK/no-hooks" \
        && "${G[@]}" -C "$1" checkout -q --detach "$2" >/dev/null 2>&1 \
        && seed_cfg "$1"
}

# field <file> <key>: the value of a `key value` line a reader printed.
field() { sed -n "s/^$2 //p" "$1"; }

# fail <message> [<log>]: this range has no verdict.
fail() {
    echo "  FAILED: $1"
    [ -z "${2:-}" ] || sed -n '1,8p' "$2" | sed 's/^/          /'
    VERDICT=FAILED
}

logos_in "$WORK/version.log" "$WORK" --version
VERSION="$(head -n 1 "$WORK/version.log")"

CFG=""
for f in config.toml rules.toml; do
    [ ! -f "$REPO/.logos/$f" ] || CFG="${CFG:+$CFG }$f"
done
echo "logos sync history replay"
echo "  binary:  $LOGOS_SRC ($VERSION)"
echo "  repo:    $REPO"
echo "  config:  ${CFG:-none (logos defaults)}"
echo "  budget:  ${TIMEOUT}s"

N_EQ=0
N_DRIFT=0
N_FAIL=0
idx=0
while [ "$idx" -lt "${#LABELS[@]}" ]; do
    CUR="${LABELS[$idx]}"
    BASE="${BASES[$idx]}"
    TIP="${TIPS[$idx]}"
    idx=$((idx + 1))
    D="$WORK/ranges/$CUR"
    rm -rf -- "$D"
    mkdir -p "$D/out"
    O="$D/out"
    VERDICT=""
    echo
    echo "range $CUR  ${BASE:0:8}..${TIP:0:8}"

    # 1. primary at the base
    P="$D/primary"
    if ! clone_at "$P" "$BASE"; then
        fail "cannot clone $REPO at ${BASE:0:8}"
    else
        logos_in "$O/primary-index.log" "$P" index
        [ "$RC" -eq 0 ] || fail "logos index at the base exited $RC" "$O/primary-index.log"
    fi

    # 2. linked worktree, seeded on first open
    W="$D/worktree"
    if [ -z "$VERDICT" ]; then
        if ! "${G[@]}" -C "$P" worktree add -q --detach "$W" "$BASE" >"$O/worktree-add.log" 2>&1 \
            || ! seed_cfg "$W"; then
            fail "cannot add a linked worktree" "$O/worktree-add.log"
        elif [ -e "$W/.logos/logos.db" ]; then
            fail "the worktree had a store before its first open"
        else
            RUST_LOG="$SEED_LOG" logos_in "$O/seed.log" "$W" status --json
            if [ "$RC" -ne 0 ]; then
                fail "logos status (first open) exited $RC" "$O/seed.log"
            elif [ ! -e "$W/.logos/logos.db" ]; then
                fail "the first open did not seed a worktree store"
            elif ! grep -qF "$SEED_LINE" "$O/seed.log"; then
                fail "the first open built a store without seeding it from the primary" "$O/seed.log"
            fi
        fi
    fi

    # 3. one sync per commit, of the paths it changed
    n=0
    paths=0
    if [ -z "$VERDICT" ]; then
        for c in $("${G[@]}" -C "$REPO" rev-list --reverse "$BASE..$TIP"); do
            n=$((n + 1))
            if ! "${G[@]}" -C "$W" checkout -q --detach "$c" >"$O/checkout-$n.log" 2>&1; then
                fail "cannot check out ${c:0:8} (commit $n)" "$O/checkout-$n.log"
                break
            fi
            files=()
            while IFS= read -r -d '' f; do
                files+=("$f")
            done < <("${G[@]}" -C "$W" diff-tree -z --no-renames --no-commit-id --name-only -r "$c")
            [ "${#files[@]}" -gt 0 ] || continue
            paths=$((paths + ${#files[@]}))
            # `--`: a changed path that starts with `-` is a path, not a flag.
            logos_in "$O/sync-$n.log" "$W" sync -- "${files[@]}"
            if [ "$RC" -ne 0 ]; then
                fail "logos sync of commit $n (${c:0:8}, ${#files[@]} paths) exited $RC" "$O/sync-$n.log"
                break
            fi
        done
        [ -n "$VERDICT" ] || echo "  replay:  $n commits, $paths paths synced"
    fi

    # 4. verify in the worktree (exit 1 is its drift verdict, not a failure)
    VERIFY_DRIFT=""
    if [ -z "$VERDICT" ]; then
        bounded "$O/verify.json" sh -c 'cd "$1" && exec "$2" verify --json 2>"$3"' sh "$W" "$L" "$O/verify.err"
        if [ "$RC" -ne 0 ] && [ "$RC" -ne 1 ]; then
            fail "logos verify exited $RC" "$O/verify.err"
        elif ! python3 -I "$WORK/bin/read_verify.py" "$O/verify.json" "$SHOW" >"$O/verify.txt"; then
            fail "logos verify wrote no readable report ($(head -n 1 "$O/verify.txt"))" "$O/verify.err"
        else
            V="$O/verify.txt"
            echo "  verify:  exit $RC, ok $(field "$V" ok); node Δ $(field "$V" node_delta), edge Δ $(field "$V" edge_delta), file Δ $(field "$V" file_delta); lost $(field "$V" lost_edges_total), extra $(field "$V" extra_edges_total)"
            grep '^edge ' "$V" | sed 's/^edge /    /'
            if [ "$RC" -ne 0 ] || [ "$(field "$V" ok)" != true ] || [ "$(field "$V" node_delta)" != 0 ] \
                || [ "$(field "$V" edge_delta)" != 0 ] || [ "$(field "$V" file_delta)" != 0 ] \
                || [ "$(field "$V" lost_edges_total)" != 0 ] || [ "$(field "$V" extra_edges_total)" != 0 ]; then
                VERIFY_DRIFT=1
            fi
        fi
    fi

    # 5. cold index of the tip, edge sets compared
    COLD_DRIFT=""
    if [ -z "$VERDICT" ]; then
        C="$D/cold"
        if ! clone_at "$C" "$TIP"; then
            fail "cannot clone $REPO at ${TIP:0:8}"
        else
            logos_in "$O/cold-index.log" "$C" index
            if [ "$RC" -ne 0 ]; then
                fail "logos index at the tip exited $RC" "$O/cold-index.log"
            elif ! python3 -I "$WORK/bin/edge_diff.py" "$C/.logos/logos.db" "$W/.logos/logos.db" "$SHOW" >"$O/edge-diff.txt"; then
                fail "cannot read the stores ($(head -n 1 "$O/edge-diff.txt"))"
            else
                X="$O/edge-diff.txt"
                echo "  cold:    $(field "$X" cold) edges at the tip, $(field "$X" incremental) incremental; lost $(field "$X" lost), extra $(field "$X" extra)"
                grep '^edge ' "$X" | sed 's/^edge /    /'
                [ "$(field "$X" lost)" = 0 ] && [ "$(field "$X" extra)" = 0 ] || COLD_DRIFT=1
            fi
        fi
    fi

    if [ -z "$VERDICT" ]; then
        if [ -n "$VERIFY_DRIFT$COLD_DRIFT" ]; then VERDICT=DRIFT; else VERDICT=EQUIVALENT; fi
    fi
    case "$VERDICT" in
        EQUIVALENT) N_EQ=$((N_EQ + 1)) ;;
        DRIFT) N_DRIFT=$((N_DRIFT + 1)) ;;
        *) N_FAIL=$((N_FAIL + 1)) ;;
    esac
    echo "  verdict: $VERDICT"
done

echo
echo "ranges ${#LABELS[@]}: equivalent $N_EQ, drift $N_DRIFT, failed $N_FAIL ($((SECONDS))s)"
[ "$N_FAIL" -eq 0 ] || exit 3
[ "$N_DRIFT" -eq 0 ] || exit 1
exit 0
