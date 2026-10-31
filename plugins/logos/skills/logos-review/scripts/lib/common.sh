# shellcheck shell=bash
#
# scripts/lib/common.sh — what every script of the logos-review skill shares
# (FR-IN-15). Sourced, never executed: it sets no shell options.
#
# Exit codes, the same in every script:
#   0  the question was answered, and the answer is the good one
#   1  the question was answered, and the answer is the other one
#      (nothing measured, nothing proven, a rule broken)
#   2  usage: an argument is missing or unknown
#   3  environment or input: no python3, no logos, a logos older than 1.20.0, no
#      .logos/, not a git repository, an input file that cannot be read
#
# Every script prints its denominator on stderr, as its last line, beginning
# `denominator:` — what it looked at, so a run that looked at nothing cannot read
# as a run that found nothing wrong.
#
# Host-neutral: the project is the --project argument, else LOGOS_PROJECT_DIR,
# else the working directory.

# `logos review` and `logos improvements` arrived in this version. It stays the
# floor although 1.20.0-1.20.2 record a unit's `node_id` as an integer or null:
# debt mode falls back to name, file and dimension for such a unit and counts it.
RV_MIN_LOGOS="1.20.0"

# Every logos call a script of this skill makes is stamped with its invoker
# (FR-OB-15), so `stats --json` counts what the skill caused. A value already set
# is kept: it is the skill that ran this script — logos-sec-review runs
# review-delta.sh — and the call is that skill's.
export LOGOS_INVOKER="${LOGOS_INVOKER:-skill:logos-review}"

rv_die() { # code message
    local code="$1"
    shift
    echo "logos-review: $*" >&2
    exit "$code"
}

# The directory this lib sits in: <skill>/scripts/lib. No `dirname`: the lib
# must load on a PATH without coreutils, so the python3 check below can speak.
RV_LIB="$(cd "${BASH_SOURCE[0]%/*}" && pwd -P)"

rv_project() {
    printf '%s\n' "${LOGOS_PROJECT_DIR:-$PWD}"
}

rv_need_python() {
    command -v python3 >/dev/null 2>&1 ||
        rv_die 3 "python3 is not on PATH; the delta is JSON and this skill reads it with python3, so there is no review"
}

# 0 when version A >= version B (numeric per component, base 10).
rv_version_ge() {
    local a="$1" b="$2" x y
    while [ -n "$a" ] || [ -n "$b" ]; do
        x="${a%%.*}" y="${b%%.*}"
        x="${x:-0}" y="${y:-0}"
        [ "$((10#$x))" -gt "$((10#$y))" ] && return 0
        [ "$((10#$x))" -lt "$((10#$y))" ] && return 1
        case "$a" in *.*) a="${a#*.}" ;; *) a="" ;; esac
        case "$b" in *.*) b="${b#*.}" ;; *) b="" ;; esac
    done
    return 0
}

# need_logos PROJECT — sets LOGOS_BIN, or exits 3: a logos at least RV_MIN_LOGOS
# (`logos --version` prints `logos X.Y.Z`; only that shape is read) and an
# enrolled project.
rv_need_logos() {
    local out
    LOGOS_BIN="$(command -v logos 2>/dev/null)" ||
        rv_die 3 "no logos binary on PATH; install logos $RV_MIN_LOGOS or newer, then run the review again"
    out="$("$LOGOS_BIN" --version 2>/dev/null)" || out=""
    [[ $out =~ ^logos[[:space:]]+v?([0-9]+\.[0-9]+\.[0-9]+) ]] ||
        rv_die 3 "$LOGOS_BIN did not report a version in the form 'logos X.Y.Z'; the review needs logos $RV_MIN_LOGOS or newer"
    rv_version_ge "${BASH_REMATCH[1]}" "$RV_MIN_LOGOS" ||
        rv_die 3 "logos ${BASH_REMATCH[1]} has no \`review\` command; the review needs logos $RV_MIN_LOGOS or newer"
    [ -d "$1/.logos" ] ||
        rv_die 3 "$1 is not an enrolled repository (no .logos/); run 'logos init' first"
}

# python3 in isolated mode (no script-directory or user-site imports) running the
# one helper this skill ships.
rv_py() {
    python3 -I "$RV_LIB/delta.py" "$@"
}
