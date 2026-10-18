#!/usr/bin/env bash
#
# review-delta.sh — run the branch delta, `logos review --json`, and keep the
# answer (FR-IN-15, FR-GV-24).
#
#   review-delta.sh --out FILE [--base REF] [--project DIR]
#
# The before-state is the merge-base commit and nothing else: a recorded snapshot
# there, else a shadow index of that tree, cached by commit. The first run on a
# cold cache costs one index of that tree (about 25 s on this repository) and logos
# says so on stderr; later runs reuse it.
#
# Writes the JSON to FILE and prints one summary line. `review` never reconciles
# the graph, so an unsynced edit reads `stale`: run `logos sync <paths>` first.
#
# Exit: 0 measured · 1 not measured (the summary says why) · 2 usage ·
#       3 no python3, no logos 1.20.0 or newer, not enrolled, or FILE unwritable

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"

OUT="" BASE=""
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || rv_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        --base) [ $# -ge 2 ] || rv_die 2 "--base needs a ref"; BASE="$2"; shift 2 ;;
        --project) [ $# -ge 2 ] || rv_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) rv_die 2 "unknown argument '$1' (usage: review-delta.sh --out FILE [--base REF] [--project DIR])" ;;
    esac
done
[ -n "$OUT" ] || rv_die 2 "--out FILE is required"
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac # the review runs in the project: FILE must not move with it
[ -d "${OUT%/*}" ] || rv_die 3 "the directory of $OUT does not exist"

rv_need_python
rv_need_logos "$PROJECT"

ARGS=(review --json)
[ -z "$BASE" ] || ARGS+=(--base "$BASE")

# Beside the target, never mktemp: $TMPDIR may be sandboxed.
RC=0
(cd "$PROJECT" && "$LOGOS_BIN" "${ARGS[@]}" >"$OUT.tmp.$$" 2>"$OUT.err.$$") || RC=$?
cat "$OUT.err.$$" >&2 || true
rm -f "$OUT.err.$$"
# 0 and 1 are the two answers `review` gives, and each prints a document.
if [ "$RC" -gt 1 ] || [ ! -s "$OUT.tmp.$$" ]; then
    rm -f "$OUT.tmp.$$"
    rv_die 3 "\`logos review\` failed with exit $RC and no answer"
fi
mv "$OUT.tmp.$$" "$OUT"

rv_py summary --delta "$OUT"
