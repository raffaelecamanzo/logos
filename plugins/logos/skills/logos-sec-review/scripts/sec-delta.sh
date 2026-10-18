#!/usr/bin/env bash
#
# sec-delta.sh — read the security section of the branch delta (FR-SC-18,
# FR-SC-19).
#
#   sec-delta.sh --out FILE [--base REF] [--project DIR]
#
# Runs the logos-review skill's review-delta.sh (`logos review --json`: the
# before-state is the merge-base commit and nothing else) and keeps the security
# section of its answer in FILE. The merge-base tree is never scanned. Prints one
# summary line and any notes the section carries.
#
# A security section that is not measured (no report is stored, or the findings
# could not be read) says why; an empty list then means nothing, and this script
# exits 1 rather than let it read as a clean branch.
#
# Exit: 0 measured · 1 not measured (the line says why) · 2 usage ·
#       3 no python3, no logos 1.20.0 or newer, not enrolled, or FILE unwritable

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/secrev.sh
. "$SCRIPT_DIR/lib/secrev.sh"

OUT="" BASE=""
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || sr_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        --base) [ $# -ge 2 ] || sr_die 2 "--base needs a ref"; BASE="$2"; shift 2 ;;
        --project) [ $# -ge 2 ] || sr_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) sr_die 2 "unknown argument '$1' (usage: sec-delta.sh --out FILE [--base REF] [--project DIR])" ;;
    esac
done
[ -n "$OUT" ] || sr_die 2 "--out FILE is required"
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac
[ -d "${OUT%/*}" ] || sr_die 3 "the directory of $OUT does not exist"

rv_need_python

REVIEW="$OUT.review.$$"
trap 'rm -f "$REVIEW"' EXIT
ARGS=(--out "$REVIEW" --project "$PROJECT")
[ -z "$BASE" ] || ARGS+=(--base "$BASE")
RC=0
bash "$SR_REVIEW_SCRIPTS/review-delta.sh" "${ARGS[@]}" >/dev/null || RC=$?
[ "$RC" -le 1 ] || exit "$RC"

sr_py extract --review "$REVIEW" --out "$OUT"
