#!/usr/bin/env bash
#
# sec-triage.sh — pre-classify each introduced or touched finding (FR-SC-19).
#
#   sec-triage.sh --delta FILE --out FILE
#
# Reads the file sec-delta.sh wrote. One row per introduced or touched finding,
# `id | proposed verdict | subject | status severity scanner | reason`, and the
# same rows as JSON in --out. The rules, in order:
#
#   redacted (secret-class)                       escalate-secret      final
#   a dependency finding                          escalate-dependency  final
#   no bound file, reachability confidence not    untrusted            final
#     `resolved`, or a file edited since the scan
#   anything else                                 fix?   the model confirms a fix, or
#                                                 downgrades it to false-positive
#
# A `final` verdict cannot be changed by the model. Findings the branch neither
# introduced nor touched, resolved or stale ones and suppressed ones are counted
# and never judged. No row at all prints NO ENTRIES.
#
# Exit: 0 triaged (zero rows included) · 1 the delta was not measured · 2 usage ·
#       3 no python3, or FILE unreadable

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/secrev.sh
. "$SCRIPT_DIR/lib/secrev.sh"

DELTA="" OUT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --delta) [ $# -ge 2 ] || sr_die 2 "--delta needs a file"; DELTA="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || sr_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        *) sr_die 2 "unknown argument '$1' (usage: sec-triage.sh --delta FILE --out FILE)" ;;
    esac
done
[ -n "$DELTA" ] && [ -n "$OUT" ] || sr_die 2 "--delta and --out are required"

rv_need_python
sr_py triage --delta "$DELTA" --out "$OUT"
