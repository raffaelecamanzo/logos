#!/usr/bin/env bash
#
# test-plugin-mcp-serve.sh — prove the logos plugin's MCP launcher
# (plugins/logos/scripts/mcp-serve.sh, S-678, FR-IN-11) starts `logos serve --mcp`
# in an enrolled repository and nothing anywhere else, because `logos serve`
# would create a .logos/ store in whatever directory it starts in. Anywhere it
# cannot serve logos, it answers the MCP handshake as an empty server rather than
# closing (a server that closes early is recorded as failed by the host and
# skipped in the next projects too).
#
# The launcher runs as the host runs it: the command plugin.json's mcpServers
# declares, with CLAUDE_PLUGIN_ROOT set, under `env -i` and a PATH holding only a
# stub `logos` and `bash`.
#
# Usage: bash scripts/tests/test-plugin-mcp-serve.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"

WORK="${TMPDIR:-/tmp}/logos-plugin-mcp-serve-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }

# The declared server: exactly one, named logos, with its command resolved
# against the plugin root.
DECL="$(python3 -I -c 'import json,sys
d = json.load(open(sys.argv[1]))["mcpServers"]
assert list(d) == ["logos"], list(d)
s = d["logos"]
print(s["command"]); print(" ".join(s.get("args", [])))' "$PLUGIN/.claude-plugin/plugin.json")" || {
    echo "FAIL  plugins/logos/.claude-plugin/plugin.json does not declare exactly one server, logos"
    echo "0 passed, 1 failed (of 1)"
    exit 1
}
CMD="$(printf '%s\n' "$DECL" | sed -n 1p)"
ARGS="$(printf '%s\n' "$DECL" | sed -n 2p)"
LAUNCHER="${CMD//\$\{CLAUDE_PLUGIN_ROOT\}/$PLUGIN}"
UNDER=no
case "$CMD" in '${CLAUDE_PLUGIN_ROOT}/'*) UNDER=yes ;; esac
expect "[decl] the server command is under the plugin root" "$UNDER" yes
# A second declaration beside it would start a second server and list every
# tool twice; plugin.json is the one place (a .mcp.json is also ignored locally
# by many checkouts, and would silently never be committed).
expect "[decl] no plugins/logos/.mcp.json declares a server beside plugin.json" \
    "$([ -e "$PLUGIN/.mcp.json" ] && echo present || echo absent)" absent
expect "[decl] the command is scripts/mcp-serve.sh, an executable file" \
    "$([ "$LAUNCHER" = "$PLUGIN/scripts/mcp-serve.sh" ] && [ -x "$LAUNCHER" ] && echo yes || echo "no: $LAUNCHER")" yes

ENROLLED="$WORK/enrolled"
PLAIN="$WORK/plain"
FILEDOT="$WORK/dotfile"
mkdir -p "$ENROLLED/.logos" "$PLAIN" "$FILEDOT" && : >"$FILEDOT/.logos"
CALLS="$WORK/calls"

# The stub logos records argv and cwd, and echoes what arrives on stdin, so a
# case can see that exec handed the host's pipe straight to the server.
mkdir -p "$WORK/bin" "$WORK/sys"
cat >"$WORK/bin/logos" <<EOF
#!/bin/sh
printf '%s|%s\n' "\$*" "\$(pwd -P)" >>'$CALLS'
while IFS= read -r line; do printf '%s\n' "\$line"; done
EOF
chmod +x "$WORK/bin/logos"
ln -sf "$(command -v bash)" "$WORK/sys/bash"

# serve <project-or-empty> <with-binary yes|no> [cwd] — sets OUT, ERR, RC. The
# launcher's stdin is $FRAMES (default: one opaque line).
serve() {
    local path="$WORK/sys" envs
    [ "$2" = yes ] && path="$WORK/bin:$WORK/sys"
    envs=(HOME="$WORK" PATH="$path" CLAUDE_PLUGIN_ROOT="$PLUGIN")
    [ -n "$1" ] && envs+=(CLAUDE_PROJECT_DIR="$1")
    : >"$CALLS"
    RC=0
    # shellcheck disable=SC2086 # ARGS is the declared argv, word-split on purpose
    (cd "${3:-$WORK}" && printf '%s\n' "${FRAMES:-initialize-frame}" | env -i "${envs[@]}" "$LAUNCHER" $ARGS) \
        >"$WORK/out" 2>"$WORK/err" || RC=$?
    OUT="$(cat "$WORK/out")" ERR="$(cat "$WORK/err")"
}

# reply <id-json> <python-expression-over-r> — evaluates over the one reply in
# $OUT whose id is <id-json>; NONE when there is none, INVALID when a line of
# $OUT is not JSON, NO-JSONRPC when a line lacks "jsonrpc":"2.0" (a client
# rejects that reply, and a failed handshake is what the empty server exists to
# prevent). The expressions are this file's own literals.
reply() {
    printf '%s\n' "$OUT" | python3 -I -c '
import json, sys
want, expr = json.loads(sys.argv[1]), sys.argv[2]
got = []
for line in sys.stdin.read().splitlines():
    if not line.strip():
        continue
    try:
        m = json.loads(line)
    except Exception:
        print("INVALID"); sys.exit()
    if m.get("jsonrpc") != "2.0":
        print("NO-JSONRPC"); sys.exit()
    if m.get("id") == want:
        got.append(m)
if len(got) != 1:
    print("NONE" if not got else "DUPLICATE"); sys.exit()
r = got[0]
print(eval(expr))' "$1" "$2"
}

serve "$ENROLLED" yes
expect "[serve] enrolled: logos serve --mcp runs once, in the project" "$(cat "$CALLS")" "serve --mcp|$ENROLLED"
expect "[serve] enrolled: the host's stdin reaches the server" "$OUT" "initialize-frame"
expect "[serve] enrolled: exit 0 and nothing on stderr" "$RC|$ERR" "0|"

serve "" yes "$ENROLLED"
expect "[serve] CLAUDE_PROJECT_DIR unset (the host does not export it to MCP servers): the cwd is the project" \
    "$(cat "$CALLS")" "serve --mcp|$ENROLLED"

# The empty server, as a host speaks to it: initialize, the initialized
# notification, tools/list, ping, an unknown request, a string id.
FRAMES='{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":2,"method":"ping"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"status","arguments":{}}}
{"method":"tools/list","jsonrpc":"2.0","id":"s-1"}
{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}'
NREPLIES=5

for c in plain cwd filedot nobin; do
    case "$c" in
        plain) serve "$PLAIN" yes; T="not enrolled" ;;
        cwd) serve "" yes "$PLAIN"; T="CLAUDE_PROJECT_DIR unset, cwd not enrolled" ;;
        filedot) serve "$FILEDOT" yes; T="a .logos that is a file (near miss)" ;;
        nobin) serve "$ENROLLED" no; T="enrolled, no binary" ;;
    esac
    expect "[empty] $T: exit 0 at end of input, nothing on stderr" "$RC|$ERR" "0|"
    expect "[empty] $T: logos is never started" "$(cat "$CALLS")" ""
    expect "[empty] $T: one reply per request, none to a notification" "$(printf '%s\n' "$OUT" | grep -c .)" "$NREPLIES"
    expect "[empty] $T: initialize echoes the client's protocol version and names the server" \
        "$(reply 0 '(r["result"]["protocolVersion"], r["result"]["serverInfo"]["name"], "tools" in r["result"]["capabilities"])')" \
        "('2025-03-26', 'logos', True)"
    expect "[empty] $T: tools/list lists no tool" "$(reply 1 'r["result"]["tools"]')" "[]"
    expect "[empty] $T: ping answers" "$(reply 2 'r["result"]')" "{}"
    expect "[empty] $T: any other request is refused as method not found" "$(reply 3 'r["error"]["code"]')" "-32601"
    expect "[empty] $T: a string id is echoed as a string" "$(reply '"s-1"' 'r["result"]["tools"]')" "[]"
    INSTR="$(reply 0 'r["result"].get("instructions", "")')"
    if [ "$c" = nobin ]; then
        SAYS="no: $INSTR"
        case "$INSTR" in *"no logos binary is on PATH"*) SAYS=yes ;; esac
        expect "[empty] $T: the instructions say the binary is missing" "$SAYS" yes
    else
        expect "[empty] $T: no instructions, so nothing reaches the agent" "$INSTR" ""
    fi
done
expect "[empty] no .logos/ is created" "$([ -e "$PLAIN/.logos" ] && echo created || echo absent)" absent

FRAMES='{"jsonrpc":"2.0","id":7,"method":"initialize","params":{}}'
serve "$PLAIN" yes
expect "[empty] an initialize without a protocol version gets the default one" \
    "$(reply 7 'r["result"]["protocolVersion"]')" "2025-06-18"
# Only a protocol version of [0-9A-Za-z.-] is echoed: one that would break out
# of its JSON string (a quote, a backslash) gets the default, in a valid reply.
FRAMES='{"jsonrpc":"2.0","id":8,"method":"initialize","params":{"protocolVersion":"1\"},\"x\":\"y"}}
{"jsonrpc":"2.0","id":10,"method":"initialize","params":{"protocolVersion":"2025\\x"}}'
serve "$PLAIN" yes
expect "[empty] a protocol version holding a quote is not echoed: the default, in valid JSON" \
    "$(reply 8 '(r["result"]["protocolVersion"], sorted(r["result"]))')" "('2025-06-18', ['capabilities', 'protocolVersion', 'serverInfo'])"
expect "[empty] a protocol version holding a backslash is not echoed: the default" \
    "$(reply 10 'r["result"]["protocolVersion"]')" "2025-06-18"

FRAMES='{"jsonrpc":"2.0","id":9,"method":"ping"}'
expect "[empty] a last line with no newline is still answered" \
    "$( (cd "$WORK" && printf '%s' "$FRAMES" | env -i HOME="$WORK" PATH="$WORK/sys" CLAUDE_PROJECT_DIR="$PLAIN" "$LAUNCHER") | grep -c '"id":9')" 1

echo
echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
