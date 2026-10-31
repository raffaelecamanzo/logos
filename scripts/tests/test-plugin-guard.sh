#!/usr/bin/env bash
#
# test-plugin-guard.sh — prove the logos plugin's PreToolUse guardrail
# (plugins/logos/hooks/guard.sh, S-680, FR-IN-14, BR-69) asks the human before an
# agent relaxes the quality contract, and lets everything else through.
#
# The hook runs as the host runs it: executed directly, under `env -i`, with a
# PATH holding only `bash`, so it is proved to need nothing but bash builtins.
# Every case runs under each distinct bash on this machine (/bin/bash is 3.2 on
# macOS).
#
# Layers:
#   1. the eight guarded actions, one fixture each, the reason naming its target;
#   2. other spellings of the same actions: the MCP twin under both tool
#      prefixes, other file tools, path spellings, shell writes and invocations;
#      (S-801: a brace expansion in a guarded position, an expansion as logos's
#      own subcommand, and a loop variable);
#   3. pass-through: unguarded .logos/ edits, reads of guarded files, unrelated
#      commands, and the near miss of each matcher;
#   4. a malformed payload asks; an unenrolled project is silent;
#   5. hooks.json registers the guard for every tool it guards, and its command
#      runs from a plugin root with a space in it.
#
# Usage: bash scripts/tests/test-plugin-guard.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"
HOOK="$PLUGIN/hooks/guard.sh"

WORK="${TMPDIR:-/tmp}/logos-plugin-guard-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }

for f in "$HOOK" "$PLUGIN/hooks/lib/common.sh" "$PLUGIN/hooks/hooks.json"; do
    [ -f "$f" ] || { echo "FAIL  missing $f"; echo "0 passed, 1 failed (of 1)"; exit 1; }
done

# Each distinct bash on this machine.
BASHES=()
for b in /bin/bash "$(command -v bash)"; do
    [ -x "$b" ] || continue
    r="$(cd "$(dirname "$b")" && pwd -P)/$(basename "$b")"
    dup=no
    for s in "${BASHES[@]+"${BASHES[@]}"}"; do [ "$s" = "$r" ] && dup=yes; done
    [ "$dup" = yes ] || BASHES+=("$r")
done

# A 300-character word and an unclosed brace, kept in variables: bash 3.2 brace-expands a
# double-quoted string inside "$( )" that holds a brace pair.
LONG_WORD="$(printf 'x%.0s' $(seq 1 300))"
OPEN_BRACE='{a,'
ENROLLED="$WORK/enrolled"
PLAIN="$WORK/plain"
mkdir -p "$ENROLLED/.logos" "$PLAIN"

# json STRING — STRING as a JSON string literal, for building payloads.
json() { python3 -I -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"; }
file_payload() { # tool key path
    printf '{"session_id":"t","hook_event_name":"PreToolUse","tool_name":"%s","tool_input":{"%s":%s,"content":"x"}}' \
        "$1" "$2" "$(json "$3")"
}
bash_payload() { # command
    printf '{"session_id":"t","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":%s,"description":"d"}}' \
        "$(json "$1")"
}
mcp_payload() { # tool
    printf '{"session_id":"t","hook_event_name":"PreToolUse","tool_name":"%s","tool_input":{}}' "$1"
}

# run <bash> <project> — the payload on stdin; sets OUT, ERR, RC.
run() {
    local sys="$WORK/sys-${1//\//_}"
    mkdir -p "$sys" && ln -sf "$1" "$sys/bash"
    RC=0
    (cd "$2" && env -i HOME="$WORK" PATH="$sys" CLAUDE_PLUGIN_ROOT="$PLUGIN" CLAUDE_PROJECT_DIR="$2" \
        ${RUN_LANG:+LANG="$RUN_LANG"} "$HOOK") >"$WORK/out" 2>"$WORK/err" || RC=$?
    OUT="$(cat "$WORK/out")" ERR="$(cat "$WORK/err")"
}

# decision — "ask|<reason>" when OUT is one valid PreToolUse ask object, "pass"
# when OUT is empty, INVALID otherwise.
decision() {
    printf '%s' "$OUT" | python3 -I -c '
import json, sys
s = sys.stdin.read()
if s == "":
    print("pass"); sys.exit()
try:
    d = json.loads(s)
    h = d["hookSpecificOutput"]
    assert set(d) == {"hookSpecificOutput"} and h["hookEventName"] == "PreToolUse"
    assert h["permissionDecision"] == "ask" and h["permissionDecisionReason"]
except Exception:
    print("INVALID"); sys.exit()
print("ask|" + h["permissionDecisionReason"])'
}

# asks <label> <payload> <target> — the guard asks, exit 0, stderr empty, and
# the reason names <target>. The malformed-payload reason lists every guarded
# file, so it would name any target: it counts only where it is the one wanted.
# passes <label> <payload> [project] — exit 0, no output.
B=""
T=""
asks() {
    local d
    printf '%s' "$2" >"$WORK/in"
    run "$B" "$ENROLLED" <"$WORK/in"
    d="$(decision)"
    case "$3:$d" in
        "could not be read:"*) ;;
        *"could not be read"*) d="malformed instead: $d" ;;
    esac
    case "$d" in
        "ask|"*"$3"*)
            if [ "$RC|$ERR" = "0|" ]; then ok "$T $1"; else bad "$T $1: exit $RC, stderr '$ERR'"; fi
            ;;
        *) bad "$T $1: wanted an ask naming '$3', got '$d' (exit $RC, stderr '$ERR')" ;;
    esac
}
passes() {
    printf '%s' "$2" >"$WORK/in"
    run "$B" "${3:-$ENROLLED}" <"$WORK/in"
    expect "$T $1" "$RC|$OUT|$ERR" "0||"
}

# bounded asks|passes <label> <payload-file> [target] — the case, decided in at
# most BOUND seconds (well inside the 10 s hooks.json timeout).
BOUND=5
bounded() {
    local kind="$1" label="$2" file="$3" start=$SECONDS
    "$kind" "$label" "$(cat "$file")" ${4+"$4"}
    expect "$T ... decided within ${BOUND}s" "$([ $((SECONDS - start)) -le "$BOUND" ] && echo yes || echo "no ($((SECONDS - start))s)")" yes
}
python3 -I - "$WORK" <<'PY'
import json, sys, os
w = sys.argv[1]
body = "\n".join(" ".join("word%d" % (i * 8 + j) for j in range(8)) for i in range(375))
def save(name, tool, tool_input):
    # Raw UTF-8, as the host sends it: the guard does not decode \u escapes.
    with open(os.path.join(w, name), "w", encoding="utf-8") as f:
        json.dump({"session_id": "t", "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": tool_input},
                  f, ensure_ascii=False)
save("long-rm.json", "Bash", {"command": "cat > notes.txt <<EOF\n" + body + "\nEOF\nrm .logos/rules.toml"})
save("long-benign.json", "Bash", {"command": "git commit -F - <<EOF\n" + body + "\nEOF"})
save("big-write.json", "Write", {"content": "x = \"y\"\n" * 120000, "file_path": ".logos/rules.toml"})
save("big-edit.json", "Edit", {"old_string": "a \"quoted\" line\n" * 15000, "new_string": "b", "file_path": ".logos/rules-baseline.json"})
save("quotes.json", "Bash", {"command": "printf %s " + "x\"y" * 8000})
save("multibyte-rm.json", "Bash", {"command": "echo " + "\u00e0b " * 6000 + "&& rm .logos/rules.toml"})
save("utf8-path.json", "Write", {"file_path": "/tmp/citt\u00e0/.logos/rules.toml"})
save("long-path.json", "Write", {"file_path": "/tmp/" + "\u00e0" * 1500 + "/.logos/rules.toml"})
save("dollars.json", "Bash", {"command": "logos scan && echo " + "$x " * 8000})
save("dollars-rm.json", "Bash", {"command": "F=.logos/rules.toml; echo " + "$x " * 8000 + "; rm $F"})
save("backticks.json", "Bash", {"command": "logos scan && echo " + "`x` " * 4000})
save("continuations.json", "Bash", {"command": "logos security status " + "a\\\n" * 8000})
save("backticks-80k.json", "Bash", {"command": "echo " + "`x` " * 80000})
save("equals-word.json", "Bash", {"command": "logos security status " + "=" * 80000 + " $X"})
save("equals-assign.json", "Bash", {"command": "F=.logos/rules.toml; X=" + "=" * 160000 + "; rm notes.txt"})
save("dollar-segments.json", "Bash", {"command": "cargo check $x;" * 64000})
save("dollars-scan.json", "Bash", {"command": "logos scan && echo " + "$x " * 80000})
# S-801: braces, an expanded subcommand and loop variables, each sized past a quadratic shape.
save("braces-security.json", "Bash", {"command": "logos security status " + "{a,b} " * 8000})
save("braces-reader.json", "Bash", {"command": "ls .logos " + "{a,b} " * 4000})
save("brace-long-word.json", "Bash", {"command": "logos scan " + "{a," * 15000})
save("expansions-scan.json", "Bash", {"command": "logos scan " + "$x " * 10000})
save("logos-words.json", "Bash", {"command": "echo $x " + "logos a " * 40000})
save("logos-then-words.json", "Bash", {"command": "logos " + "a " * 40000 + "$x"})
save("logos-long-word.json", "Bash", {"command": "logos scan $x; " + "a" * 200000})
save("for-words.json", "Bash", {"command": "for F in " + "a " * 10000 + ".logos/rules.toml; do rm $F; done"})
save("for-segments.json", "Bash", {"command": "for F in a; do echo $F; done\n" * 1000 + "ls .logos"})
PY

for B in "${BASHES[@]}"; do
    # shellcheck disable=SC2016 # expanded by the bash under test
    T="[bash $("$B" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')]"

    # --- 1. the eight guarded actions -------------------------------------------
    asks "Write .logos/rules.toml asks, naming the file" \
        "$(file_payload Write file_path "$ENROLLED/.logos/rules.toml")" ".logos/rules.toml"
    asks "Edit .logos/rules-baseline.json asks, naming the file" \
        "$(file_payload Edit file_path "$ENROLLED/.logos/rules-baseline.json")" ".logos/rules-baseline.json"
    asks "Write .logos/security-ignore.toml asks, naming the file" \
        "$(file_payload Write file_path "$ENROLLED/.logos/security-ignore.toml")" ".logos/security-ignore.toml"
    asks "Bash 'logos gate --save' asks, naming the command" \
        "$(bash_payload 'logos gate --save --label release')" "logos gate --save"
    asks "Bash 'logos check --bless' asks, naming the command" \
        "$(bash_payload 'logos check --bless')" "logos check --bless"
    asks "Bash 'logos security ingest' asks, naming the command" \
        "$(bash_payload 'logos security ingest empty.sarif')" "logos security ingest"
    asks "Write .logos/security-baseline.json asks, naming the file" \
        "$(file_payload Write file_path "$ENROLLED/.logos/security-baseline.json")" ".logos/security-baseline.json"
    asks "Bash 'logos security bless' asks, naming the command" \
        "$(bash_payload 'logos security bless')" "logos security bless"
    # S-818 (FR-SC-32): re-confirming a suppression's evidence rewrites its hashes,
    # so a verdict whose code changed reads suppressed again. Held like an entry write.
    asks "Bash 'logos security reconfirm' asks, naming the command" \
        "$(bash_payload 'logos security reconfirm 0a7c3e51d9b84f26a1c05e8d7b3f9246e1d08a5c7b2f4963d8e0a1b5c7f92e43')" \
        "logos security reconfirm"
    asks "logos security reconfirm by path, with a global flag and --evidence" \
        "$(bash_payload '~/.logos-bin/1.20.0/logos --json security reconfirm --evidence DefaultCookieConfig abc')" \
        "logos security reconfirm"
    asks "logos security reconfirm after another command in a list" \
        "$(bash_payload 'cargo build && logos security reconfirm abc')" "logos security reconfirm"
    asks "logos security reconfirm held in a variable" \
        "$(bash_payload 'L=logos; $L security reconfirm abc')" "logos security reconfirm"
    asks "logos security reconfirm inside bash -c" \
        "$(bash_payload 'bash -c "logos security reconfirm abc"')" "logos security reconfirm"
    asks "a backslash-newline inside 'reconfirm' asks" "$(bash_payload 'logos security recon\
firm abc')" "logos security reconfirm"
    asks "a subcommand left to an expansion may be reconfirm" \
        "$(bash_payload 'S=reconfirm; logos security $S abc')" 'which may expand to bless, ingest or reconfirm'
    asks "a brace expansion may complete reconfirm" \
        "$(bash_payload 'logos security {reconfirm,status} abc')" 'gives `logos security` `{reconfirm,status}`'

    # --- 2. other spellings of the same actions ---------------------------------
    # A re-bless's reason (S-755, FR-GV-16) never waives the guard: an agent is
    # still asked, before or after the flag.
    asks "logos gate --save --reason asks, naming the command" \
        "$(bash_payload 'logos gate --save --reason "Equality drift from a new dispatcher"')" "logos gate --save"
    asks "logos gate --reason=… --save asks, naming the command" \
        "$(bash_payload 'logos gate --reason=intrinsic --save')" "logos gate --save"
    asks "the MCP twin under the plugin prefix asks, naming the command" \
        "$(mcp_payload mcp__plugin_logos_logos__bless_rules)" "logos check --bless"
    asks "the MCP twin under the project .mcp.json prefix asks" \
        "$(mcp_payload mcp__logos__bless_rules)" "logos check --bless"
    asks "the security-ingest MCP tool under the plugin prefix asks, naming the command" \
        "$(mcp_payload mcp__plugin_logos_logos__security_ingest)" "logos security ingest"
    asks "the security-ingest MCP tool under the project .mcp.json prefix asks" \
        "$(mcp_payload mcp__logos__security_ingest)" "logos security ingest"
    asks "the security-reconfirm MCP tool under the plugin prefix asks, naming the command" \
        "$(mcp_payload mcp__plugin_logos_logos__security_reconfirm)" "logos security reconfirm"
    asks "the security-reconfirm MCP tool under the project .mcp.json prefix asks" \
        "$(mcp_payload mcp__logos__security_reconfirm)" "logos security reconfirm"
    # shellcheck disable=SC2088 # a literal tilde, as an agent writes it
    asks "logos security ingest by path, with a global flag and --json" \
        "$(bash_payload '~/.logos-bin/1.20.0/logos --json security ingest --json empty.sarif')" "logos security ingest"
    asks "logos security ingest behind env and an assignment" \
        "$(bash_payload 'FOO=1 env logos security ingest --project . r.sarif')" "logos security ingest"
    asks "logos security ingest after another command in a list" \
        "$(bash_payload 'cargo build && logos security ingest r.sarif')" "logos security ingest"
    # shellcheck disable=SC2016 # a literal $L, for the guard to read
    asks "logos security ingest held in a variable, with a global option whose value is logos" \
        "$(bash_payload 'L=logos; $L security --project logos ingest r.sarif')" "logos security ingest"
    asks "logos security ingest with a global option whose value is logos" \
        "$(bash_payload 'logos security --project logos ingest r.sarif')" "logos security ingest"
    asks "logos security ingest inside bash -c" \
        "$(bash_payload 'bash -c "logos security ingest r.sarif"')" "logos security ingest"
    asks "a line continuation inside logos security ingest asks" "$(bash_payload 'logos security \
    ingest r.sarif')" "logos security ingest"
    asks "a multi-line command ending in logos security ingest" "$(bash_payload 'git status
logos security ingest r.sarif')" "logos security ingest"
    # shellcheck disable=SC2016 # literal substitutions, for the guard to read
    asks "logos security ingest found by \$(which logos)" \
        "$(bash_payload '$(which logos) security ingest r.sarif')" "logos security ingest"
    # shellcheck disable=SC2016
    asks "logos security ingest found by \`command -v logos\`" \
        "$(bash_payload '`command -v logos` security ingest r.sarif')" "logos security ingest"
    # shellcheck disable=SC2016
    asks "logos security ingest held in a variable" \
        "$(bash_payload 'L=logos; $L security ingest r.sarif')" "logos security ingest"
    asks "MultiEdit of a guarded file asks" \
        "$(file_payload MultiEdit file_path "$ENROLLED/.logos/rules.toml")" ".logos/rules.toml"
    asks "NotebookEdit's notebook_path is read" \
        "$(file_payload NotebookEdit notebook_path ".logos/rules-baseline.json")" ".logos/rules-baseline.json"
    passes "NotebookEdit of an unguarded notebook passes" "$(file_payload NotebookEdit notebook_path "nb/analysis.ipynb")"
    asks "a relative path asks" "$(file_payload Edit file_path ".logos/rules.toml")" ".logos/rules.toml"
    asks "a workspace member's contract asks" \
        "$(file_payload Edit file_path "$ENROLLED/web/.logos/rules.toml")" ".logos/rules.toml"
    asks "'./.logos//rules.toml' is the same file" "$(file_payload Write file_path "./.logos//rules.toml")" ".logos/rules.toml"
    asks "'.logos/x/../rules.toml' is the same file" \
        "$(file_payload Write file_path ".logos/x/../rules.toml")" ".logos/rules.toml"
    asks "a case-folded '.LOGOS/Rules.TOML' is the same file on a case-folding disk" \
        "$(file_payload Write file_path ".LOGOS/Rules.TOML")" ".logos/rules.toml"
    asks "a JSON-escaped '\\/' path is decoded" \
        '{"tool_name":"Write","tool_input":{"file_path":"a\\\\b\/.logos\/rules.toml"}}' ".logos/rules.toml"
    asks "a redirection into a guarded file asks" \
        "$(bash_payload 'echo "[constraints]" > .logos/rules.toml')" ".logos/rules.toml"
    asks "an append redirection asks" "$(bash_payload 'printf x >>.logos/security-ignore.toml')" ".logos/security-ignore.toml"
    asks "an &> redirection asks" "$(bash_payload 'make &>.logos/rules.toml')" ".logos/rules.toml"
    asks "a quoted, concatenated path asks" "$(bash_payload "echo x > '.logos/'rules.toml")" ".logos/rules.toml"
    asks "tee asks" "$(bash_payload 'echo x | tee -a .logos/rules-baseline.json')" ".logos/rules-baseline.json"
    asks "sed -i asks" "$(bash_payload 'sed -i.bak s/a/b/ .logos/rules.toml')" ".logos/rules.toml"
    asks "sed --in-place asks" "$(bash_payload 'sed --in-place s/a/b/ .logos/rules.toml')" ".logos/rules.toml"
    asks "BSD sed -I asks" "$(bash_payload 'sed -I "" s/a/b/ .logos/rules.toml')" ".logos/rules.toml"
    asks "sed's w command asks" "$(bash_payload "sed -n '/max/w .logos/rules.toml' notes.txt")" ".logos/rules.toml"
    asks "sed's s///w flag asks" "$(bash_payload "sed 's/a/b/w .logos/rules-baseline.json' notes.txt")" ".logos/rules-baseline.json"
    asks "xxd -r asks" "$(bash_payload 'xxd -r dump.hex .logos/rules.toml')" ".logos/rules.toml"
    asks "less -o asks" "$(bash_payload 'less -o .logos/rules.toml notes.txt')" ".logos/rules.toml"
    asks "git diff --output asks" "$(bash_payload 'git diff --output=.logos/rules.toml HEAD~1')" ".logos/rules.toml"
    asks "rm asks" "$(bash_payload 'rm -f .logos/rules-baseline.json')" ".logos/rules-baseline.json"
    asks "find -delete asks" "$(bash_payload 'find .logos -name rules.toml -delete')" ".logos"
    asks "cp onto a guarded file asks" "$(bash_payload 'cp /tmp/loose.toml .logos/rules.toml')" ".logos/rules.toml"
    asks "git checkout of a guarded file asks" \
        "$(bash_payload 'git checkout HEAD~3 -- .logos/rules.toml')" ".logos/rules.toml"
    asks "git restore of a guarded file asks" "$(bash_payload 'git restore --source=HEAD~2 .logos/rules.toml')" ".logos/rules.toml"
    asks "git rm of a guarded file asks" "$(bash_payload 'git rm .logos/security-ignore.toml')" ".logos/security-ignore.toml"
    asks "rm -rf of the .logos directory asks" "$(bash_payload 'rm -rf .logos')" ".logos"
    asks "a glob that matches a guarded name asks" "$(bash_payload 'rm .logos/*.json')" ".logos/rules-baseline.json"
    asks "a ? glob that matches a guarded name asks" "$(bash_payload 'rm .logos/rules.tom?')" ".logos/rules.toml"
    asks "a brace list under .logos/ asks" "$(bash_payload 'rm .logos/{rules.toml,x}')" ".logos/"
    asks "a bare guarded name after cd .logos asks" \
        "$(bash_payload 'cd .logos && rm security-ignore.toml')" ".logos/security-ignore.toml"
    asks "a path glued to dd's of= asks" "$(bash_payload 'dd if=/tmp/loose of=.logos/rules.toml')" ".logos/rules.toml"
    asks "a path glued to a short option asks" "$(bash_payload 'sort -o.logos/rules.toml /tmp/loose')" ".logos/rules.toml"
    asks "a path glued to a long option asks" \
        "$(bash_payload 'curl --output=.logos/rules-baseline.json https://example.invalid/b')" ".logos/rules-baseline.json"
    asks "the .logos directory glued to an option asks" "$(bash_payload 'cp --target-directory=.logos /tmp/rules.toml')" ".logos"
    asks "a bare glob after cd .logos asks" "$(bash_payload 'cd .logos && rm -f rules*')" ".logos/rules.toml"
    asks "a bare * after cd .logos asks" "$(bash_payload 'cd .logos; rm *')" ".logos/"
    asks "a bare brace list after cd .logos asks" "$(bash_payload 'cd .logos && rm {rules.toml,x}')" ".logos"
    asks "a bare guarded name after a quoted cd into .logos asks" \
        "$(bash_payload 'cd ".lo"gos && rm rules-baseline.json')" ".logos/rules-baseline.json"
    # shellcheck disable=SC2016 # literal backticks, for the guard to read
    asks "a write inside backticks asks" "$(bash_payload 'echo `rm .logos/rules.toml`')" ".logos/rules.toml"
    # shellcheck disable=SC2016 # a literal $( ), for the guard to read
    asks "a write inside \$( ) asks" "$(bash_payload 'echo $(rm .logos/rules.toml)')" ".logos/rules.toml"
    # shellcheck disable=SC2088 # a literal tilde, as an agent writes it
    asks "logos by path, with a global flag first" \
        "$(bash_payload '~/.logos-bin/1.20.0/logos --json gate --save')" "logos gate --save"
    asks "logos behind env and an assignment" "$(bash_payload 'FOO=1 env logos check --rules r.toml --bless')" \
        "logos check --bless"
    asks "logos after another command in a list" "$(bash_payload 'cargo build && logos check --bless')" \
        "logos check --bless"
    asks "logos inside bash -c" "$(bash_payload 'bash -c "logos gate --save"')" "logos gate --save"
    asks "a line continuation inside logos gate --save asks" "$(bash_payload 'logos gate \
    --save')" "logos gate --save"
    asks "a line continuation after logos asks" "$(bash_payload 'logos \
    check --bless')" "logos check --bless"
    asks "an escaped backslash before a newline is no continuation: the next line is its own command" \
        "$(bash_payload 'echo a\\
rm .logos/rules.toml')" ".logos/rules.toml"
    asks "a multi-line command" "$(bash_payload 'git status
logos gate --save')" "logos gate --save"
    # The binary named in another simple command than the subcommand.
    # shellcheck disable=SC2016 # literal substitutions, for the guard to read
    asks "logos found by \$(which logos)" "$(bash_payload '$(which logos) gate --save')" "logos gate --save"
    # shellcheck disable=SC2016
    asks "logos found by \`command -v logos\`" "$(bash_payload '`command -v logos` check --bless')" "logos check --bless"
    # shellcheck disable=SC2016
    asks "logos held in a variable" "$(bash_payload 'L=logos; $L gate --save')" "logos gate --save"
    # The checked-in security baseline (S-764, FR-SC-25): its writer and the file.
    asks "logos security bless by path, with global flags" \
        "$(bash_payload '~/.logos-bin/1.20.0/logos --json security --project . bless')" "logos security bless"
    asks "logos security bless after another command in a list" \
        "$(bash_payload 'logos security scan --consent && logos security bless')" "logos security bless"
    # shellcheck disable=SC2016
    asks "logos security bless held in a variable" \
        "$(bash_payload 'L=logos; $L security bless')" "logos security bless"
    asks "an Edit of .logos/security-baseline.json asks" \
        "$(file_payload Edit file_path ".logos/security-baseline.json")" ".logos/security-baseline.json"
    asks "rm of .logos/security-baseline.json asks" \
        "$(bash_payload 'rm -f .logos/security-baseline.json')" ".logos/security-baseline.json"
    asks "a redirect into .logos/security-baseline.json asks" \
        "$(bash_payload 'echo {} > .logos/security-baseline.json')" ".logos/security-baseline.json"
    asks "a member's .logos/security-baseline.json asks" \
        "$(file_payload Write file_path "$ENROLLED/svc/.logos/security-baseline.json")" ".logos/security-baseline.json"
    asks "a bare security-baseline.json after cd .logos asks" \
        "$(bash_payload 'cd .logos && rm security-baseline.json')" ".logos/security-baseline.json"
    # Guarded words read as the shell reads them (S-784, CR-242). A backslash-newline
    # inside a word is removed, and the word joined.
    asks "a backslash-newline inside 'bless' asks" "$(bash_payload 'logos security bl\
ess')" "logos security bless"
    asks "a backslash-newline inside 'ingest' asks" "$(bash_payload 'logos security in\
gest r.sarif')" "logos security ingest"
    asks "a backslash-newline inside '--save' asks" "$(bash_payload 'logos gate --sa\
ve')" "logos gate --save"
    asks "a backslash-newline inside '--bless' asks" "$(bash_payload 'logos check --bl\
ess')" "logos check --bless"
    asks "a backslash-newline inside a guarded file name asks" "$(bash_payload 'rm .logos/rules-base\
line.json')" ".logos/rules-baseline.json"
    # An expansion where a guarded subcommand or option goes may become one.
    # shellcheck disable=SC2016 # literal expansions, for the guard to read
    asks "an assigned subcommand after logos security asks" \
        "$(bash_payload 'S=bless; logos security $S')" 'gives `logos security` `$S`'
    # shellcheck disable=SC2016
    asks "an assigned option after logos gate asks" \
        "$(bash_payload 'F=--save; logos gate $F')" 'gives `logos gate` `$F`'
    # shellcheck disable=SC2016
    asks "a braced expansion as logos security's subcommand asks, with nothing else named" \
        "$(bash_payload 'logos security ${X}')" 'gives `logos security` `${X}`'
    # shellcheck disable=SC2016
    asks "a command substitution after logos check asks" \
        "$(bash_payload 'logos check $(echo --bless)')" 'gives `logos check` a command substitution'
    # shellcheck disable=SC2016
    asks "a backtick substitution after logos check asks" \
        "$(bash_payload 'logos check `echo --bless`')" 'gives `logos check` a command substitution'
    # shellcheck disable=SC2016
    asks "a backtick substitution as logos security's subcommand asks" \
        "$(bash_payload 'logos security --json `echo bless`')" 'gives `logos security` a command substitution'
    # shellcheck disable=SC2016
    asks "an expansion completing an option word asks" \
        "$(bash_payload 'logos gate --sa$V')" 'gives `logos gate` `--sa$V`'
    # shellcheck disable=SC2016
    asks "an expansion completing a subcommand word asks" \
        "$(bash_payload 'logos security bl${X}')" 'gives `logos security` `bl${X}`'
    # shellcheck disable=SC2016
    asks "an ANSI-C quoted subcommand asks" \
        "$(bash_payload "logos security \$'bl\\x65ss'")" 'gives `logos security`'
    # shellcheck disable=SC2016
    asks "an expansion after logos check behind a binary held in a variable asks" \
        "$(bash_payload 'L=logos; $L check $B')" 'gives `logos check` `$B`'
    # A writer given an expansion, in a command line that assigns a value naming .logos.
    # shellcheck disable=SC2016
    asks "a writer given a variable assigned a guarded path asks" \
        "$(bash_payload 'F=.logos/rules.toml; rm $F')" 'F=.logos/rules.toml'
    # shellcheck disable=SC2016
    asks "a writer given a quoted variable assigned a quoted guarded path asks" \
        "$(bash_payload 'F=".logos/rules.toml" && sed -i s/a/b/ "$F"')" '`sed` is given `$F`'
    # shellcheck disable=SC2016
    asks "a writer given a variable assigned the .logos directory asks" \
        "$(bash_payload 'D=.logos; rm -rf $D')" 'D=.logos'
    # shellcheck disable=SC2016
    asks "a member's .logos assigned, then written through the variable, asks" \
        "$(bash_payload 'F=web/.logos/rules-baseline.json; cp /tmp/x $F')" 'F=web/.logos/rules-baseline.json'
    # shellcheck disable=SC2016
    asks "a redirection into a variable assigned a guarded path asks" \
        "$(bash_payload 'F=.logos/rules.toml; echo x > $F')" 'a shell redirection into `$F`'
    # shellcheck disable=SC2016
    asks "a bare redirection into a variable assigned a guarded path asks" \
        "$(bash_payload 'F=.logos/rules.toml; > $F')" 'a shell redirection into `$F`'
    # shellcheck disable=SC2016
    asks "a writer given a substitution, in a line assigning a guarded path, asks" \
        "$(bash_payload 'F=.logos/rules.toml; rm `echo $F`')" '`rm` is given a command substitution'
    # The review's reproductions and surviving mutants (S-784 Phase 6).
    # shellcheck disable=SC2016
    asks "an expansion after a global option's value, before the security subcommand, asks" \
        "$(bash_payload 'S=bless; logos security --project . $S')" 'gives `logos security` `$S` before its subcommand'
    # shellcheck disable=SC2016
    asks "an unquoted substitution glued to gate's option asks: it splits into a --save" \
        "$(bash_payload 'logos gate --label=$(echo x --save)')" 'which may expand to --save'
    # shellcheck disable=SC2016
    asks "a variable glued to check's option asks" \
        "$(bash_payload 'logos check --rules=$L')" 'gives `logos check` `--rules=$L`, which may expand to --bless'
    # shellcheck disable=SC2016
    asks "a substitution glued to security's global option asks" \
        "$(bash_payload 'logos security --project=$(echo . bless)')" 'which may expand to bless, ingest or reconfirm'
    # shellcheck disable=SC2016
    asks "a positional parameter as logos security's subcommand asks" \
        "$(bash_payload 'set -- bless; logos security $1')" 'gives `logos security` `$1`'
    # shellcheck disable=SC2016
    asks "\$@ after logos check asks" "$(bash_payload 'logos check $@')" 'gives `logos check` `$@`'
    # shellcheck disable=SC2016
    asks "an ANSI-C octal subcommand asks" \
        "$(bash_payload "logos security \$'\\142less'")" 'gives `logos security` `$142less`'
    # shellcheck disable=SC2016
    asks "an expansion after security, the binary found by \`command -v\`, asks" \
        "$(bash_payload '`command -v logos` security $S')" 'gives `logos security` `$S`'
    # shellcheck disable=SC2016
    asks "an expansion after gate, the binary found by \$(which), asks" \
        "$(bash_payload '$(which logos) gate $X')" 'gives `logos gate` `$X`'
    # shellcheck disable=SC2016
    asks "a writer given a variable assigned a member's .logos directory asks" \
        "$(bash_payload 'D=web/.logos; rm -rf $D')" 'D=web/.logos'
    # shellcheck disable=SC2016
    asks "a writer given an expansion glued to its option asks" \
        "$(bash_payload 'F=.logos/rules.toml; dd if=/tmp/x of=$F')" '`dd` is given `of=$F`'
    # shellcheck disable=SC2016
    asks "a variable name holding a digit asks" \
        "$(bash_payload 'F1=.logos/rules.toml; rm $F1')" 'F1=.logos/rules.toml, so it may write or delete what that value names under .logos/'
    # shellcheck disable=SC2016
    asks "a += assignment naming a guarded path asks" \
        "$(bash_payload 'F+=.logos/rules.toml; rm $F')" 'F+=.logos/rules.toml'
    # shellcheck disable=SC2016
    asks "a >| redirection into a variable assigned a guarded path asks" \
        "$(bash_payload 'F=.logos/rules.toml; echo x >| $F')" 'a shell redirection into `$F`'
    # shellcheck disable=SC2016
    asks "a >& redirection into a variable assigned a guarded path asks" \
        "$(bash_payload 'F=.logos/rules.toml; echo x >&$F')" 'a shell redirection into `$F`'
    # shellcheck disable=SC2016
    asks "a 2>| redirection into a variable assigned a guarded path asks" \
        "$(bash_payload 'F=.logos/rules.toml; echo x 2>| $F')" 'a shell redirection into `$F`'
    asks "a >| redirection into a guarded file asks as a redirection" \
        "$(bash_payload 'echo x >| .logos/rules.toml')" 'writes .logos/rules.toml through a shell redirection'

    # A brace expansion in a guarded position (S-801, CR-249). The shell turns one word
    # into several, so the word may spell the guarded subcommand, option or file.
    # shellcheck disable=SC2016 # literal braces, for the guard to read
    asks "a brace expansion completing gate's --save asks" \
        "$(bash_payload 'logos gate --sa{ve,}')" 'gives `logos gate` `--sa{ve,}`, which may expand to --save'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a brace expansion completing check's --bless asks" \
        "$(bash_payload 'logos check --{bless,x}')" 'gives `logos check` `--{bless,x}`, which may expand to --bless'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a brace expansion as logos security's subcommand asks" \
        "$(bash_payload 'logos security {bless,status}')" 'gives `logos security` `{bless,status}` before its subcommand'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a brace expansion before security's subcommand, after a global option, asks" \
        "$(bash_payload 'logos security --project . {ingest,status} r.sarif')" 'gives `logos security` `{ingest,status}`'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a brace sequence completing a guarded option asks" \
        "$(bash_payload 'logos check --b{k..m}ess')" 'gives `logos check` `--b{k..m}ess`'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a nested brace expansion asks" "$(bash_payload 'logos gate --sa{v{e,x},}')" 'gives `logos gate` `--sa{v{e,x},}`'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a brace expansion behind the binary held in a variable asks" \
        "$(bash_payload 'L=logos; $L gate --{save,x}')" 'gives `logos gate` `--{save,x}`'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    # shellcheck disable=SC2088 # a literal tilde, as an agent writes it
    asks "a brace expansion after gate behind a path to the binary asks" \
        "$(bash_payload '~/.logos-bin/1.21.0/logos --json gate --sa{ve,}')" 'gives `logos gate` `--sa{ve,}`'
    asks "a brace expansion over a guarded file name asks, naming the brace expansion" \
        "$(bash_payload 'rm .logos/rules{.toml,-baseline.json}')" '.logos/rules{.toml,-baseline.json} (a brace expansion beside .logos)'
    asks "a brace expansion over the .logos directory itself asks" \
        "$(bash_payload 'rm {.logos,x}/rules.toml')" '{.logos,x}/rules.toml (a brace expansion beside .logos)'
    asks "a brace expansion splitting the .logos name asks, naming the file it can spell" \
        "$(bash_payload 'rm .lo{gos,}/rules.toml')" '(a brace expansion that can spell .logos/rules.toml)'
    asks "a brace list over a guarded name beside cd .logos asks, by the older brace-list rule" \
        "$(bash_payload 'cd .logos && rm security-{ignore,x}.toml rules{.toml,x}')" '(a brace list beside a mention of .logos)'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a brace list under .logos with no comma asks, by the older brace-list rule" \
        "$(bash_payload 'rm .logos/${F}')" '(a brace list under .logos/)'
    asks "a bare one-word brace list after cd .logos asks, by the older brace-list rule" \
        "$(bash_payload 'cd .logos && rm {x}')" '(a brace list beside a mention of .logos)'
    asks "a brace alternative holding .logos, after a comma, asks" \
        "$(bash_payload 'rm {a,.logos}/x')" '{a,.logos}/x (a brace expansion beside .logos)'
    asks "a brace alternative holding .logos below a path asks" \
        "$(bash_payload 'rm -rf {a,b/.logos}')" '{a,b/.logos} (a brace expansion beside .logos)'
    asks "a redirection into a brace expansion over a guarded file asks" \
        "$(bash_payload 'echo x > .logos/rules{.toml,.bak}')" 'writes .logos/rules{.toml,.bak} (a brace expansion beside .logos) through a shell redirection'
    asks "a glued option holding a brace expansion over a guarded file asks" \
        "$(bash_payload 'dd if=/tmp/x of=.logos/rules{.toml,.bak}')" '(a brace expansion beside .logos)'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a word too long to read for a brace expansion, in a guarded position, asks" \
        "$(bash_payload "logos gate --label $OPEN_BRACE$LONG_WORD")" 'gives `logos gate`'
    passes "near miss: the same unclosed brace in a word short enough to read passes" \
        "$(bash_payload "logos gate --label $OPEN_BRACE")"
    # An expansion as logos's own subcommand (S-801): the first word after the
    # binary, or the binary's own word, before a subcommand of logos's is read.
    # shellcheck disable=SC2016 # literal expansions, for the guard to read
    asks "an assigned subcommand after logos asks" \
        "$(bash_payload 'S="gate --save"; logos $S')" 'gives `logos` `$S` where its subcommand goes'
    # shellcheck disable=SC2016
    asks "a braced parameter as logos's subcommand asks" \
        "$(bash_payload 'logos ${CMD}')" 'gives `logos` `${CMD}` where its subcommand goes'
    # shellcheck disable=SC2016
    asks "a substitution glued to logos's first argument asks" \
        "$(bash_payload 'logos check$(printf " --b%s" less)')" 'gives `logos` `check$` where its subcommand goes'
    # shellcheck disable=SC2016
    asks "a backtick substitution as logos's subcommand asks, whatever follows" \
        "$(bash_payload 'logos `echo gate` --save')" 'gives `logos` a command substitution where its subcommand goes'
    # shellcheck disable=SC2016
    asks "an expansion after logos's global option asks" \
        "$(bash_payload 'logos --json $S')" 'gives `logos` `$S` where its subcommand goes'
    # shellcheck disable=SC2016
    asks "an expansion after a global option's value asks: the value is no subcommand" \
        "$(bash_payload 'logos --project . $S')" 'gives `logos` `$S` where its subcommand goes'
    # shellcheck disable=SC2016
    asks "an expansion as the subcommand of logos behind env asks" \
        "$(bash_payload 'FOO=1 env logos $S')" 'gives `logos` `$S` where its subcommand goes'
    # shellcheck disable=SC2016
    # shellcheck disable=SC2088 # a literal tilde, as an agent writes it
    asks "an expansion as the subcommand of logos by path asks" \
        "$(bash_payload '~/.logos-bin/1.21.0/logos $S')" 'gives `logos` `$S` where its subcommand goes'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a brace expansion as logos's subcommand asks, whatever follows" \
        "$(bash_payload 'logos {gate,scan} --save')" 'gives `logos` `{gate,scan}` where its subcommand goes'
    # shellcheck disable=SC2016
    asks "logos glued to an expansion as the command word asks" \
        "$(bash_payload 'logos$X')" 'the command word `logos$X` is logos glued to an expansion'
    # shellcheck disable=SC2016
    asks "logos glued to an expansion behind env asks" \
        "$(bash_payload 'env FOO=1 logos${V} scan')" 'the command word `logos${V}` is logos glued to an expansion'
    # shellcheck disable=SC2016
    asks "logos glued to a brace pair as the command word asks" \
        "$(bash_payload 'logos{,} gate --save')" 'the command word `logos{,}` is logos glued to an expansion'
    # shellcheck disable=SC2016
    asks "logos glued to a brace pair asks whatever follows" \
        "$(bash_payload 'logos{,} $S')" 'the command word `logos{,}` is logos glued to an expansion'
    # shellcheck disable=SC2016
    asks "an expansion in the fifth word after logos asks: global options precede it" \
        "$(bash_payload 'logos --json --quiet --project . $S')" 'gives `logos` `$S` where its subcommand goes'
    # shellcheck disable=SC2016
    asks "an expansion in the eighth word after logos asks" \
        "$(bash_payload 'logos --a --b --c --d --e --f --g $S')" 'gives `logos` `$S` where its subcommand goes'
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "a brace as logos's subcommand asks when it names none of gate, check and security" \
        "$(bash_payload 'logos {ga,x}te --sa{ve,}')" 'gives `logos` `{ga,x}te` where its subcommand goes'
    # shellcheck disable=SC2016
    asks "an ANSI-C quoted subcommand of logos asks" \
        "$(bash_payload "logos \$'gate' --save")" 'gives `logos` `$gate` where its subcommand goes'
    # A loop variable (S-801): `for NAME in WORDS` assigns NAME, as `NAME=` does.
    # shellcheck disable=SC2016
    asks "a writer given a loop variable over a guarded file asks" \
        "$(bash_payload 'for F in .logos/rules.toml; do rm $F; done')" 'assigns the loop variable F (`for F in … .logos/rules.toml`)'
    # shellcheck disable=SC2016
    asks "a writer given a loop variable over the .logos directory's files asks" \
        "$(bash_payload 'for F in .logos/*; do rm "$F"; done')" 'assigns the loop variable F (`for F in … .logos/*`)'
    # shellcheck disable=SC2016
    asks "a writer given a loop variable over the .logos directory asks" \
        "$(bash_payload 'for D in .logos; do rm -rf $D; done')" 'assigns the loop variable D (`for D in … .logos`)'
    # shellcheck disable=SC2016
    asks "a loop over a member's contract asks, whichever word names .logos" \
        "$(bash_payload 'for F in a.toml web/.logos/rules-baseline.json b.toml; do cp /tmp/x $F; done')" \
        'for F in … web/.logos/rules-baseline.json'
    # shellcheck disable=SC2016
    asks "a loop over several lines asks" "$(bash_payload 'for F in a .logos/security-ignore.toml
do
    sed -i s/a/b/ $F
done')" 'assigns the loop variable F'
    # shellcheck disable=SC2016
    asks "a redirection into a loop variable asks" \
        "$(bash_payload 'for F in .logos/rules.toml; do echo x > $F; done')" 'a shell redirection into `$F`'
    # shellcheck disable=SC2016
    asks "a loop over a brace list under .logos asks" \
        "$(bash_payload 'for F in .logos/rules.{toml,bak}; do rm $F; done')" 'for F in … .logos/rules.{toml,bak}'
    # shellcheck disable=SC2016
    asks "a loop inside an if asks" \
        "$(bash_payload 'if true; then for F in .logos/rules.toml; do rm $F; done; fi')" 'assigns the loop variable F'
    # shellcheck disable=SC2016
    asks "a loop variable beside an unrelated assignment asks" \
        "$(bash_payload 'X=1; for F in .logos/rules.toml; do cp /tmp/x $F; done')" 'assigns the loop variable F'
    # shellcheck disable=SC2016
    asks "a loop variable with a digit and an underscore in its name asks" \
        "$(bash_payload 'for f_1 in .logos/rules.toml; do rm $f_1; done')" 'assigns the loop variable f_1'
    # shellcheck disable=SC2016
    asks "a loop over a member's .logos directory asks" \
        "$(bash_payload 'for D in web/.logos; do rm -rf $D; done')" 'assigns the loop variable D (`for D in … web/.logos`)'
    # shellcheck disable=SC2016
    asks "a loop variable starting with an underscore asks" \
        "$(bash_payload 'for _F in .logos/rules.toml; do rm $_F; done')" 'assigns the loop variable _F'
    # shellcheck disable=SC2016
    asks "a loop over a brace word holding .logos asks" \
        "$(bash_payload 'for F in {.logos,x}/rules.toml; do rm $F; done')" 'for F in … {.logos,x}/rules.toml'
    # shellcheck disable=SC2016
    asks "a loop over a brace word spelling .logos around a guarded name asks" \
        "$(bash_payload 'for F in .lo{gos,}/rules.toml; do rm $F; done')" 'for F in … .lo{gos,}/rules.toml'
    # shellcheck disable=SC2016
    asks "a loop over a bare guarded name beside cd .logos asks" \
        "$(bash_payload 'cd .logos && for F in rules.toml; do rm $F; done')" 'for F in … rules.toml'
    # shellcheck disable=SC2016
    asks "a variable assigned a brace word holding .logos, then written through, asks" \
        "$(bash_payload 'F={.logos,x}/rules.toml; rm $F')" 'assigns F={.logos,x}/rules.toml'
    asks "a loop whose body writes the literal file asks as before" \
        "$(bash_payload 'for i in 1 2; do rm .logos/rules.toml; done')" ".logos/rules.toml"

    # --- 3. pass-through --------------------------------------------------------
    passes "an Edit of an unguarded .logos/ file passes" \
        "$(file_payload Edit file_path "$ENROLLED/.logos/config.toml")"
    passes "a Write of an unguarded .logos/ file passes" "$(file_payload Write file_path ".logos/hooks/x.sh")"
    passes "a Read of a guarded file passes" "$(file_payload Read file_path "$ENROLLED/.logos/rules.toml")"
    passes "a Grep of a guarded file passes" \
        '{"tool_name":"Grep","tool_input":{"pattern":"max_cc","path":".logos/rules.toml"}}'
    passes "cat of a guarded file passes" "$(bash_payload 'cat .logos/rules.toml')"
    passes "grep of a guarded file, redirected elsewhere, passes" \
        "$(bash_payload 'grep -n max .logos/rules.toml > /tmp/out.txt 2>&1')"
    passes "git diff of a guarded file passes" "$(bash_payload 'git -C . diff -- .logos/rules-baseline.json')"
    passes "git add of a guarded file passes (it records, never writes, the file)" "$(bash_payload 'git add .logos/rules.toml')"
    passes "a continued cat of a guarded file passes" "$(bash_payload 'cat \
    .logos/rules.toml')"
    passes "sed without -i passes" "$(bash_payload 'sed -n 1,5p .logos/rules.toml')"
    passes "find without -delete passes" "$(bash_payload 'find .logos -name rules.toml')"
    passes "ls of the .logos directory passes" "$(bash_payload 'ls -la .logos')"
    passes "a guarded file read from with < passes" "$(bash_payload 'python3 x.py < .logos/rules.toml')"
    passes "logos check with --rules naming the contract passes" \
        "$(bash_payload 'logos check --rules .logos/rules.toml')"
    passes "logos gate without --save passes" "$(bash_payload 'logos gate --threshold 10')"
    passes "an unrelated command passes" "$(bash_payload 'cargo test -p logos-core && git status')"
    passes "an unrelated MCP tool passes" "$(mcp_payload mcp__plugin_logos_logos__check_rules)"
    passes "an unknown tool passes" '{"tool_name":"WebFetch","tool_input":{"url":"https://example.invalid"}}'
    # near misses
    passes "near miss: .logos/rules.toml.bak passes" "$(file_payload Write file_path ".logos/rules.toml.bak")"
    passes "near miss: x.logos/rules.toml passes" "$(file_payload Write file_path "x.logos/rules.toml")"
    passes "near miss: a root rules.toml passes" "$(file_payload Write file_path "$ENROLLED/rules.toml")"
    passes "near miss: logos/rules.toml (no dot) passes" "$(file_payload Write file_path "logos/rules.toml")"
    passes "near miss: a glued path read by a reader passes" "$(bash_payload 'grep --file=.logos/rules.toml x.txt')"
    passes "near miss: a glued path to an unguarded name passes" "$(bash_payload 'make OUT=.logos/rules.toml.bak')"
    passes "near miss: a glob that cannot match a guarded name passes" "$(bash_payload 'rm -f .logos/logos.db*')"
    passes "near miss: a bare glob after cd .logos that matches no guarded name passes" \
        "$(bash_payload 'cd .logos && rm -f *.db-wal')"
    passes "near miss: a bare * with no .logos in the command passes" "$(bash_payload 'cd build && rm -f *')"
    passes "near miss: a bare guarded name with no .logos in the command passes" "$(bash_payload 'rm rules.toml')"
    passes "near miss: 'gate --save' without logos passes" "$(bash_payload 'echo gate --save')"
    passes "near miss: '--save' before gate passes" "$(bash_payload 'logos --save gate')"
    passes "near miss: '--bless' without check passes" "$(bash_payload 'logos scan --bless')"
    passes "near miss: '--save' in another command of the list passes" "$(bash_payload 'logos gate; npm --save i x')"
    passes "near miss: 'logos security findings' passes" "$(bash_payload 'logos security findings --json')"
    # These name a token the early-exit filter lets through, so they reach the word walk.
    passes "near miss: 'security findings --save' reaches the walk and passes" "$(bash_payload 'logos security findings --save')"
    passes "near miss: 'security status --bless' reaches the walk and passes" "$(bash_payload 'logos security status --bless')"
    passes "near miss: 'security scan --save' reaches the walk and passes" "$(bash_payload 'logos security scan --path src --save')"
    passes "near miss: 'logos security status' passes" "$(bash_payload 'logos security status')"
    passes "near miss: 'logos security scan' passes" "$(bash_payload 'logos security scan --path src')"
    passes "near miss: 'ingest' before security passes" "$(bash_payload 'logos ingest security')"
    passes "near miss: 'security ingest' without logos passes" "$(bash_payload 'echo security ingest r.sarif')"
    passes "cat of the security baseline passes" "$(bash_payload 'cat .logos/security-baseline.json')"
    passes "git add of the security baseline passes" "$(bash_payload 'git add .logos/security-baseline.json')"
    passes "near miss: .logos/security-baseline.json.bak passes" \
        "$(file_payload Write file_path ".logos/security-baseline.json.bak")"
    passes "near miss: 'bless' before security passes" "$(bash_payload 'logos bless security')"
    passes "near miss: 'security bless' without logos passes" "$(bash_payload 'echo security bless')"
    passes "near miss: 'bless' in another command of the list passes" \
        "$(bash_payload 'logos security status; echo bless')"
    passes "near miss: 'ingest' in another command of the list passes" "$(bash_payload 'logos security status; npm ingest x')"
    passes "near miss: security_ingest under a foreign server passes" "$(mcp_payload mcp__other__security_ingest)"
    passes "near miss: security_reconfirm under a foreign server passes" "$(mcp_payload mcp__other__security_reconfirm)"
    passes "near miss: 'reconfirm' before security passes" "$(bash_payload 'logos reconfirm security')"
    passes "near miss: 'security reconfirm' without logos passes" "$(bash_payload 'echo security reconfirm abc')"
    passes "near miss: 'reconfirm' in another command of the list passes" \
        "$(bash_payload 'logos security status; npm reconfirm x')"
    passes "near miss: the other security MCP tools pass" "$(mcp_payload mcp__plugin_logos_logos__security_findings)"
    passes "near miss: bless_rules under a foreign server passes" "$(mcp_payload mcp__other__bless_rules)"
    # S-784 near misses: an expansion outside a guarded position, a value glued to its
    # option, a reader given the variable, and a variable naming nothing under .logos.
    # shellcheck disable=SC2016 # literal expansions, for the guard to read
    passes "near miss: an operand expansion after logos security status passes" \
        "$(bash_payload 'logos security status $X')"
    # shellcheck disable=SC2016
    passes "near miss: an expansion after a literal subcommand of logos security passes" \
        "$(bash_payload 'logos security scan --path $DIR')"
    # shellcheck disable=SC2016
    passes "near miss: an expansion after logos check in another segment of a path ending /logos passes" \
        "$(bash_payload 'cd /tmp/src/logos && cargo check -p logos-core $FEATURES')"
    # shellcheck disable=SC2016
    passes "near miss: 'check' and an expansion beside a crate named logos pass" \
        "$(bash_payload 'cargo check --target-dir $T -p logos')"
    # shellcheck disable=SC2016
    passes "near miss: an expansion after a security subcommand that follows a global option passes" \
        "$(bash_payload 'logos security --project . status $X')"
    # shellcheck disable=SC2016
    passes "near miss: an expansion in another command of the list passes" "$(bash_payload 'logos gate; echo $?')"
    # shellcheck disable=SC2016
    passes "near miss: an expansion after an unguarded logos subcommand passes" "$(bash_payload 'logos scan --path $DIR')"
    # shellcheck disable=SC2016
    passes "near miss: a writer given a variable assigned an unguarded path passes" "$(bash_payload 'F=src/a.rs; rm $F')"
    # shellcheck disable=SC2016
    passes "near miss: a reader given a variable assigned a guarded path passes" \
        "$(bash_payload 'F=.logos/rules.toml; cat $F')"
    # shellcheck disable=SC2016
    passes "near miss: git add of a variable assigned a guarded path passes" \
        "$(bash_payload 'F=.logos/rules.toml; git add "$F"')"
    passes "near miss: a writer given no expansion, in a line assigning a guarded path, passes" \
        "$(bash_payload 'F=.logos/rules.toml; rm notes.txt')"
    # shellcheck disable=SC2016
    passes "near miss: a value naming x.logos, not .logos, passes" "$(bash_payload 'F=x.logos/rules.toml; rm $F')"
    # shellcheck disable=SC2016
    passes "near miss: a reader's substitution assigned beside a guarded path passes" \
        "$(bash_payload 'F=.logos/rules.toml; N=$(wc -l < $F)')"
    passes "near miss: a backslash-newline between words still separates them" "$(bash_payload 'logos security status \
--json | grep bless')"
    passes "near miss: a guarded path inside another file's content passes" \
        '{"tool_name":"Write","tool_input":{"file_path":"notes.md","content":"see \"file_path\":\".logos/rules.toml\""}}'

    # S-801 near misses: a brace or an expansion outside a guarded position, a brace word that
    # is no expansion, and a loop over files the guard does not hold.
    passes "near miss: braces in a command that is not logos passes" "$(bash_payload 'echo {a,b}')"
    passes "near miss: braces after logos scan pass" "$(bash_payload 'logos scan --path src/{a,b}')"
    passes "near miss: braces after a security subcommand pass" "$(bash_payload 'logos security status {a,b}')"
    passes "near miss: a brace word with no comma or range is no expansion" "$(bash_payload 'logos gate --label {x}')"
    passes "near miss: an empty brace pair is no expansion" "$(bash_payload 'logos check --rules {} x')"
    passes "near miss: braces in logos gate's option, spaced, are two words" "$(bash_payload 'logos gate --label {a, b}')"
    passes "near miss: a brace expansion in a command beside a mention of gate passes" \
        "$(bash_payload 'cargo check --features {a,b} -p logos-core')"
    passes "near miss: braces over a path not under .logos pass" "$(bash_payload 'rm src/{a,b}.rs build/{x,y}/out')"
    passes "near miss: braces over a bare name with no .logos in the command pass" "$(bash_payload 'cp a b{1,2}.txt')"
    passes "near miss: a guarded name with braces and no .logos in the command passes" "$(bash_payload 'rm rules.toml{,.bak}')"
    passes "near miss: a guarded name in braces below another directory passes" \
        "$(bash_payload 'mv config/{rules.toml,rules.toml.bak} docs/{rules-baseline.json,x}')"
    passes "near miss: a guarded name in braces below docs passes" "$(bash_payload 'rm docs/{rules.toml,x}')"
    passes "near miss: braces over a lookalike .logosy directory pass" "$(bash_payload 'rm .logosy/{a,b}')"
    passes "near miss: braces over a lookalike .logos-old directory pass" "$(bash_payload 'rm .logos-old/{a,b}')"
    passes "near miss: a brace word with a single dot is no sequence" "$(bash_payload 'logos gate --label {x.y}')"
    passes "near miss: a reader given a brace expansion over a guarded file passes" \
        "$(bash_payload 'cat .logos/rules{.toml,-baseline.json}')"
    passes "near miss: ls of braces under .logos passes" "$(bash_payload 'ls .logos/{rules.toml,x}')"
    passes "near miss: braces over a lookalike directory pass" "$(bash_payload 'rm x.logosy/{a,b}')"
    # shellcheck disable=SC2016 # literal expansions, for the guard to read
    passes "near miss: an expansion after logos scan passes" "$(bash_payload 'logos scan $DIR')"
    # shellcheck disable=SC2016
    passes "near miss: an expansion after logos's subcommand and a global option's value passes" \
        "$(bash_payload 'logos --project . context $TASK')"
    # shellcheck disable=SC2016
    passes "near miss: an expansion after a subcommand of logos's, after an option, passes" \
        "$(bash_payload 'logos --json status $X')"
    # shellcheck disable=SC2016
    passes "near miss: an expansion in a segment after logos's passes" "$(bash_payload 'logos scan && echo gate $X')"
    # shellcheck disable=SC2016
    passes "near miss: logos as an argument, not the command, passes" "$(bash_payload 'cargo build -p logos --features $F')"
    # shellcheck disable=SC2016
    passes "near miss: logos as an echo argument passes" "$(bash_payload 'echo logos $X')"
    # shellcheck disable=SC2016
    passes "near miss: a crate path ending in logos, then an expansion, passes" "$(bash_payload 'cd /src/logos && make $T')"
    # shellcheck disable=SC2016
    passes "near miss: a command word that only starts with logos passes" "$(bash_payload 'logos-tool $X')"
    passes "near miss: logos alone passes" "$(bash_payload 'logos')"
    # shellcheck disable=SC2016
    passes "near miss: a path ending in /bin/logos, then a subcommand and an operand expansion, passes" \
        "$(bash_payload '$HOME/bin/logos scan $X')"
    # shellcheck disable=SC2016
    asks "a path to logos glued to an expansion as the command word asks" \
        "$(bash_payload '/usr/local/bin/logos$X')" 'the command word `/usr/local/bin/logos$X` is logos glued to an expansion'
    # An unclosed brace is read for an expansion only past 256 characters.
    passes "near miss: an unclosed brace of 100 characters passes" \
        "$(bash_payload "logos gate --label $OPEN_BRACE${LONG_WORD:0:97}")"
    passes "near miss: an unclosed brace of exactly 256 characters passes" \
        "$(bash_payload "logos gate --label $OPEN_BRACE${LONG_WORD:0:253}")"
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    asks "an unclosed brace of 257 characters asks" \
        "$(bash_payload "logos gate --label $OPEN_BRACE${LONG_WORD:0:254}")" 'gives `logos gate`'
    # Every subcommand of logos's is read as one: a missing name would ask for `logos NAME $X`.
    for sub in init index sync status search query context explore node callers callees impact impact-intersection \
        precedent branch-overlap affected implements referencing-docs scan quality-report health session-start \
        session-end doctor verify evolution dsm doc-gaps offenders review improvements hotspots coverage hook \
        xservice workspace stats languages serve help; do
        # shellcheck disable=SC2016
        passes "near miss: an expansion after logos $sub passes" "$(bash_payload "logos $sub \$X")"
    done
    # shellcheck disable=SC2016
    passes "near miss: a loop variable over files not under .logos passes" "$(bash_payload 'for F in src/*.rs; do rm $F; done')"
    # shellcheck disable=SC2016
    passes "near miss: a reader given a loop variable over a guarded file passes" \
        "$(bash_payload 'for F in .logos/rules.toml; do cat $F; done')"
    # shellcheck disable=SC2016
    passes "near miss: git add given a loop variable over a guarded file passes" \
        "$(bash_payload 'for F in .logos/rules.toml; do git add "$F"; done')"
    # shellcheck disable=SC2016
    passes "near miss: a writer given no loop variable, over a guarded file, passes" \
        "$(bash_payload 'for F in .logos/rules.toml; do rm notes.txt; done')"
    # shellcheck disable=SC2016
    passes "near miss: a loop over a lookalike directory passes" "$(bash_payload 'for F in x.logos/rules.toml; do rm $F; done')"
    # shellcheck disable=SC2016
    passes "near miss: a loop whose guarded name is in the body only, read, passes" \
        "$(bash_payload 'for F in a b; do grep x .logos/rules.toml; done')"
    # shellcheck disable=SC2016
    passes "near miss: 'for' as an argument is no loop" "$(bash_payload 'echo for F in .logos/rules.toml')"
    # shellcheck disable=SC2016
    passes "near miss: a loop over .logos in the header, a command writing nothing named, passes" \
        "$(bash_payload 'for F in .logos/rules.toml; do echo $F; done')"
    # shellcheck disable=SC2016 # literal braces and backticks, for the guard to read
    passes "a read of the whole contract in a loop passes" \
        "$(bash_payload 'for F in rules.toml rules-baseline.json; do wc -l .logos/$F; done')"

    # --- 4. malformed payloads, and an unenrolled project -----------------------
    for m in "empty|" "not JSON|garbage" "a JSON array|[]" "no tool_name|{\"tool_input\":{}}" \
        "Write with no file_path|{\"tool_name\":\"Write\",\"tool_input\":{\"content\":\"x\"}}" \
        "Bash with no command|{\"tool_name\":\"Bash\",\"tool_input\":{}}" \
        "a truncated payload|{\"tool_name\":\"Edit\",\"tool_input\":{\"file_path\":\".logos/con"; do
        asks "malformed payload (${m%%|*}) asks" "${m#*|}" "could not be read"
    done
    passes "an unenrolled project is silent, even for a guarded write" \
        "$(file_payload Write file_path "$PLAIN/.logos/rules.toml")" "$PLAIN"
    passes "an unenrolled project is silent, even for a malformed payload" "garbage" "$PLAIN"

    # --- 6. bounded time -------------------------------------------------------
    # The host cancels a hook at its timeout and lets the call through, so a slow
    # guard is an open guard. Each case must decide well inside hooks.json's 10 s.
    asks "every file_path in the payload is read, not only the first" \
        '{"tool_name":"Write","tool_input":{"file_path":"notes.md","file_path":".logos/rules.toml"}}' ".logos/rules.toml"
    bounded asks "a 3000-word heredoc, then a guarded rm: asks" "$WORK/long-rm.json" ".logos/rules.toml"
    bounded passes "a 3000-word heredoc that names nothing guarded: passes" "$WORK/long-benign.json"
    bounded asks "a 1 MB Write whose content precedes file_path: asks" "$WORK/big-write.json" ".logos/rules.toml"
    bounded asks "a 240 KB Edit, escaped quotes, old_string before file_path: asks" "$WORK/big-edit.json" \
        ".logos/rules-baseline.json"
    # bash 3.2's ${var//pat/rep} is far worse than linear once matches are many,
    # and slower again on multibyte text under a UTF-8 locale.
    bounded passes "a command of 8000 quotes that names nothing guarded: passes" "$WORK/quotes.json"
    bounded passes "8000 expansions beside logos, none in a guarded position: passes" "$WORK/dollars.json"
    # shellcheck disable=SC2016 # a literal reason fragment
    bounded asks "8000 expansions, then a writer given a variable assigned a guarded path: asks" \
        "$WORK/dollars-rm.json" '`rm` is given `$F`'
    bounded passes "4000 backtick substitutions beside logos: passes" "$WORK/backticks.json"
    bounded passes "8000 backslash-newlines inside one logos command: passes" "$WORK/continuations.json"
    # Sized past where a quadratic shape shows: an indexed loop over 80000 backtick
    # parts, a two-star case pattern on a word of 80000 or 160000 '=', and a walk of 64000
    # segments, or of 80000 words beside a logos that names no guarded subcommand.
    bounded passes "80000 backtick substitutions naming nothing guarded: passes" "$WORK/backticks-80k.json"
    bounded passes "a word of 80000 '=' after logos security status: passes" "$WORK/equals-word.json"
    bounded passes "an assignment of 160000 '=' beside a guarded path: passes" "$WORK/equals-assign.json"
    bounded passes "64000 segments holding check and an expansion, logos not named: passes" "$WORK/dollar-segments.json"
    bounded passes "80000 expansions beside logos scan, no guarded subcommand named: passes" "$WORK/dollars-scan.json"
    # S-801: sized past where a quadratic shape shows, as above.
    bounded passes "8000 brace expansions after logos security status: passes" "$WORK/braces-security.json"
    bounded passes "4000 brace expansions after ls .logos (a reader): passes" "$WORK/braces-reader.json"
    bounded passes "a 45000-character brace word after logos scan: passes" "$WORK/brace-long-word.json"
    bounded passes "10000 expansions after logos scan, whose subcommand is read: passes" "$WORK/expansions-scan.json"
    bounded passes "40000 'logos a' words beside an expansion, logos not the command: passes" "$WORK/logos-words.json"
    bounded passes "40000 words after logos, then an expansion past the eighth: passes" "$WORK/logos-then-words.json"
    # ${word##*/} is quadratic in bash 3.2 on one long word with no slash (100 KB: 6-10 s).
    bounded passes "a 200 KB word after logos scan \$x; : passes" "$WORK/logos-long-word.json"
    bounded asks "a loop over 10000 words, the last naming .logos, then a writer: asks" "$WORK/for-words.json" \
        'assigns the loop variable F'
    bounded passes "1000 loops that name nothing guarded, then ls .logos: passes" "$WORK/for-segments.json"
    RUN_LANG=en_US.UTF-8 bounded asks "6000 multibyte words, then a guarded rm, under a UTF-8 locale: asks" \
        "$WORK/multibyte-rm.json" ".logos/rules.toml"
    RUN_LANG=en_US.UTF-8 asks "a UTF-8 path is quoted in a valid JSON reason" "$(cat "$WORK/utf8-path.json")" \
        "citt$(printf '\303\240')/.logos/rules.toml"
    asks "an over-long path still gives one valid JSON object, its reason cut" "$(cat "$WORK/long-path.json")" "would write"
done

# --- 5. hooks.json ----------------------------------------------------------------

B="${BASHES[0]}"
REG="$(python3 -I - "$PLUGIN/hooks/hooks.json" <<'PY'
import json, re, sys
groups = json.load(open(sys.argv[1]))["hooks"].get("PreToolUse", [])
hits = [(g.get("matcher", ""), h) for g in groups for h in g["hooks"] if h["command"].endswith('/hooks/guard.sh"')]
if len(hits) != 1:
    print("registered %d times" % len(hits)); sys.exit()
m, h = hits[0]
want = ["Write", "Edit", "MultiEdit", "NotebookEdit", "Bash",
        "mcp__logos__bless_rules", "mcp__plugin_logos_logos__bless_rules",
        "mcp__logos__security_ingest", "mcp__plugin_logos_logos__security_ingest",
        "mcp__logos__security_reconfirm", "mcp__plugin_logos_logos__security_reconfirm"]
miss = [t for t in want if not re.fullmatch(m, t)]
extra = [t for t in ["Read", "Grep", "Glob", "mcp__logos__check_rules", "mcp__other__bless_rules",
    "mcp__logos__security_findings", "mcp__other__security_ingest", "mcp__other__security_reconfirm"] if re.fullmatch(m, t)]
print("missing %s" % miss if miss else "matches %s" % extra if extra else
      "ok %s %s" % (h["type"], h["command"]))
PY
)"
expect "[manifest] hooks.json registers guard.sh once, for every guarded tool and no reader" \
    "${REG%% *}" "ok"
CMD="${REG#ok command }"

SPACED="$WORK/plugin root"
mkdir -p "$SPACED" && cp -R "$PLUGIN/." "$SPACED/"
SYS="$WORK/sys-cmd" && mkdir -p "$SYS" && ln -sf "$B" "$SYS/bash"
RC=0
OUT="$(cd "$ENROLLED" && bash_payload 'logos check --bless' | env -i HOME="$WORK" PATH="$SYS" \
    CLAUDE_PLUGIN_ROOT="$SPACED" CLAUDE_PROJECT_DIR="$ENROLLED" /bin/sh -c "$CMD" 2>"$WORK/err")" || RC=$?
expect "[manifest] the hooks.json command runs from a plugin root with a space: it asks" \
    "$RC|$(decision | cut -c1-4)" "0|ask|"
expect "[manifest] ... with nothing on stderr" "$(cat "$WORK/err")" ""

echo
echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
