#!/usr/bin/env bash
#
# suppress-draft.sh — draft the ignore entry of a false positive (FR-SC-19, FR-IN-14).
#
#   suppress-draft.sh --fingerprint HEX64 --reason TEXT [--expires YYYY-MM-DD]
#
# Prints one `[[ignore]]` entry for `.logos/security-ignore.toml` and writes
# nothing. The reason is required (an entry without one rejects the whole ignore
# file), the fingerprint is the 64-character one `logos security findings --json`
# shows, and the date is a calendar date.
#
# Writing the entry is the plugin guardrail's call, not this script's: the host
# asks the human before any agent edit to that file (BR-69). The skill makes that
# one edit through the host's own file tool and stops at the answer. It never
# writes the file through the shell, which would not be seen.
#
# Exit: 0 a draft was printed · 2 usage or an invalid field · 3 no python3

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/secrev.sh
. "$SCRIPT_DIR/lib/secrev.sh"

FP="" REASON="" EXPIRES=""
while [ $# -gt 0 ]; do
    case "$1" in
        --fingerprint) [ $# -ge 2 ] || sr_die 2 "--fingerprint needs a value"; FP="$2"; shift 2 ;;
        --reason) [ $# -ge 2 ] || sr_die 2 "--reason needs text"; REASON="$2"; shift 2 ;;
        --expires) [ $# -ge 2 ] || sr_die 2 "--expires needs a date"; EXPIRES="$2"; shift 2 ;;
        *) sr_die 2 "unknown argument '$1' (usage: suppress-draft.sh --fingerprint HEX64 --reason TEXT [--expires YYYY-MM-DD])" ;;
    esac
done
[ -n "$FP" ] && [ -n "$REASON" ] || sr_die 2 "--fingerprint and --reason are required"

rv_need_python
sr_py draft --fingerprint "$FP" --reason "$REASON" --expires "$EXPIRES"
