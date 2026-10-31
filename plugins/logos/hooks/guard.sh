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
#                            baseline; or logos security reconfirm, as a shell
#                            command or its MCP twin security_reconfirm — it
#                            rewrites an ignore entry's evidence hashes, so a
#                            verdict that stopped holding reads suppressed again
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
# Words are read as the shell reads them: a backslash-newline inside a word joins
# it (`logos security bl\⏎ess`), and `>|` and `>&` redirect as `>` does. After
# `logos security`, `logos gate` or `logos check`, an expansion or command
# substitution where a subcommand or option goes asks, since it may become one
# (`S=bless; logos security $S`); after a security subcommand
# (`logos security status $X`) it passes. A writer given an expansion asks when
# the same command line assigns a value naming .logos (`F=.logos/rules.toml; rm $F`);
# `for NAME in WORDS` assigns NAME the same way (the header itself writes nothing).
#
# A brace expansion, {a,b} or {a..b}, asks where an expansion asks
# (`logos gate --sa{ve,}`, `logos security {bless,status}`) and in a writer's path
# that can spell a file under .logos/ (`rm .lo{gos,}/rules.toml`). With logos as the
# command word, an expansion or brace before any subcommand of logos's is read asks
# (`S="gate --save"; logos $S`, `logos check$(…)`), as does logos glued to one
# (`logos$X`); `logos scan $DIR` passes. A brace is read by shape, never expanded,
# and quotes are already gone, so a quoted {a,b} asks too.
#
# This is a guardrail against an agent loosening the contract by habit or by
# mistake, not a sandbox: a command that assembles the path at run time (an
# interpreter one-liner, a path piped into xargs, a variable set in another Bash
# call) is not seen.
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
# through a $( ) fork per word. The same holds, more quietly, for a `case`
# pattern with two stars around a literal (`*=*\$*`), for a loop over a long
# array by index, and for ${word##*/} on one long word with no slash (100 KB:
# 10 s) — the words that name a guarded path (reader_cmd, maybe_guarded) still
# take it, as they did before; new code asks `case` for a path's last part.

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
# logos security's subcommands: until one of them is read, an expansion may become bless, ingest or reconfirm.
SECURITY_SUBS="scan install uninstall ingest status findings bless reconfirm help"
# An expansion or brace in the first eight words after logos, in one simple command: where
# logos's own subcommand may be (`logos $S`, `logos --json --quiet --project . {gate,x}`).
# One regex over a bounded run of words, so it is linear however long the command.
EXPANSION_NEAR_LOGOS=$'logos([^[:space:];&|()`]*[${]|([[:space:]]+[^[:space:];&|()`]+){0,7}[[:space:]]+[^[:space:];&|()`]*[${])'

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

# brace_expansion WORD — 0 when WORD holds an unquoted brace expansion, {a,b} or
# {a..b}. The shell turns one word into several, so the word may spell what it
# does not show (`--sa{ve,}`). It is read by shape, never expanded: an expansion
# multiplies, and bash 3.2 gives no linear bound for that. ${X} is no brace
# expansion (the `$` rules read it); a word too long to be read for one is taken
# for one.
brace_expansion() {
    case "$1" in *\{*) ;; *) return 1 ;; esac
    [ "${#1}" -le 256 ] || return 0
    local re='\{[^{}]*(,|\.\.)[^{}]*\}'
    [[ $1 =~ $re ]]
}

# names_dot_logos WORD — 0 when WORD names .logos as a path component or reaches a
# guarded file (mention): the value of an assignment, a word of a `for` list.
names_dot_logos() {
    case "$1" in .logos | .logos/* | */.logos | */.logos/*) return 0 ;; esac
    mention "$1"
}

# maybe_guarded TOKEN MENTIONS_DOT_LOGOS — a shell word that may reach a guarded
# file without spelling it: the .logos directory itself, a brace expansion that
# can spell a path under .logos/ (`.lo{gos,}/rules.toml`), a glob or brace list
# under .logos/ that can match a guarded name, or a bare guarded name, glob or
# brace list in a command line that also names .logos (`cd .logos && rm *`). 0,
# with HIT set to what it reaches, or 1.
maybe_guarded() {
    local p n base dir='(^|[/{,])\.logos($|[/},])'
    if brace_expansion "$1"; then
        # .logos as a path component, whole or opening a brace alternative.
        if [[ $1 =~ $dir ]]; then HIT="$1 (a brace expansion beside .logos)" && return 0; fi
        # A guarded name in a path whose component opens like .logos or a brace
        # (`.lo{gos,}/rules.toml`); `config/{rules.toml,x}` is no .logos.
        if [[ $1 == *.[l\{]* ]]; then
            for n in $GUARDED_NAMES; do
                case "$1" in *"$n"*) HIT="$1 (a brace expansion that can spell .logos/$n)" && return 0 ;; esac
            done
        fi
    fi
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
# action, returns otherwise, having set ASSIGNED (an assignment, or a `for`
# loop's words, naming .logos) and EXP_WRITE (a write handed an expansion) for
# shell_command to read
# once the whole line is walked. TEXT is cut at > and at < first: the first word
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

    local cmd="" args=() mentions=() i=0 logos_at=-1 gate_at=-1 check_at=-1 security_at=-1 security_sub=no logos_sub=no shown own may
    for w in ${words[@]+"${words[@]}"}; do
        # An assignment (or +=) whose value names .logos, for shell_command to read.
        # Matched by an anchored regex: a two-star `case` pattern is quadratic in bash 3.2.
        if [ "$DOT_LOGOS" = yes ]; then
            case "$w" in
                [A-Za-z_]*=*)
                    if [[ $w =~ ^[A-Za-z_][A-Za-z0-9_]*\+?=(.*)$ ]]; then
                        if names_dot_logos "${BASH_REMATCH[1]}"; then ASSIGNED="$w"; fi
                    fi
                    ;;
            esac
        fi
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
        case "$w" in
            logos | */logos) [ "$logos_at" -lt 0 ] && logos_at=$i ;;
        esac
        # The command word itself glued to an expansion (`logos$X`) may be logos and
        # anything: nothing says which subcommand it runs.
        if [ "$i" -eq 0 ]; then
            case "$w" in
                logos\$* | */logos\$* | logos\{* | */logos\{*)
                    if brace_expansion "$w" || [[ $w == *\$* ]]; then
                        ask "the command word \`$w\` is logos glued to an expansion, which may run \`logos gate --save\`, \`logos check --bless\` or \`logos security bless\`."
                    fi
                    ;;
            esac
        fi
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
                reconfirm) [ "$security_at" -ge 0 ] &&
                    ask "the command runs \`logos security reconfirm\`, which re-records an ignore entry's evidence hashes in .logos/security-ignore.toml: a verdict whose evidence changed reads suppressed again." ;;
                --save) [ "$gate_at" -gt "$logos_at" ] &&
                    ask "the command runs \`logos gate --save\`, which replaces the blessed quality baseline." ;;
                --bless) [ "$check_at" -gt "$logos_at" ] &&
                    ask "the command runs \`logos check --bless\`, which rewrites .logos/rules-baseline.json." ;;
            esac
            # An expansion where a guarded subcommand or option goes may become one when
            # the shell runs it (`S=bless; logos security $S`, `logos check $(echo --bless)`),
            # and so may a brace expansion (`logos gate --sa{ve,}`, `logos security {bless,status}`):
            # any word after gate or check, and any word after security until one of its
            # subcommands is read. A value glued to an option is no exception: unquoted,
            # `--label=$(echo x --save)` splits into a --save of its own. The subcommand
            # must be logos's own: the binary in this segment, a command word holding it
            # (`$L check`), or the subcommand at the head of a segment a substitution
            # left (`$(which logos) gate`), never `cd ~/src/logos && cargo check $X`.
            # With logos as the command word, the same holds before any subcommand of
            # logos's is read (`S="gate --save"; logos $S`, `logos check$(…)`), which asks
            # for a harmless `logos $CMD` too.
            may=no
            case "$w" in
                *\$*) may=yes ;;
                *\{*) brace_expansion "$w" && may=yes ;;
            esac
            if [ "$may" = yes ]; then
                shown="\`$w\`"
                [ "$w" = "\$" ] && shown="a command substitution"
                own=no
                case "$cmd" in *\$*) own=yes ;; esac
                [ "$logos_at" -ge 0 ] && own=yes
                [ "$gate_at" -gt "$logos_at" ] && { [ "$own" = yes ] || [ "$gate_at" -eq 0 ]; } &&
                    ask "the command gives \`logos gate\` $shown, which may expand to --save: \`logos gate --save\` replaces the blessed quality baseline."
                [ "$check_at" -gt "$logos_at" ] && { [ "$own" = yes ] || [ "$check_at" -eq 0 ]; } &&
                    ask "the command gives \`logos check\` $shown, which may expand to --bless: \`logos check --bless\` rewrites .logos/rules-baseline.json."
                [ "$security_at" -ge 0 ] && [ "$security_sub" = no ] && { [ "$own" = yes ] || [ "$security_at" -eq 0 ]; } &&
                    ask "the command gives \`logos security\` $shown before its subcommand, which may expand to bless, ingest or reconfirm: \`logos security bless\` rewrites .logos/security-baseline.json, \`logos security ingest\` stores a SARIF report as the security evidence, and \`logos security reconfirm\` re-records an ignore entry's evidence."
                [ "$logos_at" -eq 0 ] && [ "$i" -gt 0 ] && [ "$logos_sub" = no ] &&
                    ask "the command gives \`logos\` $shown where its subcommand goes, which may expand to gate --save, check --bless or security bless: \`logos gate --save\` replaces the blessed quality baseline, \`logos check --bless\` rewrites .logos/rules-baseline.json, and \`logos security bless\` rewrites .logos/security-baseline.json."
            else
                [ "$security_at" -ge 0 ] && [ "$security_at" -lt "$i" ] && one_of "$w" "$SECURITY_SUBS" && security_sub=yes
                # logos's own subcommands (`logos --help`), as a case, not a list walked per
                # word. One missing here only keeps the rule above armed, which asks.
                if [ "$logos_sub" = no ] && [ "$logos_at" -ge 0 ] && [ "$logos_at" -lt "$i" ]; then
                    case "$w" in
                        init | index | sync | status | search | query | context | explore | node | callers | callees | \
                            impact | impact-intersection | precedent | branch-overlap | affected | implements | \
                            referencing-docs | scan | check | quality-report | gate | health | session-start | \
                            session-end | doctor | verify | evolution | dsm | doc-gaps | offenders | review | \
                            improvements | hotspots | coverage | security | hook | xservice | workspace | stats | \
                            languages | serve | help) logos_sub=yes ;;
                    esac
                fi
            fi
        fi
        i=$((i + 1))
    done

    for w in ${targets[@]+"${targets[@]}"}; do
        if mention "$w"; then ask "the command writes $HIT through a shell redirection."; fi
    done
    # A write handed an expansion is held until the whole line is walked: it asks
    # only beside an assignment naming .logos (shell_command).
    if [ "$DOT_LOGOS" = yes ] && [ -z "$EXP_WRITE" ]; then
        for w in ${targets[@]+"${targets[@]}"}; do
            case "$w" in *\$*) EXP_WRITE="the command writes through a shell redirection into \`$w\`" && break ;; esac
        done
    fi
    [ -n "$cmd" ] || return 0
    # `for NAME in WORDS` assigns NAME each of WORDS, as `NAME=` does: one naming
    # .logos is read for shell_command like an assignment. The header writes nothing.
    if [ "$cmd" = for ] && [ "${args[1]:-}" = in ] && [[ ${args[0]} =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
        for w in "${args[@]:2}"; do
            if names_dot_logos "$w"; then
                ASSIGNED="the loop variable ${args[0]} (\`for ${args[0]} in … $w\`)"
                DOT_LOGOS=yes # a word spelled around .logos (`.lo{gos,}/rules.toml`) still names it
                break
            fi
        done
        return 0
    fi
    for w in "$cmd" ${args[@]+"${args[@]}"}; do
        if mention "$w"; then mentions+=("$HIT"); fi
    done
    if [ "$DOT_LOGOS" = yes ] && [ -z "$EXP_WRITE" ]; then
        for w in ${args[@]+"${args[@]}"}; do
            case "$w" in
                *\$*)
                    shown="\`$w\`"
                    [ "$w" = "\$" ] && shown="a command substitution"
                    reader_cmd "$cmd" ${args[@]+"${args[@]}"} || EXP_WRITE="the command \`${cmd##*/}\` is given $shown"
                    break
                    ;;
            esac
        done
    fi
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
    # A backslash-newline continues the line: the shell removes the pair, so the
    # word around it is joined (`logos security bl\⏎ess` is bless) and the newline
    # is no command boundary (`logos gate \⏎ --save`). An empty field is an escaped
    # backslash, and the field after it starts with a literal character. This loop
    # indexes PARTS, so it grows with the square of the backslash count (40000 take
    # some 20 s); it is the one exception to the rule above, and predates it.
    while [ "$i" -lt "${#PARTS[@]}" ]; do
        if [ -z "${PARTS[i]}" ]; then i=$((i + 2)) && continue; fi
        case "${PARTS[i]}" in $'\n'*) PARTS[i]="${PARTS[i]:1}" ;; esac
        i=$((i + 1))
    done
    join_parts ""
    split_on "\"'" "$JOINED"
    join_parts ""
    # A command substitution is cut out at its backticks below, which would leave
    # no word where it stood: each opening backtick is read as `$`, the word a
    # $( leaves, so `logos check \`echo --bless\`` holds an expansion after check.
    # The format takes the parts two at a time, so only an opening backtick gains
    # the `$`; the cycle's tail after the last part is cut off. One printf, not a
    # loop over PARTS[i]: bash 3.2 walks the array to reach each index.
    split_on '`' "$JOINED"
    # shellcheck disable=SC2016 # a literal $ and backticks
    printf -v JOINED '%s$`%s`' "${PARTS[@]}"
    c="${JOINED%\$\`\`}"
    ASSIGNED="" EXP_WRITE=""
    # Read after the quotes go, so `cd ".lo"gos` counts as naming .logos.
    case "$c" in *.logos*) DOT_LOGOS=yes ;; *) DOT_LOGOS=no ;; esac
    # The binary named anywhere in the command, as a word or a path's last part
    # or an assignment's value: `$(which logos) gate --save` and
    # `L=logos; $L gate --save` put it in another simple command than the
    # subcommand, so a segment that saves or blesses then asks, failing safe.
    # A `$` or `{` right after it is the binary glued to an expansion (`logos$X`).
    case " $c " in
        *[[:space:]/=\(\`\;\&\|]logos[[:space:]\)\`\;\&\|\$\{]*) BIN_NAMED=yes ;;
        *) BIN_NAMED=no ;;
    esac
    # Nothing guarded is named, nothing blessed, saved, ingested or reconfirmed, and no expansion
    # or brace sits beside the binary: the call passes without the word-by-word walk,
    # so a long heredoc costs one test. `*bless*` covers both `check --bless` and
    # `security bless`. An expansion or brace walks when gate, check or security is
    # named, or when one sits where logos's subcommand goes (EXPANSION_NEAR_LOGOS).
    case "$c" in
        *.logos* | *rules.toml* | *rules-baseline.json* | *security-ignore.toml* | *security-baseline.json* | \
            *--save* | *bless* | *ingest* | *reconfirm*) ;;
        *\$* | *\{*)
            [ "$BIN_NAMED" = yes ] || return 0
            case "$c" in *gate* | *check* | *security*) ;; *) [[ $c =~ $EXPANSION_NEAR_LOGOS ]] || return 0 ;; esac
            ;;
        *) return 0 ;;
    esac
    # `>|` and `>&` write their target as `>` does. Cut at | and &, the target would
    # read as a command of its own, so the character after each > becomes a blank.
    split_on ">" "$c"
    PARTS=("${PARTS[@]/#[|&]/ }")
    join_parts ">"
    split_on $';&|()`\n' "$JOINED"
    segs=("${PARTS[@]}")
    for seg in "${segs[@]}"; do shell_segment "$seg"; done
    # Read once the whole line is walked, so the assignment counts on either side of
    # the writer. No variable is tracked across separate Bash calls.
    if [ -n "$EXP_WRITE" ] && [ -n "$ASSIGNED" ]; then
        ask "$EXP_WRITE, in a command line that assigns $ASSIGNED, so it may write or delete what that value names under .logos/."
    fi
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
    mcp__logos__security_reconfirm | mcp__plugin_logos_logos__security_reconfirm)
        ask "the MCP tool $TOOL is \`logos security reconfirm\`: it re-records an ignore entry's evidence hashes in .logos/security-ignore.toml, so a verdict whose evidence changed reads suppressed again."
        ;;
esac

exit 0
