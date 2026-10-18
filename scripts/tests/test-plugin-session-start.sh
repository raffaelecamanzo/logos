#!/usr/bin/env bash
#
# test-plugin-session-start.sh — prove the logos plugin's SessionStart hook
# (plugins/logos/hooks/session-start.sh, S-678, FR-IN-11, FR-IN-07) gives the
# readout behind the minimum-version handshake, and is silent outside an
# enrolled repository.
#
# The hook runs as the host runs it: executed directly, under `env -i`, with a
# PATH holding only a stub `logos` and `bash`. That proves the plugin scripts
# need nothing but bash builtins, and it puts no real logos binary in reach.
# Every end-to-end case runs under each distinct bash on this machine
# (/bin/bash is 3.2 on macOS).
#
# Layers:
#   1. lib/common.sh units: version compare, version parse, JSON strings;
#   2. the readout: a current, newer, older, absent and unreadable binary;
#   3. silence: not enrolled, .logos as a file, the off-switch, a failing readout;
#   4. the manifest: a minimum the hook cannot read, the hooks.json command
#      under a plugin root with a space in it.
# Versions are derived from plugin.json's metadata.minBinaryVersion, so the
# suite does not restate the pin.
#
# Usage: bash scripts/tests/test-plugin-session-start.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"
HOOK="$PLUGIN/hooks/session-start.sh"

WORK="${TMPDIR:-/tmp}/logos-plugin-session-start-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }

for f in "$HOOK" "$PLUGIN/hooks/lib/common.sh" "$PLUGIN/.claude-plugin/plugin.json"; do
    [ -f "$f" ] || { echo "FAIL  missing $f"; echo "0 passed, 1 failed (of 1)"; exit 1; }
done

MIN="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["metadata"]["minBinaryVersion"])' \
    "$PLUGIN/.claude-plugin/plugin.json")"
IFS=. read -r MAJ MNR PAT <<EOF
$MIN
EOF
if [ "$PAT" -gt 0 ]; then OLDER="$MAJ.$MNR.$((PAT - 1))"
elif [ "$MNR" -gt 0 ]; then OLDER="$MAJ.$((MNR - 1)).99"
else OLDER="$((MAJ - 1)).99.99"; fi
NEWER="$((MAJ + 1)).0.0"
# Numerically newer, lexically older: one more minor digit, led by a 1.
WIDER="$MAJ.1$(printf '%0*d' "${#MNR}" 0).0"
echo "minimum $MIN; older $OLDER; newer $NEWER; wider $WIDER"

# Each distinct bash on this machine.
BASHES=()
for b in /bin/bash "$(command -v bash)"; do
    [ -x "$b" ] || continue
    r="$(cd "$(dirname "$b")" && pwd -P)/$(basename "$b")"
    dup=no
    for s in "${BASHES[@]+"${BASHES[@]}"}"; do [ "$s" = "$r" ] && dup=yes; done
    [ "$dup" = yes ] || BASHES+=("$r")
done

# --- fixtures -------------------------------------------------------------------

ENROLLED="$WORK/enrolled"
mkdir -p "$ENROLLED/.logos" "$WORK/plain"
NOTENROLLED="$WORK/plain"
FILEDOT="$WORK/dotfile"
mkdir -p "$FILEDOT" && : >"$FILEDOT/.logos"

CALLS="$WORK/calls"
READOUT='{"systemMessage":"stub readout","hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"stub context"}}'

# stub <dir> <version-line> [version-rc] [readout-rc]: a `logos` that answers
# --version, records every other call (argv and cwd) and prints a readout.
stub() {
    mkdir -p "$1"
    cat >"$1/logos" <<EOF
#!/bin/sh
case "\$1" in
    --version) printf '%s\n' '$2'; exit ${3:-0} ;;
esac
printf '%s|%s\n' "\$*" "\$(pwd -P)" >>'$CALLS'
if [ ${4:-0} -ne 0 ]; then echo 'readout failed' >&2; exit ${4:-0}; fi
printf '%s\n' '$READOUT'
EOF
    chmod +x "$1/logos"
}
# A --version call is recorded too, for the cases that must not run logos at all.
stub_spy() {
    mkdir -p "$1"
    printf '#!/bin/sh\nprintf "%%s|spy\\n" "$*" >>"%s"\nprintf "logos %s\\n"\n' "$CALLS" "$2" >"$1/logos"
    chmod +x "$1/logos"
}

stub "$WORK/bin-current" "logos $MIN"
stub "$WORK/bin-newer" "logos $NEWER"
stub "$WORK/bin-wider" "logos $WIDER"
stub "$WORK/bin-pre" "logos $MIN-dev"
stub "$WORK/bin-build" "logos $MIN (abc1234 2026-10-16)"
stub "$WORK/bin-older" "logos $OLDER"
stub "$WORK/bin-other" "frobnicate $NEWER"
stub "$WORK/bin-vfail" "" 1
stub "$WORK/bin-rofail" "logos $MIN" 0 2
stub_spy "$WORK/bin-spy" "$MIN"
# A binary path JSON has to escape: a space, a double quote and a backslash.
ODD="$WORK/odd dir \"q\" b\\s"
stub "$ODD" "logos $OLDER"

# run <bash> <project-or-empty> <stub-dir-or-empty> [VAR=value ...] — runs the
# hook as the host does; sets OUT, ERR, RC. An empty project leaves
# CLAUDE_PROJECT_DIR unset and runs from $RUN_CWD.
run() {
    local bash="$1" proj="$2" bin="$3" sys="$WORK/sys-$$-${1//\//_}"
    shift 3
    mkdir -p "$sys" && ln -sf "$bash" "$sys/bash"
    local path="$sys"
    [ -n "$bin" ] && path="$bin:$sys"
    local envs=(HOME="$WORK" PATH="$path" CLAUDE_PLUGIN_ROOT="${PLUGIN_UNDER_TEST:-$PLUGIN}")
    [ -n "$proj" ] && envs+=(CLAUDE_PROJECT_DIR="$proj")
    : >"$CALLS"
    RC=0
    (cd "${RUN_CWD:-$WORK}" && printf '{"hook_event_name":"SessionStart","source":"startup","session_id":"t"}' |
        env -i "${envs[@]}" "$@" "${HOOK_UNDER_TEST:-$HOOK}") >"$WORK/out" 2>"$WORK/err" || RC=$?
    OUT="$(cat "$WORK/out")" ERR="$(cat "$WORK/err")"
}

# msg <field> — a field of the hook JSON in $OUT, or INVALID when $OUT is not one
# JSON object with SessionStart as its event.
msg() {
    printf '%s' "$OUT" | python3 -I -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
    assert isinstance(d, dict) and d["hookSpecificOutput"]["hookEventName"] == "SessionStart"
except Exception:
    print("INVALID"); sys.exit()
f = sys.argv[1]
print(d["systemMessage"] if f == "system" else d["hookSpecificOutput"]["additionalContext"])' "$1"
}
has() { case "$1" in *"$2"*) echo yes ;; *) echo no ;; esac; }

# --- 1. lib/common.sh units -----------------------------------------------------

lib() { (. "$PLUGIN/hooks/lib/common.sh" && "$@"); }
ge() { if lib logos_version_ge "$1" "$2"; then echo yes; else echo no; fi; }
expect "[lib] equal versions are >=" "$(ge 1.20.0 1.20.0)" yes
expect "[lib] a higher patch is >=" "$(ge 1.20.1 1.20.0)" yes
expect "[lib] a lower patch is not" "$(ge 1.20.0 1.20.1)" no
expect "[lib] a higher major beats a lower minor" "$(ge 2.0.0 1.99.99)" yes
expect "[lib] 1.19.9 is older than 1.20.0" "$(ge 1.19.9 1.20.0)" no
expect "[lib] 1.100.0 is newer than 1.20.0 (numeric, not lexical — near miss)" "$(ge 1.100.0 1.20.0)" yes
expect "[lib] 1.3.0 is older than 1.20.0 (numeric, not lexical — near miss)" "$(ge 1.3.0 1.20.0)" no
expect "[lib] a leading zero is base 10, not octal: 1.09.0 >= 1.8.0" "$(ge 1.09.0 1.8.0)" yes
expect "[lib] 0.99.99 is older than 1.0.0" "$(ge 0.99.99 1.0.0)" no

ver() { (lib logos_binary_version "$1") || echo NONE; }
expect "[lib] 'logos X.Y.Z' reads X.Y.Z" "$(ver "$WORK/bin-current/logos")" "$MIN"
expect "[lib] a pre-release suffix is dropped" "$(ver "$WORK/bin-pre/logos")" "$MIN"
expect "[lib] trailing build text is ignored" "$(ver "$WORK/bin-build/logos")" "$MIN"
expect "[lib] another program called logos has no readable version (near miss)" "$(ver "$WORK/bin-other/logos")" NONE
expect "[lib] a failing --version has no readable version" "$(ver "$WORK/bin-vfail/logos")" NONE
expect "[lib] the minimum is read from plugin.json" "$(lib logos_min_binary_version)" "$MIN"

JS="$(lib logos_json_string "a\"b\\c
d	e$(printf '\001')f")"
expect "[lib] a JSON string escapes quote, backslash, newline and tab, and drops other control bytes" \
    "$(printf '%s' "$JS" | python3 -I -c 'import json,sys; print(repr(json.loads(sys.stdin.read())))')" \
    "'a\"b\\\\c\\nd\\tef'"
JS="$(lib logos_json_string "x$(printf '\001')y")"
expect "[lib] ... a bare control byte is removed, not passed through raw" "$JS" '"xy"'

# --- 2-4. the hook, under each bash ---------------------------------------------

for B in "${BASHES[@]}"; do
    V="$("$B" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')"
    T="[bash $V]"

    # 2. the readout
    run "$B" "$ENROLLED" "$WORK/bin-current"
    expect "$T current binary: exit 0" "$RC" 0
    expect "$T current binary: stdout is the binary's readout, unchanged" "$OUT" "$READOUT"
    expect "$T current binary: the readout ran once, as --json quality-report --hook-json, in the project" \
        "$(cat "$CALLS")" "--json quality-report --hook-json|$ENROLLED"
    expect "$T current binary: nothing on stderr" "$ERR" ""

    for c in newer wider pre build; do
        run "$B" "$ENROLLED" "$WORK/bin-$c"
        expect "$T $c binary: the readout, exit 0" "$RC:$OUT" "0:$READOUT"
    done

    run "$B" "$ENROLLED" "$WORK/bin-older"
    expect "$T older binary: exit 0 (never blocks)" "$RC" 0
    expect "$T older binary: one valid SessionStart JSON object" "$(msg system | grep -c INVALID)" 0
    S="$(msg system)" C="$(msg context)"
    expect "$T older binary: the user message names the binary's version" "$(has "$S" "$OLDER")" yes
    expect "$T older binary: ... and the minimum" "$(has "$S" "$MIN")" yes
    expect "$T older binary: ... and where the binary is" "$(has "$S" "$WORK/bin-older/logos")" yes
    expect "$T older binary: the agent context names both versions" "$(has "$C" "$OLDER")$(has "$C" "$MIN")" yesyes
    expect "$T older binary: the readout is not run" "$(cat "$CALLS")" ""
    expect "$T older binary: nothing on stderr" "$ERR" ""

    run "$B" "$ENROLLED" ""
    expect "$T no binary: exit 0 (never blocks)" "$RC" 0
    S="$(msg system)" C="$(msg context)"
    expect "$T no binary: the user message says no binary was found" "$(has "$S" "no logos binary was found on PATH")" yes
    expect "$T no binary: ... and names the minimum" "$(has "$S" "$MIN or newer")" yes
    expect "$T no binary: the agent is told the MCP tools are unavailable" "$(has "$C" "MCP tools are unavailable")" yes

    run "$B" "$ENROLLED" "$WORK/bin-other"
    S="$(msg system)"
    expect "$T another program called logos: exit 0, says the version is unreadable" \
        "$RC:$(has "$S" "could not read the version of $WORK/bin-other/logos")" "0:yes"
    expect "$T another program called logos: ... names the minimum, runs no readout" \
        "$(has "$S" "$MIN or newer"):$(cat "$CALLS")" "yes:"
    run "$B" "$ENROLLED" "$WORK/bin-vfail"
    expect "$T a failing --version: exit 0, says the version is unreadable" \
        "$RC:$(has "$(msg system)" "could not read the version")" "0:yes"

    run "$B" "$ENROLLED" "$ODD"
    S="$(msg system)"
    expect "$T a binary path with a space, a quote and a backslash: still one valid JSON object" \
        "$(printf '%s' "$S" | grep -c INVALID)" 0
    expect "$T ... and the path reads back verbatim" "$(has "$S" "$ODD/logos")" yes

    # 3. silence
    for c in current older spy; do
        run "$B" "$NOTENROLLED" "$WORK/bin-$c"
        expect "$T not enrolled, $c binary: exit 0, no output at all" "$RC|$OUT|$ERR" "0||"
        expect "$T not enrolled, $c binary: logos is never run, not even --version" "$(cat "$CALLS")" ""
    done
    run "$B" "$NOTENROLLED" ""
    expect "$T not enrolled, no binary: exit 0, no output at all" "$RC|$OUT|$ERR" "0||"
    expect "$T not enrolled: no .logos/ is created" "$([ -e "$NOTENROLLED/.logos" ] && echo created || echo absent)" absent
    run "$B" "$FILEDOT" "$WORK/bin-spy"
    expect "$T a .logos that is a file is not a store (near miss): silent, no call" "$RC|$OUT|$ERR|$(cat "$CALLS")" "0|||"

    run "$B" "$ENROLLED" "$WORK/bin-spy" LOGOS_QUALITY_REPORT_DISABLE=1
    expect "$T off-switch: silent, logos never run" "$RC|$OUT|$ERR|$(cat "$CALLS")" "0|||"
    run "$B" "$ENROLLED" "" LOGOS_QUALITY_REPORT_DISABLE=1
    expect "$T off-switch with no binary: silent too" "$RC|$OUT|$ERR" "0||"
    run "$B" "$ENROLLED" "$WORK/bin-spy" LOGOS_QUALITY_REPORT_DISABLE=0
    expect "$T off-switch at 0 is not off (near miss): the readout runs" "$(grep -c 'quality-report' "$CALLS")" 1

    run "$B" "$ENROLLED" "$WORK/bin-rofail"
    expect "$T a failing readout: exit 0, no stdout, its stderr swallowed" "$RC|$OUT|$ERR" "0||"

    RUN_CWD="$ENROLLED" run "$B" "" "$WORK/bin-current"
    expect "$T CLAUDE_PROJECT_DIR unset: the cwd is the project" "$RC|$OUT|$(cat "$CALLS")" \
        "0|$READOUT|--json quality-report --hook-json|$ENROLLED"
    RUN_CWD="$NOTENROLLED" run "$B" "" "$WORK/bin-spy"
    expect "$T CLAUDE_PROJECT_DIR unset, cwd not enrolled: silent" "$RC|$OUT|$(cat "$CALLS")" "0||"
done

# --- 4. the manifest ------------------------------------------------------------

B="${BASHES[0]}"
NOMIN="$WORK/nomin"
mkdir -p "$NOMIN" && cp -R "$PLUGIN/." "$NOMIN/"
python3 -I - "$NOMIN/.claude-plugin/plugin.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["metadata"].pop("minBinaryVersion")
json.dump(d, open(p, "w"), indent=2)
PY
HOOK_UNDER_TEST="$NOMIN/hooks/session-start.sh" PLUGIN_UNDER_TEST="$NOMIN" run "$B" "$ENROLLED" "$WORK/bin-older"
expect "[manifest] no readable minimum: no handshake to make, the readout runs" "$RC|$OUT" "0|$READOUT"
HOOK_UNDER_TEST="$NOMIN/hooks/session-start.sh" PLUGIN_UNDER_TEST="$NOMIN" run "$B" "$ENROLLED" ""
S="$(msg system)"
expect "[manifest] no readable minimum, no binary: still says so, without a version" \
    "$RC|$(has "$S" "no logos binary was found on PATH")|$(has "$S" "needs logos;")" "0|yes|yes"

# The command hooks.json declares, run through a shell the way the host runs it,
# from a plugin root whose path has a space in it.
SPACED="$WORK/plugin root"
mkdir -p "$SPACED" && cp -R "$PLUGIN/." "$SPACED/"
CMD="$(python3 -I -c 'import json,sys
d = json.load(open(sys.argv[1]))
cmds = [h["command"] for g in d["hooks"]["SessionStart"] for h in g["hooks"]]
assert len(cmds) == 1, cmds
print(cmds[0])' "$PLUGIN/hooks/hooks.json")"
expect "[manifest] hooks.json names the session-start hook once" "$(has "$CMD" "session-start.sh")" yes

# The same session starts and the same budget as the readout hook `init -i`
# writes, read from its constants so the two cannot drift apart.
expect "[manifest] the matcher and timeout are the init -i hook's (quality_report.rs)" "$(python3 -I - "$PLUGIN/hooks/hooks.json" \
    "$HERE/logos-core/src/hooks/quality_report.rs" <<'PY'
import json, re, sys
g = json.load(open(sys.argv[1]))["hooks"]["SessionStart"][0]
src = open(sys.argv[2]).read()
m = re.search(r'const QUALITY_REPORT_HOOK_MATCHER: &str = "([^"]*)";', src)
t = re.search(r"const QUALITY_REPORT_HOOK_TIMEOUT_SECS: u64 = ([0-9]+);", src)
if not (m and t):
    print("constants not found")
else:
    print((g["matcher"], g["hooks"][0]["timeout"]) == (m.group(1), int(t.group(1))))
PY
)" True
SYS="$WORK/sys-cmd" && mkdir -p "$SYS" && ln -sf "$B" "$SYS/bash"
: >"$CALLS"
RC=0
OUT="$(cd "$WORK" && env -i HOME="$WORK" PATH="$WORK/bin-current:$SYS" CLAUDE_PLUGIN_ROOT="$SPACED" \
    CLAUDE_PROJECT_DIR="$ENROLLED" /bin/sh -c "$CMD" </dev/null 2>"$WORK/err")" || RC=$?
expect "[manifest] the hooks.json command runs from a plugin root with a space: the readout" "$RC|$OUT" "0|$READOUT"
expect "[manifest] ... with nothing on stderr" "$(cat "$WORK/err")" ""

echo
echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
