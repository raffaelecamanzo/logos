#!/usr/bin/env bash
#
# sec-check-report.sh — hold the security review report to its rules (FR-SC-19,
# BR-71).
#
#   sec-check-report.sh --report FILE [--triage FILE [--coverage FILE]]
#
# The report is prose, but its verdicts and evidence are lines this script reads
# (any other line is free text, scanned only for claims):
#
#   NO ENTRIES
#   NOT MEASURED | <why>
#   ENTRY | <fix|escalate-secret|escalate-dependency|false-positive|untrusted> | <subject> | <reason>
#   FIXED | <subject> | rescan: <scanner> <version> paths=<a,b> fingerprint absent | tests: <command> | <P> passed, <F> failed (of <T>)
#   UNPROVEN | <subject> | <why>
#   ROTATE | <subject> | rotate the credential; the value remains in git history
#   DEPENDENCY | <subject> | proposal: … | changelog risk: … | blast radius: …
#   SUPPRESS | <subject> | fingerprint <64 hex> | state: <held|approved|declined> at ask | reason: …
#   SCANNED | <scanner> <version> | paths: <paths>
#   COVERAGE | uncovered: <OWASP ids, or none>
#
# It fails when: a verdict is not one of the five; a fix has neither a FIXED nor an
# UNPROVEN line (or both), or a verdict that is never edited has one; a FIXED line
# does not cite the re-scan with the fingerprint absent and a passing run of at
# least one test; a secret has no ROTATE line saying to rotate and that the value
# stays in git history; a dependency finding has no proposal with its changelog
# risk and blast radius; a false positive has no SUPPRESS draft held at ask; the
# report has no COVERAGE line or no SCANNED line; or any line outside a subject
# claims safety or security (secure, safe, vulnerability-free, all clear, no
# findings). A report says what was scanned, with what, and what no scanner covers.
#
# With --triage FILE (sec-triage.sh's output) the verdicts are also held to the
# gate: every row judged once, no row invented, a final verdict kept, a FIXED line
# naming the entry's file and a SUPPRESS line the entry's fingerprint. With
# --coverage FILE (sec-coverage.sh's output) the SCANNED lines must name stored
# reports and COVERAGE must list exactly the uncovered categories. Without them
# those comparisons are not made, and the denominator says so.
#
# Exit: 0 the report holds · 1 it does not (each problem on stdout) · 2 usage ·
#       3 unreadable (or not text)

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/secrev.sh
. "$SCRIPT_DIR/lib/secrev.sh"

REPORT="" TRIAGE="" COVERAGE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --report) [ $# -ge 2 ] || sr_die 2 "--report needs a file"; REPORT="$2"; shift 2 ;;
        --triage) [ $# -ge 2 ] || sr_die 2 "--triage needs a file"; TRIAGE="$2"; shift 2 ;;
        --coverage) [ $# -ge 2 ] || sr_die 2 "--coverage needs a file"; COVERAGE="$2"; shift 2 ;;
        *) sr_die 2 "unknown argument '$1' (usage: sec-check-report.sh --report FILE [--triage FILE [--coverage FILE]])" ;;
    esac
done
[ -n "$REPORT" ] || sr_die 2 "--report FILE is required"
[ -z "$COVERAGE" ] || [ -n "$TRIAGE" ] || sr_die 2 "--coverage is compared together with --triage"

rv_need_python
sr_py check-report --report "$REPORT" --triage "$TRIAGE" --coverage "$COVERAGE"
