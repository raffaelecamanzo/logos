#!/usr/bin/env bash
#
# check-report.sh — hold the review report to the evidence rule (FR-IN-15, BR-70).
#
#   check-report.sh --report FILE [--triage FILE]
#
# The report is prose, but its verdicts and its evidence are lines this script
# reads. Any other line is free text.
#
#   NO ENTRIES
#   ENTRY | <fix|test-first|no-action-warranted|untrusted> | <file>:<line> <name> | <reason>
#   FIXED | <file>:<line> <name> | remeasured: <resolved|improved|cleared> <before> -> <after> | tests: <command> | <P> passed, <F> failed (of <T>)
#   UNPROVEN | <file>:<line> <name> | <why>
#
# It fails when: a verdict is not one of the four; an ENTRY has no reason; a fix
# has neither a FIXED nor an UNPROVEN line (or has both); a FIXED or UNPROVEN
# line names something that is not a fix; a FIXED line's test run is not a pass
# over at least one test; the report says NO ENTRIES and lists an entry or an
# edit; the report has neither; two ENTRY lines name one subject; or any line
# makes an aggregate-signal claim (the words signal, aggregate or score, or the
# number 10000). The subject of a line (a file and a name) is not scanned for
# those words; every other line, prose included, is.
#
# With --triage FILE (the output of trust-gate.sh) the verdicts are also held to
# the gate: every row judged once, a subject that is not a row refused, and a
# verdict the gate did not allow refused (a proposed fix may be downgraded to
# no-action-warranted; nothing else may change). Without it that comparison is
# not made, and the denominator says so.
#
# Exit: 0 the report holds · 1 it does not (each problem on stdout) · 2 usage ·
#       3 unreadable (or not text)

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"

REPORT="" TRIAGE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --report) [ $# -ge 2 ] || rv_die 2 "--report needs a file"; REPORT="$2"; shift 2 ;;
        --triage) [ $# -ge 2 ] || rv_die 2 "--triage needs a file"; TRIAGE="$2"; shift 2 ;;
        *) rv_die 2 "unknown argument '$1' (usage: check-report.sh --report FILE [--triage FILE])" ;;
    esac
done
[ -n "$REPORT" ] || rv_die 2 "--report FILE is required"

rv_need_python
if [ -n "$TRIAGE" ]; then rv_py check-report --report "$REPORT" --triage "$TRIAGE"; else rv_py check-report --report "$REPORT"; fi
