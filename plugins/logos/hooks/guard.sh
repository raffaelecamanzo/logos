#!/usr/bin/env bash
#
# guard.sh — the logos plugin's PreToolUse guardrail (FR-IN-14, BR-69): an agent
# never relaxes its own quality contract unseen. The host asks the human first
# when an agent tool call would
#
#   write, edit or delete    .logos/rules.toml, .logos/rules-baseline.json,
#                            .logos/security-ignore.toml or
#                            .logos/security-baseline.json (any .logos/ of the
#                            project, so a workspace member's contract is guarded too)
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
# In an unattended session (LOGOS_GUARD_MODE=unattended, FR-IN-17) no human is
# there to answer an ask, so every one of those asks, the malformed payload's
# included, is a "deny" instead: the same reason, then UNATTENDED_REASON. The
# session goes on and the contract still holds. LOGOS_GUARD_ALLOW, a
# colon-separated list of project-relative paths, names the contract files such a
# session may write with a file tool (Write, Edit, MultiEdit, NotebookEdit). It
# never admits a shell command that names one, nor any guarded command: those deny
# wherever the interactive guard would ask. Any other value of LOGOS_GUARD_MODE, or none, asks as above, and LOGOS_GUARD_ALLOW
# is then not read. No value of either turns the guard off.
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
# Four more spellings are read as the shell reads them, and a target placed outside
# the project passes (FR-IN-14 as amended by CR-267). Each relaxation holds only
# where the guard can show the shell agrees; anywhere else the text is read as above.
#   - `rtk` is a prefix, as env is, and so are `proxy`, `bypass`, `run`, `err`,
#     `test` and `summary` right after it: `rtk grep x .logos/rules.toml` is grep,
#     and rtk's own `read` (what the rtk hook makes of `cat`) is a reader.
#   - A heredoc body is data to the command that reads it (`python3 - <<'EOF'`).
#     The line holding `<<` is read as ever, so `cat > .logos/rules.toml <<EOF` asks.
#     No body is data when a shell, eval, source or a patch may read one (RUNS_TEXT:
#     `bash <<EOF`, `cat <<EOF | sh`, `$SHELL <<EOF`, `git apply <<EOF`), when an
#     unquoted body holds a substitution, or when a `<<` sits where the guard cannot
#     show it opens a heredoc (in quotes, a comment, `${…}`, arithmetic).
#   - A glob character quoted or escaped is literal (`sqlite3 db "select count(*)"`),
#     except on a line that may read its quoted text again (a shell or eval, a
#     command or process substitution), one whose command matches globs itself
#     (git, find), one holding $'…' or a comment, and under .logos/.
#   - An expansion glued to literal text, as the value of an option of logos's
#     that takes one (`--project $L/before`, `--rules=$D/r.toml`), is that value.
#     A bare one (`--rules=$L`) or a command substitution still asks: nothing in
#     the word says it is one value rather than words of its own.
#   - A target placed outside the project passes: an absolute or ~ path, or a
#     relative one read from the payload's cwd or after a leading `cd /DIR &&`,
#     whose directory lies outside the project root (CLAUDE_PROJECT_DIR; a linked
#     worktree's main checkout is the project too) or in a repository of its own
#     below it (a real .git directory, not in a logos workspace); and a guarded logos
#     command whose --project, or the directory the shell stands in, is such a
#     place (logos runs on the git top level holding it). What the guard cannot
#     place is judged as before: an expansion, `cd -`, any cd but a leading one, a
#     cd on a line with quoting, a subshell, pipe, background job or shell, a `..`
#     from the payload's cwd, a leading cd's or pushd's directory holding a `..`
#     (cd reads it as text), a file tool's path or a shell operand whose `..`
#     lands in the project read as text or through the links (git cleans a
#     pathspec as text), a relative operand under a directory option (`git -C`),
#     a target that is a symlink, a line that runs ln or mv, a directory that does
#     not exist yet, and anything at all when the host names no project.
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

# The project the guard holds the contract of: enrolment is proved on it, and a
# target is placed against it (S-847), so the two can never disagree.
PROJECT_DIR="$(logos_project_dir)"
logos_enrolled "$PROJECT_DIR" || exit 0

# Read before the payload, so a payload that cannot be read is denied too. Matched
# exactly, before nocasematch: `Unattended`, `off` or an empty value asks.
UNATTENDED=no
if [ "${LOGOS_GUARD_MODE:-}" = unattended ]; then UNATTENDED=yes; fi
UNATTENDED_REASON="This session runs unattended, so a guarded action is refused rather than asked. Record what you needed in your notes; the owner decides at review."

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
    strings cut column nl echo printf cd pushd type which true false logos read"
# git subcommands that leave the working-tree file as it is (add and commit record it).
GIT_READERS="diff log show status blame annotate ls-files ls-tree cat-file grep rev-parse shortlog check-ignore
    add commit"
# logos security's subcommands: until one of them is read, an expansion may become bless, ingest or reconfirm.
SECURITY_SUBS="scan install uninstall ingest status findings bless reconfirm help"
# An expansion or brace in the first eight words after logos, in one simple command: where
# logos's own subcommand may be (`logos $S`, `logos --json --quiet --project . {gate,x}`).
# One regex over a bounded run of words, so it is linear however long the command.
EXPANSION_NEAR_LOGOS=$'logos([^[:space:];&|()`]*[${]|([[:space:]]+[^[:space:];&|()`]+){0,7}[[:space:]]+[^[:space:];&|()`]*[${])'
# A command that may run text as a command line of its own: a shell named anywhere,
# by its last path part (`ssh h bash`, `| sh`), or eval, source or `.` as a command
# word. On a line that names one (outside a heredoc body that is data), a heredoc
# body and quoted text may be commands, and a cd may run in another shell.
SHELL_WORD_RE=$'(^|[[:space:];&|(`/])(bash|sh|zsh|dash|ksh|mksh|ash|yash|csh|tcsh|fish|busybox)([[:space:];&|)`]|$)|(^|[;&|(`{\n])[[:space:]]*((builtin|command)[[:space:]]+)?(eval|source|\\.)[[:space:]]'
# A command word that is an expansion (`$SHELL <<EOF`, `"$C" -c …`): it may be a shell.
EXPANDED_CMD_RE=$'(^|[;&|(`{\n])[[:space:]]*(([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*|env|sudo|exec|command|builtin|nohup|time|nice|xargs)[[:space:]]+)*["\']?[$`]'
# Commands that write the paths a heredoc body names (a patch).
PATCH_RE=$'(^|[[:space:];&|(`/])(patch|git[[:space:]]+(apply|am))([[:space:];&|)`]|$)'
# A comment: a # that starts a word.
COMMENT_RE=$'(^|[[:space:];&|(\n])#'
# Commands that match a glob themselves, quoted or not (git pathspecs, find -name).
PATTERN_TOOL_RE=$'(^|[[:space:];&|(`/])(git|find|fd|rsync|xargs)([[:space:];&|)`]|$)'
# A job sent to the background (`cd /x & rm …`): a lone &, not && nor a redirection's.
BACKGROUND_RE='(^|[^&<>])&([^&>]|$)'
# The options of logos's that take a value (`logos --help`, `logos check|gate|security
# reconfirm --help`, 1.22.1). One missing here only keeps its value asking.
VALUE_OPTIONS="--project --rules --threshold --label --reason --evidence"
# Stand-ins for a glob character that quotes or a backslash make literal (quote_globs).
STAR_LIT=$'\035' QMARK_LIT=$'\036' BRACKET_LIT=$'\037'

BS=\\

# decide_ask REASON — the answer wherever the guard holds a call: `ask`, or `deny`
# in an unattended session, whose reason then ends with UNATTENDED_REASON.
decide_ask() {
    local r="logos guardrail: $1" decision=ask
    # The reason quotes words the agent supplied; capped, so escaping it is cheap,
    # and cut back to a whole character, so the JSON stays valid UTF-8.
    if [ "${#r}" -gt 1000 ]; then
        r="${r:0:1000}"
        while [[ $r == *[$'\200'-$'\377'] ]]; do r="${r%?}"; done
        r="$r..."
    fi
    # Appended after the cap, so a long reason never cuts it off.
    if [ "$UNATTENDED" = yes ]; then decision=deny r="$r $UNATTENDED_REASON"; fi
    printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"%s","permissionDecisionReason":%s}}\n' \
        "$decision" "$(logos_json_string "$r")"
    exit 0
}

ask() {
    local m="$1"
    # A quoted word is shown with its literal glob characters back (quote_globs); cut
    # first, so the replacement is over a bounded text.
    case "$m" in
        *["$STAR_LIT$QMARK_LIT$BRACKET_LIT"]*)
            m="${m:0:1000}" m="${m//$STAR_LIT/*}" m="${m//$QMARK_LIT/?}" m="${m//$BRACKET_LIT/[}"
            ;;
    esac
    decide_ask "$m That relaxes the quality contract the review loop measures against, so a human decides (BR-69)."
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
# No PARTS join to "": bash 3.2 under `set -u` calls an empty "${PARTS[*]}" unbound
# and exits 1, which the host reads as a hook error and lets the call through. A
# path that normalizes to nothing (`.`, `/`, an empty LOGOS_GUARD_ALLOW entry) has none.
JOINED=""
join_parts() {
    local IFS="$1"
    JOINED="${PARTS[*]-}"
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

# allowed FILE — 0 when this session runs unattended and LOGOS_GUARD_ALLOW lists
# FILE, a file tool's path already normalized (the NORM guarded_path leaves; a
# relative one is read from the hook's working directory). It is not normalized
# again: normalize is quadratic in a path's `..` parts, and a second pass would
# halve the path length at which the guard runs past the host's timeout, which
# lets the call through. Both are compared as project-relative paths, exactly: no
# glob, no case folding. A FILE outside the project admits nothing; an entry that
# is empty, absolute or climbs out of the project with `..` never equals a path
# inside it, so it admits nothing either.
allowed() {
    [ "$UNATTENDED" = yes ] && [ -n "${LOGOS_GUARD_ALLOW:-}" ] || return 1
    local root real rel="" entry entries path="$1"
    normalize "$(logos_project_dir)"
    root="$NORM"
    real="$(cd "$root" 2>/dev/null && pwd -P)" || real=""
    case "$path" in /*) ;; *) normalize "$PWD/$path" && path="$NORM" ;; esac
    for entry in "$root" "$real"; do
        [ -n "$entry" ] && [ "${path:0:${#entry}+1}" = "$entry/" ] && rel="${path:${#entry}+1}" && break
    done
    [ -n "$rel" ] || return 1
    split_on : "$LOGOS_GUARD_ALLOW"
    entries=("${PARTS[@]}")
    for entry in "${entries[@]}"; do
        normalize "$entry"
        [ "$NORM" = "$rel" ] && return 0
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
            # A glob quoted under .logos/ is still one: git pathspecs and find -path match
            # it themselves (`git rm '.logos/*.toml'`), so its stand-ins are read back.
            case "$base" in
                *["$STAR_LIT$QMARK_LIT$BRACKET_LIT"]*) live_globs "$base" && base="$LIVE" ;;
            esac
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
# is turned away before any path work. With `scoped`, a spelling that reaches one
# is let go when placed_path puts it outside the project; the word is let go only
# when every spelling that reaches one is, so `of=/other/.logos/x` still asks (its
# whole word is a relative path the guard cannot place).
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
        if guarded_path "$t" || maybe_guarded "$t" "$DOT_LOGOS"; then
            [ "${2:-}" = scoped ] && placed_path "$t" && dotdot_placed "$t" && continue
            return 0
        fi
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

# --- The shell's text before its words (S-847) --------------------------------

# A text rebuilt piece by piece: bash 3.2 copies the whole string on every += and
# walks the whole array on every append, so either alone is quadratic (80000
# appends: 5 s). put PIECE gathers pieces in BUF, which joins the CHUNKS list every
# 512 characters; put_text sets TEXT to all of it.
BUF="" CHUNKS=() TEXT=""
put() {
    BUF+="$1"
    [ "${#BUF}" -le 512 ] || { CHUNKS+=("$BUF") && BUF=""; }
}
put_text() {
    CHUNKS+=("$BUF")
    PARTS=("${CHUNKS[@]}")
    join_parts ""
    TEXT="$JOINED" BUF="" CHUNKS=()
}

# strip_heredocs TEXT — HEREDOC_TEXT: TEXT less every heredoc body, which is data
# to the command reading it (shell_command keeps them all where a shell, eval or a
# patch may read one: RUNS_TEXT). The line holding `<<` stays, so its command, words and
# redirections are judged. Nothing is taken out when the guard cannot be sure of
# every heredoc in TEXT (HEREDOC_UNSURE): a delimiter line that never comes, a
# command substitution in a body the shell expands (`cat <<EOF` ⏎ `$(rm …)`), a
# `<<` it cannot place outside quotes, a comment or arithmetic (`$((1<<2))`, a
# quoted "<<"), or more than 64 of them.
HEREDOC_TEXT=""
HEREDOC_UNSURE=no
HEREDOC_COUNT=0
strip_heredocs() {
    local line t open=() sep=""
    HEREDOC_TEXT="$1" HEREDOC_UNSURE=no HEREDOC_COUNT=0
    BUF="" CHUNKS=()
    split_on $'\n' "$1"
    for line in "${PARTS[@]}"; do
        if [ "${#open[@]}" -gt 0 ]; then
            # An unquoted delimiter's body is expanded: a substitution in it runs, and a
            # backslash-newline may join one across lines (`$\` ⏎ `(rm …)`).
            # shellcheck disable=SC2016 # literal $( and $[, matched as text
            case "${open[0]}" in
                ?u*) case "$line" in *'$('* | *\`* | *'$['* | *"$BS") HEREDOC_UNSURE=yes && return 0 ;; esac ;;
            esac
            t="${line%$'\r'}"
            case "${open[0]}" in -*) t="${t#"${t%%[!$'\t']*}"}" ;; esac
            if [ "$t" = "${open[0]#??}" ]; then open=(${open[@]+"${open[@]:1}"}); fi
            continue
        fi
        put "$sep$line"
        sep=$'\n'
        case "$line" in *"<<"*) heredocs_of "$line" ;; esac
        [ "$HEREDOC_UNSURE" = no ] || return 0
    done
    [ "${#open[@]}" -eq 0 ] || return 0
    put_text
    HEREDOC_TEXT="$TEXT"
}

# heredocs_of LINE — onto strip_heredocs's `open`, the delimiter of each heredoc
# LINE opens, flagged - for `<<-` (its delimiter line may be indented by tabs) or +,
# then q for a quoted delimiter or u for one whose body the shell expands (a body
# line holding a command substitution then sets HEREDOC_UNSURE):
# `<<EOF`, `<<-EOF`, `<<'EOF'`, `<< "EOF"`, `<<\EOF`, never the
# here-string `<<<`. LINE is cut at
# every `<`: a run of two is `<<`. A `<<` counts only where the text before it on
# its line holds no quote, backtick, `#`, backslash, `((`, `$[`, `${` or `let` — or
# ends inside a `$(` that no single quote, backtick, `#` or backslash precedes, as
# `git commit -m "$(cat <<'EOF'` does (a " cannot make a $( literal); any other, or
# a line continued by a backslash, sets HEREDOC_UNSURE.
HEREDOC_IN_SUBST_RE=$'^[^\'`#\\\\]*\\$\\([^"\'`()#\\\\]*$'
HEREDOC_LET_RE='(^|[[:space:];&|(])let[[:space:]]'
heredocs_of() {
    local p pre="" run=-1 flag quoted re='^[[:space:]]*([^[:space:];&|<>()]+)'
    # A line continued by a backslash puts its next line before the body (`<<EOF \⏎ | sh`).
    case "$1" in *"$BS") HEREDOC_UNSURE=yes && return 0 ;; esac
    split_on "<" "$1"
    if [ "${#PARTS[@]}" -gt 66 ]; then HEREDOC_UNSURE=yes && return 0; fi
    for p in "${PARTS[@]}"; do
        run=$((run + 1))
        if [ -n "$p" ]; then
            if [ "$run" -eq 2 ]; then
                HEREDOC_COUNT=$((HEREDOC_COUNT + 1))
                [ "$HEREDOC_COUNT" -le 64 ] || { HEREDOC_UNSURE=yes && return 0; }
                # shellcheck disable=SC2016 # literal $[ and ${, matched as text
                case "${pre%<<}" in
                    *[\"\'\`#\\]* | *'(('* | *'$['* | *'${'*)
                        if ! [[ ${pre%<<} =~ $HEREDOC_IN_SUBST_RE ]]; then HEREDOC_UNSURE=yes && return 0; fi
                        ;;
                esac
                if [[ ${pre%<<} =~ $HEREDOC_LET_RE ]]; then HEREDOC_UNSURE=yes && return 0; fi
                flag=+
                case "$p" in -*) flag=- && p="${p:1}" ;; esac
                if [[ $p =~ $re ]]; then
                    quoted=u
                    case "${BASH_REMATCH[1]}" in *[\"\'\\]*) quoted=q ;; esac
                    # The delimiter is its word with the quotes gone: <<'EOF' ends at EOF.
                    split_on "\"'$BS" "${BASH_REMATCH[1]}"
                    join_parts ""
                    open+=("$flag$quoted$JOINED")
                fi
            fi
            run=0
        fi
        pre+="$p<"
    done
    return 0
}

# quote_globs TEXT — QUOTED: TEXT with each glob character that quotes or a
# backslash make literal (`"count(*)"`, `'a?'`, `\*`) replaced by its stand-in,
# which no rule reads as a glob. TEXT stays as it is where quoted text may be read
# again as a command line — a command or process substitution, or (shell_command's
# test) a shell or eval on the line (`bash -c "rm .logos/*"`) — where a command on
# it matches globs itself (git, find), where it holds $'…' or a comment, and where
# its quotes do not pair. Cut at
# every backslash, then each piece at every " and ': one pass, linear.
QUOTED=""
Q_STATE=out
quote_globs() {
    QUOTED="$1"
    case "$1" in *[\"\'\\]*) ;; *) return 0 ;; esac
    case "$1" in *[\*\?\[]*) ;; *) return 0 ;; esac
    case "$1" in *\`* | *\$\(* | *\<\(* | *\>\(*) return 0 ;; esac
    if [[ $1 =~ $PATTERN_TOOL_RE ]]; then return 0; fi
    # Quoting this scanner does not read: $'…' (where \' escapes the quote) and a
    # comment, whose apostrophe opens nothing (`# don't`).
    case "$1" in *"\$'"*) return 0 ;; esac
    if [[ $1 =~ $COMMENT_RE ]]; then return 0; fi
    local b c first=yes escaped=no
    Q_STATE=out BUF="" CHUNKS=()
    split_on "$BS" "$1"
    for b in "${PARTS[@]}"; do
        if [ "$first" = yes ]; then
            first=no
        elif [ "$Q_STATE" = sq ] || [ "$escaped" = yes ]; then
            # A backslash inside '…', or one a backslash escaped: a plain character.
            put "$BS"
            escaped=no
        else
            # This backslash makes the character after it literal. Inside "…" it
            # escapes only a few, but for what is read here the answer is the same:
            # a glob there is literal anyway, a ' is plain text, and \" is escaped.
            put "$BS"
            if [ -z "$b" ]; then escaped=yes && continue; fi
            c="${b:0:1}" b="${b:1}"
            case "$c" in
                \*) c="$STAR_LIT" ;;
                \?) c="$QMARK_LIT" ;;
                \[) c="$BRACKET_LIT" ;;
            esac
            put "$c"
        fi
        q_chunk "$b"
    done
    [ "$Q_STATE" = out ] || return 0
    put_text
    QUOTED="$TEXT"
}

# live_globs WORD — LIVE: WORD with each glob stand-in back to its glob character,
# cut and joined as q_chunk does (linear).
LIVE=""
live_globs() {
    local IFS g
    LIVE="$1"
    # shellcheck disable=SC2206 # split at the stand-in, the sentinel keeping a trailing empty field
    case "$LIVE" in *"$STAR_LIT"*)
        IFS="$STAR_LIT" && g=($LIVE$SENTINEL) && IFS='*' && LIVE="${g[*]}" && LIVE="${LIVE%"$SENTINEL"}" ;;
    esac
    # shellcheck disable=SC2206
    case "$LIVE" in *"$QMARK_LIT"*)
        IFS="$QMARK_LIT" && g=($LIVE$SENTINEL) && IFS='?' && LIVE="${g[*]}" && LIVE="${LIVE%"$SENTINEL"}" ;;
    esac
    # shellcheck disable=SC2206
    case "$LIVE" in *"$BRACKET_LIT"*)
        IFS="$BRACKET_LIT" && g=($LIVE$SENTINEL) && IFS='[' && LIVE="${g[*]}" && LIVE="${LIVE%"$SENTINEL"}" ;;
    esac
    return 0
}

# q_chunk TEXT — TEXT, which holds no backslash, put (onto BUF), read through Q_STATE
# (out, dq inside "…", sq inside '…'): each quote opens or closes as the shell
# reads it, and a glob character inside either is replaced by its stand-in.
# Called once per backslash, it splits and appends inline: a function call per
# piece costs more than the rest of the work (16000 quoted globs: 3 s through
# split_on and put, under 1 s inline).
q_chunk() {
    local d s g dnext=no snext IFS='"' dparts sparts
    # shellcheck disable=SC2206 # split at every ", the sentinel keeping a trailing empty field
    dparts=($1$SENTINEL)
    for d in "${dparts[@]}"; do
        d="${d%"$SENTINEL"}"
        if [ "$dnext" = yes ]; then
            BUF+='"'
            case "$Q_STATE" in out) Q_STATE=dq ;; dq) Q_STATE=out ;; esac
        fi
        dnext=yes snext=no
        case "$d" in
            *\'*)
                IFS="'"
                # shellcheck disable=SC2206
                sparts=($d$SENTINEL)
                ;;
            *) sparts=("$d") ;;
        esac
        for s in "${sparts[@]}"; do
            s="${s%"$SENTINEL"}"
            if [ "$snext" = yes ]; then
                BUF+="'"
                case "$Q_STATE" in out) Q_STATE=sq ;; sq) Q_STATE=out ;; esac
            fi
            snext=yes
            if [ "$Q_STATE" != out ]; then
                # Each glob character present: cut at it, join with its stand-in.
                # shellcheck disable=SC2206 # split at the glob character, as split_on does
                case "$s" in *\**)
                    IFS='*' && g=($s$SENTINEL) && IFS="$STAR_LIT" && s="${g[*]}" && s="${s%"$SENTINEL"}" ;;
                esac
                # shellcheck disable=SC2206
                case "$s" in *\?*)
                    IFS='?' && g=($s$SENTINEL) && IFS="$QMARK_LIT" && s="${g[*]}" && s="${s%"$SENTINEL"}" ;;
                esac
                # shellcheck disable=SC2206
                case "$s" in *\[*)
                    IFS='[' && g=($s$SENTINEL) && IFS="$BRACKET_LIT" && s="${g[*]}" && s="${s%"$SENTINEL"}" ;;
                esac
            fi
            BUF+="$s"
            [ "${#BUF}" -le 512 ] || { CHUNKS+=("$BUF") && BUF=""; }
        done
    done
}

# option_value WORD PREVIOUS — 0 when WORD is the value of one of VALUE_OPTIONS,
# given as `PREVIOUS WORD` or glued as `--opt=VALUE`, and the value is literal text
# built on plain parameter expansions ($NAME, ${NAME}): `$L/before`, `${OUT}.json`.
# A value that is expansions only, a command substitution (read as a word ending
# in $), ${NAME:-…}, a positional or special parameter and a brace expansion are
# not read as one: unquoted, they may split into an option of their own.
option_value() {
    local v="$1" glued='^(--[A-Za-z-]+)=(.*)$' \
        built='^([^$]+|\$[A-Za-z_][A-Za-z0-9_]*|\$\{[A-Za-z_][A-Za-z0-9_]*\})+$' \
        bare='^(\$[A-Za-z_][A-Za-z0-9_]*|\$\{[A-Za-z_][A-Za-z0-9_]*\})+$'
    if [[ $1 =~ $glued ]]; then
        one_of "${BASH_REMATCH[1]}" "$VALUE_OPTIONS" || return 1
        v="${BASH_REMATCH[2]}"
    else
        one_of "$2" "$VALUE_OPTIONS" || return 1
    fi
    [ "${#v}" -le 256 ] || return 1
    if brace_expansion "$v"; then return 1; fi
    [[ $v =~ $built ]] || return 1
    if [[ $v =~ $bare ]]; then return 1; fi
    return 0
}

# --- Where a target lies (S-847) -----------------------------------------------
# The guarded files are the project's: its root is PROJECT_DIR, the directory
# enrolment was proved on (CLAUDE_PROJECT_DIR, else this hook's own directory), so
# a payload's cwd can never move the root into a subdirectory. The shell starts in
# the payload's cwd. Both are read once, when a target is first placed, and
# compared physically (cd -P, no fork), so a symlink into the project is the
# project.

ROOT_P="" MAIN_P="" START="" PLACES=no PHYS=""
# physical DIR — PHYS: DIR with every symlink resolved; 1 when it is no directory.
physical() {
    local here="$PWD"
    builtin cd -P -- "$1" 2>/dev/null || return 1
    PHYS="$PWD"
    builtin cd -- "$here" 2>/dev/null || true
    return 0
}

init_places() {
    [ "$PLACES" = no ] || return 0
    PLACES=yes
    local cwd=""
    json_strings cwd
    [ "${#STRINGS[@]}" -eq 1 ] && cwd="${STRINGS[0]}"
    case "$cwd" in /*) physical "$cwd" && START="$PHYS" ;; esac
    # With no project named by the host, nothing is placed: the hook's own
    # directory may be a member below the project it stands for.
    [ -n "${CLAUDE_PROJECT_DIR:-}" ] || return 0
    physical "$PROJECT_DIR" && ROOT_P="$PHYS"
    # A linked worktree is its repository's: the main checkout (and the worktrees
    # it holds) is the project too, so its contract is never "elsewhere".
    local gitdir=""
    if [ -f "$ROOT_P/.git" ]; then
        read -r gitdir <"$ROOT_P/.git" 2>/dev/null || true
        case "$gitdir" in
            "gitdir: "*/.git/worktrees/*)
                gitdir="${gitdir#gitdir: }"
                physical "${gitdir%%/.git/worktrees/*}" && MAIN_P="$PHYS"
                ;;
        esac
    fi
    return 0
}

# AT — where the shell stands for the simple command being read: `start` (the
# payload's cwd), the directory a leading `cd` names (where_after), or "" where
# nothing places it.
AT=start

# where_is PATH — WHERE: PATH made absolute, a leading ~ read as $HOME and a
# relative one read from AT. 1 when PATH holds an expansion, is `~user`, or climbs
# (..) from the payload's cwd, which a session's own cd may have left stale; on a
# line that makes or moves links (PLACE_OK=no: `ln -s $ROOT /x && rm /x/.logos/…`);
# and, for a relative one, in a command with a directory of its own (REL_OK=no:
# `git -C $ROOT rm .logos/rules.toml`).
WHERE=""
PLACE_OK=yes REL_OK=yes
# A command that makes or moves a link or directory: what a path reaches may change on the line.
LINK_RE=$'(^|[[:space:];&|(`/])(ln|mv)([[:space:];&|)`]|$)'
where_is() {
    local p="$1"
    [ "$PLACE_OK" = yes ] && [ "${#p}" -le 4096 ] || return 1
    case "$p" in *\$*) return 1 ;; esac
    # shellcheck disable=SC2088 # a literal tilde, as the agent wrote it
    case "$p" in
        "~" | "~/"*)
            [ -n "${HOME:-}" ] && [ "$TILDE_OK" = yes ] || return 1
            p="$HOME${p:1}"
            ;;
        "~"*) return 1 ;;
        /*) ;;
        *)
            [ "$REL_OK" = yes ] || return 1
            case "$AT" in
                "") return 1 ;;
                start)
                    init_places
                    [ -n "$START" ] || return 1
                    case "/$p/" in */../*) return 1 ;; esac
                    p="$START/$p"
                    ;;
                *) p="$AT/$p" ;;
            esac
            ;;
    esac
    WHERE="$p"
}

# dir_elsewhere DIR — 0 when DIR, an absolute directory, is outside the project
# (its root, or the main checkout of the repository a worktree root belongs to),
# or below it inside a repository of its own (repo_dir), in a project that is no
# logos workspace, whose members are repositories too.
dir_elsewhere() {
    init_places
    [ -n "$ROOT_P" ] && [ "$ROOT_P" != / ] || return 1
    physical "$1" || return 1
    local d="$PHYS" area="$ROOT_P"
    if [ -n "$MAIN_P" ] && [[ $d == "$MAIN_P" || $d == "$MAIN_P"/* ]]; then area="$MAIN_P"; fi
    if [[ $d == "$area" || $d == "$area"/* ]]; then
        [ ! -e "$ROOT_P/logos.workspace.toml" ] || return 1
        while [ "${#d}" -gt "${#area}" ]; do
            if repo_dir "$d"; then return 0; fi
            d="${d%/*}"
        done
        return 1
    fi
    return 0
}

# repo_dir DIR — 0 when DIR holds a repository of its own, as git reads one: a
# .git directory with HEAD, objects/ and refs/. An empty .git, or a .git file (a
# worktree or submodule of the repository around it), is none.
repo_dir() {
    [ -d "$1/.git" ] && [ -f "$1/.git/HEAD" ] && [ -d "$1/.git/objects" ] && [ -d "$1/.git/refs" ]
}

# placed_path PATH — 0 when the directory holding PATH (a writer's target) is
# elsewhere. That directory must be spelled out: a glob or brace before the last
# part, or a directory that does not exist yet, is not placed; nor is a target
# that is itself a symlink, which may lead back into the project.
placed_path() {
    where_is "$1" || return 1
    local p="$WHERE"
    while :; do case "$p" in ?*/) p="${p%/}" ;; *) break ;; esac; done
    [ ! -L "$p" ] || return 1
    case "$p" in . | .. | */. | */..) ;; *) p="${p%/*}" && p="${p:-/}" ;; esac
    case "$p" in *[\*\?\[\{]*) return 1 ;; esac
    dir_elsewhere "$p"
}

# file_placed PATH NORMED — placed_path for a file tool's PATH, NORMED being PATH
# normalized (guarded_path's NORM). A `..` in it has two readings: through the
# links, as placed_path and the shell read it (PROJ/ext/.. is where ext leads,
# then up), and as text, as a host may resolve it before it writes (PROJ/ext/.. is
# PROJ). Such a path is placed only when both readings are elsewhere, so an agent's
# own `ln -s /outside ext` cannot carry a write back into the project. The `case`
# runs only on a PATH placed_path has already held to 4096 characters.
file_placed() {
    placed_path "$1" || return 1
    case "/$1/" in */../*) placed_path "$2" ;; *) return 0 ;; esac
}

# dotdot_placed WORD — the same rule for a shell word placed_path has already put
# elsewhere: a `..` in it is read through the links by the kernel, but as text by
# a writer that cleans its operand before it opens it (git's pathspecs:
# `git checkout -- PROJ/ext/../.logos/rules.toml` restores PROJ's file wherever
# ext leads). So the word is placed only when its text reading is elsewhere too.
dotdot_placed() {
    case "/$1/" in */../*) normalize "$1" && placed_path "$NORM" ;; *) return 0 ;; esac
}

# placed_dir DIR — 0 when DIR, a `--project` value or `.` for where the shell
# stands, is elsewhere.
placed_dir() {
    where_is "$1" || return 1
    case "$WHERE" in *[\*\?\[\{]*) return 1 ;; esac
    dir_elsewhere "$WHERE"
}

# logos_placed WORDS... — 0 when the logos these words run works on a project
# placed elsewhere: every --project DIR they give is, or, given none, the
# directory the shell stands in is. A word holding an expansion or a brace may
# add a --project of its own (the last one given wins), so none is placed.
logos_placed() {
    local w prev="" n=0
    for w in "$@"; do
        case "$w" in
            *\$*) return 1 ;;
            *\{*) if brace_expansion "$w"; then return 1; fi ;;
        esac
        if [ "$prev" = --project ]; then
            n=$((n + 1))
            placed_dir "$w" || return 1
        else
            case "$w" in --project=*)
                n=$((n + 1))
                placed_dir "${w#--project=}" || return 1
                ;;
            esac
        fi
        prev="$w"
    done
    [ "$n" -eq 0 ] || return 0
    placed_dir .
}

# logos_ask REASON — ask, unless this simple command's logos works on a project
# placed elsewhere (`logos --project /other check --bless` rewrites /other's
# baseline). Reads shell_segment's `words`, and caches the answer in its `lp`.
logos_ask() {
    if [ -z "$lp" ]; then
        lp=no
        if logos_placed ${words[@]+"${words[@]}"}; then lp=yes; fi
    fi
    [ "$lp" = yes ] || ask "$1"
    return 0
}

# where_after SEGMENT — AT for the simple commands after SEGMENT. A leading
# `cd DIR &&` or `pushd DIR &&` (DIR absolute or ~-rooted, nothing to expand, no
# `..`) moves it there, on a line where every command runs in this shell and nothing is quoted
# or escaped (TRACK_CD, set by shell_command); any other cd, pushd or popd, and a
# command word that is an expansion (`C=cd; $C /x`), leaves it unknown, as a
# failed cd would leave it unmoved.
CD_RE='(^|[[:space:]])(cd|pushd|popd)([[:space:]]|$)'
LEAD_CD_RE='^[[:space:]]*(cd|pushd)[[:space:]]+([^[:space:]]+)[[:space:]]*$'
LEAD_AND_RE='^[[:space:]]*(cd|pushd)[[:space:]]+[^[:space:];&|]+[[:space:]]*&&'
EXPANDED_WORD_RE='^[[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*[^[:space:]]*[$]'
LEAD=yes TRACK_CD=no TILDE_OK=yes QUOTING=no
where_after() {
    if [ "$LEAD" = yes ]; then
        case "$1" in *[![:space:]]*) LEAD=no ;; *) return 0 ;; esac
        if [ "$TRACK_CD" = yes ] && [[ $1 =~ $LEAD_CD_RE ]]; then
            local d="${BASH_REMATCH[2]}"
            AT=""
            # A .. in DIR is read by cd as text (PROJ/ext/.. is PROJ, wherever ext
            # leads), but by placed_path through the links, so DIR is not placed.
            case "/$d/" in */../*) return 0 ;; esac
            # shellcheck disable=SC2088 # a literal tilde, as the agent wrote it
            case "$d" in
                *[\$\*\?\[\{]*) ;;
                /*) AT="$d" ;;
                "~" | "~/"*) [ -z "${HOME:-}" ] || AT="$HOME${d:1}" ;;
            esac
            return 0
        fi
    fi
    case "$1" in
        *cd* | *pushd* | *popd*) if [[ $1 =~ $CD_RE ]]; then AT=""; fi ;;
    esac
    case "$1" in *\$*) if [[ $1 =~ $EXPANDED_WORD_RE ]]; then AT=""; fi ;; esac
    return 0
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
    # logos_ask reads `words` and caches its answer in `lp`, through bash's dynamic scope.
    local prev="" last="" after_rtk=no lp=""
    # A directory option (`git -C DIR`, `tar --directory=DIR`) moves the relative operands.
    REL_OK=yes
    if [ "$DOT_LOGOS" = yes ]; then
        for w in ${words[@]+"${words[@]}"}; do
            case "$w" in -C* | --directory | --directory=* | --chdir | --chdir=* | --git-dir | --git-dir=* | \
                --work-tree | --work-tree=* | --prefix | --prefix=*) REL_OK=no && break ;; esac
        done
    fi
    for w in ${words[@]+"${words[@]}"}; do
        prev="$last" last="$w"
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
                rtk) after_rtk=yes && continue ;;
            esac
            # rtk runs the command after it, as itself or through one of these.
            if [ "$after_rtk" = yes ]; then
                after_rtk=no
                case "$w" in proxy | bypass | run | err | test | summary) continue ;; esac
            fi
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
                    logos_ask "the command runs \`logos security ingest\`, which stores a SARIF report as the security evidence: an empty one clears a failing [security] gate." ;;
                bless) [ "$security_at" -ge 0 ] &&
                    logos_ask "the command runs \`logos security bless\`, which rewrites .logos/security-baseline.json, the security ratchet's checked-in baseline." ;;
                reconfirm) [ "$security_at" -ge 0 ] &&
                    logos_ask "the command runs \`logos security reconfirm\`, which re-records an ignore entry's evidence hashes in .logos/security-ignore.toml: a verdict whose evidence changed reads suppressed again." ;;
                --save) [ "$gate_at" -gt "$logos_at" ] &&
                    logos_ask "the command runs \`logos gate --save\`, which replaces the blessed quality baseline." ;;
                --bless) [ "$check_at" -gt "$logos_at" ] &&
                    logos_ask "the command runs \`logos check --bless\`, which rewrites .logos/rules-baseline.json." ;;
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
            # An option's value built on an expansion is that value (S-847).
            [ "$may" = yes ] && option_value "$w" "$prev" && may=value
            if [ "$may" = yes ]; then
                shown="\`$w\`"
                [ "$w" = "\$" ] && shown="a command substitution"
                own=no
                case "$cmd" in *\$*) own=yes ;; esac
                [ "$logos_at" -ge 0 ] && own=yes
                [ "$gate_at" -gt "$logos_at" ] && { [ "$own" = yes ] || [ "$gate_at" -eq 0 ]; } &&
                    logos_ask "the command gives \`logos gate\` $shown, which may expand to --save: \`logos gate --save\` replaces the blessed quality baseline."
                [ "$check_at" -gt "$logos_at" ] && { [ "$own" = yes ] || [ "$check_at" -eq 0 ]; } &&
                    logos_ask "the command gives \`logos check\` $shown, which may expand to --bless: \`logos check --bless\` rewrites .logos/rules-baseline.json."
                [ "$security_at" -ge 0 ] && [ "$security_sub" = no ] && { [ "$own" = yes ] || [ "$security_at" -eq 0 ]; } &&
                    logos_ask "the command gives \`logos security\` $shown before its subcommand, which may expand to bless, ingest or reconfirm: \`logos security bless\` rewrites .logos/security-baseline.json, \`logos security ingest\` stores a SARIF report as the security evidence, and \`logos security reconfirm\` re-records an ignore entry's evidence."
                [ "$logos_at" -eq 0 ] && [ "$i" -gt 0 ] && [ "$logos_sub" = no ] &&
                    logos_ask "the command gives \`logos\` $shown where its subcommand goes, which may expand to gate --save, check --bless or security bless: \`logos gate --save\` replaces the blessed quality baseline, \`logos check --bless\` rewrites .logos/rules-baseline.json, and \`logos security bless\` rewrites .logos/security-baseline.json."
            elif [ "$may" = no ]; then
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
        if mention "$w" scoped; then ask "the command writes $HIT through a shell redirection."; fi
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
        if mention "$w" scoped; then mentions+=("$HIT"); fi
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
    # Read before the escapes and quotes go: a heredoc body is data, and a quoted
    # glob is literal (S-847).
    # Decided over the text a shell would read: quoted heredoc bodies out first, so a
    # word in a data body (a commit message's "source") names no shell.
    local seen="$c"
    case "$c" in *"<<"*) strip_heredocs "$c" && seen="$HEREDOC_TEXT" ;; esac
    RUNS_TEXT=no
    if [[ $seen =~ $SHELL_WORD_RE ]] || [[ $seen =~ $EXPANDED_CMD_RE ]]; then RUNS_TEXT=yes; fi
    if [ "$RUNS_TEXT" = no ]; then
        [[ $seen =~ $PATCH_RE ]] || c="$seen"
        quote_globs "$c"
        c="$QUOTED"
    fi
    # Quotes and escapes go below, so what they held is no longer seen: a quoted `~`
    # reads as a home directory it is not (`cd "~/x"`), and a quoted `;` as a command
    # boundary. On such a line no ~ and no leading cd is placed.
    QUOTING=no TILDE_OK=yes PLACE_OK=yes
    if [[ $c =~ $LINK_RE ]]; then PLACE_OK=no; fi
    case "$c" in *[\"\'\\]*) QUOTING=yes ;; esac
    if [ "$QUOTING" = yes ]; then case "$c" in *"~"*) TILDE_OK=no ;; esac; fi
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
    # A leading cd places what follows only where every command runs in this shell:
    # a subshell, pipe, background job, substitution or other shell may cd alone.
    AT=start LEAD=yes TRACK_CD=yes
    case "$c" in *[\(\)\`\|]*) TRACK_CD=no ;; esac
    if [[ $c =~ $BACKGROUND_RE ]] || [ "$RUNS_TEXT" = yes ] || [ "$QUOTING" = yes ]; then TRACK_CD=no; fi
    # A failed cd lets a `;` or newline run on in the project: only `cd DIR &&` places.
    if ! [[ $c =~ $LEAD_AND_RE ]]; then TRACK_CD=no; fi
    for seg in "${segs[@]}"; do
        shell_segment "$seg"
        where_after "$seg"
    done
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
            # A file path is literal (no shell reads its ~), and a relative one is the
            # host's to resolve, not placed.
            TILDE_OK=no PLACE_OK=yes AT=""
            # An unattended session may write a contract file its LOGOS_GUARD_ALLOW lists.
            # file_placed leaves NORM as guarded_path set it: allowed reads that.
            if guarded_path "$f" && ! file_placed "$f" "$NORM" && ! allowed "$NORM"; then ask "the agent's $TOOL would write $HIT ($f)."; fi
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
