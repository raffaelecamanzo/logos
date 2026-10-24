# shellcheck shell=bash
#
# scripts/lib/secrev.sh — what every script of the logos-sec-review skill shares
# (FR-SC-19). Sourced, never executed: it sets no shell options.
#
# The review helpers are the logos-review skill's (FR-IN-15): this lib sources that
# skill's lib/common.sh in place — the exit codes, the version check, the project —
# and adds only what a security review needs. Nothing of logos-review is copied
# or moved.
#
# Exit codes, the same in every script:
#   0  the question was answered, and the answer is the good one
#   1  the question was answered, and the answer is the other one (nothing
#      measured, nothing proven, a rule broken)
#   2  usage: an argument is missing or unknown
#   3  environment or input: no python3, no logos, a logos too old for a scoped
#      scan, no .logos/, an input file that cannot be read
#
# Every script prints its denominator on stderr, as its last line, beginning
# `denominator:` — what it looked at, so a run that looked at nothing cannot read
# as a run that found nothing wrong.

# The directory this lib sits in: <skill>/scripts/lib.
SR_LIB="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# The sibling skill's scripts, which this skill reuses.
SR_REVIEW_SCRIPTS="$SR_LIB/../../../logos-review/scripts"

if [ ! -f "$SR_REVIEW_SCRIPTS/lib/common.sh" ]; then
    echo "logos-sec-review: the logos-review skill is missing ($SR_REVIEW_SCRIPTS/lib/common.sh); both ship in the plugin's skills/ directory" >&2
    exit 3
fi
# Declared before the sibling's lib, which keeps a value already set: the calls
# this skill makes, through its own scripts or logos-review's, are its own
# (FR-OB-15).
export LOGOS_INVOKER="${LOGOS_INVOKER:-skill:logos-sec-review}"
# shellcheck source=/dev/null
. "$SR_REVIEW_SCRIPTS/lib/common.sh"

sr_die() { # code message
    local code="$1"
    shift
    echo "logos-sec-review: $*" >&2
    exit "$code"
}

# The first logos whose `security findings` lists production code by default and
# takes `--scope` (FR-SC-28). A 1.20.x binary lists every finding and rejects the flag.
SR_SCOPE_MIN_LOGOS="1.21.0"

# sr_scope_args — SR_SCOPE_ARGS: `--scope all` when this logos has the scope filter,
# else empty. Every `security findings` read of this skill wants the full listing,
# test code included: a fix proof must not read a finding hidden as test code as
# gone, and the report's counts are of every stored finding. Needs rv_need_logos
# first; a version that cannot be read gets no flag.
SR_SCOPE_ARGS=()
sr_scope_args() {
    local out
    SR_SCOPE_ARGS=()
    out="$("$LOGOS_BIN" --version 2>/dev/null)" || out=""
    [[ $out =~ ^logos[[:space:]]+v?([0-9]+\.[0-9]+\.[0-9]+) ]] || return 0
    rv_version_ge "${BASH_REMATCH[1]}" "$SR_SCOPE_MIN_LOGOS" && SR_SCOPE_ARGS=(--scope all)
    return 0
}

# python3 in isolated mode running this skill's one helper.
sr_py() {
    python3 -I "$SR_LIB/secrev.py" "$@"
}

# sr_need_scoped_scan — exits 3 unless this logos can re-scan one scanner on some
# paths (`security scan --scanner --path`, FR-SC-17). Needs rv_need_logos first.
sr_need_scoped_scan() {
    local help
    help="$("$LOGOS_BIN" security scan --help 2>/dev/null)" || help=""
    case "$help" in
        *--scanner*--path* | *--path*--scanner*) ;;
        *) sr_die 3 "$LOGOS_BIN has no \`security scan --scanner --path\`; a fix cannot be proven by a targeted re-scan, so only escalations can be reported" ;;
    esac
}
