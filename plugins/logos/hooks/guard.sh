#!/usr/bin/env bash
#
# guard.sh — the logos plugin's PreToolUse guardrail (FR-IN-14, BR-69): an agent
# never relaxes its own quality contract unseen. The host asks the human first
# when an agent tool call would
#
#   write, edit or delete    .logos/rules.toml, .logos/rules-baseline.json,
#                            .logos/security-ignore.toml or
#                            .logos/security-baseline.json (any .logos/, so a
#                            workspace member's contract is guarded too)
#   run                      logos gate --save  or  logos check --bless, as a
#                            shell command, or check --bless's MCP twin
#                            bless_rules (gate has no MCP twin); or
#                            logos security ingest, as a shell command or its
#                            MCP twin security_ingest — an empty report clears a
#                            failing [security] gate, required scanners included;
#                            or logos security bless, as a shell command (it has
#                            no MCP twin) — it rewrites the checked-in security
#                            baseline
#
#   not enrolled (no .logos/)   exit 0, no output
#   a guarded action            one PreToolUse JSON object: permissionDecision
#                               "ask", with a reason naming the file or command
#   a malformed payload         the same "ask": it fails toward ask, never allow
#   anything else               exit 0, no output — the call passes untouched
#
# Reads pass: a Read of a guarded file, and a shell command that only reads one
# (`cat .logos/rules.toml`, `grep x .logos/rules.toml > out`). A shell command
# that names a guarded file — as a word of its own or glued to an option, as in
# `dd of=…` or `--output=…` — is a write unless its command is a known reader and
# the file is not a redirect target, so an unrecognised command asks. So does a
# writer given the .logos directory itself, a glob or brace list under it that
# can match a guarded name, or a bare guarded name, glob or brace list beside a
# mention of .logos (`cd .logos && rm *`).
#
# This is a guardrail against an agent loosening the contract by habit or by
# mistake, not a sandbox: a command that assembles the path at run time (an
# interpreter one-liner, a path piped into xargs) is not seen.
#
# Builtins only, bash 3.2, like every plugin script (hooks/lib/common.sh): the
# payload is read with fixed-key regexes, not a JSON parser.
#
# Time is part of the contract: the host cancels a hook at its timeout and lets
# the call through, so a guard that runs long is a guard that allows. Text of
# unbounded length (a command, a file's content) is therefore only ever cut by
# word splitting and matched by regex or `case`, all linear. It never goes
# through ${var//pattern/replacement}, which bash 3.2 runs in far worse than
# linear time once matches are many (8 KB of quotes: 7 s; 16 KB: 54 s), nor
# through a $( ) fork per word.

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) HOOK_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) HOOK_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$HOOK_DIR/lib/common.sh"
# The guard runs no logos today; were it to, the call is stamped (FR-OB-15).
export LOGOS_INVOKER=hook:pre-tool-use

logos_enrolled "$(logos_project_dir)" || exit 0

# No word here is ever a glob to expand: payload text is split, never globbed.
set -f
# macOS and Windows file systems fold case: .LOGOS/Rules.toml is the same file.
shopt -s nocasematch

GUARDED_NAMES="rules.toml rules-baseline.json security-ignore.toml security-baseline.json"
# Commands that never write the files they are given. Anything else that names a
# guarded file asks. sed, find and git are readers only on conditions (reader_cmd);
# less (-o) and xxd (-r) are left out because they can write.
READERS="cat head tail more grep egrep fgrep rg ag ack wc diff cmp comm stat file ls du bat jq
    md5 md5sum shasum sha1sum sha256sum cksum test [ realpath readlink basename dirname od hexdump
    strings cut column nl echo printf cd pushd type which true false logos"
# git subcommands that leave the working-tree file as it is (add and commit record it).
GIT_READERS="diff log show status blame annotate ls-files ls-tree cat-file grep rev-parse shortlog check-ignore
    add commit"

BS=\\

decide_ask() {
    local r="logos guardrail: $1"
    # The reason quotes words the agent supplied; capped, so escaping it is cheap,
    # and cut back to a whole character, so the JSON stays valid UTF-8.
    if [ "${#r}" -gt 1000 ]; then
        r="${r:0:1000}"
        while [[ $r == *[$'\200'-$'\377'] ]]; do r="${r%?}"; done
        r="$r..."
    fi
    printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":%s}}\n' \
        "$(logos_json_string "$r")"
    exit 0
}

ask() {
    decide_ask "$1 That relaxes the quality contract the review loop measures against, so a human decides (BR-69)."
}

malformed() {
    decide_ask "the PreToolUse payload could not be read ($1), so this call cannot be shown to leave the quality contract (.logos/rules.toml, .logos/rules-baseline.json, .logos/security-ignore.toml, .logos/security-baseline.json) untouched. A human decides."
}

# split_on CHARS TEXT — PARTS: TEXT cut at every one of CHARS, empty fields kept
# (a sentinel holds the trailing one; \034, because bash 3.2 reserves \001 and
# \177 for its own quoting).
SENTINEL=$'\034'
PARTS=()
split_on() {
    local IFS="$1"
    # shellcheck disable=SC2206
    PARTS=($2$SENTINEL)
    PARTS[${#PARTS[@]} - 1]="${PARTS[${#PARTS[@]} - 1]%"$SENTINEL"}"
}

# join_parts SEP — JOINED: PARTS run together, SEP (one character, or none) between.
JOINED=""
join_parts() {
    local IFS="$1"
    JOINED="${PARTS[*]}"
}

# words_of TEXT — WORDS: TEXT's blank-separated words.
WORDS=()
words_of() {
    local IFS=$' \t\r'
    # shellcheck disable=SC2206
    WORDS=($1)
}

# json_decode RAW — DECODED: a JSON string body with its escapes resolved. Cut at
# every backslash: an empty field is an escaped backslash (and the field after
# it is literal); any other field opens with the escaped character.
DECODED=""
json_decode() {
    split_on "$BS" "$1"
    local raw=("${PARTS[@]}") out=() i=1 p c
    out=("${raw[0]}")
    while [ "$i" -lt "${#raw[@]}" ]; do
        p="${raw[i]}"
        if [ -z "$p" ]; then
            out+=("$BS")
            i=$((i + 1))
            if [ "$i" -lt "${#raw[@]}" ]; then out+=("${raw[i]}"); fi
        else
            c="${p:0:1}" p="${p:1}"
            case "$c" in
                n) out+=($'\n'"$p") ;;
                t) out+=($'\t'"$p") ;;
                r) out+=($'\r'"$p") ;;
                *) out+=("$c$p") ;; # \" \/ and the rest: the character itself
            esac
        fi
        i=$((i + 1))
    done
    PARTS=("${out[@]}")
    join_parts ""
    DECODED="$JOINED"
}

# The payload, whole. `read` is a builtin; it returns 1 at end of input.
PAYLOAD=""
IFS= read -r -d '' PAYLOAD || true

[[ $PAYLOAD =~ ^[[:space:]]*\{ ]] || malformed "it is not a JSON object"

# json_strings KEY — every string value of KEY in the payload, decoded, into the
# STRINGS array. A key quoted inside another string is escaped there (\"KEY\"),
# so it never matches: the closing quote must follow the key directly. The rest
# of the payload after a match is captured by the regex, not cut off with
# ${rest#*match}, which is quadratic.
STRINGS=()
json_strings() {
    local rest="$PAYLOAD" re v
    re="\"$1\"[[:space:]]*:[[:space:]]*\"(([^\"\\\\]|\\\\.)*)\"(.*)\$"
    STRINGS=()
    while [[ $rest =~ $re ]]; do
        v="${BASH_REMATCH[1]}"
        rest="${BASH_REMATCH[3]}"
        json_decode "$v"
        STRINGS+=("$DECODED")
    done
}

# The path helpers below run in this shell, never in $( ). Each sets a global.

# normalize PATH — NORM: PATH with //, /./ and dir/.. collapsed, so a spelling
# cannot hide a name.
NORM=""
normalize() {
    local lead="" part out=()
    case "$1" in /*) lead=/ ;; esac
    split_on / "$1"
    for part in "${PARTS[@]}"; do
        case "$part" in
            "" | .) ;;
            ..)
                if [ "${#out[@]}" -gt 0 ] && [ "${out[${#out[@]}-1]}" != .. ]; then
                    unset "out[${#out[@]}-1]"
                    out=(${out[@]+"${out[@]}"})
                else
                    out+=(..)
                fi
                ;;
            *) out+=("$part") ;;
        esac
    done
    PARTS=(${out[@]+"${out[@]}"})
    join_parts /
    NORM="$lead$JOINED"
}

# guarded_path PATH — 0, with HIT set to the guarded file PATH names
# (.logos/<name>), or 1.
HIT=""
guarded_path() {
    local n
    normalize "$1"
    for n in $GUARDED_NAMES; do
        case "$NORM" in
            .logos/"$n" | */.logos/"$n") HIT=".logos/$n" && return 0 ;;
        esac
    done
    return 1
}

# maybe_guarded TOKEN MENTIONS_DOT_LOGOS — a shell word that may reach a guarded
# file without spelling it: the .logos directory itself, a glob or brace list
# under .logos/ that can match a guarded name, or a bare guarded name, glob or
# brace list in a command line that also names .logos (`cd .logos && rm *`). 0,
# with HIT set to what it reaches, or 1.
maybe_guarded() {
    local p n base
    normalize "$1"
    p="$NORM"
    case "$p" in
        .logos | */.logos) HIT="$1 (the directory holding the quality contract)" && return 0 ;;
    esac
    case "$p" in
        .logos/* | */.logos/*)
            base="${p##*/}"
            case "$base" in
                *\{*) HIT="$1 (a brace list under .logos/)" && return 0 ;;
            esac
            case "$base" in
                *[\*\?\[]*)
                    for n in $GUARDED_NAMES; do
                        # shellcheck disable=SC2053
                        [[ $n == $base ]] && HIT=".logos/$n (through $1)" && return 0
                    done
                    ;;
            esac
            ;;
        */*) ;;
        *)
            if [ "$2" = yes ]; then
                case "$p" in *\{*) HIT="$1 (a brace list beside a mention of .logos)" && return 0 ;; esac
                for n in $GUARDED_NAMES; do
                    # shellcheck disable=SC2053 # a glob word (`rm *`) matches as a glob
                    [[ $n == $p ]] && HIT=".logos/$n (as $1)" && return 0
                done
            fi
            ;;
    esac
    return 1
}

# mention WORD — 0, with HIT set, when the shell word names or may reach a
# guarded file, whole or glued to an option: `of=.logos/rules.toml`,
# `--output=…`, `-o.logos/rules.toml`. A word that holds neither .logos nor a
# guarded name — nor, beside a mention of .logos, a glob or brace — cannot, and
# is turned away before any path work.
mention() {
    case "$1" in
        *.logos* | *rules.toml* | *rules-baseline.json* | *security-ignore.toml* | *security-baseline.json*) ;;
        *[\*\?\[\{]*) [ "$DOT_LOGOS" = yes ] || return 1 ;;
        *) return 1 ;;
    esac
    local tries=("$1") t
    [[ $1 =~ ^[^=]*=(.+)$ ]] && tries+=("${BASH_REMATCH[1]}")
    [[ $1 =~ ^-[^-](.+)$ ]] && tries+=("${BASH_REMATCH[1]}")
    for t in "${tries[@]}"; do
        if guarded_path "$t" || maybe_guarded "$t" "$DOT_LOGOS"; then return 0; fi
    done
    return 1
}

# one_of WORD LIST — 0 when WORD is a word of LIST, exactly (not case-folded).
one_of() {
    local w
    for w in $2; do [ "$w" = "$1" ] && return 0; done
    return 1
}

# reader_cmd WORD ARGS... — 0 when the command never writes the files it names.
reader_cmd() {
    local cmd="${1##*/}" a sub=""
    shift
    one_of "$cmd" "$READERS" && return 0
    case "$cmd" in
        sed)
            for a in "$@"; do
                case "$a" in
                    --in-place*) return 1 ;;
                    *.logos*) ;;
                    *w*) return 1 ;; # a w command or s///w flag writes a file; a stray w only asks
                    --*) ;;
                    -*i*) return 1 ;; # -i, -i.bak, -ni; and BSD -I, as matching folds case
                esac
            done
            return 0
            ;;
        find)
            for a in "$@"; do
                case "$a" in -delete | -exec | -execdir | -ok | -okdir | -fprint | -fprint0 | -fprintf | -fls) return 1 ;; esac
            done
            return 0
            ;;
        git)
            for a in "$@"; do
                case "$a" in --output*) return 1 ;; esac # git diff/log --output=FILE
            done
            while [ $# -gt 0 ]; do
                case "$1" in
                    -C | -c | --git-dir | --work-tree) shift ;;
                    -*) ;;
                    *) sub="$1" && break ;;
                esac
                shift || true
            done
            one_of "$sub" "$GIT_READERS"
            return
            ;;
    esac
    return 1
}

# shell_segment TEXT — one simple command of a Bash call. Asks on a guarded
# action, returns otherwise. TEXT is cut at > and at < first: the first word
# after a > is a redirection target (`>>` leaves an empty field between its two
# halves, so the rule still lands on the target), and the first word after a <
# is read from, never written.
shell_segment() {
    local gts=() lts=() words=() targets=() j k w first
    split_on ">" "$1"
    gts=("${PARTS[@]}")
    for ((j = 0; j < ${#gts[@]}; j++)); do
        split_on "<" "${gts[j]}"
        lts=("${PARTS[@]}")
        for ((k = 0; k < ${#lts[@]}; k++)); do
            words_of "${lts[k]}"
            first=yes
            for w in ${WORDS[@]+"${WORDS[@]}"}; do
                if [ "$first" = yes ]; then
                    first=no
                    if [ "$k" -gt 0 ]; then continue; fi
                    if [ "$j" -gt 0 ]; then targets+=("$w") && continue; fi
                fi
                words+=("$w")
            done
        done
    done

    local cmd="" args=() mentions=() i=0 logos_at=-1 gate_at=-1 check_at=-1 security_at=-1
    for w in ${words[@]+"${words[@]}"}; do
        if [ -z "$cmd" ]; then
            case "$w" in
                [A-Za-z_]*=* | env | sudo | command | builtin | exec | nice | nohup | time | xargs | "{" | "!" | \
                    if | then | else | elif | do | while | until) continue ;;
                -*) continue ;;
            esac
            cmd="$w"
        else
            args+=("$w")
        fi
        case "${w##*/}" in
            logos) [ "$logos_at" -lt 0 ] && logos_at=$i ;;
        esac
        if [ "$logos_at" -ge 0 ] || [ "$BIN_NAMED" = yes ]; then
            case "$w" in
                gate) [ "$gate_at" -lt 0 ] && gate_at=$i ;;
                check) [ "$check_at" -lt 0 ] && check_at=$i ;;
                security) [ "$security_at" -lt 0 ] && security_at=$i ;;
                # Not ordered against logos_at, unlike gate and check: a global option's
                # value can be the word logos (`$L security --project logos ingest`).
                ingest) [ "$security_at" -ge 0 ] &&
                    ask "the command runs \`logos security ingest\`, which stores a SARIF report as the security evidence: an empty one clears a failing [security] gate." ;;
                bless) [ "$security_at" -ge 0 ] &&
                    ask "the command runs \`logos security bless\`, which rewrites .logos/security-baseline.json, the security ratchet's checked-in baseline." ;;
                --save) [ "$gate_at" -gt "$logos_at" ] &&
                    ask "the command runs \`logos gate --save\`, which replaces the blessed quality baseline." ;;
                --bless) [ "$check_at" -gt "$logos_at" ] &&
                    ask "the command runs \`logos check --bless\`, which rewrites .logos/rules-baseline.json." ;;
            esac
        fi
        i=$((i + 1))
    done

    for w in ${targets[@]+"${targets[@]}"}; do
        if mention "$w"; then ask "the command writes $HIT through a shell redirection."; fi
    done
    [ -n "$cmd" ] || return 0
    for w in "$cmd" ${args[@]+"${args[@]}"}; do
        if mention "$w"; then mentions+=("$HIT"); fi
    done
    [ "${#mentions[@]}" -gt 0 ] || return 0
    reader_cmd "$cmd" ${args[@]+"${args[@]}"} && return 0
    ask "the command \`${cmd##*/}\` may write or delete ${mentions[0]}."
}

shell_command() {
    local c="$1" seg segs=() i=1
    # Shell words, approximately: escapes and quotes dropped (so 'a'b is ab, and
    # `bash -c "..."` is read inline), then the command cut into simple commands
    # at every list and pipe operator.
    split_on "$BS" "$c"
    # A backslash-newline continues the line, so its newline is a blank, not a
    # command boundary (`logos gate \⏎ --save`). An empty field is an escaped
    # backslash, and the field after it starts with a literal character.
    while [ "$i" -lt "${#PARTS[@]}" ]; do
        if [ -z "${PARTS[i]}" ]; then i=$((i + 2)) && continue; fi
        case "${PARTS[i]}" in $'\n'*) PARTS[i]=" ${PARTS[i]:1}" ;; esac
        i=$((i + 1))
    done
    join_parts ""
    split_on "\"'" "$JOINED"
    join_parts ""
    c="$JOINED"
    # Read after the quotes go, so `cd ".lo"gos` counts as naming .logos.
    case "$c" in *.logos*) DOT_LOGOS=yes ;; *) DOT_LOGOS=no ;; esac
    # The binary named anywhere in the command, as a word or a path's last part
    # or an assignment's value: `$(which logos) gate --save` and
    # `L=logos; $L gate --save` put it in another simple command than the
    # subcommand, so a segment that saves or blesses then asks, failing safe.
    case " $c " in
        *[[:space:]/=\(\`\;\&\|]logos[[:space:]\)\`\;\&\|]*) BIN_NAMED=yes ;;
        *) BIN_NAMED=no ;;
    esac
    # Nothing guarded is named and nothing blessed, saved or ingested: the call passes
    # without the word-by-word walk, so a long heredoc costs one test. `*bless*`
    # covers both `check --bless` and `security bless`.
    case "$c" in
        *.logos* | *rules.toml* | *rules-baseline.json* | *security-ignore.toml* | *security-baseline.json* | \
            *--save* | *bless* | *ingest*) ;;
        *) return 0 ;;
    esac
    split_on $';&|()`\n' "$c"
    segs=("${PARTS[@]}")
    for seg in "${segs[@]}"; do shell_segment "$seg"; done
}

re='"tool_name"[[:space:]]*:[[:space:]]*"([^"\\]+)"'
[[ $PAYLOAD =~ $re ]] || malformed "it names no tool_name"
TOOL="${BASH_REMATCH[1]}"

case "$TOOL" in
    Write | Edit | MultiEdit | NotebookEdit)
        key=file_path
        [ "$TOOL" = NotebookEdit ] && key=notebook_path
        json_strings "$key"
        [ "${#STRINGS[@]}" -gt 0 ] || malformed "a $TOOL call with no $key"
        for f in "${STRINGS[@]}"; do
            if guarded_path "$f"; then ask "the agent's $TOOL would write $HIT ($f)."; fi
        done
        ;;
    Bash)
        json_strings command
        [ "${#STRINGS[@]}" -gt 0 ] || malformed "a Bash call with no command"
        for c in "${STRINGS[@]}"; do shell_command "$c"; done
        ;;
    mcp__logos__bless_rules | mcp__plugin_logos_logos__bless_rules)
        ask "the MCP tool $TOOL is \`logos check --bless\`: it rewrites .logos/rules-baseline.json."
        ;;
    mcp__logos__security_ingest | mcp__plugin_logos_logos__security_ingest)
        ask "the MCP tool $TOOL is \`logos security ingest\`: it stores a SARIF report as the security evidence, and an empty one clears a failing [security] gate."
        ;;
esac

exit 0
