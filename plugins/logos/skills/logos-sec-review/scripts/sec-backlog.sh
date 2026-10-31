#!/usr/bin/env bash
#
# sec-backlog.sh — select the security backlog to triage (FR-SC-19, `--backlog`).
#
#   sec-backlog.sh --out FILE [--limit N] [--rule ID]... [--path PATH]... [--project DIR]
#
# Reads every stored finding (`logos security findings --json --scope all`) and
# selects the backlog: each finding that is active (no suppression holds: an
# expired entry, or one whose evidence changed, has let it go), in production
# code, neither secret-class nor a dependency finding, and counted needs-review
# ones included. Code and infrastructure findings alike, a finding of no file
# too. Then it bounds the selection:
#
#   --rule ID    the finding's rule id, exactly (repeat for several)
#   --path PATH  a file, or a directory the file sits under (repeat for several);
#                project-relative, never a name prefix: `app` is not `app2/`
#   --limit N    the first N of what is left, by severity (highest first, a label the
#                helper does not know, `unrated` included, after `info`), then place
#
# Writes FILE as a document shaped like sec-delta.sh's, so sec-triage.sh,
# sec-rescan.sh and sec-proof.sh read it unchanged: the selected findings have
# status `backlog` and are the rows; the rest of the eligible ones are `deferred`
# and every other stored finding is `outside`. Both are kept because the proof of a
# fix compares against every finding the selection knew, so a neighbour the bound
# left out is not read as a new one. `backlog` in FILE counts what was eligible,
# what was selected and how many findings the run did not reach.
#
# It needs a logos with the code-scope filter (1.21.0): without it production code
# cannot be told from test code, and nothing is selected.
#
# Exit: 0 selected (zero included) · 1 not measured (no security report is stored) ·
#       2 usage · 3 no python3, no logos 1.21.0 or newer, not enrolled, FILE not writable

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/secrev.sh
. "$SCRIPT_DIR/lib/secrev.sh"

OUT=""
BOUNDS=()
PROJECT="$(rv_project)"
USAGE="usage: sec-backlog.sh --out FILE [--limit N] [--rule ID]... [--path PATH]... [--project DIR]"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || sr_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        --limit) [ $# -ge 2 ] || sr_die 2 "--limit needs a number"; BOUNDS+=("--limit=$2"); shift 2 ;;
        --rule) [ $# -ge 2 ] || sr_die 2 "--rule needs a rule id"; BOUNDS+=("--rule=$2"); shift 2 ;;
        --path) [ $# -ge 2 ] || sr_die 2 "--path needs a file or directory"; BOUNDS+=("--path=$2"); shift 2 ;;
        --project) [ $# -ge 2 ] || sr_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) sr_die 2 "unknown argument '$1' ($USAGE)" ;;
    esac
done
[ -n "$OUT" ] || sr_die 2 "--out FILE is required ($USAGE)"
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac
[ -d "${OUT%/*}" ] || sr_die 3 "the directory of $OUT does not exist"

rv_need_python
rv_need_logos "$PROJECT"
sr_scope_args
[ "${#SR_SCOPE_ARGS[@]}" -gt 0 ] ||
    sr_die 3 "$LOGOS_BIN has no \`security findings --scope\` (logos $SR_SCOPE_MIN_LOGOS or newer): production code cannot be told from test code, so no backlog is selected"

FINDINGS="$OUT.findings.$$"
trap 'rm -f "$FINDINGS" "$FINDINGS.err"' EXIT
RC=0
(cd "$PROJECT" && "$LOGOS_BIN" security findings --json "${SR_SCOPE_ARGS[@]}" >"$FINDINGS" 2>"$FINDINGS.err") || RC=$?
[ "$RC" -eq 0 ] ||
    sr_die 3 "\`logos security findings\` exited $RC: $(head -c 400 "$FINDINGS.err" | tr '\n' ' ')"

sr_py backlog-select --findings "$FINDINGS" --out "$OUT" ${BOUNDS[@]+"${BOUNDS[@]}"}
