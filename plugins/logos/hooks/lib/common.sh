# shellcheck shell=bash
#
# hooks/lib/common.sh — what every script of the logos Claude Code plugin shares
# (FR-IN-11, ADR-81). Sourced, never executed: it sets no shell options and runs
# nothing at load time beyond locating the plugin root.
#
#   logos_project_dir            the project the host named, else the cwd
#   logos_enrolled DIR           0 when DIR holds a .logos/ directory
#   logos_bin                    the logos binary on PATH, or exit 1
#   logos_binary_version BIN     the X.Y.Z that `BIN --version` reports, or exit 1
#   logos_version_ge A B         0 when version A >= version B
#   logos_min_binary_version     the plugin's declared minimum, or exit 1
#   logos_hook_message EVENT SYSTEM CONTEXT
#                                one hook JSON object: SYSTEM for the user,
#                                CONTEXT for the agent
#   logos_hook_block REASON      one Stop-hook object that blocks the stop
#   logos_hook_notice TEXT       one hook object that only tells the user
#   logos_json_region TEXT START END
#                                REGION: what lies between two markers
#   logos_json_split TEXT MARKER JSON_CHUNKS: the text after each MARKER
#   logos_json_str TEXT KEY      JSON_VALUE: a string field, unescaped
#   logos_json_num TEXT KEY      JSON_VALUE: a number field
#
# Builtins only, and bash 3.2 (macOS /bin/bash): no dirname, sed, jq or python.
# A hook runs under whatever PATH the host hands it, and a missing utility must
# not turn "no binary" into a crashed hook. The one exception is stop.sh, which
# measures a git working tree and so needs git (and sleep, mkdir, mv and rm); it
# exits 0 silently when git is missing.

# The plugin root: this file sits at <root>/hooks/lib/common.sh.
logos_plugin_root() {
    local src="${BASH_SOURCE[0]}" dir
    case "$src" in
        */*) dir="${src%/*}" ;;
        *) dir=. ;;
    esac
    (cd "$dir/../.." && pwd -P)
}
LOGOS_PLUGIN_ROOT="$(logos_plugin_root)"

logos_project_dir() {
    printf '%s\n' "${CLAUDE_PROJECT_DIR:-$PWD}"
}

# Enrolled means a .logos/ DIRECTORY: a regular file of that name is not a store.
logos_enrolled() {
    [ -d "$1/.logos" ]
}

logos_bin() {
    command -v logos 2>/dev/null
}

# `logos --version` prints `logos X.Y.Z`. Only that shape is read, so another
# program that happens to be called logos is an unreadable version, never a
# current one. A pre-release or build suffix is ignored: 1.20.0-dev reads 1.20.0.
logos_binary_version() {
    local out
    out="$("$1" --version 2>/dev/null)" || return 1
    [[ $out =~ ^logos[[:space:]]+v?([0-9]+\.[0-9]+\.[0-9]+) ]] || return 1
    printf '%s\n' "${BASH_REMATCH[1]}"
}

# Numeric per component (1.100.0 > 1.20.0), base 10 (08 is eight, not octal).
logos_version_ge() {
    local a="$1" b="$2" x y
    while [ -n "$a" ] || [ -n "$b" ]; do
        x="${a%%.*}" y="${b%%.*}"
        x="${x:-0}" y="${y:-0}"
        if [ "$((10#$x))" -gt "$((10#$y))" ]; then return 0; fi
        if [ "$((10#$x))" -lt "$((10#$y))" ]; then return 1; fi
        case "$a" in *.*) a="${a#*.}" ;; *) a="" ;; esac
        case "$b" in *.*) b="${b#*.}" ;; *) b="" ;; esac
    done
    return 0
}

# The minimum lives in plugin.json under `metadata`, the free-form object Claude
# Code does not read, so the manifest stays valid under `claude plugin validate
# --strict`. Read by an exact key match, not a JSON parser (there is none here).
logos_min_binary_version() {
    local manifest="$LOGOS_PLUGIN_ROOT/.claude-plugin/plugin.json" text
    [ -f "$manifest" ] || return 1
    text="$(<"$manifest")"
    [[ $text =~ \"minBinaryVersion\"[[:space:]]*:[[:space:]]*\"([0-9]+\.[0-9]+\.[0-9]+)\" ]] || return 1
    printf '%s\n' "${BASH_REMATCH[1]}"
}

logos_json_string() {
    local s="$1"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//$'\n'/\\n}"
    s="${s//$'\r'/\\r}"
    s="${s//$'\t'/\\t}"
    s="${s//[[:cntrl:]]/}"
    printf '"%s"' "$s"
}

logos_hook_message() {
    printf '{"systemMessage":%s,"hookSpecificOutput":{"hookEventName":%s,"additionalContext":%s}}\n' \
        "$(logos_json_string "$2")" "$(logos_json_string "$1")" "$(logos_json_string "$3")"
}

logos_hook_block() {
    printf '{"decision":"block","reason":%s}\n' "$(logos_json_string "$1")"
}

logos_hook_notice() {
    printf '{"systemMessage":%s}\n' "$(logos_json_string "$1")"
}

# The readers below take the one JSON the logos binary writes (`--json`: compact,
# serde field order) and need no parser. Objects are cut at the first key they
# open with and fields are read by an exact `"key":` match. A quote inside a
# string value is written \", so a key pattern can never match inside a value,
# and a brace in a value never matters because no object is matched by braces.
# All of them cost a regex or a ${var#pattern} per object, never a per-character
# loop and never ${var//pattern/replacement} (see hooks/guard.sh on bash 3.2).

# REGION is what lies after START and before the next END (or the rest). The text
# before START is cut by its length: bash 3.2 runs ${text#*START} in time
# quadratic in that text, which for a long list of entries is minutes.
logos_json_region() {
    REGION=""
    case "$1" in *"$2"*) ;; *) return 1 ;; esac
    local before="${1%%"$2"*}"
    REGION="${1:$((${#before} + ${#2}))}"
    REGION="${REGION%%"$3"*}"
}

# JSON_CHUNKS holds one element per MARKER: the marker and the text after it, up
# to the next.
logos_json_split() {
    local rest="$1" marker="$2"
    JSON_CHUNKS=()
    case "$rest" in *"$marker"*) ;; *) return 0 ;; esac
    rest="${rest#*"$marker"}"
    while :; do
        case "$rest" in
            *"$marker"*)
                JSON_CHUNKS[${#JSON_CHUNKS[@]}]="$marker${rest%%"$marker"*}"
                rest="${rest#*"$marker"}"
                ;;
            *)
                JSON_CHUNKS[${#JSON_CHUNKS[@]}]="$marker$rest"
                break
                ;;
        esac
    done
}

# JSON_VALUE is the first string value of KEY in TEXT, with \" and \\ undone.
# Fails when KEY is absent or not a string (null).
logos_json_str() {
    JSON_VALUE=""
    local re="\"$2\":\"(([^\"\\\\]|\\\\.)*)\""
    [[ $1 =~ $re ]] || return 1
    JSON_VALUE="${BASH_REMATCH[1]}"
    JSON_VALUE="${JSON_VALUE//\\\"/\"}"
    JSON_VALUE="${JSON_VALUE//\\\\/\\}"
}

# JSON_VALUE is the first number value of KEY in TEXT. Fails when KEY is absent
# or null.
logos_json_num() {
    JSON_VALUE=""
    local re="\"$2\":(-?[0-9]+(\\.[0-9]+)?)"
    [[ $1 =~ $re ]] || return 1
    JSON_VALUE="${BASH_REMATCH[1]}"
}
