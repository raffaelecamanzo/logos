#!/usr/bin/env bash
#
# test-plugin-tree.sh — the invariants of the whole logos Claude Code plugin tree
# (.claude-plugin/marketplace.json and plugins/logos/, S-678, FR-IN-11, ADR-81),
# written so a later story that adds a hook, a script or a skill is held to them
# without editing this suite.
#
#   1. manifests: the marketplace lists the plugin once; plugin.json carries the
#      version, metadata.minBinaryVersion and the one MCP server, and nothing
#      else declares a server (a second declaration lists every tool twice);
#      no file of the tree is ignored by git, so none silently stays out of a
#      commit;
#   2. scripts: every executable script is bash with `set -euo pipefail`, every
#      sourced lib sets no shell option, every command hooks.json names exists
#      and is executable, every hook script is named by hooks.json, and every
#      script is exercised by a scripts/tests/test-plugin-*.sh suite;
#   3. enrolled repositories only: every hook hooks.json declares, and the MCP
#      launcher, exits 0 with no output in a directory with no .logos/, without
#      running logos;
#   4. no extension named (BR-63): the reviewed term list finds nothing in the
#      tree with no allowlist at all, and no id of an extension that is not part
#      of this repository appears as a word. Both checks are first shown to
#      catch a planted name.
#
# Usage: bash scripts/tests/test-plugin-tree.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"
MARKET="$HERE/.claude-plugin/marketplace.json"

WORK="${TMPDIR:-/tmp}/logos-plugin-tree-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }

for f in "$MARKET" "$PLUGIN/.claude-plugin/plugin.json" "$PLUGIN/hooks/hooks.json" \
    "$PLUGIN/hooks/session-start.sh" "$PLUGIN/hooks/lib/common.sh" "$PLUGIN/scripts/mcp-serve.sh"; do
    if [ -f "$f" ]; then ok "[tree] ${f#"$HERE"/} exists"; else bad "[tree] ${f#"$HERE"/} is missing"; fi
done

# --- 1. manifests ---------------------------------------------------------------

py() { python3 -I -c "$1" "$HERE" 2>&1; }

expect "[manifest] every .json in the tree parses" "$(py '
import json, os, sys
root = sys.argv[1]
paths = [os.path.join(root, ".claude-plugin", "marketplace.json")]
for d, _, fs in os.walk(os.path.join(root, "plugins", "logos")):
    paths += [os.path.join(d, f) for f in fs if f.endswith(".json")]
bad = []
for p in paths:
    try: json.load(open(p))
    except Exception as e: bad.append(f"{os.path.relpath(p, root)}: {e}")
print("ok" if not bad and len(paths) >= 4 else bad or f"only {len(paths)} files")')" ok

expect "[manifest] the marketplace has a name, an owner and exactly one plugin, logos, at ./plugins/logos" "$(py '
import json, sys
m = json.load(open(sys.argv[1] + "/.claude-plugin/marketplace.json"))
ps = m["plugins"]
print(bool(m.get("name")), bool(m.get("owner", {}).get("name")), [(p["name"], p["source"]) for p in ps])')" \
    "True True [('logos', './plugins/logos')]"

expect "[manifest] the marketplace entry carries no version: plugin.json is the one place it is pinned" "$(py '
import json, sys
print(sorted(k for p in json.load(open(sys.argv[1] + "/.claude-plugin/marketplace.json"))["plugins"] for k in p if k == "version"))')" "[]"

expect "[manifest] plugin.json: name logos, an X.Y.Z version and minimum, minimum <= version" "$(py '
import json, re, sys
d = json.load(open(sys.argv[1] + "/plugins/logos/.claude-plugin/plugin.json"))
v, m = d.get("version", ""), d.get("metadata", {}).get("minBinaryVersion", "")
sem = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
t = lambda s: tuple(int(x) for x in s.split("."))
print(d["name"], bool(sem.fullmatch(v)), bool(sem.fullmatch(m)), sem.fullmatch(v) and sem.fullmatch(m) and t(m) <= t(v))')" \
    "logos True True True"

expect "[manifest] plugin.json declares exactly one MCP server, logos" "$(py '
import json, sys
print(sorted(json.load(open(sys.argv[1] + "/plugins/logos/.claude-plugin/plugin.json")).get("mcpServers", {})))')" "['logos']"
expect "[manifest] no .mcp.json in the plugin tree declares a second server" \
    "$(find "$PLUGIN" -name .mcp.json | grep -c .)" 0
expect "[manifest] plugin.json names no hook, skill, agent or command path: the default directories hold them" "$(py '
import json, sys
d = json.load(open(sys.argv[1] + "/plugins/logos/.claude-plugin/plugin.json"))
print(sorted(k for k in ("hooks", "skills", "agents", "commands") if k in d))')" "[]"

# Every file on disk in the tree is one git takes: an ignored file (a local
# exclude of .mcp.json, say) would pass every check here and never be committed.
IGNORED="$(cd "$HERE" && find .claude-plugin plugins -type f | LC_ALL=C sort | git check-ignore --stdin --no-index 2>/dev/null)"
expect "[manifest] no file of the plugin tree is ignored by git" "$IGNORED" ""

# The hook script plugin.json reads the minimum from must find the key verbatim.
expect "[manifest] the minimum the hooks read is the one plugin.json declares" \
    "$(. "$PLUGIN/hooks/lib/common.sh" && logos_min_binary_version)" \
    "$(py 'import json,sys; print(json.load(open(sys.argv[1] + "/plugins/logos/.claude-plugin/plugin.json"))["metadata"]["minBinaryVersion"])')"

# --- 2. scripts -----------------------------------------------------------------

# Every hook command hooks.json declares, one per line: "<event>\t<command>".
HOOKS="$(py '
import json, sys
d = json.load(open(sys.argv[1] + "/plugins/logos/hooks/hooks.json"))["hooks"]
for ev, groups in d.items():
    for g in groups:
        for h in g["hooks"]:
            assert h["type"] == "command", h
            print(ev + "\t" + h["command"])')"
NHOOKS="$(printf '%s\n' "$HOOKS" | grep -c .)"
if [ "$NHOOKS" -ge 1 ]; then ok "[scripts] hooks.json declares $NHOOKS hook command(s)"; else bad "[scripts] hooks.json declares no hook: $HOOKS"; fi

# A command is the quoted plugin-root path the validator asks for, and nothing else.
while IFS=$'\t' read -r ev cmd; do
    [ -n "$ev" ] || continue
    rel=""
    case "$cmd" in '"${CLAUDE_PLUGIN_ROOT}/'*'"') rel="${cmd#\"\$\{CLAUDE_PLUGIN_ROOT\}/}" rel="${rel%\"}" ;; esac
    if [ -n "$rel" ] && [ -f "$PLUGIN/$rel" ] && [ -x "$PLUGIN/$rel" ]; then
        ok "[scripts] $ev command \"\${CLAUDE_PLUGIN_ROOT}/$rel\" is an executable file"
    else
        bad "[scripts] $ev command is not a quoted, executable \${CLAUDE_PLUGIN_ROOT} path: $cmd"
    fi
done <<EOF
$HOOKS
EOF

for s in "$PLUGIN"/hooks/*.sh; do
    [ -e "$s" ] || continue
    n="${s##*/}"
    expect "[scripts] hooks/$n is declared by hooks.json" "$(printf '%s\n' "$HOOKS" | grep -c "/hooks/$n\"")" 1
done

SCRIPTS=()
while IFS= read -r s; do SCRIPTS+=("$s"); done <<EOF
$(find "$PLUGIN" -name '*.sh' -type f ! -path '*/lib/*' | LC_ALL=C sort)
EOF
LIBS=()
while IFS= read -r s; do [ -n "$s" ] && LIBS+=("$s"); done <<EOF
$(find "$PLUGIN" -name '*.sh' -type f -path '*/lib/*' | LC_ALL=C sort)
EOF
if [ "${#SCRIPTS[@]}" -ge 2 ]; then
    ok "[scripts] ${#SCRIPTS[@]} executable script(s) and ${#LIBS[@]} lib(s) found"
else
    bad "[scripts] found only ${#SCRIPTS[@]} script(s)"
fi

for s in "${SCRIPTS[@]}"; do
    rel="${s#"$PLUGIN"/}"
    expect "[scripts] $rel: a #!/usr/bin/env bash script, executable" \
        "$(head -1 "$s"):$([ -x "$s" ] && echo x)" "#!/usr/bin/env bash:x"
    expect "[scripts] $rel: runs under set -euo pipefail" "$(grep -c '^set -euo pipefail$' "$s")" 1
    # The suites that exercise it name it by its plugin-relative path.
    n="$(grep -l -F "$rel" "$HERE"/scripts/tests/test-plugin-*.sh 2>/dev/null | grep -v -c '/test-plugin-tree\.sh$')"
    if [ "$n" -ge 1 ]; then ok "[scripts] $rel is exercised by a test-plugin-*.sh suite"; else bad "[scripts] $rel has no test-plugin-*.sh suite naming it"; fi
done
for l in "${LIBS[@]+"${LIBS[@]}"}"; do
    expect "[scripts] ${l#"$PLUGIN"/} is sourced: it sets no shell option" "$(grep -c -E '^[[:space:]]*set -' "$l")" 0
done

# --- 3. enrolled repositories only ----------------------------------------------

PLAIN="$WORK/plain"
mkdir -p "$PLAIN" "$WORK/bin" "$WORK/sys"
CALLS="$WORK/calls"
printf '#!/bin/sh\nprintf "%%s\\n" "$*" >>"%s"\nprintf "logos 99.0.0\\n"\n' "$CALLS" >"$WORK/bin/logos"
chmod +x "$WORK/bin/logos"
ln -sf "$(command -v bash)" "$WORK/sys/bash"

payload() { # event -> a plausible stdin payload for it
    case "$1" in
        SessionStart) printf '{"hook_event_name":"SessionStart","source":"startup","session_id":"t","cwd":"%s"}' "$PLAIN" ;;
        Stop) printf '{"hook_event_name":"Stop","stop_hook_active":false,"session_id":"t","cwd":"%s"}' "$PLAIN" ;;
        PreToolUse) printf '{"hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{"file_path":"%s/.logos/rules.toml","content":"x"},"session_id":"t","cwd":"%s"}' "$PLAIN" "$PLAIN" ;;
        *) printf '{"hook_event_name":"%s","session_id":"t","cwd":"%s"}' "$1" "$PLAIN" ;;
    esac
}
noop() { # label command-for-sh event
    local rc=0 out err
    : >"$CALLS"
    out="$(cd "$PLAIN" && payload "$3" | env -i HOME="$WORK" PATH="$WORK/bin:$WORK/sys" CLAUDE_PLUGIN_ROOT="$PLUGIN" \
        CLAUDE_PROJECT_DIR="$PLAIN" /bin/sh -c "$2" 2>"$WORK/err")" || rc=$?
    err="$(cat "$WORK/err")"
    expect "[enrolled-only] $1: exit 0, no output, logos not run" "$rc|$out|$err|$(cat "$CALLS")" "0|||"
}
while IFS=$'\t' read -r ev cmd; do
    [ -n "$ev" ] && noop "$ev hook ${cmd##*/}" "$cmd" "$ev"
done <<EOF
$HOOKS
EOF
MCPCMD="$(py 'import json,sys; s=json.load(open(sys.argv[1] + "/plugins/logos/.claude-plugin/plugin.json"))["mcpServers"]["logos"]; print(" ".join(["\"" + s["command"] + "\""] + s.get("args", [])))')"
noop "the MCP server launcher" "$MCPCMD </dev/null" none
expect "[enrolled-only] no hook or launcher created a .logos/" "$([ -e "$PLAIN/.logos" ] && echo created || echo absent)" absent

# --- 4. no extension named ------------------------------------------------------
#
# The tree is copied into a fixture repository so check-public-terms.sh walks it
# alone, against the reviewed term list and an allowlist with no entry: inside
# the plugin tree no line is excused.

TREE="$WORK/tree"
mkdir -p "$TREE"
(cd "$HERE" && git ls-files -co --exclude-standard -- .claude-plugin plugins) >"$WORK/files" 2>/dev/null
NFILES="$(grep -c . "$WORK/files")"
while IFS= read -r f; do
    [ -f "$HERE/$f" ] || continue
    mkdir -p "$TREE/$(dirname "$f")" && cp -p "$HERE/$f" "$TREE/$f"
done <"$WORK/files"
(cd "$TREE" && git init -q . && git add -A) >/dev/null 2>&1
printf '# no line of the plugin tree is admitted\n' >"$WORK/no-allow"

terms() { # root [terms-file] -> "rc summary"
    local rc=0 out
    out="$(bash "$HERE/scripts/check-public-terms.sh" --root "$1" --terms "${2:-$HERE/scripts/public-terms.sha256}" \
        --allow "$WORK/no-allow" 2>&1)" || rc=$?
    printf '%s %s\n' "$rc" "$(printf '%s\n' "$out" | tail -1)"
}
R="$(terms "$TREE")"
expect "[extensions] the reviewed term list finds no hit in the plugin tree, with no allowlist" \
    "$(printf '%s' "$R" | sed -E 's/ .*hits=([0-9]+) .*/ hits=\1/')" "0 hits=0"
expect "[extensions] ... having read every file of the tree ($NFILES)" \
    "$(printf '%s' "$R" | sed -E 's/.* scanned=([0-9]+) .*/\1/')" "$NFILES"

# The probe: a planted word, under a term list of its own, is found in the copy.
PROBE="$WORK/probe"
cp -R "$TREE" "$PROBE"
printf '\n# plantedterm\n' >>"$PROBE/plugins/logos/hooks/session-start.sh"
(cd "$PROBE" && git add -A) >/dev/null 2>&1
printf '%s\n' "$(printf '%s' plantedterm | shasum -a 256 | cut -d' ' -f1)" >"$WORK/probe-terms"
expect "[extensions] probe: a planted listed word is found by the same walk (exit 1, one hit)" \
    "$(terms "$PROBE" "$WORK/probe-terms" | sed -E 's/ .*hits=([0-9]+) .*/ hits=\1/')" "1 hits=1"

# The ids of the extensions this repository does not carry, as whole words the
# way the term gate splits them:
# lower-cased, any non-alphanumeric a separator, a camelCase hump a word break.
ids() { # root -> "path:line:word" for each id found
    python3 -I - "$1" <<'PY'
import os, re, sys
IDS = {"wiki", "chat"}
root = sys.argv[1]
for d, dirs, fs in os.walk(root):
    dirs[:] = [x for x in dirs if x != ".git"]
    for f in fs:
        p = os.path.join(d, f)
        for n, line in enumerate(open(p, encoding="utf-8", errors="replace"), 1):
            line = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", line)
            line = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1 \2", line)
            for w in re.split(r"[^0-9a-z]+", line.lower()):
                if w in IDS:
                    print(f"{os.path.relpath(p, root)}:{n}:{w}")
PY
}
expect "[extensions] no id of an extension outside this repository appears as a word in the plugin tree" "$(ids "$TREE")" ""
printf '\n# see the LogosWiki pages and logos-chat\n' >>"$PROBE/plugins/logos/hooks/lib/common.sh"
expect "[extensions] probe: a planted id is found, camelCase and hyphenated alike" \
    "$(ids "$PROBE" | sed 's/^.*://' | LC_ALL=C sort | tr '\n' ' ')" "chat wiki "
printf '\n# wikipedia chatter whitelist\n' >"$PROBE/plugins/logos/near-miss.txt"
expect "[extensions] probe: a longer word containing an id is not the id (near miss)" \
    "$(ids "$PROBE" | grep -c near-miss)" 0

echo
echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
