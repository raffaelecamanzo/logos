#!/usr/bin/env bash
#
# sec-rescan.sh — re-scan the edited files with the scanner that found them
# (FR-SC-17, FR-SC-19).
#
#   sec-rescan.sh --triage FILE --out FILE --entry ID [--entry ID ...] [--project DIR]
#
# For the `fix` entries the model edited: `logos sync` their files (the scan binds
# its findings against the graph, which must hold the edit), then run
# `logos security scan --scanner <scanner> --path <file>... --json`, once per
# scanner, and keep what each answered in --out. A scanner that cannot be narrowed
# to paths (Trivy, OSV-Scanner, Dependency-Check, CodeQL) runs on the whole
# project when named, which covers the paths and more: the record says so.
#
# Only a `fix` entry is re-scanned. A secret is never proven fixed here: Gitleaks
# fingerprints differ between its content and history modes, and the value stays
# in git history whatever the file says now.
#
# Consent is the human's: this script never passes --consent. A refusal (exit 2 of
# the scan, naming the flag) is reported as NOT RESCANNED; ask the human to run
# `logos security scan --consent`.
#
# Exit: 0 every scanner re-scanned the paths asked · 1 some did not · 2 usage ·
#       3 no python3, no logos with a scoped scan, not enrolled

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/secrev.sh
. "$SCRIPT_DIR/lib/secrev.sh"

TRIAGE="" OUT=""
ENTRY_ARGS=()
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --triage) [ $# -ge 2 ] || sr_die 2 "--triage needs a file"; TRIAGE="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || sr_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        --entry) [ $# -ge 2 ] || sr_die 2 "--entry needs an id"; ENTRY_ARGS+=(--entry "$2"); shift 2 ;;
        --project) [ $# -ge 2 ] || sr_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) sr_die 2 "unknown argument '$1' (usage: sec-rescan.sh --triage FILE --out FILE --entry ID... [--project DIR])" ;;
    esac
done
[ -n "$TRIAGE" ] && [ -n "$OUT" ] && [ "${#ENTRY_ARGS[@]}" -gt 0 ] || sr_die 2 "--triage, --out and at least one --entry are required"
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac
[ -d "${OUT%/*}" ] || sr_die 3 "the directory of $OUT does not exist"

rv_need_python
rv_need_logos "$PROJECT"
sr_need_scoped_scan

# The plan stops here on an unknown id or an entry that is not a fix, before any sync or scan.
PLAN="$(sr_py plan --triage "$TRIAGE" "${ENTRY_ARGS[@]}")"
FILES_OUT="$(sr_py files --triage "$TRIAGE" "${ENTRY_ARGS[@]}")"
FILES=()
while IFS= read -r f; do [ -z "$f" ] || FILES+=("$f"); done <<EOF2
$FILES_OUT
EOF2
if [ "${#FILES[@]}" -gt 0 ]; then
    SYNC_ERR="$(cd "$PROJECT" && "$LOGOS_BIN" sync -- "${FILES[@]}" 2>&1 >/dev/null)" ||
        sr_die 3 "\`logos sync\` failed on ${FILES[*]}; the re-scan would bind against a stale graph: $SYNC_ERR"
fi

SCANS=()
N=0
FAILED=0
trap 'rm -f "$OUT".scan.*."$$"' EXIT
while IFS= read -r line; do
    [ -n "$line" ] || continue
    N=$((N + 1))
    IFS=$'\t' read -r -a ARGV <<<"$line"
    SCAN_OUT="$OUT.scan.$N.$$"
    RC=0
    (cd "$PROJECT" && "$LOGOS_BIN" security scan "${ARGV[@]}" --json >"$SCAN_OUT" 2>"$SCAN_OUT.err") || RC=$?
    if [ "$RC" -ne 0 ]; then
        echo "NOT RESCANNED: \`logos security scan ${ARGV[*]}\` exited $RC: $(head -c 400 "$SCAN_OUT.err" | tr '\n' ' ')"
        FAILED=$((FAILED + 1))
        rm -f "$SCAN_OUT" "$SCAN_OUT.err"
        continue
    fi
    rm -f "$SCAN_OUT.err"
    SCANS+=(--scan "$SCAN_OUT")
done <<EOF3
$PLAN
EOF3

if [ "${#SCANS[@]}" -eq 0 ]; then
    echo "denominator: $N scan(s) asked, 0 answered, $FAILED failed" >&2
    exit 1
fi
RC=0
sr_py rescan-record --triage "$TRIAGE" --out "$OUT" "${SCANS[@]}" "${ENTRY_ARGS[@]}" || RC=$?
exit "$RC"
