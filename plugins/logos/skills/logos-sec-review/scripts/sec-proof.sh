#!/usr/bin/env bash
#
# sec-proof.sh — prove a fix by the targeted re-scan, not by the edit (FR-SC-19).
#
#   sec-proof.sh --triage FILE --delta FILE --rescan FILE --entry ID [--entry ID ...] [--project DIR]
#
# Reads the stored findings of the scanner and file of each fix entry
# (`logos security findings --json --scanner --path`) after sec-rescan.sh stored
# the re-scan, and holds three things for each:
#
#   - the re-scan covered the file, and the file has not changed since it
#   - the entry's fingerprint is absent from the stored findings of that scanner
#     and file
#   - no finding at or above the entry's severity is there that the delta did not
#     already know (a fix that moved the offence into a new finding, or whose
#     fingerprint moved with its line, is not a fix)
#
# `--delta` is the file sec-delta.sh wrote: the before-state.
#
# Prints a line per entry:
# `PROVEN id | subject | rescan: <scanner> <version> paths=<a,b> fingerprint absent`
# (the evidence a FIXED report line cites) or `NOT PROVEN id | subject | why`. The
# tests passing is the caller's evidence: this script never runs the project's tests.
#
# Exit: 0 every entry proven · 1 some entry not proven · 2 usage ·
#       3 no python3, no logos, not enrolled

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/secrev.sh
. "$SCRIPT_DIR/lib/secrev.sh"

TRIAGE="" DELTA="" RESCAN=""
ENTRY_ARGS=()
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --triage) [ $# -ge 2 ] || sr_die 2 "--triage needs a file"; TRIAGE="$2"; shift 2 ;;
        --delta) [ $# -ge 2 ] || sr_die 2 "--delta needs a file"; DELTA="$2"; shift 2 ;;
        --rescan) [ $# -ge 2 ] || sr_die 2 "--rescan needs a file"; RESCAN="$2"; shift 2 ;;
        --entry) [ $# -ge 2 ] || sr_die 2 "--entry needs an id"; ENTRY_ARGS+=(--entry "$2"); shift 2 ;;
        --project) [ $# -ge 2 ] || sr_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) sr_die 2 "unknown argument '$1' (usage: sec-proof.sh --triage FILE --delta FILE --rescan FILE --entry ID... [--project DIR])" ;;
    esac
done
[ -n "$TRIAGE" ] && [ -n "$DELTA" ] && [ -n "$RESCAN" ] && [ "${#ENTRY_ARGS[@]}" -gt 0 ] ||
    sr_die 2 "--triage, --delta, --rescan and at least one --entry are required"

rv_need_python
rv_need_logos "$PROJECT"

sr_py proof --triage "$TRIAGE" --delta "$DELTA" --rescan "$RESCAN" "${ENTRY_ARGS[@]}" --logos "$LOGOS_BIN" --project "$PROJECT"
