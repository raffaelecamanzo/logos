#!/usr/bin/env bash
#
# remeasure.sh — prove a fix by re-running the delta, not by the signal
# (FR-IN-15, BR-70).
#
#   remeasure.sh --triage FILE --out FILE --entry ID [--entry ID ...] [--base REF] [--project DIR]
#
# For the entries the model edited: `logos sync` their files (review never
# reconciles, so an unsynced edit reads `stale` and proves nothing), run the delta
# again into --out, and compare each entry with its state in the triage.
#
#   proven    the entry is now `resolved` or `improved`, and its trust is `resolved`
#   cleared   the entry is no longer listed and was `new` on this branch: the
#             merge-base never had it, so there is no `resolved` entry to cite
#   not proven  anything else: still new or worsened, stale, back at its merge-base
#               value (`unchanged`), listed under another measure, or the re-run
#               lists an offender the triage did not have (the edit moved the offence)
#
# A fix is claimed only for `proven` or `cleared`, and only with the tests passing
# (the test run is the caller's: this script never runs the project's tests).
# The aggregate signal is never read. Pass the --base the triage was made with.
#
# Exit: 0 every entry proven or cleared · 1 some entry not proven, or the re-run
#       measured nothing · 2 usage · 3 environment

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"

TRIAGE="" OUT="" BASE=""
ENTRY_ARGS=()
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --triage) [ $# -ge 2 ] || rv_die 2 "--triage needs a file"; TRIAGE="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || rv_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        --entry) [ $# -ge 2 ] || rv_die 2 "--entry needs an id"; ENTRY_ARGS+=(--entry "$2"); shift 2 ;;
        --base) [ $# -ge 2 ] || rv_die 2 "--base needs a ref"; BASE="$2"; shift 2 ;;
        --project) [ $# -ge 2 ] || rv_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) rv_die 2 "unknown argument '$1' (usage: remeasure.sh --triage FILE --out FILE --entry ID... [--base REF] [--project DIR])" ;;
    esac
done
[ -n "$TRIAGE" ] && [ -n "$OUT" ] && [ "${#ENTRY_ARGS[@]}" -gt 0 ] || rv_die 2 "--triage, --out and at least one --entry are required"

rv_need_python
rv_need_logos "$PROJECT"

# The files of the entries; an unknown entry id stops here, before any sync or run.
FILES_OUT="$(rv_py files --triage "$TRIAGE" "${ENTRY_ARGS[@]}")"
FILES=()
while IFS= read -r f; do FILES+=("$f"); done <<EOF2
$FILES_OUT
EOF2
if [ -n "$FILES_OUT" ]; then
    SYNC_ERR="$(cd "$PROJECT" && "$LOGOS_BIN" sync -- "${FILES[@]}" 2>&1 >/dev/null)" ||
        rv_die 3 "\`logos sync\` failed on ${FILES[*]}; the re-run would read them stale: $SYNC_ERR"
fi

DELTA_ARGS=(--out "$OUT" --project "$PROJECT")
[ -z "$BASE" ] || DELTA_ARGS+=(--base "$BASE")
RC=0
bash "$SCRIPT_DIR/review-delta.sh" "${DELTA_ARGS[@]}" >/dev/null || RC=$?
[ "$RC" -le 1 ] || exit "$RC"

rv_py remeasure --triage "$TRIAGE" --after "$OUT" "${ENTRY_ARGS[@]}"
